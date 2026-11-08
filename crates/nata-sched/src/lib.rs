//! Ordered, windowed, cancellable execution of row-group tasks on worker threads: the scheduling core of the parallel scan (slice 3, step 2b).
//!
//! # Shape
//!
//! ```text
//!   files (lazy)                 workers (N threads)                        consumer (the caller)
//!   open(file i) -> tasks   -->  claim: in key order, under ONE lock,  -->  take results in sequence order (or any order),
//!                                a window of W outstanding results         hand each to the sink, then release its window slot
//! ```
//!
//! * The work is a list of FILES. A file is opened (footer read, kept row groups decided) by the worker that claims it; the tasks of an opened file are
//!   claimed one by one. Everything is claimed **in key order** (file, then row group) and every claimed task gets the next sequence number `seq`, so the
//!   sequence is dense and is the order of the single-thread scan. A file whose footer cannot be read gets a sequence number too (its error has a place).
//! * **Window.** `claimed - released < window` is checked before every claim (a task or the opening of a file). `released` counts results the consumer is
//!   done with. So at most `window` results are in flight, finished and waiting, or in the consumer's hands: the reorder buffer cannot grow, whatever
//!   the speed of the slowest task. Liveness in ordered mode: the lowest unreleased sequence number was claimed before all others, so it is running or
//!   finished and its result is taken first; a full window never blocks the task the consumer is waiting for.
//! * **Order.** `ordered`: the consumer takes sequence numbers 0, 1, 2, ... (identical to the single-thread scan, for every thread count). Unordered: it
//!   takes whatever is finished.
//! * **Errors.** A failed task (or file) stores its error as its result, stops all further claims and tells the tasks with a HIGHER sequence number to stop
//!   (`abandon_from`). Tasks with a lower number run on, so the error that is reported is always the one of the lowest sequence number, i.e. the lowest
//!   (file, row group): ordered mode takes results in that order, unordered mode keeps the minimum.
//! * **Cancellation** is one atomic, `abandon_from`: a task with `seq >= abandon_from` is cancelled. The consumer cancels everything (`Flow::Stop` for a
//!   limit, a sink error for a closed output), a task error cancels the tasks above it, [`CancelHandle::cancel`] cancels from outside. Tasks look at
//!   their [`CancelToken`] between batches; workers look before every claim; a worker waiting for the window or the opener is woken. [`Scheduler::run`]
//!   returns only after every worker thread has been joined and every undelivered result and unclaimed task has been dropped (also when the sink panics).
//! * **Footers ahead** (`Config::open_ahead`, register item P2). With `open_ahead = 1` files are opened one at a time, and only when no task is queued
//!   (slice 3). With `open_ahead = K` up to K files may be *pending* at once: being opened by a worker, opened and waiting for an earlier file, or queued with
//!   tasks not yet claimed. A worker with nothing to run opens the next file while others run earlier row groups, so many small files do not wait for one
//!   footer read after the other. The results of the opens are installed into the queue **in file order** (a file that finishes early waits in `ready`), so
//!   the sequence numbers, and with them the ordered output and the lowest-error rule, are exactly those of `open_ahead = 1`. A file that cannot be opened
//!   is queued in its place as a failure; it takes its sequence number when the workers reach it. Pending files are bounded by K whatever the number of files.
//! * **Parking** (ADR 0011, ADR 0009's sticky wake; `bridge.rs`). A task that must wait for something outside the CPU workers (a range from an object
//!   store) starts the request, polls its [`bridge::Completion`] once and returns [`Run::Parked`]; the same holds for [`Work::open`] and
//!   [`OpenStep::Parked`]. The scheduler then OWNS the task and the worker claims the next job: **a parked job holds no worker**. The job keeps the sequence
//!   number and the window slot it was claimed with, so parked jobs are what the window bounds (the window, not the worker count, is the I/O concurrency).
//!   The I/O side wakes it through the [`bridge::Waker`] the job was given (`CancelToken::waker`, `Parker::waker`); the wake puts the job on the `resume`
//!   queue and a worker calls `run` (or `open`) again. A wake that comes before the job has finished parking is remembered (`running` flag): sticky, no
//!   lost wake. Cancellation never depends on the I/O side: when claiming stops, or a task fails, `reap` settles every parked task at or above
//!   `abandon_from` (as `Slot::Cancelled`, dropping the task, which drops its request) and every parked open. A worker does not exit while a job is
//!   parked, or nobody would be left to resume it.
//! * **One lock.** All state is in one `Mutex<Core>` with two condition variables (`work_cv` for workers, `out_cv` for the consumer); every state change
//!   that can unblock someone calls `notify_all`. Tasks run outside the lock. The model checker explores exactly this code (`tests/loom_sched.rs`).
//!
//! `crossbeam-deque` (ADR 0002) is NOT used here: the window needs tasks to START in sequence order, which one FIFO under one lock gives by construction and
//! a work-stealing deque does not, and a row-group task lasts milliseconds, so a lock per claim costs nothing measurable; see the record of step 2b.

