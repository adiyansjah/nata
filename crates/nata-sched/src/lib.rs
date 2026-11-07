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
//! * **One lock.** All state is in one `Mutex<Core>` with two condition variables (`work_cv` for workers, `out_cv` for the consumer); every state change
//!   that can unblock someone calls `notify_all`. Tasks run outside the lock. The model checker explores exactly this code (`tests/loom_sched.rs`).
//!
//! `crossbeam-deque` (ADR 0002) is NOT used here: the window needs tasks to START in sequence order, which one FIFO under one lock gives by construction and
//! a work-stealing deque does not, and a row-group task lasts milliseconds, so a lock per claim costs nothing measurable; see the record of step 2b.

use std::collections::{BTreeMap, VecDeque};

pub mod memory;
pub mod sync;
use sync::{Arc, AtomicU64, Condvar, JoinHandle, Mutex, Ordering, lock, wait};

/// The work: a list of files, each of which expands (lazily, on a worker) into an ordered list of tasks.
pub trait Work: Send + Sync + 'static {
    type Task: Send + 'static;
    type Out: Send + 'static;
    type Err: Send + 'static;
    /// Number of files.
    fn files(&self) -> usize;
    /// Open file `file` (I/O, no lock is held) and return its tasks in order. An error is stored at this file's place in the sequence.
    fn open(&self, file: usize) -> Result<Vec<Self::Task>, Self::Err>;
    /// Run one task (no lock is held). Look at `cancel` between batches and return [`Run::Cancelled`] once it is set.
    fn run(&self, task: &Self::Task, cancel: &CancelToken) -> Run<Self::Out, Self::Err>;
    /// The error for a worker that died without delivering (a panic that escaped `run`), or for no thread at all.
    fn internal_error(&self, what: &str) -> Self::Err;
}

/// How a task ended.
#[derive(Debug)]
pub enum Run<O, E> {
    Done(O),
    Cancelled,
    Failed(E),
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

/// Given to a running task; cheap to ask.
pub struct CancelToken {
    abandon_from: Arc<AtomicU64>,
    seq: u64,
}

impl CancelToken {
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

impl<W: Work> Shared<W> {
    /// Stop all claims and cancel all tasks. Idempotent. Wakes every waiting worker and the consumer.
    fn cancel_all(&self) {
        let mut g = lock(&self.core);
        g.stop_claims = true;
        self.abandon_from.store(0, Ordering::SeqCst);
        drop(g);
        self.work_cv.notify_all();
        self.out_cv.notify_all();
    }
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
    Open(usize),
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
        if g.stop_claims {
            return None;
        }
        if g.exhausted {
            return None;
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
                return Some(Job::Open(file));
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
                    drop(g);
                    sh.out_cv.notify_all();
                    sh.work_cv.notify_all();
                    return None;
                }
                None => {}
            }
            if g.next_file >= sh.work.files() && g.opening == 0 && g.ready.is_empty() {
                // no file left, no task queued, nobody opening: the claims are over
                g.exhausted = true;
                drop(g);
                sh.out_cv.notify_all();
                sh.work_cv.notify_all();
                return None;
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
    match result {
        Run::Done(o) => {
            g.slots.insert(seq, Slot::Done(o));
        }
        Run::Cancelled => {
            g.slots.insert(seq, Slot::Cancelled);
        }
        Run::Failed(e) => {
            g.slots.insert(seq, Slot::Failed(e));
            g.stop_claims = true;
            let from = sh.abandon_from.load(Ordering::SeqCst).min(seq + 1);
            sh.abandon_from.store(from, Ordering::SeqCst);
        }
    }
    drop(g);
    sh.out_cv.notify_all();
    sh.work_cv.notify_all();
}

fn worker<W: Work>(sh: Arc<Shared<W>>) {
    let _exit = ExitGuard(Arc::clone(&sh));
    while let Some(job) = claim(&sh) {
        match job {
            Job::Run(seq, task) => {
                let token = CancelToken {
                    abandon_from: Arc::clone(&sh.abandon_from),
                    seq,
                };
                let result = if token.is_cancelled() {
                    Run::Cancelled
                } else {
                    sh.work.run(&task, &token)
                };
                drop(task);
                publish(&sh, seq, result);
            }
            Job::Open(file) => {
                let opened = sh.work.open(file);
                finish_open(&sh, file, opened);
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
            );
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
