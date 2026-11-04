//! The operator protocol under test.
//!
//! A pipeline is `source -> operators -> sink`. Each worker task owns its own local copies of
//! all three. Everything is synchronous: nothing in here may wait. Anything that cannot make
//! progress says so (`Blocked`) after storing the [`Waker`] it was given; the scheduler then
//! parks the task and the owner of the resource calls `wake()` when it changes.

use std::fmt;
use std::sync::Arc;

use crate::batch::Batch;

#[derive(Debug, Clone, PartialEq)]
pub struct QueryError(pub String);

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for QueryError {}

pub type Result<T> = std::result::Result<T, QueryError>;

pub fn err<T>(msg: impl Into<String>) -> Result<T> {
    Err(QueryError(msg.into()))
}

/// Re-queues the parked task that owns it. Cheap to clone; may be called from any thread, at any
/// time (before the task has parked, while it runs, or after it finished).
#[derive(Clone)]
pub struct Waker(Arc<dyn Fn() + Send + Sync>);

impl Waker {
    pub fn new(f: impl Fn() + Send + Sync + 'static) -> Self {
        Waker(Arc::new(f))
    }

    pub fn wake(&self) {
        (self.0)();
    }
}

pub enum SourceResult {
    Batch(Batch),
    /// Nothing now. The source stored the waker and will call it when that changes.
    Blocked,
    Finished,
}

pub enum OpResult {
    /// The input is fully consumed; give me the next one. `out` may hold output.
    NeedMoreInput,
    /// `out` holds output but the same input has more: call again with the same input.
    HaveMoreOutput,
    /// No further output will ever be produced (for example `LIMIT` reached). `out` may still
    /// hold the last rows. Upstream work can stop.
    Finished,
}

pub enum SinkResult {
    NeedMoreInput,
    /// The sink wants no more input (for example the consumer went away).
    Finished,
    /// Not accepted. `input` is left untouched and will be pushed again after a wake.
    Blocked,
}

pub trait Source: Send {
    fn next(&mut self, waker: &Waker) -> Result<SourceResult>;
    /// Called once when the task ends, whatever the reason (finished, stopped early, cancelled).
    /// An exchange consumer uses it to tell its producers nobody is reading any more.
    fn finish(&mut self) {}
}

/// Shared source state; one local [`Source`] per worker task.
pub trait SourceFactory: Send + Sync {
    fn make(&self) -> Box<dyn Source>;
}

pub trait Operator: Send {
    /// Must make progress or say `NeedMoreInput`/`Finished`; it never blocks.
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult>;
}

pub trait OperatorFactory: Send + Sync {
    fn make(&self) -> Box<dyn Operator>;
}

pub trait LocalSink: Send {
    /// On `Blocked`, `input` must be left as it was.
    fn push(&mut self, input: &mut Batch, waker: &Waker) -> Result<SinkResult>;
    /// Called once when this task ends normally: merge local state into the global state.
    fn combine(self: Box<Self>) -> Result<()>;
}

pub trait GlobalSink: Send + Sync {
    fn make(&self, task: usize) -> Box<dyn LocalSink>;
    /// Called once, by whichever task ends last, after every `combine`. May be long (it can
    /// read spill files). Dependent pipelines start after it returns.
    fn finalize(&self) -> Result<()>;
}

pub struct Pipeline {
    pub name: String,
    pub source: Arc<dyn SourceFactory>,
    pub ops: Vec<Arc<dyn OperatorFactory>>,
    pub sink: Arc<dyn GlobalSink>,
    /// Indexes of pipelines that must finish (including finalize) before this one starts.
    pub deps: Vec<usize>,
    /// Number of worker tasks (parallel instances), independent of the number of OS threads.
    pub tasks: usize,
}
