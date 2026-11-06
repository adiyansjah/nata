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
    /// Times each task id was run (index = id).
    pub runs: Vec<AtomicUsize>,
    /// Times each file was opened.
    pub opens: Vec<AtomicUsize>,
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
            runs: (0..(n * 10 + 10)).map(|_| AtomicUsize::new(0)).collect(),
            opens: (0..n).map(|_| AtomicUsize::new(0)).collect(),
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
        if self.fail_open == Some(file) {
            return Err(ModelErr::Open(file));
        }
        Ok((0..self.files[file])
            .map(|i| ModelTask {
                id: file * 10 + i,
                _handle: Tracked::new(&self.live),
            })
            .collect())
    }

    fn run(&self, task: &ModelTask, cancel: &CancelToken) -> Run<ModelOut, ModelErr> {
        self.runs[task.id].fetch_add(1, Ordering::SeqCst);
        if self.panic_tasks.contains(&task.id) {
            panic!("model task {} panics", task.id);
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
