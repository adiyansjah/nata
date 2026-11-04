//! Model check (loom) of the tokio -> CPU-pool hand-off.
//!
//! What runs here is the REAL code, not a hand-written model of it: `chan.rs` (channel + waker
//! slot) and `sched.rs` (the snapshotted scheduler: task state machine, queue, workers, shutdown),
//! compiled against loom's Mutex/Condvar/atomics/threads via `src/sync.rs` (`--cfg nata_loom`).
//! What IS a stand-in: the tokio side. A loom thread plays "a tokio worker running a producer
//! task": it polls the real `SendFuture` with a waker backed by a loom parker, like a tokio task
//! would (tokio's own scheduler and timer are not modelled; its contract used here is only
//! "poll again after the stored std waker is woken, from any thread").
//!
//! Run: RUSTFLAGS="--cfg nata_loom" CARGO_TARGET_DIR=target/loom \
//!      cargo test --release --test loom_bridge -- --test-threads=1
#![cfg(nata_loom)]

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake};

use loom::model::Builder;
use loom::thread;
use spike_fairness::chan::{ChanSources, Chunk, channel};
use spike_fairness::sync::{Condvar, Mutex};
use spike_fairness::*;

/// A minimal `block_on`: poll, and when `Pending` park on a loom condvar until the stored waker fires.
struct Parker {
    woken: Mutex<bool>,
    cv: Condvar,
}

impl Wake for Parker {
    fn wake(self: Arc<Self>) {
        *self.woken.lock().unwrap() = true;
        self.cv.notify_one();
    }
}

fn block_on<F: Future>(fut: F) -> F::Output {
    let parker = Arc::new(Parker {
        woken: Mutex::new(false),
        cv: Condvar::new(),
    });
    let waker = std::task::Waker::from(Arc::clone(&parker));
    let mut cx = Context::from_waker(&waker);
    let mut fut = pin!(fut);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        let mut w = parker.woken.lock().unwrap();
        while !*w {
            w = parker.cv.wait(w).unwrap();
        }
        *w = false;
    }
}

struct Collect(Arc<Mutex<Vec<i64>>>);

struct CollectLocal(Arc<Mutex<Vec<i64>>>);

impl GlobalSink for Collect {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        Box::new(CollectLocal(Arc::clone(&self.0)))
    }
    fn finalize(&self) -> Result<()> {
        Ok(())
    }
}

impl LocalSink for CollectLocal {
    fn push(&mut self, input: &mut Batch, _w: &Waker) -> Result<SinkResult> {
        self.0.lock().unwrap().extend_from_slice(&input.cols[0]);
        Ok(SinkResult::NeedMoreInput)
    }
    fn combine(self: Box<Self>) -> Result<()> {
        Ok(())
    }
}

fn bound(default: usize) -> Option<usize> {
    match std::env::var("NATA_LOOM_PREEMPTIONS") {
        Ok(v) if v == "none" => None,
        Ok(v) => Some(v.parse().unwrap()),
        Err(_) => Some(default),
    }
}

/// One model: `workers` pool threads, one consumer task over a capacity-`cap` channel, one
/// producer thread sending `items` chunks then dropping its sender. Optionally `cancel` is
/// called from the main thread, racing with both.
fn model(workers: usize, cap: usize, items: i64, cancel: bool, preemptions: usize) {
    model_p(workers, cap, items, 1, cancel, preemptions)
}

