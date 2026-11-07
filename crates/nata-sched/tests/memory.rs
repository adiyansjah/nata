//! Real manager under the scheduler's std/loom facade. No model-only budget.
use nata_sched::memory::{Grant, MemoryError, MemoryManager, Reservation, Wake};
use nata_sched::sync::{Arc, AtomicUsize, Condvar, Mutex, Ordering, lock, wait};

struct Dispatcher {
    ready: Mutex<bool>,
    cv: Condvar,
    calls: AtomicUsize,
}
impl Dispatcher {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            ready: Mutex::new(false),
            cv: Condvar::new(),
            calls: AtomicUsize::new(0),
        })
    }
    // Only the dispatcher waits. A CPU task just calls manager.park and exits.
    fn take(&self) {
        let mut ready = lock(&self.ready);
        while !*ready {
            ready = wait(&self.cv, ready);
        }
        *ready = false;
    }
}
impl Wake for Dispatcher {
    fn wake(&self) {
        *lock(&self.ready) = true;
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.cv.notify_all();
    }
}
fn grant(m: &Arc<MemoryManager>, op: usize, n: usize) -> Grant {
    match MemoryManager::request(m, op, n).expect("valid request") {
        Reservation::Granted(g) => {
            let s = m.snapshot();
            assert!(s.progress + s.held <= s.budget, "grant cap exceeded");
            assert!(s.peak <= s.budget, "peak exceeds B");
            assert_eq!(g.bytes(), n);
            g
        }
        Reservation::Pending { .. } => panic!("expected grant"),
    }
}
fn pending(m: &Arc<MemoryManager>, op: usize, n: usize, victim: usize) {
    match MemoryManager::request(m, op, n).expect("valid pending request") {
        Reservation::Pending { victim: v } => assert_eq!(v, Some(victim)),
        Reservation::Granted(_) => panic!("budget cap removed"),
    }
}
fn clean(m: &MemoryManager) {
    let s = m.snapshot();
    assert_eq!((s.held, s.live_grants, s.pending), (0, 0, 0));
    assert!(s.peak <= s.budget);
}
#[cfg(not(nata_loom))]
fn check(f: impl Fn() + Send + Sync + 'static) {
    f();
}
#[cfg(nata_loom)]
fn check(f: impl Fn() + Send + Sync + 'static) {
    let mut b = loom::model::Builder::new();
    b.preemption_bound = Some(
        std::env::var("LOOM_MAX_PREEMPTIONS")
            .ok()
            .map_or(2, |v| v.parse().expect("bound")),
    );
    b.max_branches = 2000;
    let executions = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = executions.clone();
    b.check(move || {
        count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        f();
    });
    println!(
        "[loom] memory: executions={} bound={}",
        executions.load(std::sync::atomic::Ordering::Relaxed),
        b.preemption_bound.expect("bound")
    );
}
#[cfg(nata_loom)]
use loom::thread;
#[cfg(not(nata_loom))]
use std::thread;

