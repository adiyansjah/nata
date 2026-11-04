//! A small scheduler for the protocol in `api.rs`: a pool of OS threads running worker tasks.
//!
//! Task life cycle: `Queued -> Running -> (Parked | Queued | Done)`. A task that reports
//! `Blocked` is parked; `Waker::wake` moves it back to the queue. A wake that arrives while the
//! task is still running (after it stored the waker, before it parked) is remembered in
//! `notified`, so no wake-up is lost.
//!
//! Fairness (added by the fairness spike): operators are synchronous and cannot be preempted, so
//! fairness is cooperative, at morsel boundaries. A task runs for a bounded *slice* (a number of
//! morsels, and/or a time budget checked between morsels); then, if another task is waiting, it
//! returns `Step::Yield` and is re-queued at the BACK of the run queue. `Yield` is a fourth way to
//! leave `Running`: `Running -> Queued` (never `Parked`, never `Done`), see `run_task`.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::time::{Duration, Instant};

use crate::sync::thread::{self, JoinHandle};
use crate::sync::{AtomicBool, AtomicUsize, Condvar, Mutex, MutexGuard};

use crate::api::*;
use crate::batch::Batch;

/// Test hook: microseconds a worker sleeps, holding the queue lock, between seeing an empty queue
/// and waiting for work. It widens the window in which a lost notification can happen, so a test
/// can hit that race every time instead of one run in a thousand. Zero (the default) disables it.
pub static TEST_WORKER_DELAY_US: AtomicU64 = AtomicU64::new(0);

/// How long a task may run before it must give the pool to other runnable tasks.
///
/// A slice ends at the first morsel boundary where `max_morsels` morsels have been processed or
/// `max_time` has elapsed since the slice began (whichever comes first). At least one morsel is
/// always processed per slice, so a task makes progress however small the budget is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SliceConfig {
    /// Morsels (batches taken from the source) per slice. `usize::MAX` = no limit. Minimum 1.
    pub max_morsels: usize,
    /// Wall-clock budget per slice, checked between morsels only. `None` = no limit.
    pub max_time: Option<Duration>,
}

impl SliceConfig {
    /// No time slicing: a task keeps its worker until it blocks or finishes (the behaviour before
    /// the fairness change; the control arm of the tests and measurements).
    pub const fn unlimited() -> Self {
        SliceConfig {
            max_morsels: usize::MAX,
            max_time: None,
        }
    }

    pub const fn morsels(n: usize) -> Self {
        SliceConfig {
            max_morsels: if n == 0 { 1 } else { n },
            max_time: None,
        }
    }

    fn exhausted(&self, morsels: usize, started: Option<Instant>) -> bool {
        morsels >= self.max_morsels
            || matches!((self.max_time, started), (Some(t), Some(s)) if s.elapsed() >= t)
    }
}

impl Default for SliceConfig {
    /// 32 morsels or 1 ms, whichever comes first.
    fn default() -> Self {
        SliceConfig {
            max_morsels: 32,
            max_time: Some(Duration::from_millis(1)),
        }
    }
}

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

enum TaskState {
    Queued,
    Running { notified: bool },
    Parked,
    Done,
}

struct TaskBody {
    source: Box<dyn Source>,
    ops: Vec<Box<dyn Operator>>,
    sink: Option<Box<dyn LocalSink>>,
    /// `inputs[i]` is the batch waiting to be processed by stage `i` (the operators, then the sink).
    inputs: Vec<Option<Batch>>,
    finishing: bool,
}

struct TaskCell {
    shared: Arc<Shared>,
    pipeline: usize,
    state: Mutex<TaskState>,
    body: Mutex<TaskBody>,
    /// Added for the async-bridge spike: set while a worker is inside `run_task` for this task.
    #[cfg(any(nata_loom, feature = "invariants"))]
    in_run: AtomicBool,
}

struct PipelineRt {
    name: String,
    source: Arc<dyn SourceFactory>,
    ops: Vec<Arc<dyn OperatorFactory>>,
    sink: Arc<dyn GlobalSink>,
    tasks: usize,
    dependents: Vec<usize>,
    remaining_deps: AtomicUsize,
    remaining_tasks: AtomicUsize,
    /// Set when the pipeline needs no more input (`LIMIT`, closed consumer).
    stop: AtomicBool,
}

