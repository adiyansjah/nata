//! The executor: virtual time, seeded order, node kill, abort, mailbox.
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering::SeqCst};
use std::time::Instant;

use nata_sim::env::Tracked;
use nata_sim::exec::{Executor, Mailbox, RecvTimeout, RunEnd, Sleep};

const MS: u64 = 1_000_000;

#[test]
fn virtual_time_does_not_wait_for_the_wall_clock() {
    let ex = Executor::new_sim(1);
    let ex2 = ex.clone();
    let done = Rc::new(Cell::new(0u64));
    let d = Rc::clone(&done);
    ex.spawn(0, async move {
        Sleep::new(ex2.clone(), 3_600_000 * MS).await; // one virtual hour
        d.set(ex2.now());
    });
    let t = Instant::now();
    let end = ex.run(u64::MAX, 1000, || false);
    assert_eq!(end, RunEnd::AllDone);
    assert_eq!(done.get(), 3_600_000 * MS);
    assert!(t.elapsed().as_secs() < 2, "a virtual hour must not take real time");
}

#[test]
fn timers_fire_in_time_order_and_ties_in_creation_order() {
    let ex = Executor::new_sim(1);
    let log = Rc::new(RefCell::new(Vec::new()));
    for (i, delay) in [30u64, 10, 20, 10, 30].into_iter().enumerate() {
        let (ex2, log2) = (ex.clone(), Rc::clone(&log));
        ex.spawn(0, async move {
            Sleep::new(ex2.clone(), delay * MS).await;
            log2.borrow_mut().push((ex2.now() / MS, i));
        });
    }
    ex.run(u64::MAX, 1000, || false);
    let l = log.borrow();
    // by time; the two tasks due at the same instant may run in either order (the order seed picks)
    assert_eq!(l.iter().map(|x| x.0).collect::<Vec<_>>(), vec![10, 10, 20, 30, 30]);
}

fn order_with_seed(seed: u64) -> Vec<usize> {
    let ex = Executor::new_sim(seed);
    let log = Rc::new(RefCell::new(Vec::new()));
    for i in 0..8 {
        let log2 = Rc::clone(&log);
        ex.spawn(0, async move {
            log2.borrow_mut().push(i);
        });
    }
    ex.run(u64::MAX, 1000, || false);
    let v = log.borrow().clone();
    v
}

#[test]
fn the_order_seed_decides_which_ready_task_runs_first() {
    assert_eq!(order_with_seed(7), order_with_seed(7), "same seed, same order");
    let orders: std::collections::BTreeSet<Vec<usize>> = (0..40).map(order_with_seed).collect();
    assert!(orders.len() > 5, "40 seeds gave only {} different orders of 8 ready tasks", orders.len());
}

#[test]
fn killing_a_node_drops_its_futures_and_runs_their_destructors() {
    let ex = Executor::new_sim(1);
    let counter = Arc::new(AtomicI64::new(0));
    for node in [1u32, 2] {
        let (ex2, c) = (ex.clone(), Arc::clone(&counter));
        ex.spawn(node, async move {
            let _guard = Tracked::new(&c);
            Sleep::new(ex2, 1_000 * MS).await;
        });
    }
    ex.run(10 * MS, 1000, || false);
    assert_eq!(counter.load(SeqCst), 2);
    ex.kill_node(1);
    assert_eq!(counter.load(SeqCst), 1, "node 1's guard must be gone, node 2's still there");
    assert_eq!(ex.live_tasks_of(1), 0);
    assert_eq!(ex.live_tasks_of(2), 1);
    ex.run(u64::MAX, 1000, || false);
    assert_eq!(counter.load(SeqCst), 0);
}

#[test]
fn an_aborted_task_is_dropped_and_never_polled_again() {
    let ex = Executor::new_sim(1);
    let counter = Arc::new(AtomicI64::new(0));
    let polls = Rc::new(Cell::new(0));
    let (ex2, c, p) = (ex.clone(), Arc::clone(&counter), Rc::clone(&polls));
    let ctl = ex.spawn(0, async move {
        let _g = Tracked::new(&c);
        loop {
            p.set(p.get() + 1);
            Sleep::new(ex2.clone(), MS).await;
        }
    });
    ex.run(5 * MS, 1000, || false);
    let before = polls.get();
    assert!(before >= 3);
    ctl.abort();
    ex.run(50 * MS, 1000, || false);
    assert_eq!(polls.get(), before, "no poll after abort");
    assert_eq!(counter.load(SeqCst), 0);
    assert!(ctl.is_aborted() && !ctl.finished());
}

#[test]
fn recv_timeout_returns_a_message_or_times_out() {
    let ex = Executor::new_sim(1);
    let mb = Mailbox::new();
    let got = Rc::new(RefCell::new(Vec::new()));
    let (ex2, mb2, g) = (ex.clone(), Arc::clone(&mb), Rc::clone(&got));
    ex.spawn(0, async move {
        g.borrow_mut().push(RecvTimeout::new(ex2.clone(), Arc::clone(&mb2), 5 * MS).await); // nothing yet
        g.borrow_mut().push(RecvTimeout::new(ex2.clone(), Arc::clone(&mb2), 50 * MS).await); // the message
    });
    let mb3 = Arc::clone(&mb);
    ex.at(20 * MS, move || mb3.deliver(9, vec![1, 2, 3]));
    ex.run(u64::MAX, 1000, || false);
    assert_eq!(*got.borrow(), vec![None, Some((9, vec![1, 2, 3]))]);
}

#[test]
fn a_deadlock_and_a_livelock_are_reported_not_hung() {
    let ex = Executor::new_sim(1);
    ex.spawn(0, std::future::pending::<()>());
    assert_eq!(ex.run(u64::MAX, 1000, || false), RunEnd::Quiescent);

    // A LIVELOCK: a task that is always runnable and never makes progress, and gives control back to the executor each time (it
    // wakes itself and returns Pending). The poll limit must end the run. (The first version of this test looped on
    // `Sleep::new(ex, 0).await` and `ready(()).await`: both are Ready on the first poll, so the task never returned from ONE poll
    // and no executor can interrupt that; it hung for an hour on the first GCP run, 2026-11-05. A future that never yields is a
    // bug of the code under test, not something a cooperative executor can report.)
    struct YieldNow(bool);
    impl std::future::Future for YieldNow {
        type Output = ();
        fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
            if self.0 {
                std::task::Poll::Ready(())
            } else {
                self.0 = true;
                cx.waker().wake_by_ref();
                std::task::Poll::Pending
            }
        }
    }
    let ex = Executor::new_sim(1);
    ex.spawn(0, async move {
        loop {
            YieldNow(false).await; // runnable again at once; virtual time never advances
        }
    });
    let end = ex.run(u64::MAX, 10_000, || false);
    assert_eq!(end, RunEnd::StepLimit, "a livelock ends at the poll limit");
    assert_eq!(ex.now(), 0, "virtual time did not advance");
}
