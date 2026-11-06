//! The scheduler with real threads: order, window, limit, errors, cancel, panics. (The same claims are explored exhaustively, for small sizes, by
//! `tests/loom_sched.rs` under `--cfg nata_loom`; here they run at sizes loom cannot reach, repeatedly, with delays.) Not built under loom.
#![cfg(not(nata_loom))]

mod common;
use common::*;

use std::sync::Arc;
use std::time::Duration;

use nata_sched::{Config, Finish, Flow, Scheduler};

fn run_collect(model: &Arc<Model>, cfg: Config) -> (Vec<usize>, nata_sched::Outcome<ModelErr>) {
    let mut got = vec![];
    let sched = Scheduler::new(Arc::clone(model), cfg);
    let out = sched.run(|_, o| {
        got.push(o.id);
        Ok(Flow::Continue)
    });
    (got, out)
}

fn cfg(threads: usize, window: usize, ordered: bool) -> Config {
    Config {
        threads,
        window,
        ordered,
    }
}

#[test]
fn ordered_output_is_the_sequential_order_for_every_thread_count_and_window() {
    for files in [
        vec![],
        vec![0],
        vec![3],
        vec![0, 0, 2, 0, 1, 0],
        vec![1; 12],
        vec![5, 0, 7, 2],
    ] {
        let expect = Model::new(files.clone()).all_ids();
        for threads in [1, 2, 3, 4, 8] {
            for window in [1, 2, 3, threads, 2 * threads, 17] {
                let model = Arc::new(Model::new(files.clone()));
                let (got, out) = run_collect(&model, cfg(threads, window, true));
                assert_eq!(
                    got, expect,
                    "files {files:?} threads {threads} window {window}"
                );
                assert_eq!(out.result, Ok(Finish::Complete));
                let s = out.summary;
                assert!(s.high_water <= window as u64, "window {window}: {s:?}");
                assert_eq!(s.claims_after_stop, 0);
                assert_eq!((s.workers_started, s.workers_joined), (threads, threads));
                assert_eq!(s.claimed, expect.len() as u64);
                assert_eq!(model.live_now(), 0, "tasks and outputs all dropped");
                for id in &expect {
                    assert_eq!(model.runs_of(*id), 1, "task {id} ran once");
                }
                for (f, o) in model.opens.iter().enumerate() {
                    assert_eq!(
                        o.load(std::sync::atomic::Ordering::SeqCst),
                        1,
                        "file {f} opened once"
                    );
                }
            }
        }
    }
}

#[test]
fn unordered_output_is_the_same_multiset() {
    let files = vec![4, 0, 6, 3, 1];
    let mut expect = Model::new(files.clone()).all_ids();
    expect.sort_unstable();
    for threads in [1, 2, 4, 8] {
        let model = Arc::new(Model::new(files.clone()));
        let (mut got, out) = run_collect(&model, cfg(threads, threads, false));
        got.sort_unstable();
        assert_eq!(got, expect);
        assert_eq!(out.result, Ok(Finish::Complete));
        assert_eq!(model.live_now(), 0);
    }
}

#[test]
fn a_slow_consumer_never_has_more_than_a_window_of_results_alive() {
    for (threads, window, ordered) in [(4, 4, true), (4, 2, true), (8, 3, false), (2, 1, true)] {
        let model = Arc::new(Model::new(vec![10; 6]));
        let mut n = 0;
        let sched = Scheduler::new(Arc::clone(&model), cfg(threads, window, ordered));
        let out = sched.run(|_, _o| {
            n += 1;
            std::thread::sleep(Duration::from_millis(2));
            Ok(Flow::Continue)
        });
        assert_eq!(out.result, Ok(Finish::Complete));
        assert_eq!(n, 60);
        let peak = model.outs_peak.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            peak <= window,
            "threads {threads} window {window}: {peak} outputs were alive at once"
        );
        assert!(out.summary.high_water <= window as u64);
    }
}

