//! `LIMIT n` as an operator of the protocol, shared by all tasks of the pipeline. It is its own file so that the loom
//! model (`loom-model/`, bench/results/20261104-stop-depth-loom.md) compiles this exact source: the counter comes
//! from `crate::proto::sync` (std normally, loom under `--cfg nata_loom`). Moved verbatim from `ops.rs`; only the
//! imports changed.
use crate::batch::Batch;
use crate::proto::api::*;
use crate::proto::sync::Ordering::SeqCst;
use crate::proto::sync::{Arc, AtomicUsize};

/// `LIMIT n`, shared by all tasks of the pipeline, exact however the batches interleave. It counts LIVE rows
/// (after the selection) and cuts the selection vector, not the columns.
#[derive(Clone)]
pub struct Limit(Arc<AtomicUsize>);

impl Limit {
    pub fn new(n: usize) -> Self {
        Limit(Arc::new(AtomicUsize::new(n)))
    }

    pub fn remaining(&self) -> usize {
        self.0.load(SeqCst)
    }
}

impl OperatorFactory for Limit {
    fn make(&self) -> Box<dyn Operator> {
        Box::new(LimitOp(Arc::clone(&self.0)))
    }
}

struct LimitOp(Arc<AtomicUsize>);

/// The first `take` live rows of `input`.
pub fn truncate(input: &Batch, take: usize) -> Batch {
    let sel: Vec<u32> = match input.sel() {
        Some(s) => s[..take].to_vec(),
        None => (0..take as u32).collect(),
    };
    input.with_sel(sel)
}

impl Operator for LimitOp {
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        loop {
            let remaining = self.0.load(SeqCst);
            if remaining == 0 {
                return Ok(OpResult::Finished);
            }
            let take = remaining.min(input.len());
            if self.0.compare_exchange(remaining, remaining - take, SeqCst, SeqCst).is_ok() {
                *out = truncate(input, take);
                return Ok(if take == remaining { OpResult::Finished } else { OpResult::NeedMoreInput });
            }
        }
    }
}
