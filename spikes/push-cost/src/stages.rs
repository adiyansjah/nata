//! The pieces that plug the kernels into the REAL protocol of `spikes/operator-trait`: a morsel source, operator
//! factories, a summing sink, and a chain whose stages are monomorphised (`Fused<L>`).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::SeqCst};

use crate::api::*;
use crate::batch::Batch;
use crate::kernels::*;

// ---------------------------------------------------------------- source

/// `total` morsels shared by all tasks of the pipeline; each `next` takes the next unclaimed one (one atomic per batch,
/// the same as the hand-written arms).
pub struct MorselSource {
    next: Arc<AtomicUsize>,
    total: usize,
    tmpl: Arc<Vec<i64>>,
}

impl MorselSource {
    pub fn new(tmpl: Arc<Vec<i64>>, total: usize) -> Self {
        MorselSource { next: Arc::new(AtomicUsize::new(0)), total, tmpl }
    }
}

struct MorselLocal {
    next: Arc<AtomicUsize>,
    total: usize,
    tmpl: Arc<Vec<i64>>,
}

impl SourceFactory for MorselSource {
    fn make(&self) -> Box<dyn Source> {
        Box::new(MorselLocal { next: Arc::clone(&self.next), total: self.total, tmpl: Arc::clone(&self.tmpl) })
    }
}

impl Source for MorselLocal {
    fn next(&mut self, _w: &Waker) -> Result<SourceResult> {
        let m = self.next.fetch_add(1, SeqCst);
        if m >= self.total {
            return Ok(SourceResult::Finished);
        }
        Ok(SourceResult::Batch(Batch { cols: vec![gen_morsel(&self.tmpl, m)] }))
    }
}

// ---------------------------------------------------------------- operators

/// One chain stage per scheduler stage (`Box<dyn Operator>` per stage: the arm called `dyn`).
pub struct StageFactory(pub StageOp);

impl OperatorFactory for StageFactory {
    fn make(&self) -> Box<dyn Operator> {
        Box::new(self.0)
    }
}

pub struct CompactFactory;

impl OperatorFactory for CompactFactory {
    fn make(&self) -> Box<dyn Operator> {
        Box::new(CompactOp)
    }
}

/// The whole chain of `L` stages as ONE operator whose stages are concrete (monomorphised, statically dispatched, with
/// its own reused intermediate buffers). Registered as a single stage it gives the scheduler exactly one virtual call and
/// one output batch per input batch: the closest the protocol allows to static dispatch (the scheduler itself holds
/// `Vec<Box<dyn Operator>>` and is not generic over the operator types).
pub struct Fused<const L: usize> {
    ops: [StageOp; L],
    bufs: Vec<Batch>,
}

impl<const L: usize> Fused<L> {
    pub fn new(kind: Kind) -> Self {
        const { assert!(L >= 1) };
        Fused {
            ops: std::array::from_fn(|i| StageOp { kind, k: stage_const(i) }),
            bufs: (1..L).map(|_| Batch::default()).collect(),
        }
    }
}

impl<const L: usize> Operator for Fused<L> {
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        if L == 1 {
            return self.ops[0].execute(input, out);
        }
        self.ops[0].execute(input, &mut self.bufs[0])?;
        for s in 1..L - 1 {
            let (a, b) = self.bufs.split_at_mut(s);
            self.ops[s].execute(&a[s - 1], &mut b[0])?;
        }
        self.ops[L - 1].execute(&self.bufs[L.saturating_sub(2)], out)
    }
}

pub struct FusedFactory<const L: usize>(pub Kind);

impl<const L: usize> OperatorFactory for FusedFactory<L> {
    fn make(&self) -> Box<dyn Operator> {
        Box::new(Fused::<L>::new(self.0))
    }
}

// ---------------------------------------------------------------- sink

#[derive(Default)]
struct SumInner {
    sum: AtomicU64,
    count: AtomicU64,
}

/// Adds the (selected) values of every batch into a checksum; `combine` merges the task's local checksum.
#[derive(Clone, Default)]
pub struct SumSink(Arc<SumInner>);

impl SumSink {
    pub fn result(&self) -> Acc {
        Acc { sum: self.0.sum.load(SeqCst), count: self.0.count.load(SeqCst) }
    }
}

struct SumLocal {
    global: Arc<SumInner>,
    acc: Acc,
}

impl GlobalSink for SumSink {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        Box::new(SumLocal { global: Arc::clone(&self.0), acc: Acc::default() })
    }

    fn finalize(&self) -> Result<()> {
        Ok(())
    }
}

impl LocalSink for SumLocal {
    fn push(&mut self, input: &mut Batch, _w: &Waker) -> Result<SinkResult> {
        sum_batch(input, &mut self.acc);
        Ok(SinkResult::NeedMoreInput)
    }

    fn combine(self: Box<Self>) -> Result<()> {
        // fetch_add wraps on overflow, which is what the checksum wants
        self.global.sum.fetch_add(self.acc.sum, SeqCst);
        self.global.count.fetch_add(self.acc.count, SeqCst);
        Ok(())
    }
}
