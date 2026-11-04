//! Small generic pieces: a shared queue of batches used as a source, closure-based filter and project operators, a collecting sink.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use arrow::array::{BooleanArray, RecordBatch};
use arrow::compute::filter_record_batch;

use crate::batch::Batch;
use crate::proto::api::*;
use crate::proto::sched::lock;
use crate::spill::ioerr;

/// A queue of batches that several tasks of a later pipeline drain (each batch goes to exactly one task). The pipeline that fills it
/// must be a dependency of the pipeline that reads it (the scheduler starts a pipeline only after its dependencies' `finalize`).
#[derive(Default)]
pub struct SharedQueue(pub Mutex<VecDeque<RecordBatch>>);

impl SharedQueue {
    pub fn new() -> Arc<Self> {
        Arc::new(SharedQueue::default())
    }
    pub fn fill(&self, batches: impl IntoIterator<Item = RecordBatch>) {
        lock(&self.0).extend(batches.into_iter().filter(|b| b.num_rows() > 0));
    }
}

struct QueueSource(Arc<SharedQueue>);

impl Source for QueueSource {
    fn next(&mut self, _w: &Waker) -> Result<SourceResult> {
        Ok(match lock(&self.0.0).pop_front() {
            Some(b) => SourceResult::Batch(Batch::new(b)),
            None => SourceResult::Finished,
        })
    }
}

impl SourceFactory for Arc<SharedQueue> {
    fn make(&self) -> Box<dyn Source> {
        Box::new(QueueSource(Arc::clone(self)))
    }
}

/// `WHERE <predicate>`: keeps the rows for which the closure's boolean array is true (NULL counts as false).
#[derive(Clone)]
pub struct FilterFn(pub Arc<dyn Fn(&RecordBatch) -> BooleanArray + Send + Sync>);

struct FilterOp(FilterFn);

impl OperatorFactory for FilterFn {
    fn make(&self) -> Box<dyn Operator> {
        Box::new(FilterOp(self.clone()))
    }
}

impl Operator for FilterOp {
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        let rb = input.rb();
        let mask = (self.0.0)(rb);
        let f = filter_record_batch(rb, &mask).map_err(ioerr("filter"))?;
        *out = Batch::new(f);
        Ok(OpResult::NeedMoreInput)
    }
}

/// Projection by closure (computed columns, renames, reordering).
#[derive(Clone)]
pub struct ProjectFn(pub Arc<dyn Fn(&RecordBatch) -> RecordBatch + Send + Sync>);

struct ProjectOp(ProjectFn);

impl OperatorFactory for ProjectFn {
    fn make(&self) -> Box<dyn Operator> {
        Box::new(ProjectOp(self.clone()))
    }
}

impl Operator for ProjectOp {
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        *out = Batch::new((self.0.0)(input.rb()));
        Ok(OpResult::NeedMoreInput)
    }
}

/// Collects every batch (per-task locally, merged at `combine`, in task arrival order). For ordered results run the pipeline with one task.
pub struct CollectSink {
    pub batches: Mutex<Vec<RecordBatch>>,
}

impl CollectSink {
    pub fn new() -> Arc<Self> {
        Arc::new(CollectSink { batches: Mutex::new(Vec::new()) })
    }
    pub fn result(&self) -> Vec<RecordBatch> {
        lock(&self.batches).clone()
    }
}

struct CollectLocal(Arc<CollectSink>, Vec<RecordBatch>);

impl GlobalSink for Arc<CollectSink> {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        Box::new(CollectLocal(Arc::clone(self), Vec::new()))
    }
    fn finalize(&self) -> Result<()> {
        Ok(())
    }
}

impl LocalSink for CollectLocal {
    fn push(&mut self, input: &mut Batch, _w: &Waker) -> Result<SinkResult> {
        if !input.is_empty() {
            self.1.push(input.rb().clone());
        }
        Ok(SinkResult::NeedMoreInput)
    }
    fn combine(self: Box<Self>) -> Result<()> {
        lock(&self.0.batches).extend(self.1);
        Ok(())
    }
}
