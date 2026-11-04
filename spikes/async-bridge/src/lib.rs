//! Spike for ADR 0002: the hand-off between a tokio runtime (I/O) and the synchronous CPU-pool
//! scheduler of ADR 0001. `api`, `batch`, `sched` are a snapshot of the committed operator-trait
//! spike (see SNAPSHOT.md); `chan` is the new bounded channel with a waker slot.

pub mod api;
pub mod batch;
pub mod chan;
pub mod sched;
pub mod sync;

#[cfg(not(nata_loom))]
pub mod harness;

pub use api::*;
pub use batch::Batch;
pub use sched::{Query, RunningQuery, TEST_WORKER_DELAY_US};
