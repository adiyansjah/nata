//! Synchronisation primitives, switched to `loom` when built with `--cfg nata_loom`.
//! The scheduler (`sched.rs`) and the channel (`chan.rs`) import from here, so the model checker
//! runs the same code that the tokio bridge runs.

#[cfg(nata_loom)]
pub use loom::sync::atomic::{AtomicBool, AtomicUsize};
#[cfg(nata_loom)]
pub use loom::sync::{Condvar, Mutex, MutexGuard};
#[cfg(nata_loom)]
pub use loom::thread;

#[cfg(not(nata_loom))]
pub use std::sync::atomic::{AtomicBool, AtomicUsize};
#[cfg(not(nata_loom))]
pub use std::sync::{Condvar, Mutex, MutexGuard};
#[cfg(not(nata_loom))]
pub use std::thread;