struct Shared {
    slice: SliceConfig,
    /// Number of times a task gave up its worker at the end of a slice (statistics; also a test hook).
    yields: AtomicUsize,
    queue: Mutex<VecDeque<Arc<TaskCell>>>,
    queue_cv: Condvar,
    pipelines: Vec<PipelineRt>,
    all_tasks: Mutex<Vec<Arc<TaskCell>>>,
    cancelled: AtomicBool,
    error: Mutex<Option<QueryError>>,
    live_tasks: AtomicUsize,
    pending_pipelines: AtomicUsize,
    done: Mutex<bool>,
    done_cv: Condvar,
    shutdown: AtomicBool,
}

impl Shared {
    fn queue_is_empty(&self) -> bool {
        lock(&self.queue).is_empty()
    }

    fn enqueue(&self, t: Arc<TaskCell>) {
        lock(&self.queue).push_back(t);
        self.queue_cv.notify_one();
    }

    fn fail(&self, e: QueryError) {
        {
            let mut slot = lock(&self.error);
            if slot.is_none() {
                *slot = Some(e);
            }
        }
        self.cancel();
    }

    fn cancel(&self) {
        self.cancelled.store(true, SeqCst);
        // Parked tasks must run once more to notice the cancellation and finish.
        let tasks: Vec<_> = lock(&self.all_tasks).clone();
        for t in tasks {
            t.wake();
        }
        self.check_complete();
    }

    fn wake_pipeline(&self, p: usize) {
        let tasks: Vec<_> = lock(&self.all_tasks)
            .iter()
            .filter(|t| t.pipeline == p)
            .cloned()
            .collect();
        for t in tasks {
            t.wake();
        }
    }

    fn check_complete(&self) {
        let finished = if self.cancelled.load(SeqCst) {
            self.live_tasks.load(SeqCst) == 0
        } else {
            self.pending_pipelines.load(SeqCst) == 0
        };
        if finished {
            *lock(&self.done) = true;
            self.done_cv.notify_all();
        }
    }

    fn start_pipeline(self: &Arc<Self>, p: usize) {
        let rt = &self.pipelines[p];
        rt.remaining_tasks.store(rt.tasks, SeqCst);
        self.live_tasks.fetch_add(rt.tasks, SeqCst);
        for i in 0..rt.tasks {
            let body = TaskBody {
                source: rt.source.make(),
                ops: rt.ops.iter().map(|f| f.make()).collect(),
                sink: Some(rt.sink.make(i)),
                inputs: (0..=rt.ops.len()).map(|_| None).collect(),
                finishing: false,
            };
            let cell = Arc::new(TaskCell {
                shared: Arc::clone(self),
                pipeline: p,
                state: Mutex::new(TaskState::Queued),
                body: Mutex::new(body),
                #[cfg(any(nata_loom, feature = "invariants"))]
                in_run: AtomicBool::new(false),
            });
            lock(&self.all_tasks).push(Arc::clone(&cell));
            self.enqueue(cell);
        }
    }

    fn pipeline_done(self: &Arc<Self>, p: usize) {
        self.pending_pipelines.fetch_sub(1, SeqCst);
        for &d in &self.pipelines[p].dependents {
            if self.pipelines[d].remaining_deps.fetch_sub(1, SeqCst) == 1
                && !self.cancelled.load(SeqCst)
            {
                self.start_pipeline(d);
            }
        }
        self.check_complete();
    }
}

impl TaskCell {
    fn waker(self: &Arc<Self>) -> Waker {
        let me = Arc::clone(self);
        Waker::new(move || me.wake())
    }

    fn wake(self: &Arc<Self>) {
        let mut st = lock(&self.state);
        match &mut *st {
            TaskState::Parked => {
                *st = TaskState::Queued;
                drop(st);
                self.shared.enqueue(Arc::clone(self));
            }
            TaskState::Running { notified } => *notified = true,
            TaskState::Queued | TaskState::Done => {}
        }
    }
}

enum Flow {
    NeedSource,
    Blocked,
    Stop,
}

enum Step {
    Done,
    Blocked,
    /// The slice is used up and another task is waiting: re-queue at the back.
    Yield,
}