#[test]
fn one_worker_release_before_park_makes_progress() {
    check(|| {
        let m = MemoryManager::new(10, 2, 2).expect("manager");
        let g = grant(&m, 0, 8);
        pending(&m, 1, 4, 0);
        pending(&m, 1, 4, 0);
        assert_eq!(m.snapshot().pending, 1, "requests coalesce");
        assert!(m.revoked(0).expect("operator"));
        drop(g); // I/O completion before the requester registers a waker
        let d = Dispatcher::new();
        m.park(1, d.clone()).expect("park");
        d.take();
        drop(grant(&m, 1, 4));
        assert_eq!(d.calls.load(Ordering::SeqCst), 1);
        clean(&m);
    });
}
#[test]
fn release_before_during_after_park() {
    check(|| {
        let m = MemoryManager::new(10, 2, 2).expect("manager");
        let g = grant(&m, 0, 8);
        pending(&m, 1, 4, 0);
        let d = Dispatcher::new();
        let a = m.clone();
        let t = thread::spawn(move || {
            a.park(1, d.clone()).expect("park");
            d.take();
            drop(grant(&a, 1, 4));
        });
        let r = thread::spawn(move || drop(g));
        t.join().expect("waiter");
        r.join().expect("releaser");
        clean(&m);
    });
}
#[test]
fn cancel_drop_races() {
    check(|| {
        let m = MemoryManager::new(10, 2, 2).expect("manager");
        let g = grant(&m, 0, 8);
        pending(&m, 1, 4, 0);
        let d = Dispatcher::new();
        let a = m.clone();
        let t = thread::spawn(move || {
            a.park(1, d.clone()).expect("park");
            d.take();
            match MemoryManager::request(&a, 1, 4) {
                Ok(Reservation::Granted(g)) => drop(g),
                Err(MemoryError::Cancelled) => (),
                _ => panic!("release or cancel must make progress"),
            }
        });
        let a = m.clone();
        let c = thread::spawn(move || a.cancel());
        let releaser = thread::spawn(move || drop(g));
        t.join().expect("waiter");
        c.join().expect("cancel");
        releaser.join().expect("releaser");
        assert!(matches!(
            MemoryManager::request(&m, 1, 1),
            Err(MemoryError::Cancelled)
        ));
        clean(&m);
    });
}
#[test]
fn competing_grants_never_multiply_budget() {
    check(|| {
        let m = MemoryManager::new(10, 2, 2).expect("manager");
        let a = m.clone();
        let t = thread::spawn(move || {
            if let Ok(Reservation::Granted(g)) = MemoryManager::request(&a, 0, 8) {
                assert!(a.snapshot().peak <= 10);
                drop(g);
            }
            a.finish(0).expect("finish");
        });
        if let Ok(Reservation::Granted(g)) = MemoryManager::request(&m, 1, 8) {
            assert!(m.snapshot().peak <= 10);
            drop(g);
        }
        m.finish(1).expect("finish");
        t.join().expect("worker");
        clean(&m);
    });
}
#[test]
fn fifo_successor_wakes_without_another_release() {
    check(|| {
        let m = MemoryManager::new(10, 2, 3).expect("manager");
        let g = grant(&m, 0, 8);
        pending(&m, 1, 4, 0);
        pending(&m, 2, 4, 0);
        let first = Dispatcher::new();
        let second = Dispatcher::new();
        m.park(1, first.clone()).expect("park");
        m.park(2, second.clone()).expect("park");
        drop(g);
        first.take();
        let a = grant(&m, 1, 4);
        second.take(); // fits alongside a, even before a releases
        let b = grant(&m, 2, 4);
        drop((a, b));
        clean(&m);
    });
}
#[test]
fn abandoned_fifo_head_cannot_starve_successor() {
    check(|| {
        let m = MemoryManager::new(10, 2, 3).expect("manager");
        let g = grant(&m, 0, 8);
        pending(&m, 1, 8, 0);
        pending(&m, 2, 4, 0);
        let d = Dispatcher::new();
        m.park(2, d.clone()).expect("park");
        drop(g);
        m.finish(1).expect("finish failed operator");
        d.take();
        drop(grant(&m, 2, 4));
        clean(&m);
    });
}
#[test]
fn cancellation_wakes_every_operator_and_is_sticky() {
    check(|| {
        let m = MemoryManager::new(10, 2, 3).expect("manager");
        let g = grant(&m, 0, 8);
        pending(&m, 1, 4, 0);
        pending(&m, 2, 4, 0);
        let a = Dispatcher::new();
        let b = Dispatcher::new();
        m.park(1, a.clone()).expect("park");
        m.cancel();
        m.park(2, b.clone()).expect("park after cancel");
        a.take();
        b.take();
        drop(g);
        clean(&m);
    });
}
#[test]
fn invalid_minimum_and_changed_request_fail_clearly() {
    check(|| {
        assert!(matches!(
            MemoryManager::new(10, 0, 2),
            Err(MemoryError::InvalidBudget)
        ));
        assert!(matches!(
            MemoryManager::new(2, 2, 2),
            Err(MemoryError::InvalidBudget)
        ));
        assert!(matches!(
            MemoryManager::new(10, 2, 0),
            Err(MemoryError::InvalidBudget)
        ));
        let m = MemoryManager::new(10, 2, 2).expect("manager");
        assert!(matches!(
            MemoryManager::request(&m, 0, 9),
            Err(MemoryError::MinimumExceedsBudget)
        ));
        assert!(matches!(
            MemoryManager::request(&m, 0, 0),
            Err(MemoryError::MinimumExceedsBudget)
        ));
        assert!(matches!(
            MemoryManager::request(&m, 2, 1),
            Err(MemoryError::InvalidOperator)
        ));
        let g = grant(&m, 0, 8);
        pending(&m, 1, 4, 0);
        assert!(matches!(
            MemoryManager::request(&m, 1, 3),
            Err(MemoryError::RequestChanged)
        ));
        m.finish(1).expect("finish");
        drop(g);
        clean(&m);
    });
}

#[test]
fn callbacks_run_outside_manager_lock() {
    struct Reentrant(Arc<MemoryManager>);
    impl Wake for Reentrant {
        fn wake(&self) {
            assert!(self.0.snapshot().held <= 8);
        }
    }
    check(|| {
        let m = MemoryManager::new(10, 2, 2).expect("manager");
        let g = grant(&m, 0, 8);
        pending(&m, 1, 4, 0);
        m.park(1, Arc::new(Reentrant(m.clone()))).expect("park");
        drop(g);
        m.finish(1).expect("finish");
        clean(&m);
    });
}

#[test]
fn fifo_arrival_after_release_waits_for_head_instead_of_erroring() {
    check(|| {
        let m = MemoryManager::new(10, 2, 3).expect("manager");
        let g = grant(&m, 0, 8);
        pending(&m, 1, 8, 0);
        drop(g);
        assert!(matches!(
            MemoryManager::request(&m, 2, 4),
            Ok(Reservation::Pending { victim: None })
        ));
        let d = Dispatcher::new();
        m.park(2, d.clone()).expect("park");
        let first = grant(&m, 1, 8);
        drop(first);
        d.take();
        drop(grant(&m, 2, 4));
        clean(&m);
    });
}

