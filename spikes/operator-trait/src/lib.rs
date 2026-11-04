//! Spike for ADR 0001, open check 1: can a synchronous push-based operator protocol with
//! `blocked`/waker express the hard cases (hash join, `LIMIT`, a blocking exchange, spill)?

pub mod api;
pub mod batch;
// The operators use std primitives and file I/O: they are not part of the loom model.
#[cfg(not(loom))]
pub mod ops;
pub mod sched;
pub mod sync;

pub use api::*;
pub use batch::Batch;
pub use sched::{Query, RunningQuery, TEST_WORKER_DELAY_US};