/// Same with `producers` sender threads; producer `j` sends `100*j + 0..items`.
fn model_p(
    workers: usize,
    cap: usize,
    items: i64,
    producers: usize,
    cancel: bool,
    preemptions: usize,
) {
    let mut b = Builder::new();
    b.preemption_bound = bound(preemptions);
    b.max_branches = 1_000_000;
    let executions = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let ex = std::sync::Arc::clone(&executions);
    b.check(move || {
        ex.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = channel::<Chunk>(cap);
        let got = Arc::new(Mutex::new(Vec::new()));
        let pipe = Pipeline {
            name: "consume".into(),
            source: Arc::new(ChanSources::new(vec![rx])),
            ops: vec![],
            sink: Arc::new(Collect(Arc::clone(&got))),
            deps: vec![],
            tasks: 1,
        };
        let rq = Query::new(vec![pipe]).start(workers).unwrap();
        let mut prods = Vec::new();
        let mut txs = vec![tx];
        for _ in 1..producers {
            let c = txs[0].clone();
            txs.push(c);
        }
        for (j, tx) in txs.into_iter().enumerate() {
            prods.push(thread::spawn(move || {
                for i in 0..items {
                    if block_on(tx.send(Chunk::bare(100 * j as u64 + i as u64))).is_err() {
                        break; // receiver closed (cancel): stop, drop the sender
                    }
                }
            }));
        }
        if cancel {
            rq.cancel();
        }
        let r = rq.wait();
        for p in prods {
            p.join().unwrap(); // a producer stuck in send() after cancel would deadlock here
        }
        let got = got.lock().unwrap().clone();
        // per producer: what arrived is a prefix of what it sent, in order
        for j in 0..producers as i64 {
            let mine: Vec<i64> = got
                .iter()
                .copied()
                .filter(|x| x / 100 == j)
                .map(|x| x % 100)
                .collect();
            let all: Vec<i64> = (0..items).collect();
            if cancel {
                assert!(
                    all.starts_with(&mine),
                    "cancel: producer {j} delivered {mine:?}, not a prefix"
                );
            } else {
                assert_eq!(mine, all, "lost or reordered items of producer {j}");
            }
        }
        if cancel {
            assert!(r.is_ok() || r.as_ref().unwrap_err().0 == "cancelled");
        } else {
            assert!(r.is_ok(), "{r:?}");
        }
    });
    eprintln!(
        "MODEL workers={workers} cap={cap} items={items} producers={producers} cancel={cancel} preemption_bound={:?}: {} interleavings explored, all passed",
        bound(preemptions),
        executions.load(std::sync::atomic::Ordering::Relaxed)
    );
}

// ---- one pool worker: the send races with empty-check / store-waker / Blocked / park

#[test]
fn w1_cap1_two_items() {
    model(1, 1, 2, false, 5);
}

#[test]
fn w1_cap2_one_item() {
    model(1, 2, 1, false, 5);
}

/// Producer closes without sending: the close races with the same sequence.
#[test]
fn w1_close_without_items() {
    model(1, 1, 0, false, 5);
}

#[test]
fn w1_cancel_races_with_producer() {
    model(1, 1, 2, true, 4);
}

/// Two senders: sender count, wake-all of blocked senders, and the last sender's drop as the close.
#[test]
fn w1_two_producers_cap1() {
    model_p(1, 1, 1, 2, false, 3);
}

#[test]
fn w1_two_producers_cancel() {
    model_p(1, 1, 2, 2, true, 2);
}

// ---- two pool workers: also checks a task is never run by two workers at once

#[test]
fn w2_cap1_one_item() {
    model(2, 1, 1, false, 3);
}

#[test]
fn w2_close_without_items() {
    model(2, 1, 0, false, 3);
}

#[test]
fn w2_cancel_races_with_producer() {
    model(2, 1, 1, true, 3);
}

// =====================================================================================
// Fairness spike: models of the yield / re-queue path (slice of 1 morsel, so every morsel
// boundary at which another task is queued is a yield).
// =====================================================================================

use std::sync::atomic::{AtomicUsize as StdAtomicUsize, Ordering::SeqCst as StdSeqCst};

/// Source: `n` batches `[100 * task + i]`; task ids are handed out in `make()` order.
struct Gen {
    n: i64,
    next_task: StdAtomicUsize,
}

struct GenSource {
    base: i64,
    i: i64,
    n: i64,
}

impl SourceFactory for Gen {
    fn make(&self) -> Box<dyn Source> {
        let t = self.next_task.fetch_add(1, StdSeqCst) as i64;
        Box::new(GenSource {
            base: 100 * t,
            i: 0,
            n: self.n,
        })
    }
}

