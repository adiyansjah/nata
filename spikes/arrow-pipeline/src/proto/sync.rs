//! The synchronization primitives the scheduler uses, swappable for the `loom` model checker.
//!
//! Normal builds re-export std. Under `RUSTFLAGS="--cfg loom"` the same names are loom's
//! instrumented types, so `tests/loom_sched.rs` can explore the interleavings of the real
//! `sched.rs` code. Nothing else changes: this module is the only difference between the builds.

#[cfg(not(loom))]
pub use std::sync::atomic::{AtomicBool, AtomicUsize};
#[cfg(not(loom))]
pub use std::sync::{Arc, Condvar, Mutex, MutexGuard};
#[cfg(not(loom))]
pub use std::thread;

#[cfg(loom)]
pub use loom::sync::atomic::{AtomicBool, AtomicUsize};
#[cfg(loom)]
pub use loom::sync::{Arc, Condvar, Mutex, MutexGuard};
#[cfg(loom)]
pub use loom::thread;

/// loom re-exports std's `Ordering`.
pub use std::sync::atomic::Ordering;
