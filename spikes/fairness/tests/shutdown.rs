//! Deterministic test for the lost shutdown notification found under CPU contention.
//!
//! Its own test binary: it sets a process-wide delay hook that would slow the other tests.
//! With the delay, a worker that has seen "no work, not shutting down" sleeps while holding the
//! queue lock. A scheduler that sets the shutdown flag without taking that lock sets it and
//! notifies during the sleep; the worker then waits forever and the query never returns.

use std::sync::atomic::Ordering::SeqCst;
use std::sync::mpsc;
use std::time::Duration;

use spike_fairness::ops::*;
use spike_fairness::*;

#[test]
fn shutting_down_never_leaves_a_worker_asleep() {
    TEST_WORKER_DELAY_US.store(3_000, SeqCst);
    for round in 0..25 {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let source = Generator::new(1, |m| Batch::from_cols(vec![vec![m as i64], vec![1]]));
            let p = Pipeline {
                name: "tiny".into(),
                source: std::sync::Arc::new(source),
                ops: vec![],
                sink: std::sync::Arc::new(Collect::new()),
                deps: vec![],
                tasks: 1,
            };
            let _ = tx.send(Query::new(vec![p]).run(1 + round % 3));
        });
        rx.recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("round {round}: wait() never returned: a worker missed the shutdown notification"))
            .expect("query");
    }
    TEST_WORKER_DELAY_US.store(0, SeqCst);
}