impl TaskBody {
    /// Run the stages until something is blocked, finished, or the source must be asked again.
    /// The deepest stage with pending input always runs first, so a stage's output slot is empty
    /// whenever the stage runs.
    fn advance(&mut self, waker: &Waker) -> Result<Flow> {
        let n = self.ops.len();
        loop {
            let Some(i) = (0..=n).rev().find(|&i| self.inputs[i].is_some()) else {
                return Ok(if self.finishing {
                    Flow::Stop
                } else {
                    Flow::NeedSource
                });
            };
            if i == n {
                let mut batch = self.inputs[n].take().expect("pending");
                let sink = self.sink.as_mut().expect("sink present while running");
                match sink.push(&mut batch, waker)? {
                    SinkResult::NeedMoreInput => {}
                    SinkResult::Finished => {
                        self.finishing = true;
                        self.inputs.iter_mut().for_each(|x| *x = None);
                    }
                    SinkResult::Blocked => {
                        self.inputs[n] = Some(batch);
                        return Ok(Flow::Blocked);
                    }
                }
                continue;
            }
            let input = self.inputs[i].take().expect("pending");
            let mut out = Batch::default();
            match self.ops[i].execute(&input, &mut out)? {
                OpResult::NeedMoreInput => {}
                OpResult::HaveMoreOutput => {
                    if out.is_empty() {
                        return err("operator returned HaveMoreOutput without producing output");
                    }
                    self.inputs[i] = Some(input);
                }
                OpResult::Finished => {
                    self.finishing = true;
                    // Nothing upstream of (and including) this stage is needed any more.
                    self.inputs[..=i].iter_mut().for_each(|x| *x = None);
                }
            }
            if !out.is_empty() {
                debug_assert!(self.inputs[i + 1].is_none());
                self.inputs[i + 1] = Some(out);
            }
        }
    }

    fn run(&mut self, waker: &Waker, shared: &Shared, p: &PipelineRt) -> Result<Step> {
        let slice = shared.slice;
        let mut morsels = 0usize;
        let mut started = slice_clock(&slice);
        loop {
            if shared.cancelled.load(SeqCst) || p.stop.load(SeqCst) {
                return Ok(Step::Done);
            }
            match self.advance(waker)? {
                Flow::Blocked => return Ok(Step::Blocked),
                Flow::Stop => {
                    p.stop.store(true, SeqCst);
                    return Ok(Step::Done);
                }
                Flow::NeedSource => {
                    // A morsel boundary: every stage is drained, so nothing is half-processed.
                    if morsels >= 1 && slice.exhausted(morsels, started) {
                        if !shared.queue_is_empty() {
                            return Ok(Step::Yield);
                        }
                        // Nobody is waiting: start a new slice instead of paying for a round trip
                        // through the queue. A task that becomes runnable from now on waits at
                        // most one more slice.
                        morsels = 0;
                        started = slice_clock(&slice);
                    }
                    match self.source.next(waker)? {
                        SourceResult::Batch(b) => {
                            self.inputs[0] = Some(b);
                            morsels += 1;
                        }
                        SourceResult::Blocked => return Ok(Step::Blocked),
                        SourceResult::Finished => return Ok(Step::Done),
                    }
                }
            }
        }
    }
}

