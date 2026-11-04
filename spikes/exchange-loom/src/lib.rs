//! Spike for ADR 0001, open check 1b: model-check the exchange (the bounded channel between two
//! pipeline fragments) together with the real scheduler. `api`, `batch`, `sched`, `sync` are
//! a snapshot of the committed operator-trait spike; `exchange` and `limit` are the verbatim
//! sections of its `ops.rs` (see SNAPSHOT.md).

pub mod api;
pub mod batch;
pub mod exchange;
pub mod limit;
pub mod sched;
pub mod sync;

pub use api::*;
pub use batch::Batch;
pub use exchange::{Channel, exchange};
pub use limit::Limit;
pub use sched::{Query, RunningQuery, TEST_WORKER_DELAY_US};
