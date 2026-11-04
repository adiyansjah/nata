//! The TOY DISTRIBUTED PROTOCOL: one coordinator (node 0) and workers (nodes 1..=N).
//!
//! The coordinator splits a scan into tasks (one per input object), assigns them to workers, and collects the
//! results. A worker reads its input object from the object store, runs the real operator protocol over it
//! (`engine::process`), and streams the result back in frames over the unreliable network: sequence numbers,
//! a sliding window (backpressure), cumulative acks, retransmission. Workers heartbeat; a worker that goes
//! silent or restarts has its tasks retried on another worker as a NEW ATTEMPT (attempts produce their output in
//! a different order, so the coordinator must never mix two attempts). The client may cancel the query; the
//! coordinator then makes sure every worker has dropped its tasks, and finally shuts the workers down. A worker
//! that hears nothing from the coordinator for a lease period exits by itself.
//!
//! Everything here reaches the world only through `Env`. This file is the mutation target of `mutants/`: keep
//! the mutated lines textually stable (each is found exactly once by `mutants/mutate.py`).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::engine;
use crate::env::*;
use crate::frame::*;
use crate::rng::mix;

pub const MS: Nanos = 1_000_000;
pub const TICK: Nanos = 2 * MS;
pub const HB_PERIOD: Nanos = 10 * MS;
pub const DEAD_AFTER: Nanos = 35 * MS;
pub const RTO: Nanos = 12 * MS;
pub const ASSIGN_RTO: Nanos = 15 * MS;
pub const CANCEL_RTO: Nanos = 6 * MS;
pub const SHUTDOWN_RTO: Nanos = 4 * MS;
pub const LEASE: Nanos = 120 * MS;
/// While cancelling or shutting down, a worker counts as reachable until it has been silent this long.
pub const CLOSE_GRACE: Nanos = 150 * MS;
pub const COORD: NodeId = 0;
/// Tasks a worker may run at once; keeps the work spread over the workers however the first heartbeats arrive.
pub const SLOTS: usize = 2;

type WMap<V> = BTreeMap<NodeId, V>;

#[derive(Clone, Debug)]
pub struct Config {
    pub files: u32,
    pub workers: u32,
    /// Frames a worker may have sent and not yet had acknowledged, per task.
    pub window: u32,
    pub batch_rows: usize,
}

pub fn checksum(rows: &[i64]) -> u64 {
    rows.iter().fold(0u64, |a, &v| a.wrapping_add(mix(v as u64, 0xC0FF_EE00)))
}

// ------------------------------------------------------------------------------------------ coordinator

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TState {
    Pending,
    Running,
    Complete,
}

struct TaskSt {
    attempt: u32,
    state: TState,
    worker: NodeId,
    next_seq: u32,
    last_seq: Option<u32>,
    pending: BTreeMap<u32, (Vec<i64>, bool)>,
    rows: Vec<i64>,
    got_first: bool,
    assign_sent: Nanos,
}

struct WorkerSt {
    ever_seen: bool,
    alive: bool,
    exited: bool,
    born: u64,
    last_hb: Nanos,
    cancel_acked: bool,
    cancel_sent: Option<Nanos>,
    shutdown_sent: Option<Nanos>,
}

impl WorkerSt {
    fn new() -> Self {
        WorkerSt {
            ever_seen: false,
            alive: false,
            exited: false,
            born: 0,
            last_hb: 0,
            cancel_acked: false,
            cancel_sent: None,
            shutdown_sent: None,
        }
    }