impl Source for GenSource {
    fn next(&mut self, _w: &Waker) -> Result<SourceResult> {
        if self.i == self.n {
            return Ok(SourceResult::Finished);
        }
        self.i += 1;
        Ok(SourceResult::Batch(Batch::from_cols(vec![vec![
            self.base + self.i - 1,
        ]])))
    }
}

/// Executions (over all models of one test run) in which at least one yield happened. The models
/// assert at the end that this is non-zero, so a model that never reaches the yield path fails.
static YIELDED: StdAtomicUsize = StdAtomicUsize::new(0);
static EXECUTIONS: StdAtomicUsize = StdAtomicUsize::new(0);

fn builder(preemptions: usize) -> Builder {
    let mut b = Builder::new();
    b.preemption_bound = bound(preemptions);
    b.max_branches = 1_000_000;
    b
}

fn report(name: &str, preemptions: usize) {
    let ex = EXECUTIONS.load(StdSeqCst);
    let y = YIELDED.load(StdSeqCst);
    eprintln!(
        "MODEL {name} preemption_bound={:?}: {ex} interleavings explored, {y} of them with a yield; all passed",
        bound(preemptions)
    );
    assert!(y > 0, "{name}: no explored interleaving reached the yield path");
}

/// `tasks` tasks of one pipeline, `morsels` each, slice of one morsel, `workers` threads. The main
/// thread optionally cancels. Checks: terminates; every task's output is exactly its input in order
/// (or, on cancel, a prefix); nothing lost or duplicated.
fn yield_model(workers: usize, tasks: usize, morsels: i64, cancel: bool, preemptions: usize) {
    builder(preemptions).check(move || {
        EXECUTIONS.fetch_add(1, StdSeqCst);
        let got = Arc::new(Mutex::new(Vec::new()));
        let pipe = Pipeline {
            name: "gen".into(),
            source: Arc::new(Gen {
                n: morsels,
                next_task: StdAtomicUsize::new(0),
            }),
            ops: vec![],
            sink: Arc::new(Collect(Arc::clone(&got))),
            deps: vec![],
            tasks,
        };
        let rq = Query::new(vec![pipe])
            .with_slice(SliceConfig::morsels(1))
            .start(workers)
            .unwrap();
        if cancel {
            rq.cancel();
        }
        let (r, yields) = rq.wait_with_stats();
        if yields > 0 {
            YIELDED.fetch_add(1, StdSeqCst);
        }
        let got = got.lock().unwrap().clone();
        for t in 0..tasks as i64 {
            let mine: Vec<i64> = got.iter().copied().filter(|x| x / 100 == t).collect();
            let all: Vec<i64> = (0..morsels).map(|i| 100 * t + i).collect();
            if cancel {
                assert!(all.starts_with(&mine), "task {t}: {mine:?} is not a prefix of {all:?}");
            } else {
                assert_eq!(mine, all, "task {t}: lost, duplicated or reordered items");
            }
        }
        if cancel {
            assert!(r.is_ok() || r.as_ref().unwrap_err().0 == "cancelled", "{r:?}");
        } else {
            assert!(r.is_ok(), "{r:?}");
        }
    });
}

/// One worker, two tasks, two morsels each: yield, re-queue at the back, run the other task.
#[test]
fn y_w1_two_tasks() {
    EXECUTIONS.store(0, StdSeqCst);
    YIELDED.store(0, StdSeqCst);
    yield_model(1, 2, 2, false, 5);
    report("y_w1_two_tasks (w1, 2 tasks x 2 morsels)", 5);
}

/// Two workers, two tasks, two morsels each: a task is never on two workers, never started when
/// not Queued, and every item arrives exactly once.
#[test]
fn y_w2_two_tasks() {
    EXECUTIONS.store(0, StdSeqCst);
    YIELDED.store(0, StdSeqCst);
    yield_model(2, 2, 2, false, 3);
    report("y_w2_two_tasks (w2, 2 tasks x 2 morsels)", 3);
}

