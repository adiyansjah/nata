//! Same production transition code, instrumented primitives under nata_portal_loom.
//! I/O/timer delivery are boundary events, not an emulation of Tokio or Parquet.
#[path = "../src/server/lifecycle.rs"]
mod lifecycle;
use lifecycle::{Cancel, Credit, Enter, Exchange, Lease, Once, Phase, Slot};
#[cfg(nata_portal_loom)]
use loom::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};
#[cfg(not(nata_portal_loom))]
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};

#[derive(Clone)]
struct Token(Arc<AtomicBool>);
impl Token {
    fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }
    fn cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}
impl Cancel for Token {
    fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
fn model(f: impl Fn() + Send + Sync + 'static) {
    #[cfg(nata_portal_loom)]
    {
        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = count.clone();
        let bound = std::env::var("LOOM_MAX_PREEMPTIONS")
            .unwrap_or_else(|_| "2".into())
            .parse()
            .expect("bound");
        let mut builder = loom::model::Builder::new();
        builder.preemption_bound = Some(bound);
        builder.check(move || {
            c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            f();
        });
        eprintln!(
            "[portal-loom] executions={} bound={bound}",
            count.load(std::sync::atomic::Ordering::Relaxed)
        );
    }
    #[cfg(not(nata_portal_loom))]
    f();
}

#[test]
fn execute_limit_suspend_inert_cancel_resume_and_exact_end() {
    model(|| {
        let token = Token::new();
        let slot = Slot::new(Some(token.clone()));
        let credit = Arc::new(Credit::new());
        let producer_credit = credit.clone();
        let (tx, rx) = mpsc::channel();
        let producer = thread::spawn(move || {
            let mut remaining = 2;
            for row in 0..4 {
                tx.send(Some(row)).expect("row");
                if lifecycle::row(&mut remaining) {
                    tx.send(None).expect("suspended");
                    remaining = producer_credit.wait().expect("resume");
                }
            }
        });
        let mut phase = Phase::Bound;
        let mut exchange = Exchange::new();
        exchange.begin(Some(80));
        assert_eq!(phase.enter(), Enter::Start);
        for expected in [0, 2] {
            assert_eq!(rx.recv().expect("first row"), Some(expected));
            assert_eq!(rx.recv().expect("second row"), Some(expected + 1));
            assert_eq!(rx.recv().expect("exact end suspends"), None);
            let _parked = slot.take();
            phase.suspend(10);
            assert_eq!(phase.suspended(), Some(10), "reachable suspension");
            slot.cancel();
            assert!(!token.cancelled(), "suspended cancel is inert");
            assert_eq!(exchange.deadline(), Some(80), "suspension preserves timer");
            slot.install(token.clone());
            credit.send(2).expect("resume credit");
            assert_eq!(phase.enter(), Enter::Start);
        }
        producer.join().expect("producer exhausted");
        phase.complete();
        assert_eq!(phase.enter(), Enter::Complete, "completion is terminal");
        assert_eq!(phase, Phase::Complete);
        assert!(phase.suspended().is_none());
        let shared = Lease::<u64>::new();
        shared.enter();
        shared.complete();
        assert_eq!(shared.enter(), Enter::Complete);
        let mut unlimited = 0;
        assert!(!lifecycle::row(&mut unlimited));
        assert!(exchange.sync(b'I'));
        assert_eq!(exchange.deadline(), None);
    });
}

#[test]
fn resume_races_expiry_and_cleanup_wakes_once() {
    model(|| {
        let credit = Arc::new(Credit::new());
        let join_owner = Arc::new(Once::new(()));
        let releases = Arc::new(AtomicUsize::new(0));
        let token = Token::new();
        let c = credit.clone();
        let t = token.clone();
        let producer = thread::spawn(move || {
            // Real Gate::wait semantics: expiry supplies zero credit or Drop closes.
            if let Ok(n) = c.wait() {
                assert!(n == 0 || n == 2);
            }
            // Observe final cancellation after both boundary events have finished.
            t
        });
        let c = credit.clone();
        let t = token.clone();
        let owner = join_owner.clone();
        let r = releases.clone();
        let expiry = thread::spawn(move || {
            t.cancel();
            let _ = c.send(0);
            if owner.take().is_some() {
                r.fetch_add(1, Ordering::SeqCst);
            }
        });
        let _ = credit.send(2);
        // Execution::drop: cancel, sticky wake, then take the same join owner.
        token.cancel();
        credit.close();
        if join_owner.take().is_some() {
            releases.fetch_add(1, Ordering::SeqCst);
        }
        expiry.join().expect("expiry");
        assert!(producer.join().expect("producer exit").cancelled());
        assert_eq!(releases.load(Ordering::SeqCst), 1, "exactly one release");
        assert!(join_owner.take().is_none());
        assert_eq!(credit.send(2), Err(()), "close is terminal");
        let mut phase = Phase::Suspended(0);
        assert_eq!(phase.suspended(), Some(0));
        phase.expire();
        assert_eq!(phase.enter(), Enter::Expired, "expiry is terminal");
        assert!(phase.suspended().is_none());
    });
}

#[test]
fn expiry_credit_before_or_after_park_cannot_lose_wake() {
    model(|| {
        let credit = Arc::new(Credit::new());
        let c = credit.clone();
        let producer = thread::spawn(move || assert_eq!(c.wait(), Ok(0)));
        credit.send(0).expect("expiry wake");
        producer.join().expect("expiry wakes blocked producer");
    });
}

#[test]
fn close_before_or_after_park_cannot_lose_wake() {
    model(|| {
        let credit = Arc::new(Credit::new());
        let c = credit.clone();
        let producer = thread::spawn(move || assert_eq!(c.wait(), Err(())));
        credit.close();
        producer.join().expect("close wakes blocked producer");
    });
}

#[test]
fn pinned_timer_and_cancel_do_not_follow_replacement() {
    model(|| {
        let first = Token::new();
        let next = Token::new();
        let slot = Arc::new(Slot::new(Some(first.clone())));
        let pinned = slot.current().expect("pin timer before replacement");
        let timer = thread::spawn(move || pinned.cancel());
        slot.take();
        slot.install(next.clone());
        timer.join().expect("timer");
        assert!(first.cancelled());
        assert!(!next.cancelled(), "timer must not select replacement");
        slot.cancel();
        assert!(next.cancelled(), "replacement is the current cancel target");
    });
}

#[test]
fn timeout_across_suspension_error_drain_until_sync_and_rollback() {
    model(|| {
        let mut exchange = Exchange::new();
        exchange.begin(Some(80));
        let mut phase = Phase::Bound;
        assert_eq!(phase.enter(), Enter::Start);
        phase.suspend(10);
        exchange.begin(Some(200)); // next Execute must not restart timer
        assert!(
            exchange.timed_out(120),
            "timeout includes client think time"
        );
        exchange.failed = true;
        assert!(!exchange.timed_out(120), "one error only");
        // The real event loop ignores all non-Sync messages while failed.
        for _ in ["Execute", "Flush", "Bind", "Query ROLLBACK"] {
            assert!(
                exchange.ignoring(),
                "error drains through attempted rollback"
            );
        }
        assert!(exchange.sync(b'I'));
        assert!(!exchange.failed);
        assert_eq!(exchange.deadline(), None);
        exchange.begin(Some(300));
        assert!(!exchange.timed_out(120));
        // Sync in BEGIN preserves Run; rollback/idle Sync drops its owner.
        assert!(!exchange.sync(b'T'));
        assert_eq!(phase.suspended(), Some(10));
        let owner = Once::new(phase);
        assert!(owner.take().is_some());
        assert!(owner.take().is_none(), "rollback cleanup idempotent");
    });
}

#[test]
fn cancel_selection_linearizes_before_suspension_publication() {
    model(|| {
        let token = Token::new();
        let slot = Arc::new(Slot::new(Some(token.clone())));
        let cancel_slot = slot.clone();
        let cancel = thread::spawn(move || cancel_slot.cancel());
        let _parked = slot.take();
        // Once take returns, no previously selected cancel may still be pending.
        let at_publication = token.cancelled();
        cancel.join().expect("cancel");
        assert_eq!(token.cancelled(), at_publication, "cancel after suspension");
        slot.cancel();
        assert_eq!(token.cancelled(), at_publication, "suspended cancel inert");
    });
}

// The real shared lease lock and pinned cleanup callback, with logical idle time.
#[test]
fn admission_yield_races_resume() {
    model(|| {
        let lease = Arc::new(Lease::new());
        lease.enter();
        lease.suspend(0);
        let token = Token::new();
        let l = lease.clone();
        let t = token.clone();
        let waiter = thread::spawn(move || l.expire(0, || t.cancel()));
        let resumed = lease.enter();
        let yielded = waiter.join().expect("waiter");
        assert_eq!(yielded, resumed == Enter::Expired);
        assert_eq!(token.cancelled(), yielded);
        assert!(lease.suspended().is_none());
        // A stale selection must not expire a later suspension of the same portal.
        if !yielded {
            lease.suspend(1);
            assert!(!lease.expire(0, || token.cancel()));
        }
    });
}
#[test]
fn admission_yield_races_close() {
    model(|| {
        let lease = Arc::new(Lease::new());
        lease.enter();
        lease.suspend(0);
        let owner = Arc::new(Once::new(()));
        let releases = Arc::new(AtomicUsize::new(0));
        let credit = Arc::new(Credit::new());
        let token = Token::new();
        let l = lease.clone();
        let o = owner.clone();
        let r = releases.clone();
        let c = credit.clone();
        let t = token.clone();
        let waiter = thread::spawn(move || {
            l.expire(0, || {
                t.cancel();
                let _ = c.send(0);
                if o.take().is_some() {
                    r.fetch_add(1, Ordering::SeqCst);
                }
            })
        });
        lease.close();
        token.cancel();
        credit.close();
        if owner.take().is_some() {
            releases.fetch_add(1, Ordering::SeqCst);
        }
        waiter.join().expect("waiter");
        assert!(token.cancelled());
        assert_eq!(credit.wait(), Err(()));
        assert_eq!(releases.load(Ordering::SeqCst), 1);
        assert!(lease.expired());
    });
}
#[test]
fn two_admission_waiters_one_idle_portal() {
    model(|| {
        let lease = Arc::new(Lease::new());
        lease.enter();
        lease.suspend(0);
        let releases = Arc::new(AtomicUsize::new(0));
        let l = lease.clone();
        let r = releases.clone();
        let first = thread::spawn(move || {
            l.expire(0, || {
                r.fetch_add(1, Ordering::SeqCst);
            })
        });
        let second = lease.expire(0, || {
            releases.fetch_add(1, Ordering::SeqCst);
        });
        assert_ne!(first.join().expect("waiter"), second);
        assert_eq!(releases.load(Ordering::SeqCst), 1);
        assert_eq!(lease.enter(), Enter::Expired);
    });
}
