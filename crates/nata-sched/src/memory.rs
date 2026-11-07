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
        let mut s = lock(&manager.state);
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
        let mut s = lock(&self.state);
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