#[test]
fn a_slow_early_task_does_not_make_the_reorder_buffer_grow() {
    // task 0 is slow; the others finish at once. In ordered mode they wait, but only `window - 1` of them can: the others are not even started.
    struct Slow(Model);
    impl nata_sched::Work for Slow {
        type Task = ModelTask;
        type Out = ModelOut;
        type Err = ModelErr;
        fn files(&self) -> usize {
            self.0.files()
        }
        fn open(&self, f: usize) -> Result<Vec<ModelTask>, ModelErr> {
            self.0.open(f)
        }
        fn run(
            &self,
            t: &ModelTask,
            c: &nata_sched::CancelToken,
        ) -> nata_sched::Run<ModelOut, ModelErr> {
            if t.id == 0 {
                std::thread::sleep(Duration::from_millis(150));
            }
            self.0.run(t, c)
        }
        fn internal_error(&self, w: &str) -> ModelErr {
            self.0.internal_error(w)
        }
    }
    let work = Arc::new(Slow(Model::new(vec![9; 23])));
    let sched = Scheduler::new(Arc::clone(&work), cfg(4, 4, true));
    let mut got = vec![];
    let out = sched.run(|_, o| {
        got.push(o.id);
        Ok(Flow::Continue)
    });
    assert_eq!(out.result, Ok(Finish::Complete));
    assert_eq!(got, work.0.all_ids());
    assert!(work.0.outs_peak.load(std::sync::atomic::Ordering::SeqCst) <= 4);
}

#[test]
fn a_limit_stops_after_exactly_the_first_k_results_and_leaves_nothing_behind() {
    for threads in [1, 2, 4, 8] {
        for ordered in [true, false] {
            for k in [0usize, 1, 5, 17, 100] {
                let model = Arc::new(Model::new(vec![10; 20]));
                let mut got = vec![];
                let sched = Scheduler::new(Arc::clone(&model), cfg(threads, threads, ordered));
                let out = sched.run(|_, o| {
                    got.push(o.id);
                    Ok(if got.len() >= k {
                        Flow::Stop
                    } else {
                        Flow::Continue
                    })
                });
                let expect_len = k.clamp(1, 200);
                assert_eq!(
                    got.len(),
                    expect_len,
                    "threads {threads} ordered {ordered} k {k}"
                );
                if ordered {
                    assert_eq!(got, Model::new(vec![10; 20]).all_ids()[..expect_len]);
                }
                assert_eq!(out.summary.finish, Finish::Stopped);
                assert_eq!(out.result, Ok(Finish::Stopped));
                assert_eq!(out.summary.claims_after_stop, 0);
                assert_eq!(out.summary.workers_joined, threads);
                assert_eq!(model.live_now(), 0, "everything dropped");
                assert_eq!(
                    Arc::strong_count(&model),
                    1,
                    "no thread holds the work any more"
                );
                assert!(
                    out.summary.claimed <= (expect_len + threads) as u64,
                    "no new claims after the stop: {:?}",
                    out.summary
                );
            }
        }
    }
}

#[test]
fn the_error_of_the_lowest_task_wins_whoever_is_first() {
    // tasks 3 and 11 and 25 fail; 11 and 25 are cheap, 3 is delayed by running behind 0..2; in every run the lowest one is reported
    for threads in [1, 2, 4, 8] {
        for ordered in [true, false] {
            for _ in 0..40 {
                let mut m = Model::new(vec![10; 4]);
                m.fail_tasks = vec![13, 21, 22, 33];
                let model = Arc::new(m);
                let (got, out) = run_collect(&model, cfg(threads, 8, ordered));
                assert_eq!(
                    out.result,
                    Err(ModelErr::Task(13)),
                    "threads {threads} ordered {ordered}"
                );
                if ordered {
                    assert_eq!(
                        got,
                        (0..13).map(|i| (i / 10) * 10 + i % 10).collect::<Vec<_>>(),
                        "everything before the error, nothing after"
                    );
                }
                assert_eq!(out.summary.claims_after_stop, 0);
                assert_eq!(model.live_now(), 0);
            }
        }
    }
}

#[test]
fn a_file_that_cannot_be_opened_has_its_place_in_the_order() {
    for threads in [1, 3, 8] {
        let mut m = Model::new(vec![3, 2, 4]);
        m.fail_open = Some(1);
        m.fail_tasks = vec![21];
        let model = Arc::new(m);
        let (got, out) = run_collect(&model, cfg(threads, threads, true));
        assert_eq!(out.result, Err(ModelErr::Open(1)));
        assert_eq!(got, vec![0, 1, 2]);
        assert_eq!(
            model.opens[2].load(std::sync::atomic::Ordering::SeqCst),
            0,
            "files after the failure are never opened (single opener, in order)"
        );
        assert_eq!(model.live_now(), 0);
    }
}