use std::collections::{BTreeMap, VecDeque};

use bridge::Waker;

pub mod bridge;
#[cfg(not(nata_loom))]
pub mod cooperative;
pub mod memory;
pub mod sync;
#[cfg(not(nata_loom))]
pub mod unordered;
use sync::{Arc, AtomicU64, Condvar, JoinHandle, Mutex, Ordering, lock, wait};

/// The work: a list of files, each of which expands (lazily, on a worker) into an ordered list of tasks.
pub trait Work: Send + Sync + 'static {
    type Task: Send + 'static;
    type Out: Send + 'static;
    type Err: Send + 'static;
    /// What an opening that is waiting for the I/O side keeps between its calls (`()` for a work that never parks an open).
    type Open: Send + 'static;
    /// Number of files.
    fn files(&self) -> usize;
    /// Open file `file` (no lock is held) and return its tasks in order. An error is stored at this file's place in the sequence. `resume` is `None` the
    /// first time and the state the opening returned in [`OpenStep::Parked`] after its wake. Instead of waiting for the I/O side the opening may return
    /// `Parked` after it arranged for `park.waker()` to be woken.
    fn open(
        &self,
        file: usize,
        resume: Option<Self::Open>,
        park: &Parker<'_>,
    ) -> OpenStep<Self::Task, Self::Open, Self::Err>;
    /// Run one task (no lock is held). Look at `cancel` between batches and return [`Run::Cancelled`] once it is set. A task that has to wait for the I/O
    /// side starts its request (the first time), polls it with `cancel.waker()` and returns [`Run::Parked`] when there is no answer yet; it is called again
    /// after the wake, with the state it left in `task`.
    fn run(&self, task: &mut Self::Task, cancel: &CancelToken<'_>) -> Run<Self::Out, Self::Err>;
    /// The error for a worker that died without delivering (a panic that escaped `run`), or for no thread at all.
    fn internal_error(&self, what: &str) -> Self::Err;
}

/// How a task ended.
#[derive(Debug)]
pub enum Run<O, E> {
    Done(O),
    Cancelled,
    Failed(E),
    /// Not finished: the task waits for the I/O side and has arranged to be woken. The scheduler keeps it (and its window slot) and frees the worker.
    Parked,
}

/// How the opening of a file went.
pub enum OpenStep<T, S, E> {
    /// The tasks of the file in order, or why it cannot be opened.
    Ready(Result<Vec<T>, E>),
    /// Waiting for the I/O side; `S` is handed back to `open` after the wake.
    Parked(S),
}

/// What the consumer's sink answers after a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    /// Enough: cancel everything and finish (a `LIMIT` that is satisfied).
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Worker threads (at least 1).
    pub threads: usize,
    /// Results in flight, waiting, or in the consumer's hands (at least 1). The memory bound is `window` times the largest task.
    pub window: usize,
    /// Deliver results in sequence order (identical to the single-thread scan); `false`: in completion order.
    pub ordered: bool,
    /// Files that may be pending at once (being opened, opened and waiting for an earlier file, or queued with unclaimed tasks); at least 1. `1` is one
    /// file at a time.
    pub open_ahead: usize,
}

/// Which job of the scheduler a wake is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Open(usize),
    Run(u64),
}

/// Makes the wakers of a scheduler's jobs; implemented by the scheduler's shared state.
trait WakeSource {
    fn waker(&self, key: Key) -> Waker;
}

/// Given to an opening that may park: where its wake comes from.
pub struct Parker<'a> {
    source: &'a dyn WakeSource,
    key: Key,
}

impl Parker<'_> {
    /// A waker for this job. Wake it when the request the job waits for has an answer (or failed); a wake for a job that no longer waits is ignored.
    pub fn waker(&self) -> Waker {
        self.source.waker(self.key)
    }
}

