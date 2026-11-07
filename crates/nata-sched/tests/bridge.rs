//! Production park/wake, fill subscriptions and reservation handshake under loom.
mod common;
use common::Model;
use nata_sched::bridge::{Broadcast, Completion, ReserveStep, Waker, reserve_step};
use nata_sched::memory::{MemoryManager, Reservation};
use nata_sched::sync::{Arc, AtomicUsize, Condvar, Mutex, Ordering, join, lock, spawn_named, wait};
use nata_sched::{Config, Finish, Flow, Scheduler};

fn model(f: impl Fn() + Send + Sync + 'static) {
    #[cfg(nata_loom)]
    {
        let mut b = loom::model::Builder::new();
        b.preemption_bound = Some(
            std::env::var("LOOM_MAX_PREEMPTIONS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(2),
        );
        b.check(f);
    }
    #[cfg(not(nata_loom))]
    f();
}
fn cfg() -> Config {
    Config {
        threads: 1,
        window: 2,
        ordered: true,
        open_ahead: 2,
    }
}
#[test]
fn io_completion_before_or_after_park_is_not_lost() {
    model(|| {
        let mut m = Model::new(vec![2]);
        m.park_tasks = vec![0, 1];
        let m = Arc::new(m);
        let sched = Scheduler::new(m.clone(), cfg());
        let mut got = Vec::new();
        let out = sched.run(|_, o| {
            got.push(o.id);
            Ok(Flow::Continue)
        });
        m.join_io();
        assert_eq!(out.result, Ok(Finish::Complete));
        assert_eq!(got, [0, 1]);
        assert!(out.summary.parks <= 2);
        assert!(out.summary.high_water <= 2);
        assert_eq!(m.live_now(), 0);
    });
}
#[test]
fn footer_open_can_park() {
    model(|| {
        let mut m = Model::new(vec![1]);
        m.park_opens = vec![0];
        let m = Arc::new(m);
        let sched = Scheduler::new(m.clone(), cfg());
        let out = sched.run(|_, _| Ok(Flow::Continue));
        m.join_io();
        assert_eq!(out.result, Ok(Finish::Complete));
        assert!(out.summary.parks <= 1);
        assert_eq!(m.live_now(), 0);
    });
}
#[test]
fn cancel_reaches_a_parked_waiter_without_io() {
    model(|| {
        // Task 1 can only run after task 0 returned Parked: no timing/spin oracle.
        let mut m = Model::new(vec![2]);
        m.stall_tasks = vec![0];
        let m = Arc::new(m);
        let sched = Scheduler::new(
            m.clone(),
            Config {
                ordered: false,
                ..cfg()
            },
        );
        let cancel = sched.cancel_handle();
        let out = sched.run(|_, _| {
            cancel.cancel();
            Ok(Flow::Continue)
        });
        assert_eq!(out.result, Ok(Finish::Cancelled));
        assert_eq!(
            out.summary.parks, 1,
            "cancel reaches an actually parked waiter"
        );
        assert_eq!(m.live_now(), 0);
    });
}
#[test]
fn completion_registration_and_shared_fill_cancellation() {
    model(|| {
        let done = Broadcast::new();
        let a = Broadcast::subscribe(&done, 2).expect("first");
        let b = Broadcast::subscribe(&done, 2).expect("second");
        assert!(Broadcast::subscribe(&done, 2).is_none());
        let signal = Arc::new((Mutex::new(false), Condvar::new()));
        let s = signal.clone();
        let wake = Waker::new(move || {
            *lock(&s.0) = true;
            s.1.notify_one();
        });
        assert!(a.poll(&wake).is_none());
        let d = done.clone();
        let h = spawn_named("fill".into(), move || d.complete(7)).expect("valid model");
        drop(a); // one cancelled query must not cancel the other
        if b.poll(&wake).is_none() {
            let mut g = lock(&signal.0);
            while !*g {
                g = wait(&signal.1, g);
            }
        }
        assert_eq!(b.poll(&wake), Some(7));
        assert!(join(h));
    });
}
#[test]
fn real_grant_gate_stays_within_budget_and_wakes() {
    model(|| {
        let query = MemoryManager::new(5, 1, 1).expect("query");
        let own = |parent| {
            let child = match MemoryManager::request(&query, 0, 4).expect("query grant") {
                Reservation::Granted(g) => g,
                _ => panic!("query grant"),
            };
            child.with_parent(parent)
        };
        let m = MemoryManager::new(5, 1, 2).expect("valid model");
        let held = match MemoryManager::request(&m, 0, 4).expect("valid model") {
            Reservation::Granted(g) => g,
            _ => panic!("grant"),
        };
        let signal = Arc::new((Mutex::new(false), Condvar::new()));
        let s = signal.clone();
        let wake = Waker::new(move || {
            *lock(&s.0) = true;
            s.1.notify_one();
        });
        let h = spawn_named("release".into(), move || drop(held)).expect("valid model");
        match reserve_step(&m, 1, 4, wake.clone()) {
            ReserveStep::Granted(g) => drop(own(g)),
            ReserveStep::Wait => {
                let mut g = lock(&signal.0);
                while !*g {
                    g = wait(&signal.1, g);
                }
                drop(g);
                match reserve_step(&m, 1, 4, wake) {
                    ReserveStep::Granted(g) => drop(own(g)),
                    _ => panic!("released grant"),
                }
            }
            ReserveStep::Failed(_) => panic!("reservation"),
        }
        m.finish(1).expect("valid model");
        assert!(join(h));
        let s = m.snapshot();
        assert!(s.peak <= s.budget);
        assert_eq!(s.held, 0);
        assert_eq!(query.snapshot().held, 0);
        assert!(query.snapshot().peak <= query.snapshot().budget);
    });
}
#[test]
fn completion_wake_is_sticky() {
    model(|| {
        let c = Completion::new();
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        let wake = Waker::new(move || {
            n2.fetch_add(1, Ordering::SeqCst);
        });
        assert!(c.poll(&wake).is_none());
        c.complete(1);
        assert_eq!(c.poll(&wake), Some(1));
        assert_eq!(n.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn grant_wait_does_not_wake_itself_repeatedly() {
    model(|| {
        let m = MemoryManager::new(5, 1, 1).expect("memory");
        let held = match MemoryManager::request(&m, 0, 4).expect("grant") {
            Reservation::Granted(g) => g,
            _ => panic!("grant"),
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let w = Waker::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        });
        assert!(matches!(
            reserve_step(&m, 0, 4, w.clone()),
            ReserveStep::Wait
        ));
        let first = calls.load(Ordering::SeqCst);
        assert!(matches!(reserve_step(&m, 0, 4, w), ReserveStep::Wait));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            first,
            "no capacity changed: must stay parked"
        );
        drop(held);
        assert!(calls.load(Ordering::SeqCst) > first);
        m.finish(0).expect("finish");
    });
}

#[test]
#[ignore = "full five-thread model: GCP heavy only"]
fn two_workers_two_waiters_keep_the_window_bound() {
    model(|| {
        let mut m = Model::new(vec![2]);
        m.park_tasks = vec![0, 1];
        let m = Arc::new(m);
        let out = Scheduler::new(
            m.clone(),
            Config {
                threads: 2,
                ..cfg()
            },
        )
        .run(|_, _| Ok(Flow::Continue));
        m.join_io();
        assert_eq!(out.result, Ok(Finish::Complete));
        assert!(out.summary.high_water <= 2);
        assert_eq!(m.live_now(), 0);
    });
}

#[test]
fn shrinking_construction_returns_query_and_node_capacity_once() {
    model(|| {
        let query = MemoryManager::new(5, 1, 1).expect("query");
        let node = MemoryManager::new(5, 1, 1).expect("node");
        let child = match MemoryManager::request(&query, 0, 4).expect("query grant") {
            Reservation::Granted(g) => g,
            _ => panic!("query grant"),
        };
        let parent = match MemoryManager::request(&node, 0, 4).expect("node grant") {
            Reservation::Granted(g) => g,
            _ => panic!("node grant"),
        };
        let mut held = child.with_parent(parent);
        held.shrink_to(2).expect("shrink");
        assert_eq!(query.snapshot().held, 2);
        assert_eq!(node.snapshot().held, 2);
        assert!(held.shrink_to(3).is_err(), "growing needs a new grant");
        drop(held);
        assert_eq!(query.snapshot().held, 0);
        assert_eq!(node.snapshot().held, 0);
        assert_eq!(query.snapshot().releases, 1);
        assert_eq!(node.snapshot().releases, 1);
    });
}
