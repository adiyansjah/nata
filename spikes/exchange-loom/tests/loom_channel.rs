//! Scheduler-free models of single exchange operations, each aimed at one property (so a mutant of
//! that operation is caught by a test that is about it, not only by a whole-query model).
//! They drive the REAL `exchange()` sink and source directly with counting wakers.
//!
//! x1-x4, x7 are sequential (deterministic: no interleaving is needed to show the property; they are
//! run inside `model` so the same code is also a one-execution loom model). x5 and x6 are real
//! races between two loom threads. Same file runs as a std stress loop without `--cfg nata_loom`.

#[cfg(nata_loom)]
use std::sync::Arc as StdArc;
#[cfg(nata_loom)]
use std::sync::atomic::{AtomicUsize as StdAtomic, Ordering::Relaxed};

use spike_exchange_loom::sync::{Arc, AtomicBool, AtomicUsize, thread};
use spike_exchange_loom::sync::Ordering::SeqCst;
use spike_exchange_loom::*;

#[cfg(nata_loom)]
fn model(name: &str, bound: Option<usize>, f: impl Fn() + Sync + Send + 'static) {
    let runs = StdArc::new(StdAtomic::new(0));
    let r = StdArc::clone(&runs);
    let mut b = loom::model::Builder::new();
    if b.preemption_bound.is_none() {
        b.preemption_bound = bound;
    }
    b.check(move || {
        r.fetch_add(1, Relaxed);
        f();
    });
    eprintln!("[loom] {name}: {} executions, preemption bound {:?}", runs.load(Relaxed), b.preemption_bound);
}

#[cfg(not(nata_loom))]
fn model(name: &str, _bound: Option<usize>, f: impl Fn() + Sync + Send + 'static) {
    let n: usize = std::env::var("EXL_STRESS").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
    for _ in 0..n {
        f();
    }
    eprintln!("[stress] {name}: {n} runs with std threads");
}

fn batch(v: i64) -> Batch {
    Batch::from_cols(vec![vec![v]])
}

/// A waker that counts how often it was fired.
fn counting_waker() -> (Waker, Arc<AtomicUsize>) {
    let n = Arc::new(AtomicUsize::new(0));
    let m = Arc::clone(&n);
    (Waker::new(move || { m.fetch_add(1, SeqCst); }), n)
}

fn push(s: &mut Box<dyn LocalSink>, v: i64, w: &Waker) -> Option<bool> {
    // Some(true) accepted, Some(false) blocked (batch must be untouched), None finished.
    let mut b = batch(v);
    match s.push(&mut b, w).unwrap() {
        SinkResult::NeedMoreInput => { assert!(b.is_empty(), "accepted batch must be taken"); Some(true) }
        SinkResult::Blocked => { assert_eq!(b, batch(v), "a blocked push must leave the batch untouched"); Some(false) }
        SinkResult::Finished => None,
    }
}

fn pop(s: &mut Box<dyn Source>, w: &Waker) -> Option<Option<i64>> {
    // Some(Some(v)) batch, Some(None) blocked, None finished.
    match s.next(w).unwrap() {
        SourceResult::Batch(b) => Some(Some(b.row(0)[0])),
        SourceResult::Blocked => Some(None),
        SourceResult::Finished => None,
    }
}

/// x1: FIFO. Capacity 3, three pushes queue up, the pops return them oldest first.
#[test]
fn x1_fifo_order_with_capacity_3() {
    model("x1_fifo_order_with_capacity_3", None, || {
        let (xs, xr, _c) = exchange(3, 1, 1);
        let (mut p, mut c) = (xs.make(0), xr.make());
        let (w, _) = counting_waker();
        for v in 0..3 {
            assert_eq!(push(&mut p, v, &w), Some(true));
        }
        for v in 0..3 {
            assert_eq!(pop(&mut c, &w), Some(Some(v)), "items must come out in the order they went in");
        }
    });
}

/// x2: the capacity is exact: `cap` pushes are accepted, the next is `Blocked` (and leaves the batch
/// alone), one pop frees exactly one slot. For capacity 1 and 2.
#[test]
fn x2_capacity_is_exact() {
    model("x2_capacity_is_exact", None, || {
        for cap in 1..=2usize {
            let (xs, xr, c) = exchange(cap, 1, 1);
            let (mut p, mut r) = (xs.make(0), xr.make());
            let (w, _) = counting_waker();
            for v in 0..cap {
                assert_eq!(push(&mut p, v as i64, &w), Some(true), "cap {cap}: push {v} must fit");
            }
            assert_eq!(push(&mut p, 99, &w), Some(false), "cap {cap}: push {cap} must block");
            assert_eq!(c.queue_len(), cap);
            assert_eq!(pop(&mut r, &w), Some(Some(0)));
            assert_eq!(push(&mut p, 99, &w), Some(true), "cap {cap}: one pop frees one slot");
            assert_eq!(push(&mut p, 100, &w), Some(false), "cap {cap}: and only one");
        }
    });
}

/// x3: the consumer's close (last `Source::finish`) fires the waker of EVERY producer blocked on the
/// full exchange, exactly once each, and later pushes see `Finished`.
#[test]
fn x3_close_wakes_every_blocked_producer() {
    model("x3_close_wakes_every_blocked_producer", None, || {
        let (xs, xr, _c) = exchange(1, 2, 1);
        let (mut a, mut b) = (xs.make(0), xs.make(1));
        let mut r = xr.make();
        let (wa, na) = counting_waker();
        let (wb, nb) = counting_waker();
        assert_eq!(push(&mut a, 0, &wa), Some(true));
        assert_eq!(push(&mut a, 1, &wa), Some(false));
        assert_eq!(push(&mut b, 2, &wb), Some(false));
        r.finish();
        assert_eq!(na.load(SeqCst), 1, "producer A was not woken by the close");
        assert_eq!(nb.load(SeqCst), 1, "producer B was not woken by the close");
        assert_eq!(push(&mut a, 1, &wa), None, "a closed exchange must answer Finished");
        assert_eq!(push(&mut b, 2, &wb), None);
    });
}