#[test]
fn an_error_after_a_satisfied_limit_is_never_reported() {
    for threads in [1, 4] {
        for ordered in [true, false] {
            let mut m = Model::new(vec![10; 3]);
            m.fail_tasks = vec![25];
            let model = Arc::new(m);
            let mut n = 0;
            let out = Scheduler::new(Arc::clone(&model), cfg(threads, 3, ordered)).run(|_, _| {
                n += 1;
                Ok(if n == 3 { Flow::Stop } else { Flow::Continue })
            });
            assert_eq!(out.result, Ok(Finish::Stopped));
        }
    }
}

#[test]
fn a_sink_error_cancels_everything_and_wins_over_a_task_error() {
    let mut m = Model::new(vec![10; 3]);
    m.fail_tasks = vec![7];
    let model = Arc::new(m);
    let mut n = 0;
    let out = Scheduler::new(Arc::clone(&model), cfg(4, 4, true)).run(|_, _| {
        n += 1;
        if n == 2 {
            Err(ModelErr::Internal)
        } else {
            Ok(Flow::Continue)
        }
    });
    assert_eq!(out.result, Err(ModelErr::Internal));
    assert_eq!(model.live_now(), 0);
    assert_eq!(out.summary.workers_joined, 4);
}

#[test]
fn cancel_from_outside_reaches_blocked_tasks_a_blocked_consumer_and_idle_workers() {
    for threads in [1, 2, 4] {
        // two tasks never finish by themselves; the consumer waits for the first
        let mut m = Model::new(vec![6]);
        m.spin_tasks = vec![0, 1];
        let model = Arc::new(m);
        let sched = Scheduler::new(Arc::clone(&model), cfg(threads, 2, true));
        let handle = sched.cancel_handle();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            handle.cancel();
        });
        let out = sched.run(|_, _| Ok(Flow::Continue));
        canceller.join().expect("canceller");
        assert_eq!(out.result, Ok(Finish::Cancelled));
        assert_eq!(out.summary.workers_joined, threads);
        assert_eq!(model.live_now(), 0);
        assert_eq!(Arc::strong_count(&model), 1);
    }
}

#[test]
fn a_panic_in_a_task_is_an_error_not_a_hang() {
    for threads in [1, 2, 4] {
        let mut m = Model::new(vec![5, 5]);
        m.panic_tasks = vec![3];
        let model = Arc::new(m);
        let (_, out) = run_collect(&model, cfg(threads, threads, true));
        assert_eq!(out.result, Err(ModelErr::Internal), "threads {threads}");
        assert_eq!(out.summary.workers_joined, threads);
        assert_eq!(model.live_now(), 0);
    }
}

#[test]
fn a_panic_in_the_sink_still_joins_every_worker() {
    let model = Arc::new(Model::new(vec![50]));
    let m2 = Arc::clone(&model);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let mut n = 0;
        Scheduler::new(m2, cfg(4, 4, true)).run(|_, _| {
            n += 1;
            assert!(n < 3, "sink panics");
            Ok(Flow::Continue)
        })
    }));
    assert!(r.is_err());
    assert_eq!(
        Arc::strong_count(&model),
        1,
        "workers were joined during the unwinding"
    );
    assert_eq!(model.live_now(), 0);
}

#[test]
fn a_run_with_no_files_and_one_with_empty_files_finish() {
    for threads in [1, 8] {
        let (got, out) = run_collect(&Arc::new(Model::new(vec![])), cfg(threads, 1, true));
        assert!(got.is_empty());
        assert_eq!(out.result, Ok(Finish::Complete));
        let (got, out) = run_collect(&Arc::new(Model::new(vec![0; 100])), cfg(threads, 2, true));
        assert!(got.is_empty());
        assert_eq!(out.result, Ok(Finish::Complete));
        assert_eq!(out.summary.files_opened, 100);
    }
}
