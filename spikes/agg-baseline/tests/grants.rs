#[cfg(nata_loom)]
mod models {
    use agg_baseline::grants::{Manager, Mutation};
    use loom::{sync::Arc, thread};
    use std::sync::{
        Arc as CountArc,
        atomic::{AtomicUsize, Ordering},
    };

    fn check(f: impl Fn() + Send + Sync + 'static) {
        let count = CountArc::new(AtomicUsize::new(0));
        let c = count.clone();
        let mut b = loom::model::Builder::new();
        b.preemption_bound = Some(
            std::env::var("LOOM_MAX_PREEMPTIONS")
                .ok()
                .map_or(2, |v| v.parse().unwrap()),
        );
        b.max_branches = 2000;
        b.check(move || {
            c.fetch_add(1, Ordering::Relaxed);
            f();
        });
        println!(
            "executions={} bound={}",
            count.load(Ordering::Relaxed),
            b.preemption_bound.unwrap()
        );
    }
    fn progress(mutation: Mutation) {
        check(move || {
            let m = Manager::new(10, 2, mutation);
            let victim = Manager::request(&m, 0, 8).unwrap().unwrap();
            // Two failed requests coalesce. Victim is above the fair share (4).
            assert!(Manager::request(&m, 1, 4).unwrap().is_none());
            assert!(Manager::request(&m, 1, 4).unwrap().is_none());
            assert!(m.revoked(0));
            let a = Arc::clone(&m);
            let waiter = thread::spawn(move || {
                a.park(1);
                a.dispatch(1);
                let g = Manager::request(&a, 1, 4)
                    .unwrap()
                    .expect("victim release makes progress");
                drop(g);
            });
            let releaser = thread::spawn(move || drop(victim));
            waiter.join().unwrap();
            releaser.join().unwrap();
            let (held, live, peak, pressure, _, wakes) = m.snapshot();
            assert_eq!((held, live, peak, pressure), (0, 0, 10, 2));
            assert!(wakes > 0);
        });
    }
    #[test]
    fn release_before_during_after_park() {
        progress(Mutation::None);
    }
    #[test]
    fn cancel_drop_races() {
        check(|| {
            let m = Manager::new(10, 2, Mutation::None);
            let g = Manager::request(&m, 0, 8).unwrap().unwrap();
            assert!(Manager::request(&m, 1, 4).unwrap().is_none());
            let a = Arc::clone(&m);
            let t = thread::spawn(move || {
                a.park(1);
                a.dispatch(1);
                // Release may win the race, so a grant or cancellation is legal.
                if let Ok(Some(g)) = Manager::request(&a, 1, 4) {
                    drop(g);
                }
            });
            let a = Arc::clone(&m);
            let c = thread::spawn(move || {
                a.cancel();
                drop(g);
            });
            t.join().unwrap();
            c.join().unwrap();
            assert_eq!(m.snapshot().0, 0);
            assert_eq!(m.snapshot().1, 0);
            assert!(Manager::request(&m, 1, 1).is_err());
        });
    }
    #[test]
    fn one_worker_requeues() {
        check(|| {
            let m = Manager::new(10, 2, Mutation::None);
            let g = Manager::request(&m, 0, 8).unwrap().unwrap();
            assert!(Manager::request(&m, 1, 4).unwrap().is_none());
            m.park(1);
            assert!(m.revoked(0));
            drop(g);
            m.dispatch(1);
            drop(Manager::request(&m, 1, 4).unwrap().unwrap());
            assert_eq!(m.snapshot(), (0, 0, 10, 1, 1, 1));
        });
    }
    #[test]
    fn competing_grants() {
        check(|| {
            let m = Manager::new(10, 2, Mutation::None);
            let a = Arc::clone(&m);
            let t = thread::spawn(move || {
                if let Ok(Some(g)) = Manager::request(&a, 0, 8) {
                    drop(g);
                }
            });
            if let Ok(Some(g)) = Manager::request(&m, 1, 8) {
                drop(g);
            }
            t.join().unwrap();
            assert_eq!((m.snapshot().0, m.snapshot().1), (0, 0));
        });
    }
    #[test]
    #[ignore]
    fn mutant_cap_removed() {
        check(|| {
            let m = Manager::new(10, 2, Mutation::NoCap);
            let _a = Manager::request(&m, 0, 8).unwrap().unwrap();
            let _b = Manager::request(&m, 1, 4);
        });
    }
    #[test]
    #[ignore]
    fn mutant_wake_dropped() {
        progress(Mutation::NoWake);
    }
    #[test]
    #[ignore]
    fn mutant_release_twice() {
        check(|| {
            let m = Manager::new(10, 2, Mutation::ReleaseTwice);
            drop(Manager::request(&m, 0, 8).unwrap().unwrap());
        });
    }
}
#[cfg(not(nata_loom))]
#[test]
fn single_worker_requeues_instead_of_waiting() {
    use agg_baseline::grants::{Manager, Mutation};
    let m = Manager::new(10, 2, Mutation::None);
    let g = Manager::request(&m, 0, 8).unwrap().unwrap();
    assert!(Manager::request(&m, 1, 4).unwrap().is_none());
    m.park(1); // frees the sole CPU worker to run the victim
    assert!(m.revoked(0));
    drop(g); // stand-in disk completion, pre-reserved progress buffers
    m.dispatch(1);
    drop(Manager::request(&m, 1, 4).unwrap().unwrap());
    assert_eq!(m.snapshot(), (0, 0, 10, 1, 1, 1));
    assert!(Manager::request(&m, 0, 9).is_err());
}