/// Given to a running task; cheap to ask.
pub struct CancelToken<'a> {
    abandon_from: Arc<AtomicU64>,
    seq: u64,
    park: Parker<'a>,
}

impl CancelToken<'_> {
    /// A waker for this task (see [`Parker::waker`]).
    pub fn waker(&self) -> Waker {
        self.park.waker()
    }

    /// True once this task must stop: it was cancelled, or a task below it failed.
    pub fn is_cancelled(&self) -> bool {
        self.abandon_from.load(Ordering::SeqCst) <= self.seq
    }

    /// The sequence number of this task.
    pub fn seq(&self) -> u64 {
        self.seq
    }
}

/// How the run ended when it did not fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finish {
    /// Every task was run and delivered.
    Complete,
    /// The sink said [`Flow::Stop`].
    Stopped,
    /// [`CancelHandle::cancel`] was called before the end.
    Cancelled,
}

/// What the scheduler did; the models and the tests check these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub finish: Finish,
    /// Sequence numbers handed out (tasks started or skipped, and files that failed to open).
    pub claimed: u64,
    /// Results the consumer took.
    pub taken: u64,
    /// Largest `claimed - released` seen at any claim: never above the window.
    pub high_water: u64,
    /// Claims made after claiming was stopped: always 0.
    pub claims_after_stop: u64,
    /// Files whose opening was claimed.
    pub files_opened: usize,
    /// The most files pending at once (see `Config::open_ahead`): never above it.
    pub max_pending_files: u64,
    pub workers_started: usize,
    pub workers_joined: usize,
    /// Jobs that returned `Parked` (each time counts).
    pub parks: u64,
    /// Wakes that found their job parked or running (a wake for a finished job is not counted).
    pub wakes: u64,
    /// Wakes that came before the job had finished parking (the sticky case).
    pub early_wakes: u64,
    /// The most jobs parked at once: a parked job holds no worker, so this can exceed the thread count.
    pub peak_parked: u64,
}

#[derive(Debug)]
pub struct Outcome<E> {
    pub result: Result<Finish, E>,
    pub summary: Summary,
}

enum Slot<W: Work> {
    Done(W::Out),
    Failed(W::Err),
    Cancelled,
}

struct Core<W: Work> {
    /// The next file to hand to an opener.
    next_file: usize,
    /// Files being opened right now (at most `open_ahead` files are pending).
    opening: usize,
    /// Opened files that wait for an earlier file to be installed, by file number.
    ready: BTreeMap<usize, Result<Vec<W::Task>, W::Err>>,
    /// The next file to install into the queue: files are installed in file order.
    next_install: usize,
    /// Files in the queue that still have a task (or a failure) unclaimed.
    queued_files: usize,
    /// Tasks of installed files, not yet claimed, in key order.
    queue: VecDeque<Queued<W>>,
    /// Sequence numbers handed out.
    claimed: u64,
    /// Nothing is left to claim.
    exhausted: bool,
    /// No claim may happen from now on (an error, a stop, a cancel).
    stop_claims: bool,
    /// [`CancelHandle::cancel`] was called.
    cancelled: bool,
    /// A worker died by panic.
    worker_failed: bool,
    /// Finished results the consumer has not taken.
    slots: BTreeMap<u64, Slot<W>>,
    /// Results taken by the consumer.
    taken: u64,
    /// Results the consumer is done with (taken and handed to the sink and dropped).
    released: u64,
    high_water: u64,
    claims_after_stop: u64,
    files_opened: usize,
    max_pending_files: u64,
    /// Tasks that returned `Parked`, by sequence number: they hold no worker.
    parked_tasks: BTreeMap<u64, W::Task>,
    /// Openings that returned `Parked`, by file.
    parked_opens: BTreeMap<usize, W::Open>,
    /// Woken jobs waiting for a worker. They were claimed before: no window check, and they run also after claiming stopped.
    resume: VecDeque<Job<W>>,
    /// The jobs a worker is running now; the flag is a wake that came meanwhile (sticky: the park that follows resumes the job at once).
    running: BTreeMap<Key, bool>,
    parks: u64,
    wakes: u64,
    early_wakes: u64,
    peak_parked: u64,
}

/// An entry of the queue, in key order.
enum Queued<W: Work> {
    /// A task; `last` marks the last task of its file.
    Task { task: W::Task, last: bool },
    /// The file at this place could not be opened: it takes the next sequence number when it is reached, and stops the claims.
    Failed(W::Err),
}

