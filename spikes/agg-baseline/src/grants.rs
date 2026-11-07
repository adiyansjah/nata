//! One lock covers reservations, waiter registration and the park/wake token.
//! No operator callback or blocking I/O runs under that lock. The condvar below
//! belongs to the test dispatcher, never to an operator/CPU worker.
#[cfg(nata_loom)]
use loom::sync::{Arc, Condvar, Mutex};
use std::collections::{BTreeMap, VecDeque};
#[cfg(not(nata_loom))]
use std::sync::{Arc, Condvar, Mutex};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mutation {
    None,
    NoCap,
    NoWake,
    ReleaseTwice,
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum Task {
    Running,
    Parked,
    Ready,
}
struct State {
    held: [usize; 2],
    live: BTreeMap<usize, (usize, usize)>,
    next: usize,
    queue: VecDeque<(usize, usize)>,
    revoke: [bool; 2],
    notified: [bool; 2],
    task: [Task; 2],
    cancelled: bool,
    peak: usize,
    pressure: usize,
    parks: usize,
    wakes: usize,
}
pub struct Manager {
    budget: usize,
    // Charged up front and retained until query teardown. Reclaim allocates none.
    progress: usize,
    mutation: Mutation,
    state: Mutex<State>,
    dispatch: Condvar,
}
pub struct Grant {
    manager: Arc<Manager>,
    id: usize,
}
impl Manager {
    pub fn new(budget: usize, progress: usize, mutation: Mutation) -> Arc<Self> {
        assert!(progress <= budget);
        Arc::new(Self {
            budget,
            progress,
            mutation,
            state: Mutex::new(State {
                held: [0; 2],
                live: BTreeMap::new(),
                next: 0,
                queue: VecDeque::new(),
                revoke: [false; 2],
                notified: [false; 2],
                task: [Task::Running; 2],
                cancelled: false,
                peak: progress,
                pressure: 0,
                parks: 0,
                wakes: 0,
            }),
            dispatch: Condvar::new(),
        })
    }
    /// Err is cancellation or an impossible minimum, never an indefinite wait.
    pub fn request(
        manager: &Arc<Self>,
        op: usize,
        bytes: usize,
    ) -> Result<Option<Grant>, &'static str> {
        let mut s = manager.state.lock().unwrap();
        if s.cancelled {
            return Err("cancelled");
        }
        if bytes > manager.budget - manager.progress {
            return Err("minimum exceeds budget");
        }
        let used = manager.progress + s.held.iter().sum::<usize>();
        let turn = s.queue.front().is_none_or(|&(who, _)| who == op);
        if turn && (used + bytes <= manager.budget || manager.mutation == Mutation::NoCap) {
            if s.queue.front().is_some_and(|&(who, _)| who == op) {
                s.queue.pop_front();
            }
            s.held[op] += bytes;
            let id = s.next;
            s.next += 1;
            s.live.insert(id, (op, bytes));
            s.peak = s.peak.max(used + bytes);
            drop(s);
            assert!(used + bytes <= manager.budget, "grant cap exceeded");
            return Ok(Some(Grant {
                manager: Arc::clone(manager),
                id,
            }));
        }
        if !s.queue.iter().any(|&(who, _)| who == op) {
            s.queue.push_back((op, bytes));
        }
        s.pressure += 1;
        let share = (manager.budget - manager.progress) / 2;
        let victim = (0..2)
            .filter(|&v| s.held[v] > share)
            .max_by_key(|&v| (s.held[v], std::cmp::Reverse(v)))
            .unwrap_or(op);
        if s.held[victim] == 0 {
            return Err("no reclaimable state");
        }
        s.revoke[victim] = true;
        Ok(None)
    }
    pub fn revoked(&self, op: usize) -> bool {
        self.state.lock().unwrap().revoke[op]
    }
    /// Called AFTER returning Blocked: notification during Running is retained.
    pub fn park(&self, op: usize) {
        let mut s = self.state.lock().unwrap();
        if s.notified[op] || s.cancelled {
            s.notified[op] = false;
            s.task[op] = Task::Ready;
        } else {
            s.task[op] = Task::Parked;
            s.parks += 1;
        }
    }
    /// Test dispatcher waits for runnable work. This is not the operator API.
    pub fn dispatch(&self, op: usize) {
        let mut s = self.state.lock().unwrap();
        while s.task[op] == Task::Parked {
            s = self.dispatch.wait(s).unwrap();
        }
        s.task[op] = Task::Running;
        s.notified[op] = false;
    }
    fn wake(&self, s: &mut State, op: usize) {
        if self.mutation == Mutation::NoWake {
            return;
        }
        s.notified[op] = true;
        if s.task[op] == Task::Parked {
            s.task[op] = Task::Ready;
        }
        s.wakes += 1;
        self.dispatch.notify_all();
    }
    fn release(&self, id: usize) {
        let mut s = self.state.lock().unwrap();
        let (op, bytes) = s.live.remove(&id).expect("double release");
        s.held[op] = s.held[op].checked_sub(bytes).expect("release underflow");
        s.revoke[op] = false;
        // FIFO notification order; a notification does NOT grant any bytes.
        let waiters: Vec<_> = s.queue.iter().map(|&(op, _)| op).collect();
        for op in waiters {
            self.wake(&mut s, op);
        }
    }
    pub fn cancel(&self) {
        let mut s = self.state.lock().unwrap();
        s.cancelled = true;
        s.queue.clear();
        for op in 0..2 {
            self.wake(&mut s, op);
        }
    }
    pub fn snapshot(&self) -> (usize, usize, usize, usize, usize, usize) {
        let s = self.state.lock().unwrap();
        (
            s.held.iter().sum(),
            s.live.len(),
            s.peak,
            s.pressure,
            s.parks,
            s.wakes,
        )
    }
}
impl Drop for Grant {
    fn drop(&mut self) {
        self.manager.release(self.id);
        if self.manager.mutation == Mutation::ReleaseTwice {
            self.manager.release(self.id);
        }
    }
}
