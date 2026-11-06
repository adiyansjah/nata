//! Loom models of the REAL scheduler (`src/lib.rs`, built with `RUSTFLAGS="--cfg nata_loom"` so that `src/sync.rs` hands out loom primitives): every
//! thread interleaving, up to a preemption bound, of the workers, the consumer and a canceller. A deadlock (every loom thread blocked) fails the
//! execution: that is how a lost wake-up shows. A loom `Arc` still alive at the end of an execution fails it too (the leak check), and the model's
//! `Tracked` counters check that every task and every result was dropped.
//!
//! Claims checked (all in every model unless noted): every task runs exactly once, or not at all when cancelled; the window is never exceeded (`high_water
//! <= window`); no claim after claiming stopped; every worker thread is joined; every task and result is dropped; the error of the lowest sequence number is
//! the one reported; ordered output is exactly the sequential order.
//!
//! Run: `crates/nata-sched/run_loom.sh` (one model per process; prints a row per model). Bounds are the defaults below; `LOOM_MAX_PREEMPTIONS` overrides.
#![cfg(nata_loom)]

mod common;
use common::*;

use std::sync::atomic::{AtomicUsize as StdAtomicUsize, Ordering as StdOrdering};

use loom::sync::Arc;
use loom::thread;
use nata_sched::{Config, Finish, Flow, Scheduler};

static EXECUTIONS: StdAtomicUsize = StdAtomicUsize::new(0);

fn check<F>(name: &str, default_bound: usize, f: F)
where
    F: Fn() + Sync + Send + 'static,
{
    let mut b = loom::model::Builder::new();
    if std::env::var("LOOM_MAX_PREEMPTIONS").is_err() {
        b.preemption_bound = Some(default_bound);
    }
    b.max_branches = 400_000;
    let start = std::time::Instant::now();
    b.check(move || {
        EXECUTIONS.fetch_add(1, StdOrdering::Relaxed);
        f();
    });
    println!(
        "[loom] {name}: executions={} bound={:?} wall={:.1}s",
        EXECUTIONS.load(StdOrdering::Relaxed),
        b.preemption_bound,
        start.elapsed().as_secs_f64()
    );
}

fn cfg(threads: usize, window: usize, ordered: bool) -> Config {
    Config {
        threads,
        window,
        ordered,
    }
}

/// The checks every finished run must pass.
fn common_checks(model: &Model, s: &nata_sched::Summary, threads: usize, window: usize) {
    assert!(s.high_water <= window as u64, "window exceeded: {s:?}");
    assert_eq!(
        s.claims_after_stop, 0,
        "a claim after claiming stopped: {s:?}"
    );
    assert_eq!(s.workers_started, threads);
    assert_eq!(s.workers_joined, threads, "a worker was not joined: {s:?}");
    assert_eq!(model.live_now(), 0, "a task or a result was not dropped");
    for r in &model.runs {
        assert!(r.load(StdOrdering::SeqCst) <= 1, "a task ran twice");
    }
    for o in &model.opens {
        assert!(o.load(StdOrdering::SeqCst) <= 1, "a file was opened twice");
    }
}

fn run_ordered(model: &Arc<Model>, c: Config) -> (Vec<usize>, nata_sched::Outcome<ModelErr>) {
    let mut got = vec![];
    let out = Scheduler::new(Arc::clone(model), c).run(|_, o| {
        got.push(o.id);
        Ok(Flow::Continue)
    });
    (got, out)
}

fn complete(files: Vec<usize>, threads: usize, window: usize, ordered: bool) {
    let model = Arc::new(Model::new(files));
    let (mut got, out) = run_ordered(&model, cfg(threads, window, ordered));
    let mut expect = model.all_ids();
    if !ordered {
        got.sort_unstable();
        expect.sort_unstable();
    }
    assert_eq!(got, expect);
    assert_eq!(out.result, Ok(Finish::Complete));
    common_checks(&model, &out.summary, threads, window);
    assert_eq!(out.summary.claimed, expect.len() as u64);
    for id in expect {
        assert_eq!(model.runs_of(id), 1, "task {id} must run exactly once");
    }
    for o in &model.opens {
        assert_eq!(o.load(StdOrdering::SeqCst), 1);
    }
}