struct Shared<W: Work> {
    work: Arc<W>,
    cfg: Config,
    core: Mutex<Core<W>>,
    work_cv: Condvar,
    out_cv: Condvar,
    /// Tasks with `seq >= abandon_from` must stop. Written only under the `core` lock, read anywhere.
    abandon_from: Arc<AtomicU64>,
}

impl<W: Work> WakeSource for Arc<Shared<W>> {
    fn waker(&self, key: Key) -> Waker {
        let sh = Arc::clone(self);
        Waker::new(move || sh.wake_key(key))
    }
}

impl<W: Work> Shared<W> {
    /// Stop all claims and cancel all tasks. Idempotent. Wakes every waiting worker and the consumer.
    fn cancel_all(&self) {
        let mut g = lock(&self.core);
        g.stop_claims = true;
        self.abandon_from.store(0, Ordering::SeqCst);
        let doomed = Self::reap(&mut g, 0);
        drop(g);
        self.work_cv.notify_all();
        self.out_cv.notify_all();
        drop(doomed);
    }

    /// The I/O side says the request of `key` has an answer. A parked job goes to the `resume` queue for the next free worker; a job that is still running
    /// (it has not finished parking yet) remembers the wake, so that its park resumes it at once; a wake for any other job (finished, cancelled, already
    /// woken) is ignored.
    fn wake_key(&self, key: Key) {
        if cfg!(feature = "bridge-drop-wake") {
            return;
        }
        let mut g = lock(&self.core);
        let job = match key {
            Key::Run(seq) => g.parked_tasks.remove(&seq).map(|t| Job::Run(seq, t)),
            Key::Open(file) => g
                .parked_opens
                .remove(&file)
                .map(|s| Job::Open(file, Some(s))),
        };
        match job {
            Some(job) => {
                g.wakes += 1;
                g.resume.push_back(job);
                drop(g);
                self.work_cv.notify_all();
            }
            None => {
                if let Some(flag) = g.running.get_mut(&key) {
                    *flag = true;
                    g.wakes += 1;
                }
            }
        }
    }

    /// Under the lock: settle the parked jobs that can no longer matter. A parked task at or above `abandon_from` can never be delivered: it becomes a
    /// cancelled result (the consumer needs a result for every sequence number it has claimed) and is returned to be dropped OUTSIDE the lock, which drops
    /// its request. When claiming has stopped, no opening can matter any more: the parked ones are dropped too.
    fn reap(g: &mut Core<W>, abandon_from: u64) -> Doomed<W> {
        let tasks = if cfg!(feature = "bridge-drop-cancel") {
            BTreeMap::new()
        } else {
            g.parked_tasks.split_off(&abandon_from)
        };
        let mut doomed = Doomed {
            tasks: Vec::new(),
            opens: Vec::new(),
        };
        for (seq, task) in tasks {
            g.slots.insert(seq, Slot::Cancelled);
            doomed.tasks.push(task);
        }
        if g.stop_claims {
            for (_, state) in std::mem::take(&mut g.parked_opens) {
                g.opening -= 1;
                doomed.opens.push(state);
            }
        }
        doomed
    }
}

/// Parked jobs removed by `reap`, to be dropped after the lock is released.
struct Doomed<W: Work> {
    tasks: Vec<W::Task>,
    opens: Vec<W::Open>,
}

/// Cancel a running scan from another thread (a signal handler, a test, a closed connection).
pub struct CancelHandle<W: Work> {
    sh: Arc<Shared<W>>,
}

impl<W: Work> CancelHandle<W> {
    pub fn cancel(&self) {
        let mut g = lock(&self.sh.core);
        g.cancelled = true;
        drop(g);
        self.sh.cancel_all();
    }
}

enum Job<W: Work> {
    Run(u64, W::Task),
    /// The state is the one `Work::open` left when it parked; `None` the first time.
    Open(usize, Option<W::Open>),
}

impl<W: Work> Job<W> {
    fn key(&self) -> Key {
        match self {
            Job::Run(seq, _) => Key::Run(*seq),
            Job::Open(file, _) => Key::Open(*file),
        }
    }
}

/// Waits for the end of the worker's life; on a panic it makes the consumer and the other workers stop instead of waiting forever.
struct ExitGuard<W: Work>(Arc<Shared<W>>);

impl<W: Work> Drop for ExitGuard<W> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let mut g = lock(&self.0.core);
            g.worker_failed = true;
            drop(g);
            self.0.cancel_all();
        }
    }
}