#[test]
fn spill_pressure_wakes_a_parked_victim() {
    check(|| {
        let m = MemoryManager::new(10, 2, 2).expect("manager");
        let g = grant(&m, 0, 8);
        let d = Dispatcher::new();
        m.park(0, d.clone()).expect("victim parked on other input");
        pending(&m, 1, 4, 0);
        d.take();
        assert!(m.revoked(0).expect("operator"));
        drop(g);
        drop(grant(&m, 1, 4));
        clean(&m);
    });
}

#[test]
fn pressure_does_not_revoke_another_operators_fair_share() {
    check(|| {
        let m = MemoryManager::new(10, 2, 2).expect("manager");
        let g = grant(&m, 0, 4);
        assert!(matches!(
            MemoryManager::request(&m, 1, 8),
            Err(MemoryError::NoReclaimableState)
        ));
        assert!(!m.revoked(0).expect("operator"));
        assert_eq!(m.snapshot().pending, 0);
        drop(g);
        clean(&m);
    });
}

#[test]
fn spill_request_racing_release_keeps_notification_and_progress() {
    check(|| {
        let m = MemoryManager::new(10, 2, 2).expect("manager");
        let state = grant(&m, 0, 8);
        let a = m.clone();
        let request = thread::spawn(move || {
            let d = Dispatcher::new();
            match MemoryManager::request(&a, 1, 4).expect("request") {
                Reservation::Granted(g) => drop(g),
                Reservation::Pending { victim } => {
                    assert_eq!(victim, Some(0));
                    a.park(1, d.clone()).expect("park spill requester");
                    d.take();
                    drop(grant(&a, 1, 4));
                }
            }
            a.finish(1).expect("finish requester");
        });
        let disk = thread::spawn(move || {
            // Completion owns the table until the write has finished. A release
            // before pressure is also legal: request then grants immediately.
            drop(state);
        });
        request.join().expect("requester");
        disk.join().expect("disk completion");
        assert!(!m.revoked(0).expect("victim"));
        clean(&m);
    });
}

#[test]
fn cancellation_during_spill_keeps_grant_until_disk_completion() {
    check(|| {
        let m = MemoryManager::new(10, 2, 2).expect("manager");
        let state = grant(&m, 0, 8);
        pending(&m, 1, 4, 0);
        let d = Dispatcher::new();
        m.park(1, d.clone()).expect("park requester");
        let a = m.clone();
        let canceller = thread::spawn(move || {
            a.cancel();
            assert!(matches!(
                MemoryManager::request(&a, 1, 4),
                Err(MemoryError::Cancelled)
            ));
        });
        let a = m.clone();
        let disk = thread::spawn(move || {
            // Even if cancellation won, memory is held until the disk job has
            // dropped the corresponding allocation, never released at cancel.
            assert_eq!(a.snapshot().held, 8);
            drop(state);
        });
        d.take();
        canceller.join().expect("cancel");
        disk.join().expect("disk completion");
        clean(&m);
    });
}

#[test]
fn chunked_allocations_keep_capacity_charged_until_spill_completes() {
    use nata_sched::memory::Chunks;
    check(|| {
        let m = MemoryManager::new(1024, 128, 2).expect("manager");
        let mut chunks = Chunks::new();
        for _ in 0..8 {
            assert!(chunks.reserve(&m, 0, 32, 256).expect("chunk grant"));
        }
        assert_eq!(m.snapshot().requests, 2, "chunk credit inflated");
        assert_eq!(chunks.reserved(), 512);
        assert_eq!(m.snapshot().held, 512);
        pending(&m, 1, 512, 0);
        let d = Dispatcher::new();
        let a = m.clone();
        let waiter = thread::spawn(move || {
            a.park(1, d.clone()).expect("park");
            d.take();
            drop(grant(&a, 1, 512));
        });
        let disk = thread::spawn(move || drop(chunks));
        waiter.join().expect("waiter");
        disk.join().expect("spill completion");
        clean(&m);
    });
}

#[test]
fn chunk_pending_retry_keeps_request_size_after_spill_changes_free_memory() {
    use nata_sched::memory::Chunks;
    check(|| {
        let m = MemoryManager::new(1024, 128, 2).expect("manager");
        let victim = grant(&m, 0, 768);
        let mut requester = Chunks::new();
        assert!(!requester.reserve(&m, 1, 32, 256).expect("pending"));
        let d = Dispatcher::new();
        m.park(1, d.clone()).expect("park");
        drop(victim);
        d.take();
        assert!(
            requester
                .reserve(&m, 1, 32, 256)
                .expect("same queued request")
        );
        drop(requester);
        clean(&m);
    });
}
