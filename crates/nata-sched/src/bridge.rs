//! The parts of the parked bridge (ADR 0011, ADR 0009's sticky wake) that are shared between the CPU side and the I/O side. Every synchronisation primitive
//! comes from `crate::sync`, so the loom models (`tests/bridge.rs`) explore this code, not a copy of it.
//!
//! # The bridge
//!
//! ```text
//!   CPU worker                              scheduler (one lock)                       I/O side (any thread)
//!   run(task) -> Parked  ---------------->  task moves to `parked_tasks`; the
//!   (it started a request and polled its    worker is free to claim another
//!    Completion once: no result yet)                         ^                         ... request runs ...
//!                                                            | Waker::wake(key)        Completion::complete(result)
//!                                                            +-------------------------  stores the result, THEN wakes
//!   a worker takes the woken task from `resume` and calls run(task) again: Completion::poll gives the result
//! ```
//!
//! * **Sticky, at both layers.** [`Completion::complete`] stores the result before it wakes, and [`Completion::poll`] looks at the result before it
//!   registers the waker, so a completion that lands between two polls is found by the next poll. The scheduler keeps a flag per RUNNING job: a wake that
//!   arrives before the worker has parked the job (the worker is still on its way back from `run`) resumes the job at once when it parks. Neither a
//!   completion before the first poll nor a wake before the park is lost.
//! * **A parked job holds no worker.** The scheduler owns the parked task; the worker that returned `Parked` claims the next job.
//! * **Cancel does not depend on the I/O side.** The scheduler settles every parked job itself when claiming stops (see `Shared::reap` in `lib.rs`): a task
//!   is dropped, its [`Completion`] with it, and the dropped request is the I/O side's cue to stop.

use crate::memory::{Grant, MemoryError, MemoryManager, Reservation, Wake};
use crate::sync::{Arc, Mutex, lock};

/// Wakes one parked job of a scheduler. Cheap to clone. A wake for a job that has finished, was cancelled or was already woken is ignored.
#[derive(Clone)]
pub struct Waker(Arc<WakerInner>);

struct WakerInner {
    wake: Box<dyn Fn() + Send + Sync>,
}

impl Waker {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Waker {
        Waker(Arc::new(WakerInner {
            wake: Box::new(wake),
        }))
    }

    /// A waker that does nothing (a caller that polls).
    pub fn noop() -> Waker {
        Waker::new(|| {})
    }

    pub fn wake(&self) {
        (self.0.wake)();
    }
}

impl Wake for Waker {
    fn wake(&self) {
        Waker::wake(self);
    }
}

impl std::fmt::Debug for Waker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Waker")
    }
}

struct Cell<T> {
    result: Option<T>,
    waker: Option<Waker>,
}

/// The place where the I/O side leaves the answer of one request for the CPU task that is waiting for it.
pub struct Completion<T> {
    cell: Mutex<Cell<T>>,
}

impl<T> Completion<T> {
    pub fn new() -> Arc<Completion<T>> {
        Arc::new(Completion {
            cell: Mutex::new(Cell {
                result: None,
                waker: None,
            }),
        })
    }

    /// The I/O side: leave `value`, then wake whoever registered. Stores the result BEFORE it wakes, and takes the waker out (a completed request keeps no
    /// reference to the scheduler). A second completion replaces the first (the request is over either way).
    pub fn complete(&self, value: T) {
        let waker = {
            let mut c = lock(&self.cell);
            c.result = Some(value);
            c.waker.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
    }

    /// The CPU side: the answer if it is there, otherwise remember `waker` (replacing an older one) and return `None`: the caller returns `Parked`.
    /// Looks at the result BEFORE it registers, under the same lock `complete` stores under, so a completion between two polls is never missed.
    pub fn poll(&self, waker: &Waker) -> Option<T> {
        let mut c = lock(&self.cell);
        if let Some(v) = c.result.take() {
            return Some(v);
        }
        c.waker = Some(waker.clone());
        None
    }

    /// Forget the registered waker (the waiting task is gone). A request that completes later finds nobody to wake.
    pub fn abandon(&self) {
        let w = lock(&self.cell).waker.take();
        drop(w);
    }
}

/// Removes a reader's place in the memory service's queue however its wait ends (a grant, a cancel, a dropped future): a task must not abandon its
/// queue slot, or it could starve every later request.
pub struct QueueSlot<'a> {
    memory: &'a MemoryManager,
    operator: usize,
}