/// Take the next unit of work, or wait for one, or learn that there is none. `None` = this worker is done.
fn claim<W: Work>(sh: &Shared<W>) -> Option<Job<W>> {
    let window = sh.cfg.window.max(1) as u64;
    let ahead = sh.cfg.open_ahead.max(1);
    let mut g = lock(&sh.core);
    loop {
        // a woken job first: it was claimed before (it holds its window slot), and it must run also after claiming stopped (a task below a failed one still
        // has to finish)
        if let Some(job) = g.resume.pop_front() {
            let key = job.key();
            g.running.insert(key, false);
            return Some(job);
        }
        if g.stop_claims || g.exhausted {
            // nothing more to claim, but a parked job needs a worker when its wake comes: this one stays until none is parked
            if g.parked_tasks.is_empty() && g.parked_opens.is_empty() {
                return None;
            }
            g = wait(&sh.work_cv, g);
            continue;
        }
        if g.claimed - g.released < window {
            // open the next file when fewer than `open_ahead` files are pending (with 1: nothing is pending, so the queue is empty and nobody is opening)
            let pending = g.opening + g.ready.len() + g.queued_files;
            if pending < ahead && g.next_file < sh.work.files() {
                let file = g.next_file;
                g.next_file += 1;
                g.opening += 1;
                g.files_opened += 1;
                g.max_pending_files = g.max_pending_files.max(pending as u64 + 1);
                if g.stop_claims {
                    g.claims_after_stop += 1;
                }
                g.running.insert(Key::Open(file), false);
                return Some(Job::Open(file, None));
            }
            match g.queue.pop_front() {
                Some(Queued::Task { task, last }) => {
                    if last {
                        g.queued_files -= 1;
                    }
                    let seq = g.claimed;
                    g.claimed += 1;
                    let out = g.claimed - g.released;
                    g.high_water = g.high_water.max(out);
                    if g.stop_claims {
                        g.claims_after_stop += 1;
                    }
                    g.running.insert(Key::Run(seq), false);
                    return Some(Job::Run(seq, task));
                }
                Some(Queued::Failed(e)) => {
                    // the file at this place could not be opened: its error takes the next sequence number and stops the claims
                    g.queued_files -= 1;
                    let seq = g.claimed;
                    g.claimed += 1;
                    let out = g.claimed - g.released;
                    g.high_water = g.high_water.max(out);
                    g.slots.insert(seq, Slot::Failed(e));
                    g.stop_claims = true;
                    let from = sh.abandon_from.load(Ordering::SeqCst).min(seq + 1);
                    sh.abandon_from.store(from, Ordering::SeqCst);
                    let doomed = Shared::reap(&mut g, from);
                    drop(g);
                    sh.out_cv.notify_all();
                    sh.work_cv.notify_all();
                    drop(doomed);
                    // the loop's top ends this worker, or keeps it while a task below the failed file is parked
                    g = lock(&sh.core);
                    continue;
                }
                None => {}
            }
            if g.next_file >= sh.work.files() && g.opening == 0 && g.ready.is_empty() {
                // no file left, no task queued, nobody opening: the claims are over
                g.exhausted = true;
                drop(g);
                sh.out_cv.notify_all();
                sh.work_cv.notify_all();
                // the loop's top ends this worker, or keeps it while a task is parked
                g = lock(&sh.core);
                continue;
            }
        }
        g = wait(&sh.work_cv, g);
    }
}

/// Store the result of the opening of `file`: it joins the queue when every earlier file has been installed (files are installed in file order, so the
/// queue and the sequence numbers are those of a scan that opens one file at a time); until then it waits in `ready`.
fn finish_open<W: Work>(sh: &Shared<W>, file: usize, opened: Result<Vec<W::Task>, W::Err>) {
    let mut unused: Vec<Result<Vec<W::Task>, W::Err>> = Vec::new();
    let mut g = lock(&sh.core);
    g.opening -= 1;
    g.running.remove(&Key::Open(file));
    g.ready.insert(file, opened);
    loop {
        let next = g.next_install;
        let Some(result) = g.ready.remove(&next) else {
            break;
        };
        g.next_install += 1;
        if g.stop_claims {
            unused.push(result);
            continue;
        }
        match result {
            Ok(tasks) => {
                let n = tasks.len();
                if n > 0 {
                    g.queued_files += 1;
                }
                for (i, task) in tasks.into_iter().enumerate() {
                    g.queue.push_back(Queued::Task {
                        task,
                        last: i + 1 == n,
                    });
                }
            }
            Err(e) => {
                g.queued_files += 1;
                g.queue.push_back(Queued::Failed(e));
            }
        }
    }
    drop(g);
    sh.work_cv.notify_all();
    sh.out_cv.notify_all();
    drop(unused);
}