#[test]
fn m01_complete_ordered_two_workers_two_files() {
    check("m01_complete_ordered_two_workers_two_files", 2, || {
        complete(vec![2, 1], 2, 2, true)
    });
}

#[test]
fn m02_complete_unordered_two_workers_two_files() {
    check("m02_complete_unordered_two_workers_two_files", 2, || {
        complete(vec![2, 1], 2, 2, false)
    });
}

#[test]
fn m03_window_of_one_two_workers_release_wakes_the_blocked_worker() {
    check(
        "m03_window_of_one_two_workers_release_wakes_the_blocked_worker",
        2,
        || complete(vec![3], 2, 1, true),
    );
}

#[test]
fn m04_window_of_two_three_tasks_never_three_outstanding() {
    check(
        "m04_window_of_two_three_tasks_never_three_outstanding",
        2,
        || complete(vec![3], 2, 2, true),
    );
}

#[test]
fn m05_files_are_opened_one_at_a_time_in_order_and_empty_files_pass() {
    check(
        "m05_files_are_opened_one_at_a_time_in_order_and_empty_files_pass",
        2,
        || complete(vec![1, 0, 1], 2, 3, true),
    );
}

#[test]
fn m06_single_worker_whole_scan() {
    check("m06_single_worker_whole_scan", 4, || {
        complete(vec![2, 2], 1, 1, true)
    });
}

#[test]
fn m07_limit_stop_wakes_a_worker_blocked_on_the_window_and_drops_everything() {
    check(
        "m07_limit_stop_wakes_a_worker_blocked_on_the_window_and_drops_everything",
        2,
        || {
            let model = Arc::new(Model::new(vec![3]));
            let mut got = vec![];
            let out = Scheduler::new(Arc::clone(&model), cfg(2, 1, true)).run(|_, o| {
                got.push(o.id);
                Ok(Flow::Stop)
            });
            assert_eq!(got, vec![0], "exactly the first result");
            assert_eq!(out.result, Ok(Finish::Stopped));
            common_checks(&model, &out.summary, 2, 1);
            assert_eq!(model.runs_of(0), 1);
        },
    );
}

#[test]
fn m08_limit_stop_with_room_in_the_window_cancels_the_running_tasks() {
    check(
        "m08_limit_stop_with_room_in_the_window_cancels_the_running_tasks",
        2,
        || {
            let model = Arc::new(Model::new(vec![2, 1]));
            let mut got = vec![];
            let out = Scheduler::new(Arc::clone(&model), cfg(2, 3, true)).run(|_, o| {
                got.push(o.id);
                Ok(Flow::Stop)
            });
            assert_eq!(got, vec![0]);
            common_checks(&model, &out.summary, 2, 3);
        },
    );
}

#[test]
fn m09_external_cancel_reaches_blocked_tasks_and_the_blocked_consumer() {
    check(
        "m09_external_cancel_reaches_blocked_tasks_and_the_blocked_consumer",
        2,
        || {
            // task 0 never finishes by itself, task 1 does: the consumer is blocked waiting for 0 with a result in the buffer
            let mut m = Model::new(vec![2]);
            m.spin_tasks = vec![0];
            let model = Arc::new(m);
            let sched = Scheduler::new(Arc::clone(&model), cfg(2, 2, true));
            let handle = sched.cancel_handle();
            let t = thread::spawn(move || handle.cancel());
            let out = sched.run(|_, _| Ok(Flow::Continue));
            t.join().expect("canceller");
            assert_eq!(out.result, Ok(Finish::Cancelled));
            common_checks(&model, &out.summary, 2, 2);
            if model.runs_of(0) == 1 {
                assert!(
                    model.stopped.load(StdOrdering::SeqCst) >= 1,
                    "the blocked task saw the cancel"
                );
            }
        },
    );
}