    /// Still worth talking to while cancelling or shutting down. A worker never heard from counts as reachable for
    /// the first CLOSE_GRACE of the coordinator's life (its first heartbeat may just have been lost).
    fn reachable(&self, now: Nanos) -> bool {
        !self.exited && now.saturating_sub(self.last_hb) <= CLOSE_GRACE
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Running,
    Cancelling,
    Closing,
}

struct Coord<E: Env> {
    env: E,
    cfg: Config,
    tasks: Vec<TaskSt>,
    workers: WMap<WorkerSt>,
    phase: Phase,
    cancel_started: bool,
}

pub async fn coordinator<E: Env>(env: E, cfg: Config) {
    let tasks = (0..cfg.files)
        .map(|_| TaskSt {
            attempt: 0,
            state: TState::Pending,
            worker: 0,
            next_seq: 0,
            last_seq: None,
            pending: BTreeMap::new(),
            rows: Vec::new(),
            got_first: false,
            assign_sent: 0,
        })
        .collect();
    let workers = (1..=cfg.workers).map(|w| (w, WorkerSt::new())).collect();
    Coord { env, cfg, tasks, workers, phase: Phase::Running, cancel_started: false }.run().await;
}

impl<E: Env> Coord<E> {
    async fn run(mut self) {
        loop {
            let now = self.env.now();
            self.update_liveness(now);
            match self.phase {
                Phase::Running => {
                    self.retry_dead();
                    self.assign_pending(now);
                    self.resend_assigns(now);
                    if self.tasks.iter().all(|t| t.state == TState::Complete) {
                        self.publish().await;
                    }
                }
                Phase::Cancelling => self.cancelling(now),
                Phase::Closing => {
                    if self.closing(now) {
                        break;
                    }
                }
            }
            if let Some((from, bytes)) = self.env.recv_timeout(TICK).await {
                match Msg::decode(&bytes) {
                    Ok(m) => self.on_msg(from, m),
                    Err(_) => self.env.event("bad_frame", from as u64, bytes.len() as u64),
                }
            }
        }
        self.env.event("coordinator_exit", 0, 0);
    }

    fn update_liveness(&mut self, now: Nanos) {
        for (id, w) in self.workers.iter_mut() {
            if w.alive && now.saturating_sub(w.last_hb) > DEAD_AFTER {
                w.alive = false;
                self.env.event("worker_dead", *id as u64, 0);
            }
        }
    }

    fn on_msg(&mut self, from: NodeId, m: Msg) {
        let now = self.env.now();
        match m {
            Msg::ClientCancel => {
                if self.phase == Phase::Running {
                    self.phase = Phase::Cancelling;
                    self.env.event("cancel_requested", 0, 0);
                }
            }
            Msg::Heartbeat { born } => self.on_heartbeat(from, born, now),
            Msg::Data { task, attempt, seq, last, rows } => self.on_data(from, task, attempt, seq, last, rows),
            Msg::CancelAck { born } => {
                if let Some(w) = self.workers.get_mut(&from) {
                    if w.born == born {
                        w.cancel_acked = true;
                        self.env.event("cancel_ack", from as u64, 0);
                    }
                }
            }
            Msg::Bye { born } => {
                if let Some(w) = self.workers.get_mut(&from) {
                    if w.born == born {
                        w.exited = true;
                        w.alive = false;
                        self.env.event("bye", from as u64, 0);
                    }
                }
            }
            _ => self.env.event("unexpected_msg", from as u64, 0),
        }
    }

    fn on_heartbeat(&mut self, from: NodeId, born: u64, now: Nanos) {
        let Some(w) = self.workers.get_mut(&from) else { return };
        if w.ever_seen && born < w.born {
            return; // a late heartbeat of an earlier incarnation
        }
        if w.exited && born == w.born {
            return; // a late heartbeat of an incarnation that already said goodbye
        }
        let restarted = w.ever_seen && born > w.born;
        let revived = !w.alive;
        w.ever_seen = true;
        w.born = born;
        w.last_hb = now;
        w.alive = true;
        if restarted {
            w.exited = false;
            w.cancel_acked = false; // the new incarnation must be cancelled too
            w.shutdown_sent = None;
        }
        if revived {
            self.env.event("worker_alive", from as u64, 0);
        }
        if restarted {
            self.env.event("worker_restart", from as u64, 0);
            self.retry_stranded(from);
        }
        self.env.send(from, Msg::HbAck.encode());
    }

    fn on_data(&mut self, from: NodeId, task: u32, attempt: u32, seq: u32, last: bool, rows: Vec<i64>) {
        let window = self.cfg.window;
        let running = self.phase == Phase::Running;
        let Some(t) = self.tasks.get_mut(task as usize) else {
            self.env.event("bad_task", task as u64, 0);
            return;
        };
        let stale = !running || t.state != TState::Running || t.attempt != attempt;
        if stale {
            self.env.event("stale_data", task as u64, attempt as u64);
            self.env.send(from, Msg::Ack { task, attempt, next: 0, stale: true }.encode());
            return;
        }
        t.got_first = true;
        if seq >= t.next_seq.saturating_add(window) {
            self.env.violation(format!(
                "backpressure window exceeded: task {task} frame {seq} while next expected is {} (window {window})",
                t.next_seq
            ));
            return;
        }
        if seq == t.next_seq {
            t.rows.extend_from_slice(&rows);
            if last {
                t.last_seq = Some(seq);
            }
            t.next_seq += 1;
            while let Some((more, l)) = t.pending.remove(&t.next_seq) {
                t.rows.extend_from_slice(&more);
                if l {
                    t.last_seq = Some(t.next_seq);
                }
                t.next_seq += 1;
            }
        } else if seq > t.next_seq {
            if last {
                t.last_seq = Some(seq);
            }
            t.pending.entry(seq).or_insert((rows, last));
        } else {
            self.env.event("dup_frame", task as u64, seq as u64);
        }
        let next = t.next_seq;
        if matches!(t.last_seq, Some(l) if next > l) {
            t.state = TState::Complete;
            self.env.event("task_complete", task as u64, attempt as u64);
        }
        self.env.send(from, Msg::Ack { task, attempt, next, stale: false }.encode());
    }