/// Cancel races with yield / re-queue, one worker.
#[test]
fn y_w1_cancel_races_with_yield() {
    EXECUTIONS.store(0, StdSeqCst);
    YIELDED.store(0, StdSeqCst);
    yield_model(1, 2, 2, true, 4);
    report("y_w1_cancel_races_with_yield (w1, 2 tasks x 2 morsels, cancel)", 4);
}

/// Cancel races with yield / re-queue, two workers.
#[test]
fn y_w2_cancel_races_with_yield() {
    EXECUTIONS.store(0, StdSeqCst);
    YIELDED.store(0, StdSeqCst);
    yield_model(2, 2, 2, true, 3);
    report("y_w2_cancel_races_with_yield (w2, 2 tasks x 2 morsels, cancel)", 3);
}

/// A consumer task over the bridge channel next to a second task that yields. The producer's wake
/// of the consumer races with the consumer yielding, being re-queued, running and blocking.
/// `cancel` adds a cancel from the main thread.
fn consumer_model(workers: usize, items: i64, cancel: bool, preemptions: usize) {
    builder(preemptions).check(move || {
        EXECUTIONS.fetch_add(1, StdSeqCst);
        let (tx, rx) = channel::<Chunk>(1);
        let got = Arc::new(Mutex::new(Vec::new()));
        let consume = Pipeline {
            name: "consume".into(),
            source: Arc::new(ChanSources::new(vec![rx])),
            ops: vec![],
            sink: Arc::new(Collect(Arc::clone(&got))),
            deps: vec![],
            tasks: 1,
        };
        let other = Pipeline {
            name: "other".into(),
            source: Arc::new(Gen {
                n: 2,
                next_task: StdAtomicUsize::new(1), // ids 100.. so the outputs are told apart
            }),
            ops: vec![],
            sink: Arc::new(Collect(Arc::new(Mutex::new(Vec::new())))),
            deps: vec![],
            tasks: 1,
        };
        let rq = Query::new(vec![other, consume])
            .with_slice(SliceConfig::morsels(1))
            .start(workers)
            .unwrap();
        let prod = thread::spawn(move || {
            for i in 0..items {
                if block_on(tx.send(Chunk::bare(i as u64))).is_err() {
                    break;
                }
            }
        });
        if cancel {
            rq.cancel();
        }
        let (r, yields) = rq.wait_with_stats();
        prod.join().unwrap();
        if yields > 0 {
            YIELDED.fetch_add(1, StdSeqCst);
        }
        let got = got.lock().unwrap().clone();
        let all: Vec<i64> = (0..items).collect();
        if cancel {
            assert!(all.starts_with(&got), "{got:?} is not a prefix of {all:?}");
            assert!(r.is_ok() || r.as_ref().unwrap_err().0 == "cancelled", "{r:?}");
        } else {
            assert_eq!(got, all, "lost or reordered items");
            assert!(r.is_ok(), "{r:?}");
        }
    });
}

#[test]
fn y_w1_consumer_with_yielding_neighbour() {
    EXECUTIONS.store(0, StdSeqCst);
    YIELDED.store(0, StdSeqCst);
    consumer_model(1, 2, false, 3);
    report("y_w1_consumer_with_yielding_neighbour (w1, cap 1, 2 items)", 3);
}

#[test]
fn y_w1_consumer_cancel() {
    EXECUTIONS.store(0, StdSeqCst);
    YIELDED.store(0, StdSeqCst);
    consumer_model(1, 2, true, 3);
    report("y_w1_consumer_cancel (w1, cap 1, 2 items, cancel)", 3);
}

#[test]
fn y_w2_consumer_with_yielding_neighbour() {
    EXECUTIONS.store(0, StdSeqCst);
    YIELDED.store(0, StdSeqCst);
    consumer_model(2, 1, false, 3);
    report("y_w2_consumer_with_yielding_neighbour (w2, cap 1, 1 item)", 3);
}