fn run_task(shared: &Arc<Shared>, task: Arc<TaskCell>) {
    #[cfg(any(nata_loom, feature = "invariants"))]
    assert!(
        !task.in_run.swap(true, SeqCst),
        "invariant violated: task run by two workers at once"
    );
    {
        let mut st = lock(&task.state);
        // Added for the fairness spike: a task is only ever started from `Queued` (so never from
        // `Parked`, never twice, never after `Done`).
        #[cfg(any(nata_loom, feature = "invariants"))]
        assert!(
            matches!(*st, TaskState::Queued),
            "invariant violated: task started from a state other than Queued"
        );
        *st = TaskState::Running { notified: false };
    }
    let waker = task.waker();
    let p = &shared.pipelines[task.pipeline];
    let result = lock(&task.body).run(&waker, shared, p);
    match result {
        Ok(Step::Blocked) => {
            let mut st = lock(&task.state);
            // Cleared while holding the state lock, before the task becomes runnable again.
            #[cfg(any(nata_loom, feature = "invariants"))]
            task.in_run.store(false, SeqCst);
            let notified = matches!(*st, TaskState::Running { notified: true });
            if notified {
                *st = TaskState::Queued;
                drop(st);
                shared.enqueue(task);
            } else {
                *st = TaskState::Parked;
            }
        }
        Ok(Step::Yield) => {
            let mut st = lock(&task.state);
            // Cleared while holding the state lock, before the task becomes runnable again.
            #[cfg(any(nata_loom, feature = "invariants"))]
            task.in_run.store(false, SeqCst);
            // A wake that arrived while Running (`notified`) is subsumed: the task is runnable
            // anyway. Not `Parked`: nobody would wake a task that is not waiting for anything.
            *st = TaskState::Queued;
            drop(st);
            shared.yields.fetch_add(1, SeqCst);
            shared.enqueue(task); // back of the queue
        }
        Ok(Step::Done) => finish_task(shared, &task),
        Err(e) => {
            shared.fail(e);
            finish_task(shared, &task);
        }
    }
}

fn finish_task(shared: &Arc<Shared>, task: &Arc<TaskCell>) {
    let p = &shared.pipelines[task.pipeline];
    let local = {
        let mut body = lock(&task.body);
        body.source.finish();
        body.sink.take()
    };
    let stopped_early = p.stop.load(SeqCst);
    if let Some(local) = local {
        if shared.cancelled.load(SeqCst) {
            drop(local);
        } else if let Err(e) = local.combine() {
            shared.fail(e);
        }
    }
    *lock(&task.state) = TaskState::Done;
    if stopped_early {
        shared.wake_pipeline(task.pipeline);
    }
    if p.remaining_tasks.fetch_sub(1, SeqCst) == 1 && !shared.cancelled.load(SeqCst) {
        match p.sink.finalize() {
            Ok(()) => shared.pipeline_done(task.pipeline),
            Err(e) => shared.fail(e),
        }
    }
    shared.live_tasks.fetch_sub(1, SeqCst);
    shared.check_complete();
}

