//! A small scheduler for the protocol in `api.rs`: a pool of OS threads running worker tasks.
//!
//! Task life cycle: `Queued -> Running -> (Parked | Queued | Done)`. A task that reports
//! `Blocked` is parked; `Waker::wake` moves it back to the queue. A wake that arrives while the
//! task is still running (after it stored the waker, before it parked) is remembered in
//! `notified`, so no wake-up is lost.

use std::collections::VecDeque;
use std::sync::atomic::AtomicU64;

use crate::proto::api::*;
use crate::batch::Batch;
// The synchronization primitives come from `crate::sync`: std normally, `loom` under `--cfg loom`.
// The trait objects of `api.rs` (`Arc<dyn SourceFactory>` and friends) stay on std's `Arc`.
use crate::proto::sync::Ordering::SeqCst;
use crate::proto::sync::thread::JoinHandle;
use crate::proto::sync::{Arc, AtomicBool, AtomicUsize, Condvar, Mutex, MutexGuard};

/// Test hook: microseconds a worker sleeps, holding the queue lock, between seeing an empty queue
/// and waiting for work. It widens the window in which a lost notification can happen, so a test
/// can hit that race every time instead of one run in a thousand. Zero (the default) disables it.
/// Always a std atomic (a loom atomic cannot live in a `static`); the loom build ignores it, since
/// loom explores the interleavings itself.
pub static TEST_WORKER_DELAY_US: AtomicU64 = AtomicU64::new(0);

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
    /// ARROW-PIPELINE CHANGE (see bench/results/20261104-arrow-pipeline.md): how many leading `inputs` slots this
    /// task's early finish made useless (`i + 1` for operator `i`, `ops.len() + 1` for the sink).
    finish_depth: usize,
}

struct TaskCell {
    shared: Arc<Shared>,
    pipeline: usize,
    state: Mutex<TaskState>,
    body: Mutex<TaskBody>,
}

struct PipelineRt {
    name: String,
    source: std::sync::Arc<dyn SourceFactory>,
    ops: Vec<std::sync::Arc<dyn OperatorFactory>>,
    sink: std::sync::Arc<dyn GlobalSink>,
    tasks: usize,
    dependents: Vec<usize>,
    remaining_deps: AtomicUsize,
    remaining_tasks: AtomicUsize,
    /// Set when the pipeline needs no more input (`LIMIT`, closed consumer).
    stop: AtomicBool,
    /// ARROW-PIPELINE CHANGE: with `stop`, the largest `finish_depth` any task reported. Input pending in a stage
    /// BEFORE that depth is not needed any more and is dropped by every task; input pending AFTER it has already
    /// passed the finishing operator (a LIMIT admitted it) and must still reach the sink. The prototype had only
    /// the bool, so a task parked on a blocked sink dropped admitted rows when another task stopped the pipeline.
    stop_depth: AtomicUsize,
}

struct Shared {
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
            // Mutant for the loom validation: cancellation forgets to wake the tasks.
            if !cfg!(feature = "bug-cancel-does-not-wake") {
                TaskCell::wake(&t);
            }
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
            TaskCell::wake(&t);
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

    fn start_pipeline(this: &Arc<Self>, p: usize) {
        let rt = &this.pipelines[p];
        rt.remaining_tasks.store(rt.tasks, SeqCst);
        this.live_tasks.fetch_add(rt.tasks, SeqCst);
        for i in 0..rt.tasks {
            let body = TaskBody {
                source: rt.source.make(),
                ops: rt.ops.iter().map(|f| f.make()).collect(),
                sink: Some(rt.sink.make(i)),
                inputs: (0..=rt.ops.len()).map(|_| None).collect(),
                finishing: false,
                finish_depth: 0,
            };
            let cell = Arc::new(TaskCell {
                shared: Arc::clone(this),
                pipeline: p,
                state: Mutex::new(TaskState::Queued),
                body: Mutex::new(body),
            });
            lock(&this.all_tasks).push(Arc::clone(&cell));
            this.enqueue(cell);
        }
    }

    fn pipeline_done(this: &Arc<Self>, p: usize) {
        this.pending_pipelines.fetch_sub(1, SeqCst);
        for &d in &this.pipelines[p].dependents {
            if this.pipelines[d].remaining_deps.fetch_sub(1, SeqCst) == 1
                && !this.cancelled.load(SeqCst)
            {
                Shared::start_pipeline(this, d);
            }
        }
        this.check_complete();
    }
}

impl TaskCell {
    fn waker(this: &Arc<Self>) -> Waker {
        let me = Arc::clone(this);
        Waker::new(move || TaskCell::wake(&me))
    }

