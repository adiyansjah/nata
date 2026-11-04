//! Operators of the protocol over Arrow batches with a selection vector. Each takes `&Batch`, builds a new `Batch`
//! that shares the columns, and never copies column data.
use crate::batch::{Batch, Ref};
use crate::genops::engine::Step;
use crate::genops::kernels::{EngineError, R, filter_range, mul, sub_const_left};
use crate::proto::api::*;

pub fn q<T>(r: R<T>) -> Result<T> {
    r.map_err(|EngineError(m)| QueryError(m))
}

// ---------------------------------------------------------------- filters

/// Narrows the selection to rows whose date column is in `[lo, hi]` (NULL rows drop out).
#[derive(Clone)]
pub struct FilterDate {
    pub col: &'static str,
    pub lo: i32,
    pub hi: i32,
}

struct FilterDateOp(FilterDate);

impl OperatorFactory for FilterDate {
    fn make(&self) -> Box<dyn Operator> {
        Box::new(FilterDateOp(self.clone()))
    }
}

impl Operator for FilterDateOp {
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        let new = {
            let (c, v) = input.date(self.0.col);
            filter_range(c, v, self.0.lo, self.0.hi, input.rows(), input.sel())
        };
        *out = input.with_sel(new);
        Ok(OpResult::NeedMoreInput)
    }
}

#[derive(Clone)]
pub struct FilterDec {
    pub col: &'static str,
    pub lo: i64,
    pub hi: i64,
}

struct FilterDecOp(FilterDec);

impl OperatorFactory for FilterDec {
    fn make(&self) -> Box<dyn Operator> {
        Box::new(FilterDecOp(self.clone()))
    }
}

impl Operator for FilterDecOp {
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        let new = {
            let (c, v) = input.dec(&Ref::Col(self.0.col));
            filter_range(c, v, self.0.lo, self.0.hi, input.rows(), input.sel())
        };
        *out = input.with_sel(new);
        Ok(OpResult::NeedMoreInput)
    }
}

// ---------------------------------------------------------------- project

/// Computes expression columns over the selected rows, one primitive kernel per step.
#[derive(Clone)]
pub struct ProjectOpFactory(pub Vec<StepSpec>);

/// `Step` is not `Clone` in the snapshot; this is its cloneable twin.
#[derive(Clone)]
pub enum StepSpec {
    SubConstLeft { c: i64, a: Ref },
    Mul { a: Ref, b: Ref },
}

struct ProjectOp(Vec<StepSpec>);

impl OperatorFactory for ProjectOpFactory {
    fn make(&self) -> Box<dyn Operator> {
        Box::new(ProjectOp(self.0.clone()))
    }
}

impl Operator for ProjectOp {
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        // Steps may refer to the columns computed by earlier steps of this same project (`Tmp(k)`), so the
        // working batch grows as the steps run; only the last is handed on.
        let mut cur = input.clone();
        for step in &self.0 {
            let o = match step {
                StepSpec::SubConstLeft { c, a } => q(sub_const_left(*c, cur.dec(a), cur.rows(), cur.sel()))?,
                StepSpec::Mul { a, b } => q(mul(cur.dec(a), cur.dec(b), cur.rows(), cur.sel()))?,
            };
            cur = cur.with_tmp(vec![o]);
        }
        *out = cur;
        Ok(OpResult::NeedMoreInput)
    }
}

impl From<Step> for StepSpec {
    fn from(s: Step) -> Self {
        match s {
            Step::SubConstLeft { c, a } => StepSpec::SubConstLeft { c, a },
            Step::Mul { a, b } => StepSpec::Mul { a, b },
        }
    }
}

// LIMIT lives in `limit.rs` (the loom model compiles that file as it is); re-exported here so paths do not change.
pub use crate::limit::{Limit, truncate};