/// Store the result of a task.
fn publish<W: Work>(sh: &Shared<W>, seq: u64, result: Run<W::Out, W::Err>) {
    let mut g = lock(&sh.core);
    g.running.remove(&Key::Run(seq));
    let mut doomed = None;
    match result {
        Run::Done(o) => {
            g.slots.insert(seq, Slot::Done(o));
        }
        Run::Cancelled | Run::Parked => {
            g.slots.insert(seq, Slot::Cancelled);
        }
        Run::Failed(e) => {
            g.slots.insert(seq, Slot::Failed(e));
            g.stop_claims = true;
            let from = sh.abandon_from.load(Ordering::SeqCst).min(seq + 1);
            sh.abandon_from.store(from, Ordering::SeqCst);
            doomed = Some(Shared::reap(&mut g, from));
        }
    }
    drop(g);
    sh.out_cv.notify_all();
    sh.work_cv.notify_all();
    drop(doomed);
}

/// A task returned `Parked`: keep it until its wake, and free the worker. Three cases decide at once, under the lock, so that no wake and no cancel can
/// fall between the worker's return from `run` and the task's entry in `parked_tasks`: a wake that already came resumes it; a cancel that already came
/// (`abandon_from`) settles it as cancelled; otherwise it parks.
fn park_task<W: Work>(sh: &Shared<W>, seq: u64, task: W::Task) {
    let mut g = lock(&sh.core);
    let woken = g.running.remove(&Key::Run(seq)).unwrap_or(false);
    g.parks += 1;
    if sh.abandon_from.load(Ordering::SeqCst) <= seq {
        g.slots.insert(seq, Slot::Cancelled);
        drop(g);
        sh.out_cv.notify_all();
        sh.work_cv.notify_all();
        drop(task);
    } else if woken {
        g.early_wakes += 1;
        g.resume.push_back(Job::Run(seq, task));
        drop(g);
        sh.work_cv.notify_all();
    } else {
        g.parked_tasks.insert(seq, task);
        let now = (g.parked_tasks.len() + g.parked_opens.len()) as u64;
        g.peak_parked = g.peak_parked.max(now);
    }
}

/// An opening returned `Parked` (the same three cases; a cancel is `stop_claims`: nobody wants the file any more).
fn park_open<W: Work>(sh: &Shared<W>, file: usize, state: W::Open) {
    let mut g = lock(&sh.core);
    let woken = g.running.remove(&Key::Open(file)).unwrap_or(false);
    g.parks += 1;
    if g.stop_claims {
        g.opening -= 1;
        drop(g);
        sh.out_cv.notify_all();
        sh.work_cv.notify_all();
        drop(state);
    } else if woken {
        g.early_wakes += 1;
        g.resume.push_back(Job::Open(file, Some(state)));
        drop(g);
        sh.work_cv.notify_all();
    } else {
        g.parked_opens.insert(file, state);
        let now = (g.parked_tasks.len() + g.parked_opens.len()) as u64;
        g.peak_parked = g.peak_parked.max(now);
    }
}

fn worker<W: Work>(sh: Arc<Shared<W>>) {
    let _exit = ExitGuard(Arc::clone(&sh));
    while let Some(job) = claim(&sh) {
        match job {
            Job::Run(seq, mut task) => {
                let token = CancelToken {
                    abandon_from: Arc::clone(&sh.abandon_from),
                    seq,
                    park: Parker {
                        source: &sh,
                        key: Key::Run(seq),
                    },
                };
                let result = if token.is_cancelled() {
                    Run::Cancelled
                } else {
                    sh.work.run(&mut task, &token)
                };
                drop(token);
                if matches!(result, Run::Parked) {
                    park_task(&sh, seq, task);
                    continue;
                }
                drop(task);
                publish(&sh, seq, result);
            }
            Job::Open(file, state) => {
                let park = Parker {
                    source: &sh,
                    key: Key::Open(file),
                };
                match sh.work.open(file, state, &park) {
                    OpenStep::Ready(opened) => finish_open(&sh, file, opened),
                    OpenStep::Parked(state) => park_open(&sh, file, state),
                }
            }
        }
    }
}