    /// Tasks that were running on `w`, which died or restarted, start over as a new attempt.
    fn retry_stranded(&mut self, w: NodeId) {
        for i in 0..self.tasks.len() {
            let t = &mut self.tasks[i];
            if t.state == TState::Running && t.worker == w {
                t.attempt += 1;
                t.state = TState::Pending;
                t.next_seq = 0;
                t.last_seq = None;
                t.pending.clear();
                t.rows.clear();
                t.got_first = false;
                self.env.event("retry", i as u64, t.attempt as u64);
            }
        }
    }

    fn retry_dead(&mut self) {
        let dead: Vec<NodeId> = self.workers.iter().filter(|(_, w)| w.ever_seen && !w.alive).map(|(id, _)| *id).collect();
        for id in dead {
            self.retry_stranded(id);
        }
    }

    fn load(&self, w: NodeId) -> usize {
        self.tasks.iter().filter(|t| t.state == TState::Running && t.worker == w).count()
    }

    fn pick_worker(&self) -> Option<NodeId> {
        self.workers
            .iter()
            .filter(|(id, w)| w.alive && self.load(**id) < SLOTS)
            .map(|(id, _)| (self.load(*id), *id))
            .min()
            .map(|(_, id)| id)
    }

    fn assign_pending(&mut self, now: Nanos) {
        for i in 0..self.tasks.len() {
            if self.tasks[i].state != TState::Pending {
                continue;
            }
            let Some(w) = self.pick_worker() else { break };
            let t = &mut self.tasks[i];
            t.state = TState::Running;
            t.worker = w;
            t.assign_sent = now;
            t.got_first = false;
            let m = Msg::Assign { task: i as u32, attempt: t.attempt, file: i as u32 };
            self.env.send(w, m.encode());
            self.env.event("assign", i as u64, ((t.attempt as u64) << 32) | w as u64);
        }
    }

    fn resend_assigns(&mut self, now: Nanos) {
        for i in 0..self.tasks.len() {
            let t = &mut self.tasks[i];
            if t.state == TState::Running && !t.got_first && now.saturating_sub(t.assign_sent) > ASSIGN_RTO {
                t.assign_sent = now;
                let m = Msg::Assign { task: i as u32, attempt: t.attempt, file: i as u32 };
                self.env.send(t.worker, m.encode());
                self.env.event("assign_resend", i as u64, t.attempt as u64);
            }
        }
    }

    /// Every task is complete: write the rows, then the manifest (the commit marker), each until it is accepted.
    async fn publish(&mut self) {
        let mut all: Vec<i64> = Vec::new();
        for t in &self.tasks {
            all.extend_from_slice(&t.rows);
        }
        let mut manifest = Vec::new();
        manifest.extend_from_slice(&(all.len() as u64).to_le_bytes());
        manifest.extend_from_slice(&checksum(&all).to_le_bytes());
        let objects = [("result/rows", encode_i64s(&all)), ("result/manifest", manifest)];
        for (key, val) in objects {
            let mut tries = 0u64;
            loop {
                match self.env.store_put(key, val.clone()).await {
                    Ok(()) => break,
                    Err(_) => {
                        tries += 1;
                        self.env.event("put_retry", tries, 0);
                        if tries > 500 {
                            self.env.violation(format!("could not write {key}"));
                            break;
                        }
                        self.env.sleep(MS).await;
                    }
                }
            }
        }
        self.env.gauge_set("outcome", 1);
        self.env.event("completed", all.len() as u64, 0);
        self.phase = Phase::Closing;
    }

