//! The protocol's batch for this spike: an optional Arrow `RecordBatch`. (Selection vectors were verified in `arrow-pipeline`;
//! they are not the question here, so operators in this crate compact their output.)
use arrow::array::RecordBatch;

#[derive(Clone, Default)]
pub struct Batch(pub Option<RecordBatch>);

impl Batch {
    pub fn new(rb: RecordBatch) -> Self {
        Batch(Some(rb))
    }
    pub fn len(&self) -> usize {
        self.0.as_ref().map_or(0, |b| b.num_rows())
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn rb(&self) -> &RecordBatch {
        self.0.as_ref().expect("a batch with data")
    }
}