/// The worker threads of one run. Dropping it (also by unwinding) cancels and joins them.
struct Workers<W: Work> {
    sh: Arc<Shared<W>>,
    handles: Vec<JoinHandle<()>>,
    joined: usize,
}

impl<W: Work> Workers<W> {
    /// Cancel, then wait for every thread. Returns false if one of them panicked.
    fn shutdown(&mut self) -> bool {
        self.sh.cancel_all();
        let mut all_ok = true;
        for h in self.handles.drain(..) {
            if !sync::join(h) {
                all_ok = false;
            }
            self.joined += 1;
        }
        all_ok
    }
}

impl<W: Work> Drop for Workers<W> {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// One run of one scan.
pub struct Scheduler<W: Work> {
    sh: Arc<Shared<W>>,
}

impl<W: Work> Scheduler<W> {
    pub fn new(work: Arc<W>, cfg: Config) -> Scheduler<W> {
        let core = Core {
            next_file: 0,
            opening: 0,
            ready: BTreeMap::new(),
            next_install: 0,
            queued_files: 0,
            queue: VecDeque::new(),
            claimed: 0,
            exhausted: false,
            stop_claims: false,
            cancelled: false,
            worker_failed: false,
            slots: BTreeMap::new(),
            taken: 0,
            released: 0,
            high_water: 0,
            claims_after_stop: 0,
            files_opened: 0,
            max_pending_files: 0,
            parked_tasks: BTreeMap::new(),
            parked_opens: BTreeMap::new(),
            resume: VecDeque::new(),
            running: BTreeMap::new(),
            parks: 0,
            wakes: 0,
            early_wakes: 0,
            peak_parked: 0,
        };
        Scheduler {
            sh: Arc::new(Shared {
                work,
                cfg,
                core: Mutex::new(core),
                work_cv: Condvar::new(),
                out_cv: Condvar::new(),
                abandon_from: Arc::new(AtomicU64::new(u64::MAX)),
            }),
        }
    }

    pub fn cancel_handle(&self) -> CancelHandle<W> {
        CancelHandle {
            sh: Arc::clone(&self.sh),
        }
    }

