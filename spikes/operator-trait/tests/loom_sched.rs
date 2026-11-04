//! Exhaustive interleaving checks of the REAL scheduler (`src/sched.rs`) with `loom`.
//!
//! Run: `RUSTFLAGS="--cfg loom" CARGO_TARGET_DIR=/tmp/nata-loom-target cargo test --release --test loom_sched`
//! (see `bench/results/20261104-loom-scheduler.md` for bounds, results and the mutation table).
//!
//! `sched.rs` compiles against `loom`'s `Arc`/`Mutex`/`Condvar`/atomics/`thread` under `cfg(loom)`
//! (`src/sync.rs`). The operators of `ops.rs` are not compiled in; these tests use tiny sources
//! and sinks defined here, built on loom primitives so the checker sees every synchronization point.
//!
//! Bounds: every test runs 2 worker threads (the minimum for a race between workers) and 1 or 2
//! tasks (c4: 2 pipelines of 1 task). loom explores every interleaving with at most N preemptions
//! (a preemption = the scheduler switches away from a thread that could have continued). N is 3
//! for every test except a1 and a2 (the shutdown tests), which are unbounded; override with
//! `LOOM_MAX_PREEMPTIONS=<n>`. A deadlock (all threads blocked) is a failure.
#![cfg(loom)]

use std::sync::Arc as StdArc;

use loom::model::Builder;
use loom::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use loom::sync::{Arc, Mutex};

use spike_operator_trait::*;

/// Run `f` under every interleaving loom generates. `default_bound` is the preemption bound used
/// when `LOOM_MAX_PREEMPTIONS` is not set; `None` means unbounded (full exploration). Prints the
/// number of executions (distinct interleavings) loom ran, to stderr (`--nocapture`).
fn model(name: &str, default_bound: Option<usize>, f: impl Fn() + Sync + Send + 'static) {
    let runs = StdArc::new(std::sync::atomic::AtomicUsize::new(0));
    let r = StdArc::clone(&runs);
    let mut b = Builder::new(); // reads LOOM_MAX_PREEMPTIONS etc. from the environment
    if b.preemption_bound.is_none() {
        b.preemption_bound = default_bound;
    }
    b.max_branches = 100_000;
    b.check(move || {
        r.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        f();
    });
    eprintln!(
        "[loom] {name}: {} executions, preemption bound {:?}",
        runs.load(std::sync::atomic::Ordering::Relaxed),
        b.preemption_bound
    );
}

// ---------------------------------------------------------------- test fixtures

struct Counters {
    /// `Source::finish` calls: one per task that ended.
    finish: AtomicUsize,
    /// `LocalSink::combine` calls: one per task that ended without cancellation.
    combine: AtomicUsize,
    /// `GlobalSink::finalize` calls: one per finished pipeline.
    finalize: AtomicUsize,
}

impl Counters {
    fn new() -> Arc<Self> {
        Arc::new(Counters {
            finish: AtomicUsize::new(0),
            combine: AtomicUsize::new(0),
            finalize: AtomicUsize::new(0),
        })
    }
}

struct GateState {
    open: bool,
    waker: Option<Waker>,
}

/// A one-shot event a source can block on, with the usual check-then-store-waker under one lock.
struct Gate(Mutex<GateState>);

impl Gate {
    fn new() -> Arc<Self> {
        Arc::new(Gate(Mutex::new(GateState {
            open: false,
            waker: None,
        })))
    }

    fn open(&self) {
        let waker = {
            let mut st = self.0.lock().unwrap();
            st.open = true;
            st.waker.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
    }
}

/// Source of one task: blocks on each gate in order, then finishes; or fails at once.
struct ScriptSource {
    gates: Vec<Arc<Gate>>,
    fail: bool,
    finished: bool,
    c: Arc<Counters>,
}

impl Source for ScriptSource {
    fn next(&mut self, waker: &Waker) -> Result<SourceResult> {
        assert!(!self.finished, "task was run after it ended");
        if self.fail {
            return err("boom");
        }
        for g in &self.gates {
            let mut st = g.0.lock().unwrap();
            if !st.open {
                st.waker = Some(waker.clone());
                return Ok(SourceResult::Blocked);
            }
        }
        Ok(SourceResult::Finished)
    }