    fn cancelling(&mut self, now: Nanos) {
        let first = !self.cancel_started;
        self.cancel_started = true;
        let mut waiting = 0;
        for (id, w) in self.workers.iter_mut() {
            if !w.reachable(now) || w.cancel_acked {
                continue;
            }
            waiting += 1;
            if w.cancel_sent.is_none_or(|t| now.saturating_sub(t) >= CANCEL_RTO) {
                self.env.send(*id, Msg::Cancel.encode());
                w.cancel_sent = Some(now);
            }
        }
        if first {
            self.env.fault_point("coord.cancel_sent");
        }
        if waiting == 0 {
            self.env.gauge_set("outcome", 2);
            self.env.gauge_set("cancel_done", 1);
            self.env.event("cancel_done", 0, 0);
            self.phase = Phase::Closing;
        }
    }

    /// Tell every reachable worker to shut down; true when none is left to tell.
    fn closing(&mut self, now: Nanos) -> bool {
        let mut pending = 0;
        for (id, w) in self.workers.iter_mut() {
            if !w.reachable(now) {
                continue;
            }
            pending += 1;
            if w.shutdown_sent.is_none_or(|t| now.saturating_sub(t) >= SHUTDOWN_RTO) {
                self.env.send(*id, Msg::Shutdown.encode());
                w.shutdown_sent = Some(now);
            }
        }
        pending == 0
    }
}

// ---------------------------------------------------------------------------------------------- worker

struct WTask {
    attempt: u32,
    ready: bool,
    frames: Vec<Vec<i64>>,
    sent: Vec<Option<Nanos>>,
    base: u32,
    next_new: u32,
    ctl: Option<TaskCtl>,
    _buf: Tracked,
}

struct WState {
    tasks: BTreeMap<u32, WTask>,
    cancelled: bool,
    shutdown: bool,
}

impl WState {
    /// Stop every task and free its buffers (cancel, shutdown, lease expiry).
    fn drop_all(&mut self) {
        for t in self.tasks.values() {
            if let Some(c) = &t.ctl {
                c.abort();
            }
        }
        self.tasks.clear();
    }
}

fn send_frame<E: Env>(env: &E, task: u32, t: &WTask, seq: u32) {
    let m = Msg::Data {
        task,
        attempt: t.attempt,
        seq,
        last: seq as usize + 1 == t.frames.len(),
        rows: t.frames[seq as usize].clone(),
    };
    env.send(COORD, m.encode());
}

/// Send what the window allows; retransmit the oldest unacknowledged frame when its timer has run out.
fn pump<E: Env>(env: &E, s: &mut WState, window: u32, now: Nanos) {
    let ids: Vec<u32> = s.tasks.keys().copied().collect();
    for id in ids {
        let Some(t) = s.tasks.get_mut(&id) else { continue };
        if !t.ready {
            continue;
        }
        let n = t.frames.len() as u32;
        if t.base < t.next_new {
            if let Some(sent_at) = t.sent[t.base as usize] {
                if now.saturating_sub(sent_at) > RTO {
                    send_frame(env, id, t, t.base);
                    t.sent[t.base as usize] = Some(now);
                    env.event("retransmit", id as u64, t.base as u64);
                }
            }
        }
        while t.next_new < n && t.next_new.saturating_sub(t.base) < window {
            let seq = t.next_new;
            send_frame(env, id, t, seq);
            t.sent[seq as usize] = Some(now);
            t.next_new += 1;
            if seq == 0 {
                env.fault_point("worker.first_frame_sent");
            }
            if seq + 1 == n {
                env.fault_point("worker.final_frame_sent");
            }
        }
        // the invariant of backpressure, checked by the sender itself
        if t.next_new.saturating_sub(t.base) > window {
            env.violation(format!(
                "sender window overrun: task {id} has {} frames in flight (window {window})",
                t.next_new.saturating_sub(t.base)
            ));
        }
    }
}

/// Read the task's input object (retrying transient store errors), run the fragment, cut the output into frames.
async fn read_and_compute<E: Env>(env: E, st: Rc<RefCell<WState>>, cfg: Config, task: u32, attempt: u32, file: u32) {
    let key = format!("input/{file}");
    let mut tries = 0u64;
    let data = loop {
        match env.store_get(&key).await {
            Ok(d) => break d,
            Err(_) => {
                tries += 1;
                env.event("store_retry", task as u64, tries);
                env.sleep(MS * (1 + tries.min(6)) + env.rand() % MS).await;
            }
        }
    };
    let ids = decode_i64s(&data);
    let mut rows = engine::process(&ids, cfg.batch_rows, 1);
    if attempt % 2 == 1 {
        rows.reverse(); // the output order of a task is not the same from one attempt to the next
    }
    let mut frames: Vec<Vec<i64>> = rows.chunks(cfg.batch_rows.max(1)).map(|c| c.to_vec()).collect();
    frames.push(Vec::new()); // end-of-stream frame
    let n = frames.len();
    {
        let mut s = st.borrow_mut();
        if let Some(t) = s.tasks.get_mut(&task) {
            if t.attempt == attempt {
                t.frames = frames;
                t.sent = vec![None; n];
                t.ready = true;
            }
        }
    }
    env.event("computed", task as u64, n as u64);
}

fn worker_msg<E: Env>(env: &E, st: &Rc<RefCell<WState>>, cfg: &Config, born: u64, m: Msg) {
    match m {
        Msg::Assign { task, attempt, file } => {
            let mut s = st.borrow_mut();
            if s.cancelled || s.shutdown {
                return;
            }
            if s.tasks.get(&task).is_some_and(|t| t.attempt >= attempt) {
                drop(s);
                env.event("assign_dup", task as u64, attempt as u64);
                return;
            }
            if let Some(old) = s.tasks.remove(&task) {
                if let Some(c) = &old.ctl {
                    c.abort();
                }
            }
            let buf = env.track("outbox");
            s.tasks.insert(
                task,
                WTask { attempt, ready: false, frames: Vec::new(), sent: Vec::new(), base: 0, next_new: 0, ctl: None, _buf: buf },
            );
            drop(s);
            env.event("assign_accepted", task as u64, attempt as u64);
            let ctl = env.spawn(read_and_compute(env.clone(), Rc::clone(st), cfg.clone(), task, attempt, file));
            if let Some(t) = st.borrow_mut().tasks.get_mut(&task) {
                if t.attempt == attempt {
                    t.ctl = Some(ctl);
                }
            }
        }
        Msg::Ack { task, attempt, next, stale } => {
            let mut s = st.borrow_mut();
            let (remove, what) = {
                let Some(t) = s.tasks.get_mut(&task) else { return };
                if t.attempt != attempt {
                    return;
                }
                if stale {
                    (true, "task_dropped_stale")
                } else {
                    t.base = t.base.max(next);
                    (t.ready && t.base as usize >= t.frames.len(), "task_acked")
                }
            };
            if remove {
                if let Some(old) = s.tasks.remove(&task) {
                    if let Some(c) = &old.ctl {
                        c.abort();
                    }
                }
                drop(s);
                env.event(what, task as u64, attempt as u64);
            }
        }
        Msg::Cancel => {
            {
                let mut s = st.borrow_mut();
                s.cancelled = true;
                s.drop_all();
            }
            env.gauge_set("cancel_seen", 1);
            env.event("cancel_seen", 0, 0);
            env.send(COORD, Msg::CancelAck { born }.encode());
        }
        Msg::Shutdown => st.borrow_mut().shutdown = true,
        Msg::HbAck => {}
        _ => env.event("unexpected_msg", 0, 0),
    }
}

pub async fn worker<E: Env>(env: E, cfg: Config) {
    let born = env.now() + 1;
    let st = Rc::new(RefCell::new(WState { tasks: BTreeMap::new(), cancelled: false, shutdown: false }));
    let mut next_hb = env.now();
    let mut last_coord = env.now();
    loop {
        let now = env.now();
        if st.borrow().shutdown {
            break;
        }
        if now.saturating_sub(last_coord) > LEASE {
            env.event("lease_expired", 0, 0);
            break;
        }
        if now >= next_hb {
            env.send(COORD, Msg::Heartbeat { born }.encode());
            next_hb = now + HB_PERIOD;
        }
        pump(&env, &mut st.borrow_mut(), cfg.window, now);
        if let Some((from, bytes)) = env.recv_timeout(TICK).await {
            if from != COORD {
                continue;
            }
            match Msg::decode(&bytes) {
                Ok(m) => {
                    last_coord = env.now();
                    worker_msg(&env, &st, &cfg, born, m);
                }
                Err(_) => env.event("bad_frame", from as u64, bytes.len() as u64),
            }
        }
    }
    st.borrow_mut().drop_all();
    env.send(COORD, Msg::Bye { born }.encode());
    env.event("worker_exit", 0, 0);
}
