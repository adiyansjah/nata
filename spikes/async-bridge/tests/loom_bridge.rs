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
use spike_async_bridge::chan::{ChanSources, Chunk, channel};
use spike_async_bridge::sync::{Condvar, Mutex};
use spike_async_bridge::*;

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
