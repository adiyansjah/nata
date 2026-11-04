//! SNAPSHOT of spikes/arrow-pipeline/src/proto (itself a snapshot of spikes/operator-trait): the operator protocol and its scheduler, UNCHANGED.
//! protocol and its scheduler. Only the `use crate::...` paths changed. `ops.rs` (toy operators, exchange, spill)
//! and `batch.rs` (the toy `i64` batch) are NOT carried over; the batch type is `crate::batch::Batch` (Arrow).
//!
//! Loom (bench/results/20261104-stop-depth-loom.md): `sync.rs` and the three `#[cfg]` guards in `sched.rs` use the cfg name
//! `nata_loom` (was `loom`; a plain `--cfg loom` also switches on other crates' loom code). This directory is compiled by
//! `loom-model/` with `#[path]`; this crate itself is not loom-instrumented.
pub mod api;
pub mod sched;
pub mod sync;

pub use api::*;
pub use sched::{Query, RunningQuery, TEST_WORKER_DELAY_US};