#[test]
fn m10_external_cancel_while_the_window_is_full_and_workers_wait() {
    check(
        "m10_external_cancel_while_the_window_is_full_and_workers_wait",
        2,
        || {
            // window 1: task 0 never finishes by itself, the second worker waits at the window; only the cancel can end this
            let mut m = Model::new(vec![3]);
            m.spin_tasks = vec![0];
            let model = Arc::new(m);
            let sched = Scheduler::new(Arc::clone(&model), cfg(2, 1, true));
            let handle = sched.cancel_handle();
            let t = thread::spawn(move || handle.cancel());
            let out = sched.run(|_, _| Ok(Flow::Continue));
            t.join().expect("canceller");
            assert_eq!(out.result, Ok(Finish::Cancelled));
            common_checks(&model, &out.summary, 2, 1);
        },
    );
}

#[test]
fn m11_the_lowest_error_wins_unordered() {
    check("m11_the_lowest_error_wins_unordered", 2, || {
        let mut m = Model::new(vec![3]);
        m.fail_tasks = vec![1, 2];
        let model = Arc::new(m);
        let (_, out) = run_ordered(&model, cfg(2, 3, false));
        assert_eq!(
            out.result,
            Err(ModelErr::Task(1)),
            "the lowest failing task, whichever finished first"
        );
        common_checks(&model, &out.summary, 2, 3);
    });
}

#[test]
fn m12_the_lowest_error_wins_ordered_and_only_earlier_results_are_delivered() {
    check(
        "m12_the_lowest_error_wins_ordered_and_only_earlier_results_are_delivered",
        2,
        || {
            let mut m = Model::new(vec![3]);
            m.fail_tasks = vec![1, 2];
            let model = Arc::new(m);
            let (got, out) = run_ordered(&model, cfg(2, 3, true));
            assert_eq!(out.result, Err(ModelErr::Task(1)));
            assert_eq!(got, vec![0]);
            common_checks(&model, &out.summary, 2, 3);
        },
    );
}

#[test]
fn m13_a_file_that_cannot_be_opened_takes_its_place_and_stops_the_scan() {
    check(
        "m13_a_file_that_cannot_be_opened_takes_its_place_and_stops_the_scan",
        2,
        || {
            let mut m = Model::new(vec![1, 2, 1]);
            m.fail_open = Some(1);
            let model = Arc::new(m);
            let (got, out) = run_ordered(&model, cfg(2, 2, true));
            assert_eq!(out.result, Err(ModelErr::Open(1)));
            assert_eq!(got, vec![0]);
            assert_eq!(
                model.opens[2].load(StdOrdering::SeqCst),
                0,
                "no file after the failing one is opened"
            );
            assert_eq!(
                out.summary.claimed, 2,
                "task 0 and the file that failed each have a sequence number"
            );
            common_checks(&model, &out.summary, 2, 2);
        },
    );
}

#[test]
fn m14_an_error_cancels_a_higher_blocked_task_and_a_lower_task_still_finishes() {
    check(
        "m14_an_error_cancels_a_higher_blocked_task_and_a_lower_task_still_finishes",
        2,
        || {
            // task 1 fails; task 2 spins until cancelled (only the error of task 1 can end it); task 0 is lower and must still be delivered
            let mut m = Model::new(vec![3]);
            m.fail_tasks = vec![1];
            m.spin_tasks = vec![2];
            let model = Arc::new(m);
            let (got, out) = run_ordered(&model, cfg(3, 3, true));
            assert_eq!(out.result, Err(ModelErr::Task(1)));
            assert_eq!(got, vec![0], "the result below the error is delivered");
            common_checks(&model, &out.summary, 3, 3);
        },
    );
}

#[test]
fn m15_a_sink_error_cancels_everything() {
    check("m15_a_sink_error_cancels_everything", 2, || {
        let mut m = Model::new(vec![3]);
        m.fail_tasks = vec![2];
        let model = Arc::new(m);
        let out =
            Scheduler::new(Arc::clone(&model), cfg(2, 2, true)).run(|_, _| Err(ModelErr::Internal));
        assert_eq!(out.result, Err(ModelErr::Internal));
        common_checks(&model, &out.summary, 2, 2);
    });
}

#[test]
fn m16_no_files_and_only_empty_files() {
    check("m16_no_files_and_only_empty_files", 2, || {
        complete(vec![], 2, 1, true);
        complete(vec![0, 0], 2, 1, true);
    });
}
