//! Spike for ADR 0001, verification-plan item 5: the cost of the push protocol with cheap kernels, static dispatch and a
//! branching pipeline, measured as a paired, interleaved benchmark with a control arm.
//!
//! The protocol, the batch type, the scheduler and the exchange are the REAL files of `spikes/operator-trait`, included
//! by path (never copied), so this crate cannot drift from them. Everything else in here is new: the kernels, the arms,
//! the statistics and the harness self-test.

#[path = "../../operator-trait/src/api.rs"]
pub mod api;
#[path = "../../operator-trait/src/batch.rs"]
pub mod batch;
// The exchange (`ops::exchange`) is the one used by the branching topology. The rest of ops.rs is not used here.
#[path = "../../operator-trait/src/ops.rs"]
pub mod ops;
#[path = "../../operator-trait/src/sched.rs"]
pub mod sched;
#[path = "../../operator-trait/src/sync.rs"]
pub mod sync;

pub use api::*;
pub use batch::Batch;
pub use sched::Query;

pub mod arms;
pub mod bench;
pub mod branch;
pub mod kernels;
pub mod selftest;
pub mod stages;
pub mod stats;
pub mod sys;