impl<'a> QueueSlot<'a> {
    pub fn new(memory: &'a MemoryManager, operator: usize) -> QueueSlot<'a> {
        QueueSlot { memory, operator }
    }
}

impl Drop for QueueSlot<'_> {
    fn drop(&mut self) {
        let _ = self.memory.finish(self.operator);
    }
}

/// Why a reservation failed for good.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReserveFail {
    /// The request can never fit (larger than the budget, or nothing could be reclaimed for it).
    TooLarge,
    Cancelled,
    Other,
}

/// What one step of a reservation did.
pub enum ReserveStep {
    Granted(Grant),
    /// No grant yet. The caller's `wake` is registered with the service, sticky: a release (or a cancel) between the request and the registration is
    /// consumed by the registration and wakes it at once. Wait for the wake, then call again.
    Wait,
    Failed(ReserveFail),
}

/// One step of the reservation protocol of ADR 0009 for a reader that must not block a worker: request; when the answer is "pending", register `wake` with
/// the service and tell the caller to wait. The caller loops (keeping a [`QueueSlot`] for the whole reservation and taking turns with the other readers
/// of the same operator: the service keeps ONE outstanding request per operator).
///
/// The same function runs in the I/O runtime (`wake` = a Tokio `Notify`) and in the loom models (`wake` = a condition variable).
pub fn reserve_step(
    memory: &Arc<MemoryManager>,
    operator: usize,
    bytes: usize,
    wake: impl Wake + 'static,
) -> ReserveStep {
    match MemoryManager::request(memory, operator, bytes) {
        Ok(Reservation::Granted(g)) => ReserveStep::Granted(g),
        Ok(Reservation::Pending { .. }) => match memory.park(operator, wake) {
            Ok(()) => ReserveStep::Wait,
            Err(_) => ReserveStep::Failed(ReserveFail::Other),
        },
        Err(MemoryError::MinimumExceedsBudget | MemoryError::NoReclaimableState) => {
            ReserveStep::Failed(ReserveFail::TooLarge)
        }
        Err(MemoryError::Cancelled) => ReserveStep::Failed(ReserveFail::Cancelled),
        Err(_) => ReserveStep::Failed(ReserveFail::Other),
    }
}

/// A bounded shared fill. Each subscriber owns its wake registration; dropping
/// one never cancels another. The owner aborts I/O when its last subscriber drops.
pub struct Broadcast<T> {
    cell: Mutex<BroadcastCell<T>>,
}
struct BroadcastCell<T> {
    result: Option<T>,
    next: usize,
    waiters: std::collections::BTreeMap<usize, Option<Waker>>,
}
pub struct Subscription<T> {
    shared: Arc<Broadcast<T>>,
    id: usize,
}
impl<T: Clone> Broadcast<T> {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            cell: Mutex::new(BroadcastCell {
                result: None,
                next: 0,
                waiters: std::collections::BTreeMap::new(),
            }),
        })
    }
    pub fn subscribe(shared: &Arc<Self>, cap: usize) -> Option<Subscription<T>> {
        let mut c = lock(&shared.cell);
        if c.waiters.len() >= cap {
            return None;
        }
        let id = c.next;
        c.next += 1;
        c.waiters.insert(id, None);
        Some(Subscription {
            shared: shared.clone(),
            id,
        })
    }
    pub fn complete(&self, result: T) {
        let wakes = {
            let mut c = lock(&self.cell);
            c.result = Some(result);
            c.waiters
                .values_mut()
                .filter_map(Option::take)
                .collect::<Vec<_>>()
        };
        for w in wakes {
            w.wake();
        }
    }
}
impl<T: Clone> Subscription<T> {
    pub fn poll(&self, wake: &Waker) -> Option<T> {
        let mut c = lock(&self.shared.cell);
        if let Some(r) = &c.result {
            return Some(r.clone());
        }
        c.waiters.insert(self.id, Some(wake.clone()));
        None
    }
}
impl<T> Drop for Subscription<T> {
    fn drop(&mut self) {
        lock(&self.shared.cell).waiters.remove(&self.id);
    }
}