    fn finish(&mut self) {
        assert!(!self.finished, "finish() called twice");
        self.finished = true;
        self.c.finish.fetch_add(1, SeqCst);
        // Like an exchange consumer: do not keep a waker (a reference cycle) past the task's end.
        for g in &self.gates {
            g.0.lock().unwrap().waker = None;
        }
    }
}

/// Hands out the scripts of the pipeline's tasks in order (`make` runs on the starting thread).
struct ScriptFactory {
    per_task: Mutex<Vec<(Vec<Arc<Gate>>, bool)>>,
    c: Arc<Counters>,
}

impl SourceFactory for ScriptFactory {
    fn make(&self) -> Box<dyn Source> {
        let (gates, fail) = self.per_task.lock().unwrap().remove(0);
        Box::new(ScriptSource {
            gates,
            fail,
            finished: false,
            c: Arc::clone(&self.c),
        })
    }
}

struct CountSink(Arc<Counters>);
struct CountLocal(Arc<Counters>);

impl GlobalSink for CountSink {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        Box::new(CountLocal(Arc::clone(&self.0)))
    }
    fn finalize(&self) -> Result<()> {
        self.0.finalize.fetch_add(1, SeqCst);
        Ok(())
    }
}

impl LocalSink for CountLocal {
    fn push(&mut self, _input: &mut Batch, _waker: &Waker) -> Result<SinkResult> {
        Ok(SinkResult::NeedMoreInput)
    }
    fn combine(self: Box<Self>) -> Result<()> {
        self.0.combine.fetch_add(1, SeqCst);
        Ok(())
    }
}

/// One pipeline whose tasks follow `per_task` scripts.
fn pipeline(
    name: &str,
    per_task: Vec<(Vec<Arc<Gate>>, bool)>,
    deps: &[usize],
    c: &Arc<Counters>,
) -> Pipeline {
    let tasks = per_task.len();
    Pipeline {
        name: name.into(),
        source: StdArc::new(ScriptFactory {
            per_task: Mutex::new(per_task),
            c: Arc::clone(c),
        }),
        ops: vec![],
        sink: StdArc::new(CountSink(Arc::clone(c))),
        deps: deps.to_vec(),
        tasks,
    }
}

const WORKERS: usize = 2;

// ---------------------------------------------------------------- (a) shutdown / notification

/// Workers are idle on an empty queue (here: there is no work at all) while `wait` shuts down.
#[test]
fn a1_idle_workers_always_terminate_on_an_empty_query() {
    model(
        "a1_idle_workers_always_terminate_on_an_empty_query",
        None,
        || {
            let q = Query::new(vec![]).start(WORKERS).unwrap();
            q.wait().unwrap();
        },
    );
}

/// The same race right after real work: one task finishes, the other worker was idle all along.
#[test]
fn a2_idle_workers_always_terminate_after_a_task_finishes() {
    model(
        "a2_idle_workers_always_terminate_after_a_task_finishes",
        None,
        || {
            let c = Counters::new();
            let p = pipeline("p", vec![(vec![], false)], &[], &c);
            Query::new(vec![p]).start(WORKERS).unwrap().wait().unwrap();
            assert_eq!(c.finish.load(SeqCst), 1);
            assert_eq!(c.combine.load(SeqCst), 1);
            assert_eq!(c.finalize.load(SeqCst), 1);
        },
    );
}

// ---------------------------------------------------------------- (b) task wake protocol

/// One task blocks on two events in turn; the opener (main thread) opens them while the task is
/// Queued, Running or Parked. Every wake must lead to another run; the task ends exactly once.
/// With 2 workers, a task must also never be started while another worker has it (loom-only
/// assertion in `run_task`: a task starts only from `Queued`) nor after `Done` (source assertion).
#[test]
fn b1_wake_while_running_is_never_lost_one_task() {
    model(
        "b1_wake_while_running_is_never_lost_one_task",
        Some(3),
        || {
            let c = Counters::new();
            let (g1, g2) = (Gate::new(), Gate::new());
            let p = pipeline("p", vec![(vec![g1.clone(), g2.clone()], false)], &[], &c);
            let q = Query::new(vec![p]).start(WORKERS).unwrap();
            g1.open();
            g2.open();
            q.wait().unwrap(); // a lost wake would leave the task parked: wait() never returns -> deadlock
            assert_eq!(c.finish.load(SeqCst), 1);
            assert_eq!(c.combine.load(SeqCst), 1);
            assert_eq!(c.finalize.load(SeqCst), 1);
        },
    );
}

/// Two tasks of one pipeline, each with its own event, opened from the main thread.
#[test]
fn b2_wake_while_running_is_never_lost_two_tasks() {
    model(
        "b2_wake_while_running_is_never_lost_two_tasks",
        Some(3),
        || {
            let c = Counters::new();
            let (g1, g2) = (Gate::new(), Gate::new());
            let p = pipeline(
                "p",
                vec![(vec![g1.clone()], false), (vec![g2.clone()], false)],
                &[],
                &c,
            );
            let q = Query::new(vec![p]).start(WORKERS).unwrap();
            g1.open();
            g2.open();
            q.wait().unwrap();
            assert_eq!(c.finish.load(SeqCst), 2);
            assert_eq!(c.combine.load(SeqCst), 2);
            assert_eq!(c.finalize.load(SeqCst), 1);
        },
    );
}

