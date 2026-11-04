//! Build-or-buy trial: can `shuttle` drive OUR scheduler and exchange?
//!
//! `sched.rs`, `api.rs`, `batch.rs`, `ops.rs` (spikes/operator-trait) and `exchange.rs` (spikes/exchange-loom) are
//! included by path, never copied. Only `sync` differs: the shim the scheduler already had for loom is pointed at
//! shuttle's types. That is the whole port.
#![allow(clippy::new_without_default)]

pub mod sync {
    pub use shuttle::sync::atomic::{AtomicBool, AtomicUsize};
    pub use shuttle::sync::{Arc, Condvar, Mutex, MutexGuard};
    pub use shuttle::thread;
    pub use std::sync::atomic::Ordering;
}

#[path = "../../../../operator-trait/src/api.rs"]
pub mod api;
#[path = "../../../../operator-trait/src/batch.rs"]
pub mod batch;
#[path = "../../../../exchange-loom/src/exchange.rs"]
#[allow(dead_code)]
pub mod exchange;
// `ops.rs` of the operator-trait spike cannot be included: it uses `std::sync::Mutex` together with
// `sched::lock`, which takes the shim's Mutex. (Finding: only `sched.rs` and `exchange.rs` follow the shim.)
// The two small helpers below are written against the shim instead.
pub mod ops;
#[path = "../../../../operator-trait/src/sched.rs"]
pub mod sched;
