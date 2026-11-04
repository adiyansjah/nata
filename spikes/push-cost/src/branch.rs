//! The branching topology: ONE producer feeding TWO consumers through an exchange.
//!
//! ```text
//!   producer pipeline (1 task):   morsel source -> stage 0 [-> compact, medium kernel only] -> exchange sink
//!   consumer pipeline (2 tasks):  exchange source -> stage 1 -> stage 2 -> sum sink
//! ```
//!
//! Both pipelines run at the same time on 3 worker threads (`proto`: the real scheduler and the real exchange of
//! `spikes/operator-trait`, tasks parked and woken through wakers). The hand-written equivalent (`hand`) is 3 OS threads
//! and a bounded queue made of a `Mutex` and two `Condvar`s, calling the same kernels directly. The exchange hands each
//! batch to exactly one consumer task (an MPMC queue, as in the prototype), so the two consumers split the batches.
//! The checksum equals the linear chain of 3 stages over every morsel.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use crate::Query;
use crate::api::*;
use crate::batch::Batch;
use crate::kernels::*;
use crate::ops::exchange;
use crate::stages::*;

/// Worker threads of the branching topology: 1 producer + 2 consumers.
pub const BRANCH_THREADS: usize = 3;
pub const BRANCH_LEN: usize = 3;

#[derive(Clone, Copy, Debug)]
pub struct BCell {
    pub kind: Kind,
    pub batch: usize,
    /// Capacity of the exchange (batches in flight between the producer and the consumers).
    pub cap: usize,
    pub total_rows: usize,
}

impl BCell {
    pub fn morsels(&self) -> usize {
        assert!(self.batch > 0 && self.total_rows % self.batch == 0, "total rows must be a multiple of the batch size");
        self.total_rows / self.batch
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BArm {
    Hand,
    HandTwin,
    Proto,
}

pub const BRANCH_ARMS: [BArm; 3] = [BArm::Hand, BArm::HandTwin, BArm::Proto];

impl BArm {
    pub fn name(self) -> &'static str {
        match self {
            BArm::Hand => "hand",
            BArm::HandTwin => "hand-twin",
            BArm::Proto => "proto",
        }
    }
}

pub fn run_branch(arm: BArm, cell: &BCell, tmpl: &Arc<Vec<i64>>) -> Acc {
    match arm {
        BArm::Hand | BArm::HandTwin => run_hand(cell, tmpl),
        BArm::Proto => run_proto(cell, tmpl),
    }
}

// ---------------------------------------------------------------- the protocol version

fn run_proto(cell: &BCell, tmpl: &Arc<Vec<i64>>) -> Acc {
    let kind = cell.kind;
    let (xsink, xsource, _chan) = exchange(cell.cap, 1, 2);
    let mut producer_ops: Vec<Arc<dyn OperatorFactory>> =
        vec![Arc::new(StageFactory(StageOp { kind, k: stage_const(0) }))];
    if kind == Kind::Medium {
        producer_ops.push(Arc::new(CompactFactory));
    }
    let producer = Pipeline {
        name: "producer".into(),
        source: Arc::new(MorselSource::new(Arc::clone(tmpl), cell.morsels())),
        ops: producer_ops,
        sink: xsink,
        deps: vec![],
        tasks: 1,
    };
    let sink = SumSink::default();
    let consumers = Pipeline {
        name: "consumers".into(),
        source: xsource,
        ops: vec![
            Arc::new(StageFactory(StageOp { kind, k: stage_const(1) })),
            Arc::new(StageFactory(StageOp { kind, k: stage_const(2) })),
        ],
        sink: Arc::new(sink.clone()),
        deps: vec![],
        tasks: 2,
    };
    Query::new(vec![producer, consumers]).run(BRANCH_THREADS).expect("query failed");
    sink.result()
}

// ---------------------------------------------------------------- the hand-written version

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A bounded multi-consumer queue: the simplest correct thing one would write by hand.
struct HandQueue {
    state: Mutex<(VecDeque<Batch>, bool)>,
    not_empty: Condvar,
    not_full: Condvar,
    cap: usize,
}

impl HandQueue {
    fn new(cap: usize) -> Self {
        HandQueue {
            state: Mutex::new((VecDeque::new(), false)),
            not_empty: Condvar::new(),
            not_full: Condvar::new(),
            cap,
        }
    }

    fn push(&self, b: Batch) {
        let mut g = lock(&self.state);
        while g.0.len() >= self.cap {
            g = self.not_full.wait(g).unwrap_or_else(|e| e.into_inner());
        }
        g.0.push_back(b);
        drop(g);
        self.not_empty.notify_one();
    }

    /// `None` once the queue is closed and empty.
    fn pop(&self) -> Option<Batch> {
        let mut g = lock(&self.state);
        loop {
            if let Some(b) = g.0.pop_front() {
                drop(g);
                self.not_full.notify_one();
                return Some(b);
            }
            if g.1 {
                return None;
            }
            g = self.not_empty.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn close(&self) {
        lock(&self.state).1 = true;
        self.not_empty.notify_all();
    }
}

/// Closes the queue when the producer ends, even by a panic, so the consumers never wait for ever.
struct CloseOnDrop<'a>(&'a HandQueue);

impl Drop for CloseOnDrop<'_> {
    fn drop(&mut self) {
        self.0.close();
    }
}

fn run_hand(cell: &BCell, tmpl: &Arc<Vec<i64>>) -> Acc {
    let kind = cell.kind;
    let morsels = cell.morsels();
    let q = HandQueue::new(cell.cap);
    let (q, t) = (&q, &tmpl[..]);
    std::thread::scope(|s| {
        let producer = s.spawn(move || {
            let _close = CloseOnDrop(q);
            let mut stage0 = StageOp { kind, k: stage_const(0) };
            for m in 0..morsels {
                let first = Batch { cols: vec![gen_morsel(t, m)] };
                let mut out = Batch::default();
                stage0.execute(&first, &mut out).expect("operator failed");
                if kind == Kind::Medium {
                    let mut dense = Batch::default();
                    CompactOp.execute(&out, &mut dense).expect("operator failed");
                    out = dense;
                }
                if !out.cols[0].is_empty() {
                    q.push(out);
                }
            }
        });
        let consumers: Vec<_> = (0..2)
            .map(|_| {
                s.spawn(move || {
                    let mut stages = [StageOp { kind, k: stage_const(1) }, StageOp { kind, k: stage_const(2) }];
                    let mut bufs = [Batch::default(), Batch::default()];
                    let mut acc = Acc::default();
                    while let Some(b) = q.pop() {
                        let [b0, b1] = &mut bufs;
                        stages[0].execute(&b, b0).expect("operator failed");
                        stages[1].execute(b0, b1).expect("operator failed");
                        sum_batch(b1, &mut acc);
                    }
                    acc
                })
            })
            .collect();
        producer.join().expect("producer panicked");
        consumers.into_iter().map(|h| h.join().expect("consumer panicked")).fold(Acc::default(), Acc::merge)
    })
}
