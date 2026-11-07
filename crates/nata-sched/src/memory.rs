//! Query-wide reservations and cooperative spill pressure (ADR 0009).
//!
//! A request never waits on a CPU worker. `Pending` means: spill the selected
//! victim at its next batch boundary, then park the requesting task using
//! `park`. A notification between request and park is sticky. Notifications
//! grant no memory; the resumed task must retry. Callbacks run outside the lock.
//!
//! Operator registration is bounded and fixed at admission. One outstanding
//! request per operator is coalesced, with FIFO grant order. The progress grant
//! is charged at admission and cannot be borrowed by a spillable operator.
use crate::sync::{Arc, Mutex, lock};
use std::collections::{BTreeMap, VecDeque};

/// Bound scheduler notification. Implementations must enqueue work, never run
/// an operator or wait for I/O. The same trait is used in the loom models.
pub trait Wake: Send + Sync {
    fn wake(&self);
}

impl<T: Wake + ?Sized> Wake for Arc<T> {
    fn wake(&self) {
        (**self).wake();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryError {
    Cancelled,
    InvalidOperator,
    InvalidBudget,
    MinimumExceedsBudget,
    RequestChanged,
    NoReclaimableState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    pub budget: usize,
    pub progress: usize,
    pub held: usize,
    pub peak: usize,
    pub live_grants: usize,
    pub pending: usize,
    pub pressure: usize,
    pub requests: usize,
    pub releases: usize,
    pub lock_wait_nanos: u64,
    pub waits: usize,
    pub wakes: usize,
    pub cancelled: bool,
}

struct Operator {
    held: usize,
    revoke: bool,
    notified: bool,
    waker: Option<Box<dyn Wake>>,
}

struct State {
    operators: Vec<Operator>,
    live: BTreeMap<u64, (usize, usize)>,
    next_id: u64,
    queue: VecDeque<(usize, usize)>,
    held: usize,
    peak: usize,
    pressure: usize,
    requests: usize,
    releases: usize,
    lock_wait_nanos: u64,
    waits: usize,
    wakes: usize,
    cancelled: bool,
}

/// One service per admitted query, shared by *all* its execution tasks.
/// `operators` counts spillable operators, not workers. Workers of an operator
/// subdivide its reservations; they do not get independent copies of B.
pub struct MemoryManager {
    budget: usize,
    progress: usize,
    state: Mutex<State>,
}

/// A unique, transferable capacity reservation. Drop releases it exactly once.
/// Keep it until the charged allocation has been freed, including during I/O.
#[derive(Debug)]
pub struct Grant {
    manager: Arc<MemoryManager>,
    id: u64,
    bytes: usize,
}

pub enum Reservation {
    Granted(Grant),
    /// Wait for the FIFO head, or ask the named operator to reclaim state.
    /// None means free memory exists but an earlier request owns the next turn.
    Pending {
        victim: Option<usize>,
    },
}

impl MemoryManager {
    /// Admission reserves `progress` for bounded decode/output/spill workspace.
    /// The caller must allocate that workspace only *after* this succeeds.
    pub fn new(budget: usize, progress: usize, operators: usize) -> Result<Arc<Self>, MemoryError> {
        if progress == 0 || progress >= budget || operators == 0 || operators > 64 {
            return Err(MemoryError::InvalidBudget);
        }
        Ok(Arc::new(Self {
            budget,
            progress,
            state: Mutex::new(State {
                operators: (0..operators)
                    .map(|_| Operator {
                        held: 0,
                        revoke: false,
                        notified: false,
                        waker: None,
                    })
                    .collect(),
                live: BTreeMap::new(),
                next_id: 0,
                queue: VecDeque::with_capacity(operators),
                held: 0,
                peak: progress,
                pressure: 0,
                requests: 0,
                releases: 0,
                lock_wait_nanos: 0,
                waits: 0,
                wakes: 0,
                cancelled: false,
            }),
        }))
    }

    /// Reserve capacity before allocating. Borrow idle shares, but never B's
    /// progress workspace. Failure queues at most one request per operator.
    pub fn request(
        manager: &Arc<Self>,
        operator: usize,
        bytes: usize,
    ) -> Result<Reservation, MemoryError> {
        let clock = clock();
        let mut s = lock(&manager.state);
        s.lock_wait_nanos = s.lock_wait_nanos.saturating_add(elapsed(clock));
        s.requests = s.requests.saturating_add(1);
        if s.cancelled {
            return Err(MemoryError::Cancelled);
        }
        if operator >= s.operators.len() {
            return Err(MemoryError::InvalidOperator);
        }
        let available = manager.budget - manager.progress;
        if bytes == 0 || bytes > available {
            return Err(MemoryError::MinimumExceedsBudget);
        }
        if s.queue.iter().any(|&(op, n)| op == operator && n != bytes) {
            return Err(MemoryError::RequestChanged);
        }
        let turn = s.queue.front().is_none_or(|&(op, _)| op == operator);
        if turn && bytes <= available - s.held {
            if s.queue.front().is_some_and(|&(op, _)| op == operator) {
                s.queue.pop_front();
            }
            let id = s.next_id;
            s.next_id = id.checked_add(1).ok_or(MemoryError::InvalidBudget)?;
            s.held += bytes;
            s.operators[operator].held += bytes;
            s.live.insert(id, (operator, bytes));
            s.peak = s.peak.max(manager.progress + s.held);
            debug_assert!(
                manager.progress + s.held <= manager.budget,
                "live reservations exceed B"
            );
            let grant = Grant {
                manager: Arc::clone(manager),
                id,
                bytes,
            };
            // A smaller FIFO successor may fit even though the predecessor
            // failed earlier. Its retry must not need another release event.
            let wake = manager.notify_front(&mut s);
            drop(s);
            wake_all(wake);
            return Ok(Reservation::Granted(grant));
        }
        let share = available / s.operators.len();
        let victim = s
            .operators
            .iter()
            .enumerate()
            .filter(|(_, op)| op.held > share)
            .max_by_key(|(id, op)| (op.held, std::cmp::Reverse(*id)))
            .map(|(id, _)| id)
            .or_else(|| (s.operators[operator].held > 0).then_some(operator));
        if victim.is_none() && turn {
            return Err(MemoryError::NoReclaimableState);
        }
        if !s.queue.iter().any(|&(op, _)| op == operator) {
            s.queue.push_back((operator, bytes));
        }
        s.pressure = s.pressure.saturating_add(1);
        let mut wakes = manager.notify_front(&mut s);
        if let Some(victim) = victim {
            s.operators[victim].revoke = true;
            s.operators[victim].notified = true;
            if let Some(waker) = s.operators[victim].waker.take() {
                s.wakes = s.wakes.saturating_add(1);
                wakes.push(waker);
            }
        }
        drop(s);
        wake_all(wakes);
        Ok(Reservation::Pending { victim })
    }

    /// Read pressure at a batch boundary, without invoking the operator while
    /// the service is locked. The flag remains set until its state is released.
    pub fn revoked(&self, operator: usize) -> Result<bool, MemoryError> {
        lock(&self.state)
            .operators
            .get(operator)
            .map(|op| op.revoke)
            .ok_or(MemoryError::InvalidOperator)
    }

    /// Register the task after returning `Blocked`. A release/cancel that raced
    /// ahead of registration is consumed here and immediately enqueues it.
    /// Replaces the previous registration; one operator has one blocked owner.
    pub fn park<W: Wake + 'static>(&self, operator: usize, waker: W) -> Result<(), MemoryError> {
        let waker: Box<dyn Wake> = Box::new(waker);
        let mut s = lock(&self.state);
        let cancelled = s.cancelled;
        let op = s
            .operators
            .get_mut(operator)
            .ok_or(MemoryError::InvalidOperator)?;
        if op.notified || cancelled {
            op.notified = false;
            let previous = op.waker.take();
            s.wakes = s.wakes.saturating_add(1);
            drop(s);
            drop(previous);
            waker.wake();
        } else {
            let previous = op.waker.replace(waker);
            s.waits = s.waits.saturating_add(1);
            drop(s);
            drop(previous);
        }
        Ok(())
    }

    /// Remove a pending operator that has finished/failed. A task must not
    /// abandon its queue slot: doing so could starve every later operator.
    /// Existing grants remain owned until their allocations have been dropped.
    pub fn finish(&self, operator: usize) -> Result<(), MemoryError> {
        let mut s = lock(&self.state);
        let op = s
            .operators
            .get_mut(operator)
            .ok_or(MemoryError::InvalidOperator)?;
        let previous = op.waker.take();
        op.notified = false;
        op.revoke = false;
        s.queue.retain(|&(id, _)| id != operator);
        let wake = self.notify_front(&mut s);
        drop(s);
        drop(previous);
        wake_all(wake);
        Ok(())
    }

    /// Cancellation is sticky, wakes every blocked task, and grants no bytes.
    /// The owner joins tasks and disk jobs before dropping allocations/grants.
    pub fn cancel(&self) {
        let mut s = lock(&self.state);
        s.cancelled = true;
        s.queue.clear();
        let mut wake = Vec::new();
        for op in &mut s.operators {
            op.notified = true;
            op.revoke = false;
            if let Some(w) = op.waker.take() {
                wake.push(w);
            }
        }
        s.wakes = s.wakes.saturating_add(wake.len());
        drop(s);
        wake_all(wake);
    }

    fn notify_front(&self, s: &mut State) -> Vec<Box<dyn Wake>> {
        let Some(&(operator, bytes)) = s.queue.front() else {
            return Vec::new();
        };
        if bytes > self.budget - self.progress - s.held {
            return Vec::new();
        }
        let op = &mut s.operators[operator];
        op.notified = true;
        match op.waker.take() {
            Some(w) => {
                s.wakes = s.wakes.saturating_add(1);
                vec![w]
            }
            None => Vec::new(),
        }
    }

    fn release(&self, id: u64) {
        let clock = clock();
        let mut s = lock(&self.state);
        s.lock_wait_nanos = s.lock_wait_nanos.saturating_add(elapsed(clock));
        s.releases = s.releases.saturating_add(1);
        // The ledger is an independent invariant, not the budget enforcement.
        let (operator, bytes) = s.live.remove(&id).expect("reservation released twice");
        s.held = s.held.checked_sub(bytes).expect("reservation underflow");
        let op = &mut s.operators[operator];
        op.held = op.held.checked_sub(bytes).expect("operator underflow");
        if op.held == 0 {
            op.revoke = false;
        }
        let wake = self.notify_front(&mut s);
        drop(s);
        wake_all(wake);
    }

    pub fn snapshot(&self) -> Snapshot {
        let s = lock(&self.state);
        Snapshot {
            budget: self.budget,
            progress: self.progress,
            held: s.held,
            peak: s.peak,
            live_grants: s.live.len(),
            pending: s.queue.len(),
            pressure: s.pressure,
            requests: s.requests,
            releases: s.releases,
            lock_wait_nanos: s.lock_wait_nanos,
            waits: s.waits,
            wakes: s.wakes,
            cancelled: s.cancelled,
        }
    }
}

fn wake_all(wakers: Vec<Box<dyn Wake>>) {
    for w in wakers {
        w.wake();
    }
}

impl Grant {
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

impl Drop for Grant {
    fn drop(&mut self) {
        self.manager.release(self.id);
    }
}

impl std::fmt::Debug for MemoryManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.snapshot().fmt(f)
    }
}