/// The waker is called from a thread other than the opener's: a separate producer thread opens the
/// event while the main thread already waits.
#[test]
fn b3_wake_from_a_third_thread() {
    model("b3_wake_from_a_third_thread", Some(3), || {
        let c = Counters::new();
        let g = Gate::new();
        let p = pipeline("p", vec![(vec![g.clone()], false)], &[], &c);
        let q = Query::new(vec![p]).start(WORKERS).unwrap();
        let g2 = Arc::clone(&g);
        let t = loom::thread::spawn(move || g2.open());
        q.wait().unwrap();
        t.join().unwrap();
        assert_eq!(c.finish.load(SeqCst), 1);
        assert_eq!(c.finalize.load(SeqCst), 1);
    });
}

// ---------------------------------------------------------------- (c) cancellation vs completion

/// A task that finishes by itself races with `cancel`: the result is Ok or the cancellation, the
/// task ends exactly once, `finalize` runs at most once and, if the query reports Ok, exactly once.
#[test]
fn c1_cancel_races_with_completion() {
    model("c1_cancel_races_with_completion", Some(3), || {
        let c = Counters::new();
        let p = pipeline("p", vec![(vec![], false)], &[], &c);
        let q = Query::new(vec![p]).start(WORKERS).unwrap();
        q.cancel();
        let r = q.wait();
        assert_eq!(c.finish.load(SeqCst), 1, "the task must end exactly once");
        let fin = c.finalize.load(SeqCst);
        assert!(fin <= 1);
        match r {
            Ok(()) => assert_eq!((fin, c.combine.load(SeqCst)), (1, 1)),
            Err(e) => assert_eq!(e, QueryError("cancelled".into())),
        }
    });
}

/// The task is blocked on an event that never fires; `cancel` must wake it wherever it is
/// (Queued, Running about to park, Parked) so that it ends. The result is the cancellation.
#[test]
fn c2_cancel_wakes_a_blocked_task() {
    model("c2_cancel_wakes_a_blocked_task", Some(3), || {
        let c = Counters::new();
        let g = Gate::new(); // never opened
        let p = pipeline("p", vec![(vec![g.clone()], false)], &[], &c);
        let q = Query::new(vec![p]).start(WORKERS).unwrap();
        q.cancel();
        let r = q.wait();
        assert_eq!(r, Err(QueryError("cancelled".into())));
        assert_eq!(c.finish.load(SeqCst), 1);
        assert_eq!(
            c.combine.load(SeqCst),
            0,
            "a cancelled task must not combine"
        );
        assert_eq!(c.finalize.load(SeqCst), 0);
    });
}

/// A task failing by itself races with `cancel`: exactly one error is reported (the first), the
/// query ends, the task ends once.
#[test]
fn c3_cancel_races_with_an_error() {
    model("c3_cancel_races_with_an_error", Some(3), || {
        let c = Counters::new();
        let p = pipeline("p", vec![(vec![], true)], &[], &c);
        let q = Query::new(vec![p]).start(WORKERS).unwrap();
        q.cancel();
        let r = q.wait();
        assert_eq!(c.finish.load(SeqCst), 1);
        assert_eq!(c.finalize.load(SeqCst), 0);
        let e = r.unwrap_err();
        assert!(
            e == QueryError("boom".into()) || e == QueryError("cancelled".into()),
            "{e}"
        );
    });
}

/// Two pipelines with a dependency: `cancel` races with the dependent pipeline being started.
/// No task may be left behind, and a started pipeline is finalized at most once.
#[test]
fn c4_cancel_races_with_a_dependent_pipeline_start() {
    model(
        "c4_cancel_races_with_a_dependent_pipeline_start",
        Some(3),
        || {
            let (ca, cb) = (Counters::new(), Counters::new());
            let a = pipeline("a", vec![(vec![], false)], &[], &ca);
            let b = pipeline("b", vec![(vec![], false)], &[0], &cb);
            let q = Query::new(vec![a, b]).start(WORKERS).unwrap();
            q.cancel();
            let r = q.wait();
            // Each pipeline's tasks ended as often as they were started (0 or 1), never twice.
            assert!(ca.finish.load(SeqCst) == 1);
            assert!(cb.finish.load(SeqCst) <= 1);
            assert!(ca.finalize.load(SeqCst) <= 1 && cb.finalize.load(SeqCst) <= 1);
            match r {
                Ok(()) => assert_eq!((ca.finalize.load(SeqCst), cb.finalize.load(SeqCst)), (1, 1)),
                Err(e) => assert_eq!(e, QueryError("cancelled".into())),
            }
        },
    );
}
