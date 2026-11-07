//! Execution of the logical plans of `nata-sql` (slice 4, step 3): the scan of slice 3 with the pushed conjuncts, `Filter` and `Project` evaluated on
//! Arrow batches with strict, checked, NULL-aware, selection-aware code, `Offset`/`Limit` with cancellation of the scan, the text of `EXPLAIN ANALYZE`,
//! and the output writers of `nata sql` (CSV and an aligned table) with positioned error rendering.
//!
//! * [`eval`]: the expression evaluator; [`cast`]: `CAST`; [`num`]: exact number helpers; [`value`]: columns.
//! * [`exec`]: `execute` runs a plan; [`explain`]: `EXPLAIN ANALYZE` text; [`output`]: result and error rendering.

pub mod cast;
pub mod error;
pub mod eval;
pub mod exec;
pub mod explain;
pub mod num;
pub mod output;
pub mod value;

pub use error::EvalError;
pub use exec::{ExecConfig, ExecReport, Observed, OperatorStats, execute, execute_observed};

mod aggregate;
