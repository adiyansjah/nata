//! The arms of the linear-chain comparison. Every arm runs the same work: morsels of `batch` rows, each passed through
//! `len` stages of the chosen kernel, summed into a checksum, using `threads` OS threads that take morsels from one
//! shared counter. Only the plumbing differs:
//!
//! | arm | what it is |
//! |---|---|
//! | `control` | a hand-fused loop: no operators, no per-stage batch, no dispatch, no scheduler (const-generic chain length) |
//! | `control-twin` | the SAME code again: the difference between it and `control` is the noise floor of the harness |
//! | `static` | the real `Operator` trait and the unfused kernels, `Vec<Box<StageOp>>` (concrete type, direct calls, monomorphised driver), reused stage buffers, no scheduler |
//! | `dyn-direct` | the same driver with `Vec<Box<dyn Operator>>`: isolates the virtual call |
//! | `dyn` | the real scheduler (`Query::run`) with one `Box<dyn Operator>` per stage: the protocol as it would be used |
//! | `static-in-sched` | the real scheduler with the whole chain as ONE operator whose stages are concrete (`Fused<L>`) |
//!
//! Differences read as: `static - control` = what the unfused operator shape costs (a materialised batch per stage);
//! `dyn-direct - static` = the virtual call; `dyn - dyn-direct` = the scheduler and its per-stage batches;
//! `static-in-sched - static` = the scheduler's fixed cost per batch.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::time::{Duration, Instant};

use crate::Query;
use crate::api::*;
use crate::batch::Batch;
use crate::kernels::*;
use crate::stages::*;

#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub kind: Kind,
    pub len: usize,
    pub batch: usize,
    pub threads: usize,
    pub total_rows: usize,
}

