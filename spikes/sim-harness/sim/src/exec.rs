//! The one executor both runtimes use: a single-threaded, node-aware async executor with a timer wheel.
//!
//! * `Mode::Sim`: virtual time. When nothing is runnable the clock jumps to the next timer; the next task to
//!   poll is picked from the runnable set by a SEEDED random draw. No wall clock is ever read.
//! * `Mode::Real`: wall-clock time (`Instant`), FIFO order, the thread parks until a timer or a cross-thread
//!   wake. One executor per node thread.
//!
//! Tasks belong to a node. Killing a node drops every future of that node (a crash). Dropped futures run
//! their destructors, so buffers and handles die with the node, as they would with a process.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

use crate::rng::Rng;

pub type Nanos = u64;
pub type NodeId = u32;
pub type TaskId = u64;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Tasks that were woken (from any thread) and wait to be polled.
pub struct Ready {
    q: Mutex<Vec<TaskId>>,
    cv: Condvar,
}

impl Ready {
    fn new() -> Arc<Self> {
        Arc::new(Ready { q: Mutex::new(Vec::new()), cv: Condvar::new() })
    }

    fn push(&self, id: TaskId) {
        lock(&self.q).push(id);
        self.cv.notify_one();
    }

    fn take(&self) -> Vec<TaskId> {
        std::mem::take(&mut *lock(&self.q))
    }

    fn wait(&self, d: Duration) {
        let g = lock(&self.q);
        if g.is_empty() {
            let _ = self.cv.wait_timeout(g, d);
        }
    }
}

struct TaskWaker {
    id: TaskId,
    ready: Arc<Ready>,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.ready.push(self.id);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.ready.push(self.id);
    }
}

type BoxFuture = Pin<Box<dyn Future<Output = ()>>>;

struct CtlInner {
    id: Cell<TaskId>,
    aborted: Cell<bool>,
    done: Cell<bool>,
    exec: Weak<Inner>,
    /// For a task that lives in somebody else's executor (the madsim and turmoil trials): what `abort` calls.
    hook: RefCell<Option<Box<dyn Fn()>>>,
}

/// Handle to a spawned task: abort it, or ask whether it ran to its end.
#[derive(Clone)]
pub struct TaskCtl(Rc<CtlInner>);

impl TaskCtl {
    /// A handle for a task that was never started (spawn on a dead node).
    pub fn dead() -> TaskCtl {
        TaskCtl(Rc::new(CtlInner {
            id: Cell::new(0),
            aborted: Cell::new(true),
            done: Cell::new(false),
            exec: Weak::new(),
            hook: RefCell::new(None),
        }))
    }

    /// A handle whose `abort` calls `abort` (a task spawned on another runtime).
    pub fn external(abort: impl Fn() + 'static) -> TaskCtl {
        TaskCtl(Rc::new(CtlInner {
            id: Cell::new(0),
            aborted: Cell::new(false),
            done: Cell::new(false),
            exec: Weak::new(),
            hook: RefCell::new(Some(Box::new(abort))),
        }))
    }

    /// Ask the executor to drop the task's future (at its next loop iteration, never in the middle of a poll).
    pub fn abort(&self) {
        if !self.0.aborted.get() && !self.0.done.get() {
            self.0.aborted.set(true);
            if let Some(h) = self.0.hook.borrow().as_ref() {
                h();
            }
            if let Some(e) = self.0.exec.upgrade() {
                e.pending_aborts.borrow_mut().push(self.0.id.get());
            }
        }
    }

    /// The future ran to completion.
    pub fn finished(&self) -> bool {
        self.0.done.get()
    }

    pub fn is_aborted(&self) -> bool {
        self.0.aborted.get()
    }
}

struct Task {
    node: NodeId,
    fut: Option<BoxFuture>,
    waker: Waker,
    ctl: TaskCtl,
}

