//! The two helpers the trial needs (a generator source and a collecting sink), written against the sync shim
//! so that shuttle sees their atomics and mutexes too.

use crate::api::*;
use crate::batch::Batch;
use crate::sched::lock;
use crate::sync::Ordering::SeqCst;
use crate::sync::{Arc, AtomicUsize, Mutex};

pub struct Generator {
    next: Arc<AtomicUsize>,
    total: usize,
}

impl Generator {
    pub fn new(total: usize, _produce: impl Fn(usize) -> Batch + Send + Sync + 'static) -> Self {
        Generator { next: Arc::new(AtomicUsize::new(0)), total }
    }
}

struct GenLocal {
    next: Arc<AtomicUsize>,
    total: usize,
}

impl SourceFactory for Generator {
    fn make(&self) -> Box<dyn Source> {
        Box::new(GenLocal { next: Arc::clone(&self.next), total: self.total })
    }
}

impl Source for GenLocal {
    fn next(&mut self, _w: &Waker) -> Result<SourceResult> {
        let m = self.next.fetch_add(1, SeqCst);
        if m >= self.total {
            return Ok(SourceResult::Finished);
        }
        Ok(SourceResult::Batch(Batch::from_cols(vec![vec![m as i64]])))
    }
}

#[derive(Clone)]
pub struct Collect(Arc<Mutex<Vec<Batch>>>);

impl Collect {
    pub fn new() -> Self {
        Collect(Arc::new(Mutex::new(Vec::new())))
    }

    pub fn rows(&self) -> Vec<Vec<i64>> {
        let mut rows: Vec<Vec<i64>> = lock(&self.0).iter().flat_map(Batch::rows).collect();
        rows.sort();
        rows
    }
}

struct CollectLocal {
    batches: Vec<Batch>,
    global: Arc<Mutex<Vec<Batch>>>,
}

impl GlobalSink for Collect {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        Box::new(CollectLocal { batches: Vec::new(), global: Arc::clone(&self.0) })
    }

    fn finalize(&self) -> Result<()> {
        Ok(())
    }
}

impl LocalSink for CollectLocal {
    fn push(&mut self, input: &mut Batch, _w: &Waker) -> Result<SinkResult> {
        self.batches.push(std::mem::take(input));
        Ok(SinkResult::NeedMoreInput)
    }

    fn combine(self: Box<Self>) -> Result<()> {
        lock(&self.global).extend(self.batches);
        Ok(())
    }
}