    /// Run the scan: start the workers, hand every result to `sink` (in sequence order if `cfg.ordered`), and return when the work is done, the sink
    /// said stop, the sink failed, a task failed, or a cancel arrived. Every worker has been joined and every result and task dropped when this returns.
    ///
    /// The sink is called on the calling thread. An error from the sink cancels everything and is returned in preference to a task error (the output
    /// is gone: nothing a later task says matters).
    pub fn run<S>(self, mut sink: S) -> Outcome<W::Err>
    where
        S: FnMut(u64, W::Out) -> Result<Flow, W::Err>,
    {
        let sh = self.sh;
        let ordered = sh.cfg.ordered;
        let threads = sh.cfg.threads.max(1);
        let mut workers = Workers {
            sh: Arc::clone(&sh),
            handles: Vec::new(),
            joined: 0,
        };
        let mut started = 0usize;
        for i in 0..threads {
            let w = Arc::clone(&sh);
            match sync::spawn_named(format!("nata-scan-{i}"), move || worker(w)) {
                Ok(h) => {
                    workers.handles.push(h);
                    started += 1;
                }
                // the OS refuses more threads: go on with the ones that started (the window still bounds memory), or fail if there is none
                Err(_) => break,
            }
        }

        let mut task_err: Option<(u64, W::Err)> = None;
        let mut sink_err: Option<W::Err> = None;
        let mut emitting = started > 0;
        let mut stopped = false;
        let mut next_out: u64 = 0;
        let mut worker_failed = false;

        if started == 0 {
            sink_err = Some(sh.work.internal_error("no worker thread could be started"));
        }
        loop {
            if started == 0 {
                break;
            }
            // wait for the next result, or for the end
            let item: Option<(u64, Slot<W>)> = {
                let mut g = lock(&sh.core);
                loop {
                    let pick = if ordered && emitting {
                        g.slots.remove(&next_out).map(|s| (next_out, s))
                    } else {
                        g.slots.pop_first()
                    };
                    if let Some(p) = pick {
                        g.taken += 1;
                        if ordered && emitting {
                            next_out += 1;
                        }
                        break Some(p);
                    }
                    if g.worker_failed {
                        worker_failed = true;
                        break None;
                    }
                    if (g.exhausted || g.stop_claims) && g.taken == g.claimed {
                        break None;
                    }
                    g = wait(&sh.out_cv, g);
                }
            };
            let Some((seq, slot)) = item else { break };
            match slot {
                Slot::Done(out) => {
                    if emitting {
                        match sink(seq, out) {
                            Ok(Flow::Continue) => {}
                            Ok(Flow::Stop) => {
                                stopped = true;
                                emitting = false;
                            }
                            Err(e) => {
                                sink_err = Some(e);
                                emitting = false;
                            }
                        }
                    } else {
                        drop(out);
                    }
                }
                Slot::Failed(e) => {
                    emitting = false;
                    if !stopped
                        && sink_err.is_none()
                        && task_err.as_ref().is_none_or(|(s, _)| seq < *s)
                    {
                        task_err = Some((seq, e));
                    }
                }
                Slot::Cancelled => {
                    emitting = false;
                }
            }
            if stopped || sink_err.is_some() {
                // enough (a satisfied limit), or the output is gone: stop the claims BEFORE the slot is released, or the freed slot lets a worker start one
                // more task in the moment before the shutdown (a scan with `window = 1` and a limit read one row group too many under load)
                sh.cancel_all();
            }
            {
                let mut g = lock(&sh.core);
                g.released += 1;
            }
            sh.work_cv.notify_all();
            if stopped || sink_err.is_some() {
                break;
            }
            // ordered: nothing after the first error or cancellation is deliverable; unordered goes on only to find the lowest error
            if !emitting && (ordered || task_err.is_none()) {
                break;
            }
        }

        // shutdown: cancel, join every worker, then drop what is left (outside the lock)
        let joined_ok = workers.shutdown();
        let (leftover_slots, leftover_queue, leftover_ready, summary_core) = {
            let mut g = lock(&sh.core);
            let slots = std::mem::take(&mut g.slots);
            let queue = std::mem::take(&mut g.queue);
            let ready = std::mem::take(&mut g.ready);
            // nothing is parked or waiting to resume after the workers were joined (shutdown reaped it); a job left here would be dropped, never leaked
            let resume = std::mem::take(&mut g.resume);
            let parked_tasks = std::mem::take(&mut g.parked_tasks);
            let parked_opens = std::mem::take(&mut g.parked_opens);
            let cancelled = g.cancelled;
            let sc = (
                g.claimed,
                g.taken,
                g.high_water,
                g.claims_after_stop,
                g.files_opened,
                g.worker_failed,
                cancelled,
                g.max_pending_files,
                (g.parks, g.wakes, g.early_wakes, g.peak_parked),
            );
            drop(g);
            drop((resume, parked_tasks, parked_opens));
            (slots, queue, ready, sc)
        };
        // in error mode (unordered) a task with a lower number than the error found so far may have failed after it
        let mut leftover_slots = leftover_slots;
        if task_err.is_some() && !stopped && sink_err.is_none() {
            while let Some((seq, slot)) = leftover_slots.pop_first() {
                if let Slot::Failed(e) = slot
                    && task_err.as_ref().is_none_or(|(s, _)| seq < *s)
                {
                    task_err = Some((seq, e));
                }
            }
        }
        drop(leftover_slots);
        drop(leftover_queue);
        drop(leftover_ready);

        let (
            claimed,
            taken,
            high_water,
            claims_after_stop,
            files_opened,
            failed_flag,
            cancelled,
            max_pending_files,
            (parks, wakes, early_wakes, peak_parked),
        ) = summary_core;
        let finish = if stopped {
            Finish::Stopped
        } else if cancelled {
            Finish::Cancelled
        } else {
            Finish::Complete
        };
        let summary = Summary {
            finish,
            claimed,
            taken,
            high_water,
            claims_after_stop,
            files_opened,
            max_pending_files,
            workers_started: started,
            workers_joined: workers.joined,
            parks,
            wakes,
            early_wakes,
            peak_parked,
        };
        let result = if let Some(e) = sink_err {
            Err(e)
        } else if let Some((_, e)) = task_err {
            Err(e)
        } else if worker_failed || failed_flag || !joined_ok {
            Err(sh.work.internal_error("a worker thread panicked"))
        } else {
            Ok(finish)
        };
        Outcome { result, summary }
    }
}

#[cfg(test)]
mod parked_core;

/// Opt-in same-execution counters and bounded diagnostic events.
pub mod query_stats;