// Wall clock instrumentation is native only: the exact same accounting and
// synchronization are modelled, without making model assertions clock-dependent.
fn clock() -> Option<std::time::Instant> {
    #[cfg(not(nata_loom))]
    {
        Some(std::time::Instant::now())
    }
    #[cfg(nata_loom)]
    {
        None
    }
}
fn elapsed(start: Option<std::time::Instant>) -> u64 {
    start.map_or(0, |t| {
        u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX)
    })
}

/// Task-owned chunks amortize manager locking over many small allocations.
/// The entire capacity (including unused credit) remains reserved until the
/// owning allocations have been freed. This is not a second/per-task budget.
#[derive(Default)]
pub struct Chunks {
    grants: Vec<Grant>,
    spare: usize,
    pending: Option<usize>,
}
impl Chunks {
    const METADATA: usize = 128;
    pub fn new() -> Self {
        Self::default()
    }
    /// Reserve before allocating. `false` is pressure: retain the queued request
    /// and yield/spill before retrying; never allocate on a pending request.
    pub fn reserve(
        &mut self,
        manager: &Arc<MemoryManager>,
        operator: usize,
        bytes: usize,
        chunk: usize,
    ) -> Result<bool, MemoryError> {
        if self.spare >= bytes {
            self.spare -= bytes;
            return Ok(true);
        }
        let needed = bytes
            .checked_sub(self.spare)
            .and_then(|n| n.checked_add(Self::METADATA))
            .ok_or(MemoryError::MinimumExceedsBudget)?;
        let snapshot = manager.snapshot();
        let free = snapshot.budget - snapshot.progress - snapshot.held;
        // Borrow only what's available. A small budget need not fit a 1 MiB
        // chunk; it must fit this operation and its reservation bookkeeping.
        let request = self
            .pending
            .unwrap_or_else(|| chunk.max(needed).min(free).max(needed));
        match MemoryManager::request(manager, operator, request)? {
            Reservation::Granted(grant) => {
                self.pending = None;
                self.grants
                    .try_reserve(1)
                    .map_err(|_| MemoryError::InvalidBudget)?;
                self.spare = self.spare.saturating_add(grant.bytes() - Self::METADATA);
                self.grants.push(grant);
                self.spare -= bytes;
                Ok(true)
            }
            Reservation::Pending { .. } => {
                self.pending = Some(request);
                Ok(false)
            }
        }
    }
    /// Move charges along with allocations during partition ownership transfer.
    pub fn append(&mut self, other: &mut Self) {
        self.grants.append(&mut other.grants);
        self.spare += std::mem::take(&mut other.spare);
    }
    pub fn reserved(&self) -> usize {
        self.grants.iter().map(Grant::bytes).sum()
    }
}
