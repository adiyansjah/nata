//! EVERY synchronisation primitive of this crate comes from here: `std` normally, `loom` under `RUSTFLAGS="--cfg nata_loom"`. Nothing else in the crate may
//! touch `std::sync`, `std::thread` or `std::sync::atomic` directly (`tests/facade.rs` greps the sources for it), so that the code the loom models explore is
//! the code that runs. `Arc` is here too: loom fails an execution in which a loom `Arc` is still alive at its end, which is the leak check of the models.

#[cfg(not(nata_loom))]
pub use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
#[cfg(not(nata_loom))]
pub use std::sync::{Arc, Condvar, Mutex, MutexGuard};
#[cfg(not(nata_loom))]
pub use std::thread::JoinHandle;

#[cfg(nata_loom)]
pub use loom::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
#[cfg(nata_loom)]
pub use loom::sync::{Arc, Condvar, Mutex, MutexGuard};
#[cfg(nata_loom)]
pub use loom::thread::JoinHandle;

/// Lock, ignoring poisoning: a panic in a worker is reported by the scheduler (see `ExitGuard`), and the state under the lock is only ever changed in
/// short sections that leave it consistent.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `Condvar::wait` with the same poisoning rule.
pub fn wait<'a, T>(cv: &Condvar, g: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    cv.wait(g)
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Start a named thread. `Err` is the OS refusing (too many threads): the caller degrades or reports, it never panics.
#[cfg(not(nata_loom))]
pub fn spawn_named<F>(name: String, f: F) -> std::io::Result<JoinHandle<()>>
where
    F: FnOnce() + Send + 'static,
{
    std::thread::Builder::new().name(name).spawn(f)
}

#[cfg(nata_loom)]
pub fn spawn_named<F>(name: String, f: F) -> std::io::Result<JoinHandle<()>>
where
    F: FnOnce() + Send + 'static,
{
    loom::thread::Builder::new().name(name).spawn(f)
}

/// Wait for a thread to end; `false` if it panicked.
pub fn join(h: JoinHandle<()>) -> bool {
    h.join().is_ok()
}

/// A hint to the OS scheduler (and to loom: a point where another thread may run). Used by model tests that spin until a flag is set.
#[cfg(not(nata_loom))]
pub fn yield_now() {
    std::thread::yield_now();
}

#[cfg(nata_loom)]
pub fn yield_now() {
    loom::thread::yield_now();
}

// The native deque path is excluded from loom: crossbeam and scoped std
// channels are not instrumented by loom. Its concurrency checks are native.
#[cfg(not(nata_loom))]
pub use std::sync::mpsc::sync_channel;
#[cfg(not(nata_loom))]
pub use std::thread::scope;
