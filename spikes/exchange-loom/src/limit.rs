//! The REAL `LIMIT` operator of the prototype: `ops.rs` lines 141-184, copied verbatim. Only the
//! imports differ: its atomic and `Arc` come from `crate::sync`.

use crate::api::*;
use crate::batch::Batch;
use crate::sync::Ordering::SeqCst;
use crate::sync::{Arc, AtomicUsize};

/// `LIMIT n`, shared by all tasks of the pipeline, exact however the batches interleave.
#[derive(Clone)]
pub struct Limit(Arc<AtomicUsize>);

impl Limit {
    pub fn new(n: usize) -> Self {
        Limit(Arc::new(AtomicUsize::new(n)))
    }
}

impl OperatorFactory for Limit {
    fn make(&self) -> Box<dyn Operator> {
        Box::new(LimitOp(Arc::clone(&self.0)))
    }
}

struct LimitOp(Arc<AtomicUsize>);

impl Operator for LimitOp {
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        loop {
            let remaining = self.0.load(SeqCst);
            if remaining == 0 {
                return Ok(OpResult::Finished);
            }
            let take = remaining.min(input.len());
            if self
                .0
                .compare_exchange(remaining, remaining - take, SeqCst, SeqCst)
                .is_ok()
            {
                *out = Batch::new(input.cols.len());
                for i in 0..take {
                    out.push_row(&input.row(i));
                }
                return Ok(if take == remaining {
                    OpResult::Finished
                } else {
                    OpResult::NeedMoreInput
                });
            }
        }
    }
}