impl Cell {
    pub fn morsels(&self) -> usize {
        assert!(self.batch > 0 && self.total_rows % self.batch == 0, "total rows must be a multiple of the batch size");
        self.total_rows / self.batch
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arm {
    Control,
    ControlTwin,
    Static,
    DynDirect,
    Dyn,
    StaticInSched,
    /// The control plus a busy-wait of this many nanoseconds after every batch: only used by the harness self-test.
    ControlSpin(u64),
}

/// The arms of one chain cell, in the order they are reported. Index 0 is the control.
pub const CHAIN_ARMS: [Arm; 6] =
    [Arm::Control, Arm::ControlTwin, Arm::Static, Arm::DynDirect, Arm::Dyn, Arm::StaticInSched];

impl Arm {
    pub fn name(self) -> String {
        match self {
            Arm::Control => "control".into(),
            Arm::ControlTwin => "control-twin".into(),
            Arm::Static => "static".into(),
            Arm::DynDirect => "dyn-direct".into(),
            Arm::Dyn => "dyn".into(),
            Arm::StaticInSched => "static-in-sched".into(),
            Arm::ControlSpin(ns) => format!("control+spin{ns}ns"),
        }
    }
}

/// Busy-wait for `ns` nanoseconds of wall time (the injected cost of the harness self-test).
#[inline(never)]
pub fn spin_ns(ns: u64) {
    let t = Instant::now();
    let d = Duration::from_nanos(ns);
    while t.elapsed() < d {}
}

pub fn run_chain(arm: Arm, cell: &Cell, tmpl: &Arc<Vec<i64>>) -> Acc {
    let (kind, len) = (cell.kind, cell.len);
    match arm {
        Arm::Control | Arm::ControlTwin => run_control(cell, tmpl, 0),
        Arm::ControlSpin(ns) => run_control(cell, tmpl, ns),
        Arm::Static => run_direct(cell, tmpl, move || {
            (0..len).map(|i| Box::new(StageOp { kind, k: stage_const(i) })).collect::<Vec<Box<StageOp>>>()
        }),
        Arm::DynDirect => run_direct(cell, tmpl, move || {
            (0..len)
                .map(|i| Box::new(StageOp { kind, k: stage_const(i) }) as Box<dyn Operator>)
                .collect::<Vec<Box<dyn Operator>>>()
        }),
        Arm::Dyn => run_sched(cell, tmpl, dyn_ops(cell)),
        Arm::StaticInSched => run_sched(cell, tmpl, fused_ops(cell)),
    }
}

// ---------------------------------------------------------------- control

fn run_control(cell: &Cell, tmpl: &Arc<Vec<i64>>, spin: u64) -> Acc {
    let t: &[i64] = tmpl;
    match cell.len {
        1 => control::<1>(cell, t, spin),
        3 => control::<3>(cell, t, spin),
        6 => control::<6>(cell, t, spin),
        n => panic!("unsupported chain length {n}: the control and Fused are instantiated for 1, 3 and 6"),
    }
}

fn control<const L: usize>(cell: &Cell, tmpl: &[i64], spin: u64) -> Acc {
    let ks: [i64; L] = std::array::from_fn(stage_const);
    let (kind, morsels) = (cell.kind, cell.morsels());
    let counter = AtomicUsize::new(0);
    let (counter, ks) = (&counter, &ks);
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..cell.threads)
            .map(|_| {
                s.spawn(move || {
                    let mut acc = Acc::default();
                    loop {
                        let m = counter.fetch_add(1, SeqCst);
                        if m >= morsels {
                            break;
                        }
                        let vals = gen_morsel(tmpl, m);
                        match kind {
                            Kind::Cheap => fused_cheap(ks, &vals, &mut acc),
                            Kind::Medium => fused_medium(ks, &vals, &mut acc),
                        }
                        if spin > 0 {
                            spin_ns(spin);
                        }
                    }
                    acc
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("worker panicked")).fold(Acc::default(), Acc::merge)
    })
}

// ---------------------------------------------------------------- direct drivers (no scheduler)

/// A minimal push driver: source -> stage 0 -> ... -> stage n-1 -> sum, on the caller's thread. Each stage writes into
/// its own reused output batch. Generic over the operator type: `O = StageOp` is statically dispatched (and
/// monomorphised), `O = dyn Operator` costs one virtual call per stage per batch.
fn drive_direct<O: Operator + ?Sized>(ops: &mut [Box<O>], counter: &AtomicUsize, morsels: usize, tmpl: &[i64]) -> Acc {
    let n = ops.len();
    let mut bufs: Vec<Batch> = (0..n).map(|_| Batch::default()).collect();
    let mut acc = Acc::default();
    loop {
        let m = counter.fetch_add(1, SeqCst);
        if m >= morsels {
            break;
        }
        let first = Batch { cols: vec![gen_morsel(tmpl, m)] };
        for s in 0..n {
            let (done, rest) = bufs.split_at_mut(s);
            let input = if s == 0 { &first } else { &done[s - 1] };
            let r = ops[s].execute(input, &mut rest[0]).expect("operator failed");
            assert!(matches!(r, OpResult::NeedMoreInput), "these operators only ever need more input");
        }
        sum_batch(&bufs[n - 1], &mut acc);
    }
    acc
}

fn run_direct<O, F>(cell: &Cell, tmpl: &Arc<Vec<i64>>, mk: F) -> Acc
where
    O: Operator + ?Sized,
    F: Fn() -> Vec<Box<O>> + Sync,
{
    let morsels = cell.morsels();
    let counter = AtomicUsize::new(0);
    let (counter, mk, t) = (&counter, &mk, &tmpl[..]);
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..cell.threads)
            .map(|_| {
                s.spawn(move || {
                    // like the scheduler's per-task factories: every thread makes its own operators
                    let mut ops = mk();
                    drive_direct(&mut ops, counter, morsels, t)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("worker panicked")).fold(Acc::default(), Acc::merge)
    })
}

// ---------------------------------------------------------------- the real scheduler

fn dyn_ops(cell: &Cell) -> Vec<Arc<dyn OperatorFactory>> {
    (0..cell.len)
        .map(|i| Arc::new(StageFactory(StageOp { kind: cell.kind, k: stage_const(i) })) as Arc<dyn OperatorFactory>)
        .collect()
}

fn fused_ops(cell: &Cell) -> Vec<Arc<dyn OperatorFactory>> {
    let k = cell.kind;
    let f: Arc<dyn OperatorFactory> = match cell.len {
        1 => Arc::new(FusedFactory::<1>(k)),
        3 => Arc::new(FusedFactory::<3>(k)),
        6 => Arc::new(FusedFactory::<6>(k)),
        n => panic!("unsupported chain length {n}: the control and Fused are instantiated for 1, 3 and 6"),
    };
    vec![f]
}

fn run_sched(cell: &Cell, tmpl: &Arc<Vec<i64>>, ops: Vec<Arc<dyn OperatorFactory>>) -> Acc {
    let sink = SumSink::default();
    let p = Pipeline {
        name: "push-cost".into(),
        source: Arc::new(MorselSource::new(Arc::clone(tmpl), cell.morsels())),
        ops,
        sink: Arc::new(sink.clone()),
        deps: vec![],
        tasks: cell.threads,
    };
    Query::new(vec![p]).run(cell.threads).expect("query failed");
    sink.result()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spin_takes_at_least_what_it_was_asked() {
        for ns in [0u64, 100, 1_000, 20_000] {
            let t = Instant::now();
            spin_ns(ns);
            assert!(t.elapsed() >= Duration::from_nanos(ns));
        }
    }

    #[test]
    fn arm_names_are_distinct() {
        let mut names: Vec<String> = CHAIN_ARMS.iter().map(|a| a.name()).collect();
        names.push(Arm::ControlSpin(100).name());
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 7);
    }
}
