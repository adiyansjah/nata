//! A model `Work` shared by the std tests and the loom models. Everything it synchronises with comes from `nata_sched::sync`, so under
//! `--cfg nata_loom` it is made of loom primitives and loom explores the real scheduler together with it.
#![allow(dead_code, reason = "each test file uses a different part")]

use nata_sched::sync::{Arc, AtomicUsize, Ordering, yield_now};
use nata_sched::{CancelToken, Run, Work};

/// A counted resource (the `Tracked` pattern of spikes/sim-harness): counted while alive. A scheduler that forgets to drop a task, a result or a handle
/// leaves the counter above zero.
pub struct Tracked(Arc<AtomicUsize>);

impl Tracked {
    pub fn new(c: &Arc<AtomicUsize>) -> Tracked {
        c.fetch_add(1, Ordering::SeqCst);
        Tracked(Arc::clone(c))
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Task id = file * 10 + index in the file.
pub struct ModelTask {
    pub id: usize,
    _handle: Tracked,
}

pub struct ModelOut {
    pub id: usize,
    _buffer: Tracked,
    gauge: Arc<AtomicUsize>,
}

impl Drop for ModelOut {
    fn drop(&mut self) {
        self.gauge.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelErr {
    Open(usize),
    Task(usize),
    Internal,
}

pub struct Model {
    /// Tasks per file.
    pub files: Vec<usize>,
    /// Opening this file fails.
    pub fail_open: Option<usize>,
    /// These task ids fail.
    pub fail_tasks: Vec<usize>,
    /// These task ids do not finish until they are cancelled (a blocked task).
    pub spin_tasks: Vec<usize>,
    /// These task ids panic.
    pub panic_tasks: Vec<usize>,
    /// (task id, n): the task does not finish until `n` tasks have stopped because of a cancellation (or it is cancelled itself). It makes a lower
    /// task depend on a higher task being told to stop.
    pub wait_for_stopped: Vec<(usize, usize)>,
    /// (task id, other id): the task does not start its work until the other task has been started.
    pub wait_for_started: Vec<(usize, usize)>,
    /// Times each task id was run (index = id).
    pub runs: Vec<AtomicUsize>,
    /// Times each file was opened (counted when `open` is called).
    pub opens: Vec<AtomicUsize>,
    /// (file, other file): opening `file` does not return before the opening of `other` has returned (it makes a later file finish first; only with
    /// `open_ahead` of 2 or more, or the two would wait for each other).
    pub open_after: Vec<(usize, usize)>,
    /// Openings that have returned, by file.
    pub opens_done: Vec<AtomicUsize>,
    /// Openings in progress now, and the most at once.
    pub opens_now: AtomicUsize,
    pub opens_peak: AtomicUsize,
    /// (task id, file): the task does not finish before the opening of `file` has been started (a task that needs a worker to open ahead while it runs).
    pub wait_for_open_started: Vec<(usize, usize)>,
    /// A std-only pause in every opening, in milliseconds (a slow footer read).
    pub open_pause_ms: u64,
    /// Live tasks + outputs.
    pub live: Arc<AtomicUsize>,
    /// Tasks that saw the cancellation and stopped.
    pub stopped: AtomicUsize,
    /// Outputs alive (made by `run`, not yet dropped), and the highest value that counter reached.
    pub outs: Arc<AtomicUsize>,
    pub outs_peak: AtomicUsize,
}

impl Model {
    pub fn new(files: Vec<usize>) -> Model {
        let n = files.len();
        Model {
            files,
            fail_open: None,
            fail_tasks: vec![],
            spin_tasks: vec![],
            panic_tasks: vec![],
            wait_for_stopped: vec![],
            wait_for_started: vec![],
            runs: (0..(n * 10 + 10)).map(|_| AtomicUsize::new(0)).collect(),
            opens: (0..n).map(|_| AtomicUsize::new(0)).collect(),
            open_after: vec![],
            opens_done: (0..n).map(|_| AtomicUsize::new(0)).collect(),
            opens_now: AtomicUsize::new(0),
            opens_peak: AtomicUsize::new(0),
            wait_for_open_started: vec![],
            open_pause_ms: 0,
            live: Arc::new(AtomicUsize::new(0)),
            stopped: AtomicUsize::new(0),
            outs: Arc::new(AtomicUsize::new(0)),
            outs_peak: AtomicUsize::new(0),
        }
    }

    /// Every task id in order.
    pub fn all_ids(&self) -> Vec<usize> {
        let mut v = vec![];
        for (f, n) in self.files.iter().enumerate() {
            for i in 0..*n {
                v.push(f * 10 + i);
            }
        }
        v
    }

    pub fn runs_of(&self, id: usize) -> usize {
        self.runs[id].load(Ordering::SeqCst)
    }

    pub fn live_now(&self) -> usize {
        self.live.load(Ordering::SeqCst)
    }
}

impl Work for Model {
    type Task = ModelTask;
    type Out = ModelOut;
    type Err = ModelErr;

    fn files(&self) -> usize {
        self.files.len()
    }

    fn open(&self, file: usize) -> Result<Vec<ModelTask>, ModelErr> {
        self.opens[file].fetch_add(1, Ordering::SeqCst);
        let now = self.opens_now.fetch_add(1, Ordering::SeqCst) + 1;
        self.opens_peak.fetch_max(now, Ordering::SeqCst);
        #[cfg(not(nata_loom))]
        if self.open_pause_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(self.open_pause_ms));
        }
        if let Some((_, other)) = self.open_after.iter().find(|(f, _)| *f == file) {
            while self.opens_done[*other].load(Ordering::SeqCst) == 0 {
                yield_now();
            }
        }
        let result = if self.fail_open == Some(file) {
            Err(ModelErr::Open(file))
        } else {
            Ok((0..self.files[file])
                .map(|i| ModelTask {
                    id: file * 10 + i,
                    _handle: Tracked::new(&self.live),
                })
                .collect())
        };
        self.opens_now.fetch_sub(1, Ordering::SeqCst);
        self.opens_done[file].fetch_add(1, Ordering::SeqCst);
        result
    }

    fn run(&self, task: &ModelTask, cancel: &CancelToken) -> Run<ModelOut, ModelErr> {
        self.runs[task.id].fetch_add(1, Ordering::SeqCst);
        if self.panic_tasks.contains(&task.id) {
            panic!("model task {} panics", task.id);
        }
        if let Some((_, other)) = self.wait_for_started.iter().find(|(id, _)| *id == task.id) {
            while self.runs[*other].load(Ordering::SeqCst) == 0 && !cancel.is_cancelled() {
                yield_now();
            }
        }
        if let Some((_, file)) = self
            .wait_for_open_started
            .iter()
            .find(|(id, _)| *id == task.id)
        {
            while self.opens[*file].load(Ordering::SeqCst) == 0 && !cancel.is_cancelled() {
                yield_now();
            }
        }
        if let Some((_, n)) = self.wait_for_stopped.iter().find(|(id, _)| *id == task.id) {
            while self.stopped.load(Ordering::SeqCst) < *n && !cancel.is_cancelled() {
                yield_now();
            }
        }
        if self.spin_tasks.contains(&task.id) {
            while !cancel.is_cancelled() {
                yield_now();
            }
            self.stopped.fetch_add(1, Ordering::SeqCst);
            return Run::Cancelled;
        }
        if cancel.is_cancelled() {
            self.stopped.fetch_add(1, Ordering::SeqCst);
            return Run::Cancelled;
        }
        if self.fail_tasks.contains(&task.id) {
            return Run::Failed(ModelErr::Task(task.id));
        }
        let now = self.outs.fetch_add(1, Ordering::SeqCst) + 1;
        self.outs_peak.fetch_max(now, Ordering::SeqCst);
        Run::Done(ModelOut {
            id: task.id,
            _buffer: Tracked::new(&self.live),
            gauge: Arc::clone(&self.outs),
        })
    }

    fn internal_error(&self, _what: &str) -> ModelErr {
        ModelErr::Internal
    }
}