fn worker(shared: Arc<Shared>) {
    loop {
        let task = {
            let mut q = lock(&shared.queue);
            loop {
                if let Some(t) = q.pop_front() {
                    break t;
                }
                if shared.shutdown.load(SeqCst) {
                    return;
                }
                let delay = TEST_WORKER_DELAY_US.load(SeqCst);
                if delay > 0 {
                    std::thread::sleep(std::time::Duration::from_micros(delay));
                }
                q = shared.queue_cv.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        run_task(&shared, task);
    }
}

pub struct Query {
    pipelines: Vec<Pipeline>,
    slice: SliceConfig,
}

pub struct RunningQuery {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl Query {
    pub fn new(pipelines: Vec<Pipeline>) -> Self {
        Query {
            pipelines,
            slice: default_slice(),
        }
    }

    /// Set the time-slice policy (default: [`SliceConfig::default`]).
    pub fn with_slice(mut self, slice: SliceConfig) -> Self {
        slice_check(&slice);
        self.slice = slice;
        self
    }

    pub fn run(self, threads: usize) -> Result<()> {
        self.start(threads)?.wait()
    }

    pub fn start(self, threads: usize) -> Result<RunningQuery> {
        let n = self.pipelines.len();
        let mut dependents = vec![Vec::new(); n];
        for (i, p) in self.pipelines.iter().enumerate() {
            if p.tasks == 0 {
                return err(format!("pipeline {} has no tasks", p.name));
            }
            for &d in &p.deps {
                if d >= n || d == i {
                    return err(format!("pipeline {} has a bad dependency {d}", p.name));
                }
                dependents[d].push(i);
            }
        }
        let pipelines: Vec<PipelineRt> = self
            .pipelines
            .into_iter()
            .zip(dependents)
            .map(|(p, dependents)| PipelineRt {
                name: p.name,
                source: p.source,
                ops: p.ops,
                sink: p.sink,
                tasks: p.tasks,
                dependents,
                remaining_deps: AtomicUsize::new(p.deps.len()),
                remaining_tasks: AtomicUsize::new(0),
                stop: AtomicBool::new(false),
            })
            .collect();
        let shared = Arc::new(Shared {
            slice: self.slice,
            yields: AtomicUsize::new(0),
            queue: Mutex::new(VecDeque::new()),
            queue_cv: Condvar::new(),
            pending_pipelines: AtomicUsize::new(pipelines.len()),
            pipelines,
            all_tasks: Mutex::new(Vec::new()),
            cancelled: AtomicBool::new(false),
            error: Mutex::new(None),
            live_tasks: AtomicUsize::new(0),
            done: Mutex::new(false),
            done_cv: Condvar::new(),
            shutdown: AtomicBool::new(false),
        });
        // Dependencies must form a DAG: at least one pipeline must be startable.
        let roots: Vec<usize> = (0..n)
            .filter(|&i| shared.pipelines[i].remaining_deps.load(SeqCst) == 0)
            .collect();
        if roots.is_empty() && n > 0 {
            return err("pipeline dependencies contain a cycle");
        }
        let workers = (0..threads.max(1))
            .map(|_| {
                let s = Arc::clone(&shared);
                thread::spawn(move || worker(s))
            })
            .collect();
        for r in roots {
            shared.start_pipeline(r);
        }
        if n == 0 {
            *lock(&shared.done) = true;
        }
        Ok(RunningQuery { shared, workers })
    }
}

/// Test hook (feature `slice-env`, off by default): `NATA_SLICE_MORSELS=n` makes every `Query::new`
/// use `n` morsels per slice and no time budget, so the whole existing test suite can be re-run with
/// a slice of one morsel, which exercises the yield path at every morsel boundary.
fn default_slice() -> SliceConfig {
    #[cfg(feature = "slice-env")]
    if let Some(n) = std::env::var("NATA_SLICE_MORSELS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        return SliceConfig::morsels(n);
    }
    SliceConfig::default()
}

/// Start of a slice for the time budget. The model checker has no clock, so under `--cfg nata_loom`
/// only the morsel budget exists (the time budget changes when the yield decision fires, not what
/// the yield does).
fn slice_clock(slice: &SliceConfig) -> Option<Instant> {
    #[cfg(not(nata_loom))]
    return slice.max_time.map(|_| Instant::now());
    #[cfg(nata_loom)]
    {
        let _ = slice;
        None
    }
}

fn slice_check(s: &SliceConfig) {
    assert!(s.max_morsels >= 1, "a slice must allow at least one morsel");
}

impl RunningQuery {
    /// How many times a task gave up its worker at the end of a slice so far.
    pub fn yields(&self) -> usize {
        self.shared.yields.load(SeqCst)
    }

    /// Ask the query to stop. `wait` then returns the cancellation error.
    pub fn cancel(&self) {
        self.shared.fail(QueryError("cancelled".into()));
    }

    pub fn wait(self) -> Result<()> {
        self.wait_with_stats().0
    }

    /// Like [`wait`](Self::wait), also returning the total number of yields.
    pub fn wait_with_stats(self) -> (Result<()>, usize) {
        {
            let mut done = lock(&self.shared.done);
            while !*done {
                done = self
                    .shared
                    .done_cv
                    .wait(done)
                    .unwrap_or_else(|e| e.into_inner());
            }
        }
        // Set the flag while holding the queue lock the workers wait on. Without the lock a worker
        // can check the flag, then miss the notification, and sleep forever.
        {
            let _queue = lock(&self.shared.queue);
            self.shared.shutdown.store(true, SeqCst);
        }
        self.shared.queue_cv.notify_all();
        for w in self.workers {
            let _ = w.join();
        }
        // Break the reference cycles (tasks hold the scheduler; wakers hold tasks).
        lock(&self.shared.queue).clear();
        lock(&self.shared.all_tasks).clear();
        let err = lock(&self.shared.error).take();
        let yields = self.shared.yields.load(SeqCst);
        (
            match err {
                Some(e) => Err(e),
                None => Ok(()),
            },
            yields,
        )
    }

    /// Names of the pipelines, for diagnostics.
    pub fn pipeline_names(&self) -> Vec<String> {
        self.shared
            .pipelines
            .iter()
            .map(|p| p.name.clone())
            .collect()
    }
}
