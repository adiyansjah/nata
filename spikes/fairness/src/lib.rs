//! Spike for ADR 0002, open check (2): fairness of the CPU-pool scheduler. `api`, `batch`, `chan`,
//! `sched`, `sync` are copies of `spikes/async-bridge/src` plus the slice/yield change (SNAPSHOT.md).

pub mod api;
pub mod batch;
pub mod chan;
#[cfg(not(nata_loom))]
pub mod ops;
pub mod sched;
pub mod sync;

#[cfg(not(nata_loom))]
pub mod load;

pub use api::*;
pub use batch::Batch;
pub use sched::{Query, RunningQuery, SliceConfig, TEST_WORKER_DELAY_US};