enum Timer {
    Wake(Waker),
    Call(Box<dyn FnOnce()>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Sim,
    Real,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RunEnd {
    /// The caller's stop condition became true.
    Stopped,
    /// No task is left and no scheduled call is pending.
    AllDone,
    /// Tasks are left but nothing can ever wake them (a deadlock).
    Quiescent,
    /// The next event is after the deadline.
    Deadline,
    /// The poll budget is spent (a livelock).
    StepLimit,
}

type PollHook = Box<dyn FnMut(Nanos, TaskId, NodeId, usize)>;

struct Inner {
    mode: Mode,
    start: Instant,
    now: Cell<Nanos>,
    next_id: Cell<TaskId>,
    seq: Cell<u64>,
    tasks: RefCell<BTreeMap<TaskId, Task>>,
    ready: Arc<Ready>,
    queued: RefCell<BTreeSet<TaskId>>,
    runq: RefCell<VecDeque<TaskId>>,
    timers: RefCell<BTreeMap<(Nanos, u64), Timer>>,
    pending_calls: Cell<usize>,
    pending_aborts: RefCell<Vec<TaskId>>,
    pending_kills: RefCell<Vec<NodeId>>,
    polling: Cell<bool>,
    order: RefCell<Rng>,
    polls: Cell<u64>,
    poll_hook: RefCell<Option<PollHook>>,
}

#[derive(Clone)]
pub struct Executor(Rc<Inner>);

impl Executor {
    pub fn new_sim(order_seed: u64) -> Executor {
        Executor::new(Mode::Sim, order_seed, Instant::now())
    }

    pub fn new_real() -> Executor {
        Executor::new(Mode::Real, 0, Instant::now())
    }

    /// A real-time executor whose clock starts at `start`, so several of them (one per node thread, and a
    /// restarted node's new one) share one timeline.
    pub fn new_real_at(start: Instant) -> Executor {
        Executor::new(Mode::Real, 0, start)
    }

    fn new(mode: Mode, order_seed: u64, start: Instant) -> Executor {
        Executor(Rc::new(Inner {
            mode,
            start,
            now: Cell::new(0),
            next_id: Cell::new(1),
            seq: Cell::new(0),
            tasks: RefCell::new(BTreeMap::new()),
            ready: Ready::new(),
            queued: RefCell::new(BTreeSet::new()),
            runq: RefCell::new(VecDeque::new()),
            timers: RefCell::new(BTreeMap::new()),
            pending_calls: Cell::new(0),
            pending_aborts: RefCell::new(Vec::new()),
            pending_kills: RefCell::new(Vec::new()),
            polling: Cell::new(false),
            order: RefCell::new(Rng::new(order_seed)),
            polls: Cell::new(0),
            poll_hook: RefCell::new(None),
        }))
    }

    pub fn mode(&self) -> Mode {
        self.0.mode
    }

    /// Virtual time in `Sim`, nanoseconds since the executor was made in `Real`.
    pub fn now(&self) -> Nanos {
        match self.0.mode {
            Mode::Sim => self.0.now.get(),
            Mode::Real => self.0.start.elapsed().as_nanos() as Nanos,
        }
    }

    pub fn set_poll_hook(&self, f: PollHook) {
        *self.0.poll_hook.borrow_mut() = Some(f);
    }

    pub fn polls(&self) -> u64 {
        self.0.polls.get()
    }

    pub fn live_tasks(&self) -> usize {
        self.0.tasks.borrow().len()
    }

    pub fn live_tasks_of(&self, node: NodeId) -> usize {
        self.0.tasks.borrow().values().filter(|t| t.node == node).count()
    }

    pub fn spawn(&self, node: NodeId, fut: impl Future<Output = ()> + 'static) -> TaskCtl {
        let i = &self.0;
        let id = i.next_id.get();
        i.next_id.set(id + 1);
        let ctl = TaskCtl(Rc::new(CtlInner {
            id: Cell::new(id),
            aborted: Cell::new(false),
            done: Cell::new(false),
            exec: Rc::downgrade(i),
            hook: RefCell::new(None),
        }));
        let c2 = ctl.clone();
        let wrapped: BoxFuture = Box::pin(async move {
            fut.await;
            c2.0.done.set(true);
        });
        let waker = Waker::from(Arc::new(TaskWaker { id, ready: Arc::clone(&i.ready) }));
        i.tasks.borrow_mut().insert(id, Task { node, fut: Some(wrapped), waker, ctl: ctl.clone() });
        i.ready.push(id);
        ctl
    }

    /// Wake `waker` at time `t` (now if `t` is in the past).
    pub fn add_wake(&self, t: Nanos, waker: Waker) {
        self.add_timer(t, Timer::Wake(waker));
    }

    /// Run `f` at time `t`, outside any task poll. Used for message delivery and for injected faults.
    pub fn at(&self, t: Nanos, f: impl FnOnce() + 'static) {
        self.0.pending_calls.set(self.0.pending_calls.get() + 1);
        self.add_timer(t, Timer::Call(Box::new(f)));
    }

    fn add_timer(&self, t: Nanos, timer: Timer) {
        let i = &self.0;
        let t = t.max(self.now());
        let s = i.seq.get();
        i.seq.set(s + 1);
        i.timers.borrow_mut().insert((t, s), timer);
    }

    /// Drop every task of `node` (a crash). Immediate when called outside a poll, otherwise at the end of it.
    pub fn kill_node(&self, node: NodeId) {
        self.0.pending_kills.borrow_mut().push(node);
        if !self.0.polling.get() {
            self.reap();
        }
    }

    fn reap(&self) {
        let i = &self.0;
        let kills = std::mem::take(&mut *i.pending_kills.borrow_mut());
        let aborts = std::mem::take(&mut *i.pending_aborts.borrow_mut());
        if kills.is_empty() && aborts.is_empty() {
            return;
        }
        let mut doomed: Vec<Task> = Vec::new();
        {
            let mut tasks = i.tasks.borrow_mut();
            for id in aborts {
                if let Some(t) = tasks.remove(&id) {
                    doomed.push(t);
                }
            }
            if !kills.is_empty() {
                let ids: Vec<TaskId> =
                    tasks.iter().filter(|(_, t)| kills.contains(&t.node)).map(|(id, _)| *id).collect();
                for id in ids {
                    if let Some(t) = tasks.remove(&id) {
                        doomed.push(t);
                    }
                }
            }
        }
        for t in &doomed {
            t.ctl.0.aborted.set(true);
        }
        // Dropped here, with no executor borrow held: destructors run now.
        drop(doomed);
    }

    /// Drop everything (tasks, timers, queues). Breaks the reference cycles tasks -> env -> world -> executor.
    pub fn clear(&self) {
        let i = &self.0;
        loop {
            let tasks = std::mem::take(&mut *i.tasks.borrow_mut());
            let timers = std::mem::take(&mut *i.timers.borrow_mut());
            let hook = i.poll_hook.borrow_mut().take();
            let empty = tasks.is_empty() && timers.is_empty() && hook.is_none();
            drop(tasks);
            drop(timers);
            drop(hook);
            if empty {
                break;
            }
        }
        i.runq.borrow_mut().clear();
        i.queued.borrow_mut().clear();
        i.pending_calls.set(0);
    }

    fn fire_due(&self) {
        let i = &self.0;
        let now = self.now();
        loop {
            let due = {
                let mut timers = i.timers.borrow_mut();
                match timers.first_key_value() {
                    Some((k, _)) if k.0 <= now => timers.pop_first().map(|(_, v)| v),
                    _ => None,
                }
            };
            match due {
                Some(Timer::Wake(w)) => w.wake(),
                Some(Timer::Call(f)) => {
                    i.pending_calls.set(i.pending_calls.get().saturating_sub(1));
                    f();
                }
                None => break,
            }
        }
    }

    fn poll_task(&self, id: TaskId) {
        let i = &self.0;
        let taken = {
            let mut tasks = i.tasks.borrow_mut();
            tasks.get_mut(&id).map(|t| (t.fut.take(), t.waker.clone(), t.ctl.0.aborted.get(), t.node))
        };
        let Some((fut, waker, aborted, node)) = taken else { return };
        let Some(mut fut) = fut else { return };
        if aborted {
            let removed = i.tasks.borrow_mut().remove(&id);
            drop(fut);
            drop(removed);
            return;
        }
        if let Some(h) = i.poll_hook.borrow_mut().as_mut() {
            h(self.now(), id, node, i.runq.borrow().len());
        }
        let mut cx = Context::from_waker(&waker);
        i.polling.set(true);
        let r = fut.as_mut().poll(&mut cx);
        i.polling.set(false);
        match r {
            Poll::Ready(()) => {
                let removed = i.tasks.borrow_mut().remove(&id);
                drop(fut);
                drop(removed);
            }
            Poll::Pending => {
                let mut tasks = i.tasks.borrow_mut();
                match tasks.get_mut(&id) {
                    Some(t) => t.fut = Some(fut),
                    None => {
                        drop(tasks);
                        drop(fut);
                    }
                }
            }
        }
    }

    /// Run until `stop()` is true, everything is done, nothing can progress, `deadline` (in this executor's
    /// time) is reached, or `max_polls` polls have happened.
    pub fn run(&self, deadline: Nanos, max_polls: u64, mut stop: impl FnMut() -> bool) -> RunEnd {
        let i = &self.0;
        loop {
            self.reap();
            if stop() {
                return RunEnd::Stopped;
            }
            for id in i.ready.take() {
                let alive = i.tasks.borrow().contains_key(&id);
                if alive && i.queued.borrow_mut().insert(id) {
                    i.runq.borrow_mut().push_back(id);
                }
            }
            let pick = {
                let q = i.runq.borrow();
                if q.is_empty() {
                    None
                } else {
                    Some(match i.mode {
                        Mode::Sim => i.order.borrow_mut().below(q.len() as u64) as usize,
                        Mode::Real => 0,
                    })
                }
            };
            if let Some(ix) = pick {
                let id = i.runq.borrow_mut().remove(ix).expect("index in range");
                i.queued.borrow_mut().remove(&id);
                if i.polls.get() >= max_polls {
                    return RunEnd::StepLimit;
                }
                i.polls.set(i.polls.get() + 1);
                self.poll_task(id);
                continue;
            }
            if i.tasks.borrow().is_empty() && i.pending_calls.get() == 0 {
                return RunEnd::AllDone;
            }
            let next = i.timers.borrow().keys().next().map(|k| k.0);
            match i.mode {
                Mode::Sim => match next {
                    None => return RunEnd::Quiescent,
                    Some(t) if t > deadline => {
                        i.now.set(deadline.max(i.now.get()));
                        return RunEnd::Deadline;
                    }
                    Some(t) => {
                        if t > i.now.get() {
                            i.now.set(t);
                        }
                        self.fire_due();
                    }
                },
                Mode::Real => {
                    let now = self.now();
                    if now >= deadline {
                        return RunEnd::Deadline;
                    }
                    match next {
                        Some(t) if t <= now => self.fire_due(),
                        Some(t) => {
                            i.ready.wait(Duration::from_nanos((t - now).min(deadline - now)));
                            self.fire_due();
                        }
                        None => {
                            i.ready.wait(Duration::from_millis(5));
                        }
                    }
                }
            }
        }
    }
}

/// Completes after `d` nanoseconds of (virtual or real) time.
pub struct Sleep {
    exec: Executor,
    deadline: Nanos,
    registered: bool,
}

impl Sleep {
    pub fn new(exec: Executor, d: Nanos) -> Sleep {
        let deadline = exec.now().saturating_add(d);
        Sleep { exec, deadline, registered: false }
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.exec.now() >= self.deadline {
            return Poll::Ready(());
        }
        if !self.registered {
            self.exec.add_wake(self.deadline, cx.waker().clone());
            self.registered = true;
        }
        Poll::Pending
    }
}

struct MbInner {
    q: VecDeque<(NodeId, Vec<u8>)>,
    waiter: Option<Waker>,
}

/// A node's inbox. `Send + Sync` so another thread (the real runtime) can deliver into it.
pub struct Mailbox {
    inner: Mutex<MbInner>,
}

impl Mailbox {
    pub fn new() -> Arc<Mailbox> {
        Arc::new(Mailbox { inner: Mutex::new(MbInner { q: VecDeque::new(), waiter: None }) })
    }

    pub fn deliver(&self, from: NodeId, bytes: Vec<u8>) {
        let waker = {
            let mut g = lock(&self.inner);
            g.q.push_back((from, bytes));
            g.waiter.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
    }

    /// Pop a message, or, when there is none, store the waker. One critical section: a message delivered
    /// between "looked, nothing" and "stored the waker" would otherwise be a lost wake-up.
    fn poll_recv(&self, waker: &Waker) -> Option<(NodeId, Vec<u8>)> {
        let mut g = lock(&self.inner);
        if let Some(m) = g.q.pop_front() {
            return Some(m);
        }
        g.waiter = Some(waker.clone());
        None
    }

    pub fn clear(&self) -> usize {
        let mut g = lock(&self.inner);
        let n = g.q.len();
        g.q.clear();
        g.waiter = None;
        n
    }

    pub fn len(&self) -> usize {
        lock(&self.inner).q.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Next message, or `None` after `d` nanoseconds.
pub struct RecvTimeout {
    exec: Executor,
    mb: Arc<Mailbox>,
    deadline: Nanos,
    registered: bool,
}

impl RecvTimeout {
    pub fn new(exec: Executor, mb: Arc<Mailbox>, d: Nanos) -> RecvTimeout {
        let deadline = exec.now().saturating_add(d);
        RecvTimeout { exec, mb, deadline, registered: false }
    }
}

impl Future for RecvTimeout {
    type Output = Option<(NodeId, Vec<u8>)>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(m) = self.mb.poll_recv(cx.waker()) {
            return Poll::Ready(Some(m));
        }
        if self.exec.now() >= self.deadline {
            return Poll::Ready(None);
        }
        if !self.registered {
            self.exec.add_wake(self.deadline, cx.waker().clone());
            self.registered = true;
        }
        Poll::Pending
    }
}