    fn wake(this: &Arc<Self>) {
        let mut st = lock(&this.state);
        match &mut *st {
            TaskState::Parked => {
                *st = TaskState::Queued;
                drop(st);
                this.shared.enqueue(Arc::clone(this));
            }
            TaskState::Running { notified } => {
                // Mutant for the loom validation: forget a wake that arrives while running.
                if !cfg!(feature = "bug-wake-ignored-while-running") {
                    *notified = true;
                }
                // Mutant: also queue the task although a worker is running it.
                if cfg!(feature = "bug-wake-enqueues-running") {
                    drop(st);
                    this.shared.enqueue(Arc::clone(this));
                }
            }
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
                        self.finish_depth = self.finish_depth.max(n + 1);
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
                    self.finish_depth = self.finish_depth.max(i + 1);
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
        loop {
            if shared.cancelled.load(SeqCst) {
                return Ok(Step::Done);
            }
            if p.stop.load(SeqCst) {
                // Another task stopped the pipeline. Drop what is pending before the finishing stage; whatever
                // is pending after it was admitted already and is still delivered (then this task ends).
                let depth = p.stop_depth.load(SeqCst).min(self.inputs.len());
                self.inputs[..depth].iter_mut().for_each(|x| *x = None);
                self.finishing = true;
            }
            match self.advance(waker)? {
                Flow::Blocked => return Ok(Step::Blocked),
                Flow::Stop => {
                    p.stop_depth.fetch_max(self.finish_depth, SeqCst);
                    p.stop.store(true, SeqCst);
                    return Ok(Step::Done);
                }
                Flow::NeedSource => match self.source.next(waker)? {
                    SourceResult::Batch(b) => self.inputs[0] = Some(b),
                    SourceResult::Blocked => return Ok(Step::Blocked),
                    SourceResult::Finished => return Ok(Step::Done),
                },
            }
        }
    }
}

fn run_task(shared: &Arc<Shared>, task: Arc<TaskCell>) {
    {
        let mut st = lock(&task.state);
        // Only checked by the loom tests: a task may start only from `Queued` (never while another
        // worker runs it, never after `Done`).
        #[cfg(loom)]
        assert!(
            matches!(*st, TaskState::Queued),
            "task started from a state other than Queued"
        );
        *st = TaskState::Running { notified: false };
    }
    let waker = TaskCell::waker(&task);
    let p = &shared.pipelines[task.pipeline];
    let result = lock(&task.body).run(&waker, shared, p);
    match result {
        Ok(Step::Blocked) => {
            let mut st = lock(&task.state);
            // Mutant for the loom validation: clear `Running` without looking at `notified`.
            let notified = matches!(*st, TaskState::Running { notified: true })
                && !cfg!(feature = "bug-park-ignores-notified");
            if notified {
                *st = TaskState::Queued;
                drop(st);
                shared.enqueue(task);
            } else {
                *st = TaskState::Parked;
            }
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
            Ok(()) => Shared::pipeline_done(shared, task.pipeline),
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
                #[cfg(not(loom))]
                {
                    let delay = TEST_WORKER_DELAY_US.load(SeqCst);
                    if delay > 0 {
                        std::thread::sleep(std::time::Duration::from_micros(delay));
                    }
                }
                q = shared.queue_cv.wait(q).unwrap_or_else(|e| e.into_inner());
            }
        };
        run_task(&shared, task);
    }
}

pub struct Query {
    pipelines: Vec<Pipeline>,
}

pub struct RunningQuery {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl Query {
    pub fn new(pipelines: Vec<Pipeline>) -> Self {
        Query { pipelines }
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
                stop_depth: AtomicUsize::new(0),
            })
            .collect();
        let shared = Arc::new(Shared {
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
                crate::proto::sync::thread::spawn(move || worker(s))
            })
            .collect();
        for r in roots {
            Shared::start_pipeline(&shared, r);
        }
        if n == 0 {
            *lock(&shared.done) = true;
        }
        Ok(RunningQuery { shared, workers })
    }
}

impl RunningQuery {
    /// Ask the query to stop. `wait` then returns the cancellation error.
    pub fn cancel(&self) {
        self.shared.fail(QueryError("cancelled".into()));
    }

    pub fn wait(self) -> Result<()> {
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
        if cfg!(feature = "bug-lost-shutdown") {
            // Mutant for the loom validation: the original bug, flag set outside the lock.
            self.shared.shutdown.store(true, SeqCst);
        } else {
            let _queue = lock(&self.shared.queue);
            self.shared.shutdown.store(true, SeqCst);
        }
        self.shared.queue_cv.notify_all();
        for w in self.workers {
            let _ = w.join();
        }
        // Only checked by the loom tests: when `wait` returns, the queue is empty and every task
        // is `Done` (none is left Queued, Running or Parked).
        #[cfg(loom)]
        {
            assert!(
                lock(&self.shared.queue).is_empty(),
                "task left in the queue"
            );
            for t in lock(&self.shared.all_tasks).iter() {
                assert!(
                    matches!(*lock(&t.state), TaskState::Done),
                    "task not Done when wait() returned"
                );
            }
        }
        // Break the reference cycles (tasks hold the scheduler; wakers hold tasks).
        lock(&self.shared.queue).clear();
        lock(&self.shared.all_tasks).clear();
        let err = lock(&self.shared.error).take();
        match err {
            Some(e) => Err(e),
            None => Ok(()),
        }
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