/// x4: a push fires the waker of EVERY consumer blocked on the empty exchange; so does the last
/// producer finishing (`combine`).
#[test]
fn x4_push_and_producer_done_wake_every_blocked_consumer() {
    model("x4_push_and_producer_done_wake_every_blocked_consumer", None, || {
        // push
        let (xs, xr, _c) = exchange(1, 1, 2);
        let mut p = xs.make(0);
        let (mut r1, mut r2) = (xr.make(), xr.make());
        let (w1, n1) = counting_waker();
        let (w2, n2) = counting_waker();
        assert_eq!(pop(&mut r1, &w1), Some(None));
        assert_eq!(pop(&mut r2, &w2), Some(None));
        assert_eq!(push(&mut p, 7, &w1), Some(true));
        assert_eq!((n1.load(SeqCst), n2.load(SeqCst)), (1, 1), "a push must wake both blocked consumers");
        // producer done
        let (xs, xr, _c) = exchange(1, 1, 2);
        let p = xs.make(0);
        let (mut r1, mut r2) = (xr.make(), xr.make());
        let (w1, n1) = counting_waker();
        let (w2, n2) = counting_waker();
        assert_eq!(pop(&mut r1, &w1), Some(None));
        assert_eq!(pop(&mut r2, &w2), Some(None));
        p.combine().unwrap();
        assert_eq!((n1.load(SeqCst), n2.load(SeqCst)), (1, 1), "the last producer finishing must wake both");
        assert_eq!(pop(&mut r1, &w1), None);
    });
}

/// x7: a pop fires the waker of EVERY producer blocked on the full exchange.
#[test]
fn x7_pop_wakes_every_blocked_producer() {
    model("x7_pop_wakes_every_blocked_producer", None, || {
        let (xs, xr, _c) = exchange(1, 2, 1);
        let (mut a, mut b) = (xs.make(0), xs.make(1));
        let mut r = xr.make();
        let (wa, na) = counting_waker();
        let (wb, nb) = counting_waker();
        assert_eq!(push(&mut a, 0, &wa), Some(true));
        assert_eq!(push(&mut a, 1, &wa), Some(false));
        assert_eq!(push(&mut b, 2, &wb), Some(false));
        assert_eq!(pop(&mut r, &wa), Some(Some(0)));
        assert_eq!((na.load(SeqCst), nb.load(SeqCst)), (1, 1), "a pop must wake both blocked producers");
    });
}

/// x5 (race): a producer blocked on the full exchange vs. the consumer's close. In every
/// interleaving: either the producer saw the close at once (`Finished`), or it was `Blocked` and its
/// waker has been fired by the time both sides are done. (No blocking, so no deadlock to look for:
/// the assertion IS the lost-wake-up check.)
#[test]
fn x5_race_blocked_producer_vs_consumer_close() {
    model("x5_race_blocked_producer_vs_consumer_close", Some(4), || {
        let (xs, xr, _c) = exchange(1, 1, 1);
        let mut p = xs.make(0);
        let mut r = xr.make();
        let (w0, _) = counting_waker();
        assert_eq!(push(&mut p, 0, &w0), Some(true)); // the exchange is now full
        let woken = Arc::new(AtomicUsize::new(0));
        let blocked = Arc::new(AtomicBool::new(false));
        let (wk, bl) = (Arc::clone(&woken), Arc::clone(&blocked));
        let producer = thread::spawn(move || {
            let m = Arc::clone(&wk);
            let w = Waker::new(move || { m.fetch_add(1, SeqCst); });
            if push(&mut p, 1, &w) == Some(false) {
                bl.store(true, SeqCst);
            }
        });
        r.finish(); // closes the channel (the only consumer)
        producer.join().unwrap();
        if blocked.load(SeqCst) {
            assert_eq!(woken.load(SeqCst), 1, "producer returned Blocked but the close never woke it");
        }
    });
}

/// x6 (race): a consumer blocked on the empty exchange vs. a push. Either it got the item, or it was
/// `Blocked` and the push fired its waker.
#[test]
fn x6_race_blocked_consumer_vs_push() {
    model("x6_race_blocked_consumer_vs_push", Some(4), || {
        let (xs, xr, _c) = exchange(1, 1, 1);
        let mut p = xs.make(0);
        let mut r = xr.make();
        let woken = Arc::new(AtomicUsize::new(0));
        let got = Arc::new(AtomicBool::new(false));
        let blocked = Arc::new(AtomicBool::new(false));
        let (wk, g, bl) = (Arc::clone(&woken), Arc::clone(&got), Arc::clone(&blocked));
        let consumer = thread::spawn(move || {
            let m = Arc::clone(&wk);
            let w = Waker::new(move || { m.fetch_add(1, SeqCst); });
            match pop(&mut r, &w) {
                Some(Some(_)) => g.store(true, SeqCst),
                Some(None) => bl.store(true, SeqCst),
                None => panic!("finished while a producer is alive"),
            }
        });
        let (w0, _) = counting_waker();
        assert_eq!(push(&mut p, 5, &w0), Some(true));
        consumer.join().unwrap();
        assert!(got.load(SeqCst) || blocked.load(SeqCst));
        if blocked.load(SeqCst) {
            assert_eq!(woken.load(SeqCst), 1, "consumer returned Blocked but the push never woke it");
        }
    });
}
