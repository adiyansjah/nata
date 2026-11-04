//! The SIMULATED runtime: virtual time, a seeded task order, a network with latency, jitter (so reordering),
//! drop, duplicate, corrupt, partition and node crash/restart, and an object store with latency and injected
//! failures (including "the write happened but the reply was lost"). Single-threaded and deterministic: the
//! seed decides everything, and nothing here reads the wall clock, the OS random source or a HashMap.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;

use crate::env::*;
use crate::exec::{Executor, Mailbox, RecvTimeout, Sleep};
use crate::frame::Msg;
use crate::rng::{Rng, mix};
use crate::trace::{Trace, fnv};

pub const COORD: NodeId = 0;
/// The pseudo node that injects client requests (cancel) straight into the coordinator's inbox.
pub const CLIENT: NodeId = 99;

#[derive(Clone, Debug)]
pub struct NetConfig {
    pub base: Nanos,
    pub jitter: Nanos,
    pub drop_ppm: u32,
    pub dup_ppm: u32,
    pub corrupt_ppm: u32,
}

#[derive(Clone, Debug)]
pub struct StoreConfig {
    pub lat_min: Nanos,
    pub lat_max: Nanos,
    pub fail_ppm: u32,
}

#[derive(Clone, Copy, Debug)]
pub enum Victim {
    /// The node that reached the fault point.
    Trigger,
    Node(NodeId),
}

/// "Crash a node when fault point `name` is reached for the `nth` time" (counted over all nodes).
#[derive(Clone, Debug)]
pub struct CrashPoint {
    pub name: &'static str,
    pub nth: u32,
    pub victim: Victim,
    pub restart_after: Option<Nanos>,
    seen: u32,
    fired: bool,
}

impl CrashPoint {
    pub fn new(name: &'static str, nth: u32, victim: Victim, restart_after: Option<Nanos>) -> Self {
        CrashPoint { name, nth, victim, restart_after, seen: 0, fired: false }
    }
}

#[derive(Clone, Debug)]
pub enum Ev {
    Crash(NodeId),
    Restart(NodeId),
    Partition(NodeId, NodeId),
    Heal(NodeId, NodeId),
    ClientCancel,
}

pub type LocalFut = Pin<Box<dyn Future<Output = ()>>>;
pub type Factory = Rc<dyn Fn(SimEnv) -> LocalFut>;

struct NodeSlot {
    alive: bool,
    inc: u32,
    mailbox: Arc<Mailbox>,
    main: Option<TaskCtl>,
    factory: Factory,
    rng: Rc<RefCell<Rng>>,
    gauges: BTreeMap<&'static str, i64>,
}

struct Inner {
    nodes: BTreeMap<NodeId, NodeSlot>,
    blocked: BTreeSet<(NodeId, NodeId)>,
    net: NetConfig,
    net_rng: Rng,
    store: BTreeMap<String, Vec<u8>>,
    store_cfg: StoreConfig,
    store_rng: Rng,
    crash_points: Vec<CrashPoint>,
    violations: Vec<String>,
}

pub struct World {
    pub exec: Executor,
    pub seed: u64,
    inner: RefCell<Inner>,
    trace: RefCell<Trace>,
    /// Live `Tracked` resources of all nodes. Must be zero at the end of a run.
    pub buffers: Arc<AtomicI64>,
}

fn pair(a: NodeId, b: NodeId) -> (NodeId, NodeId) {
    (a.min(b), a.max(b))
}

impl World {
    pub fn new(
        seed: u64,
        net: NetConfig,
        store_cfg: StoreConfig,
        crash_points: Vec<CrashPoint>,
        keep_log: bool,
    ) -> Rc<World> {
        let w = Rc::new(World {
            exec: Executor::new_sim(mix(seed, 1)),
            seed,
            inner: RefCell::new(Inner {
                nodes: BTreeMap::new(),
                blocked: BTreeSet::new(),
                net,
                net_rng: Rng::stream(seed, 2),
                store: BTreeMap::new(),
                store_cfg,
                store_rng: Rng::stream(seed, 3),
                crash_points,
                violations: Vec::new(),
            }),
            trace: RefCell::new(Trace::new(keep_log)),
            buffers: Arc::new(AtomicI64::new(0)),
        });
        let wk = Rc::downgrade(&w);
        w.exec.set_poll_hook(Box::new(move |now, id, node, runq| {
            if let Some(w) = wk.upgrade() {
                w.trace.borrow_mut().poll(now, id, node, runq);
            }
        }));
        w
    }

    pub fn ev(&self, node: NodeId, kind: &'static str, a: u64, b: u64) {
        let now = self.exec.now();
        self.trace.borrow_mut().ev(now, node, kind, a, b);
    }

    pub fn trace_hash(&self) -> u64 {
        self.trace.borrow().hash()
    }

    pub fn trace_events(&self) -> u64 {
        self.trace.borrow().events
    }

    pub fn trace_stats(&self) -> BTreeMap<&'static str, u64> {
        self.trace.borrow().stats.clone()
    }

    pub fn trace_log(&self) -> Vec<String> {
        self.trace.borrow().log.clone().unwrap_or_default()
    }

    pub fn put_object(&self, key: &str, val: Vec<u8>) {
        self.inner.borrow_mut().store.insert(key.to_string(), val);
    }

    pub fn peek_object(&self, key: &str) -> Option<Vec<u8>> {
        self.inner.borrow().store.get(key).cloned()
    }

    pub fn object_keys(&self) -> Vec<String> {
        self.inner.borrow().store.keys().cloned().collect()
    }

    pub fn gauge(&self, node: NodeId, name: &'static str) -> i64 {
        self.inner.borrow().nodes.get(&node).and_then(|n| n.gauges.get(name).copied()).unwrap_or(0)
    }

    pub fn node_up(&self, node: NodeId) -> bool {
        self.inner.borrow().nodes.get(&node).is_some_and(|n| n.alive)
    }

    /// The current incarnation's main task ran to its end.
    pub fn node_finished(&self, node: NodeId) -> bool {
        self.inner.borrow().nodes.get(&node).and_then(|n| n.main.as_ref().map(|c| c.finished())).unwrap_or(false)
    }

    pub fn violations(&self) -> Vec<String> {
        self.inner.borrow().violations.clone()
    }

    pub fn node_ids(&self) -> Vec<NodeId> {
        self.inner.borrow().nodes.keys().copied().collect()
    }

    /// Break the reference cycles at the end of a run (tasks hold the world, the world holds the executor).
    pub fn shutdown(&self) {
        self.exec.clear();
        let mut g = self.inner.borrow_mut();
        for n in g.nodes.values_mut() {
            n.main = None;
        }
    }

    pub fn add_node(self: &Rc<Self>, node: NodeId, factory: Factory) {
        {
            let mut i = self.inner.borrow_mut();
            i.nodes.insert(
                node,
                NodeSlot {
                    alive: false,
                    inc: 0,
                    mailbox: Mailbox::new(),
                    main: None,
                    factory,
                    rng: Rc::new(RefCell::new(Rng::new(0))),
                    gauges: BTreeMap::new(),
                },
            );
        }
        self.start_node(node);
    }

    fn start_node(self: &Rc<Self>, node: NodeId) {
        let (factory, inc) = {
            let mut i = self.inner.borrow_mut();
            let Some(n) = i.nodes.get_mut(&node) else { return };
            if n.alive {
                return;
            }
            n.alive = true;
            n.inc += 1;
            n.mailbox = Mailbox::new();
            n.gauges.clear();
            n.rng = Rc::new(RefCell::new(Rng::stream(self.seed, mix(node as u64, n.inc as u64))));
            (Rc::clone(&n.factory), n.inc)
        };
        self.ev(node, "start", inc as u64, 0);
        let env = SimEnv { w: Rc::clone(self), node, inc };
        let fut = factory(env);
        let ctl = self.exec.spawn(node, fut);
        if let Some(n) = self.inner.borrow_mut().nodes.get_mut(&node) {
            n.main = Some(ctl);
        }
    }

    fn crash_node(self: &Rc<Self>, node: NodeId) {
        {
            let mut i = self.inner.borrow_mut();
            let Some(n) = i.nodes.get_mut(&node) else { return };
            if !n.alive {
                return;
            }
            n.alive = false;
            n.mailbox.clear();
            n.gauges.clear();
        }
        self.ev(node, "crash", 0, 0);
        self.exec.kill_node(node);
    }

    pub fn schedule(self: &Rc<Self>, at: Nanos, ev: Ev) {
        let wk = Rc::downgrade(self);
        self.exec.at(at, move || {
            let Some(w) = wk.upgrade() else { return };
            match ev {
                Ev::Crash(n) => w.crash_node(n),
                Ev::Restart(n) => w.start_node(n),
                Ev::Partition(a, b) => {
                    w.inner.borrow_mut().blocked.insert(pair(a, b));
                    w.ev(a, "partition", b as u64, 0);
                }
                Ev::Heal(a, b) => {
                    w.inner.borrow_mut().blocked.remove(&pair(a, b));
                    w.ev(a, "heal", b as u64, 0);
                }
                Ev::ClientCancel => w.inject(COORD, Msg::ClientCancel.encode()),
            }
        });
    }

    /// A client request straight into a node's inbox (no network faults).
    pub fn inject(&self, to: NodeId, bytes: Vec<u8>) {
        let mb = self.inner.borrow().nodes.get(&to).filter(|n| n.alive).map(|n| Arc::clone(&n.mailbox));
        self.ev(to, "inject", bytes.len() as u64, fnv(&bytes));
        if let Some(mb) = mb {
            mb.deliver(CLIENT, bytes);
        }
    }

    fn mailbox_of(&self, node: NodeId) -> Arc<Mailbox> {
        Arc::clone(&self.inner.borrow().nodes.get(&node).expect("node exists").mailbox)
    }

    fn net_send(self: &Rc<Self>, from: NodeId, inc: u32, to: NodeId, bytes: Vec<u8>) {
        let now = self.exec.now();
        let len = bytes.len() as u64;
        let mut notes: Vec<(&'static str, u64, u64)> = Vec::new();
        let mut copies: Vec<(Nanos, Vec<u8>)> = Vec::new();
        {
            let mut g = self.inner.borrow_mut();
            let inner = &mut *g;
            match inner.nodes.get(&from) {
                Some(n) if n.alive && n.inc == inc => {}
                _ => return,
            }
            if inner.blocked.contains(&pair(from, to)) {
                notes.push(("net_blocked", to as u64, len));
            } else if inner.net_rng.chance_ppm(inner.net.drop_ppm) {
                notes.push(("net_drop", to as u64, len));
            } else {
                let n = if inner.net_rng.chance_ppm(inner.net.dup_ppm) { 2 } else { 1 };
                for k in 0..n {
                    let lat = inner.net.base + inner.net_rng.below(inner.net.jitter + 1);
                    let mut b = bytes.clone();
                    if !b.is_empty() && inner.net_rng.chance_ppm(inner.net.corrupt_ppm) {
                        let pos = inner.net_rng.below(b.len() as u64 * 8) as usize;
                        b[pos / 8] ^= 1 << (pos % 8);
                        notes.push(("net_corrupt", to as u64, pos as u64));
                    }
                    if k == 0 {
                        notes.push(("send", ((to as u64) << 32) | len, fnv(&bytes)));
                    } else {
                        notes.push(("net_dup", to as u64, lat));
                    }
                    copies.push((lat, b));
                }
            }
        }
        for (kind, a, b) in notes {
            self.ev(from, kind, a, b);
        }
        for (lat, b) in copies {
            let wk = Rc::downgrade(self);
            self.exec.at(now + lat, move || {
                if let Some(w) = wk.upgrade() {
                    w.net_deliver(from, to, b);
                }
            });
        }
    }

    fn net_deliver(&self, from: NodeId, to: NodeId, bytes: Vec<u8>) {
        let target = {
            let g = self.inner.borrow();
            let blocked = g.blocked.contains(&pair(from, to));
            g.nodes.get(&to).filter(|n| n.alive).map(|n| (Arc::clone(&n.mailbox), blocked))
        };
        match target {
            None => self.ev(to, "net_dead", from as u64, bytes.len() as u64),
            Some((_, true)) => self.ev(to, "net_cut", from as u64, bytes.len() as u64),
            Some((mb, false)) => {
                self.ev(to, "deliver", from as u64, fnv(&bytes));
                mb.deliver(from, bytes);
            }
        }
    }

    fn fault_point(self: &Rc<Self>, node: NodeId, inc: u32, name: &'static str) {
        let mut victim: Option<(NodeId, Option<Nanos>)> = None;
        {
            let mut g = self.inner.borrow_mut();
            match g.nodes.get(&node) {
                Some(n) if n.alive && n.inc == inc => {}
                _ => return,
            }
            for cp in g.crash_points.iter_mut() {
                if cp.fired || cp.name != name {
                    continue;
                }
                cp.seen += 1;
                if cp.seen == cp.nth {
                    cp.fired = true;
                    let v = match cp.victim {
                        Victim::Trigger => node,
                        Victim::Node(n) => n,
                    };
                    victim = Some((v, cp.restart_after));
                    break;
                }
            }
        }
        if let Some((v, restart)) = victim {
            self.ev(node, "crash_point", fnv(name.as_bytes()), v as u64);
            self.crash_node(v);
            if let Some(d) = restart {
                self.schedule(self.exec.now() + d, Ev::Restart(v));
            }
        }
    }

    fn store_draw(&self) -> (Nanos, bool, bool) {
        let mut g = self.inner.borrow_mut();
        let inner = &mut *g;
        let lat = inner.store_cfg.lat_min + inner.store_rng.below(inner.store_cfg.lat_max - inner.store_cfg.lat_min + 1);
        let fail = inner.store_rng.chance_ppm(inner.store_cfg.fail_ppm);
        let applied = inner.store_rng.next() & 1 == 1;
        (lat, fail, applied)
    }

    async fn store_get(self: Rc<Self>, node: NodeId, key: String) -> Result<Vec<u8>, StoreError> {
        let (lat, fail, _) = self.store_draw();
        Sleep::new(self.exec.clone(), lat).await;
        self.ev(node, "store_get", fnv(key.as_bytes()), fail as u64);
        if fail {
            return Err(StoreError::Unavailable);
        }
        self.inner.borrow().store.get(&key).cloned().ok_or(StoreError::NotFound)
    }

    async fn store_put(self: Rc<Self>, node: NodeId, key: String, val: Vec<u8>) -> Result<(), StoreError> {
        let (lat, fail, applied) = self.store_draw();
        Sleep::new(self.exec.clone(), lat).await;
        self.ev(node, "store_put", fnv(key.as_bytes()), (fail as u64) | ((applied as u64) << 1));
        if fail && !applied {
            return Err(StoreError::Unavailable);
        }
        self.inner.borrow_mut().store.insert(key, val);
        if fail { Err(StoreError::Unavailable) } else { Ok(()) }
    }
}

#[derive(Clone)]
pub struct SimEnv {
    w: Rc<World>,
    node: NodeId,
    inc: u32,
}

impl SimEnv {
    fn up(&self) -> bool {
        self.w.inner.borrow().nodes.get(&self.node).is_some_and(|n| n.alive && n.inc == self.inc)
    }
}

impl Env for SimEnv {
    fn me(&self) -> NodeId {
        self.node
    }

    fn now(&self) -> Nanos {
        self.w.exec.now()
    }

    fn rand(&self) -> u64 {
        let rng = {
            let g = self.w.inner.borrow();
            Rc::clone(&g.nodes.get(&self.node).expect("node exists").rng)
        };
        let v = rng.borrow_mut().next();
        v
    }

    fn sleep(&self, d: Nanos) -> impl Future<Output = ()> {
        Sleep::new(self.w.exec.clone(), d)
    }

    fn send(&self, to: NodeId, bytes: Vec<u8>) {
        self.w.net_send(self.node, self.inc, to, bytes);
    }

    fn recv_timeout(&self, d: Nanos) -> impl Future<Output = Option<(NodeId, Vec<u8>)>> {
        RecvTimeout::new(self.w.exec.clone(), self.w.mailbox_of(self.node), d)
    }

    fn spawn(&self, f: impl Future<Output = ()> + 'static) -> TaskCtl {
        if !self.up() {
            return TaskCtl::dead();
        }
        self.w.exec.spawn(self.node, f)
    }

    fn store_get(&self, key: &str) -> impl Future<Output = Result<Vec<u8>, StoreError>> {
        Rc::clone(&self.w).store_get(self.node, key.to_string())
    }

    fn store_put(&self, key: &str, val: Vec<u8>) -> impl Future<Output = Result<(), StoreError>> {
        Rc::clone(&self.w).store_put(self.node, key.to_string(), val)
    }

    fn fault_point(&self, name: &'static str) {
        self.w.fault_point(self.node, self.inc, name);
    }

    fn track(&self, _what: &'static str) -> Tracked {
        Tracked::new(&self.w.buffers)
    }

    fn event(&self, kind: &'static str, a: u64, b: u64) {
        if self.up() {
            self.w.ev(self.node, kind, a, b);
        }
    }

    fn gauge_set(&self, name: &'static str, v: i64) {
        if !self.up() {
            return;
        }
        self.w.ev(self.node, "gauge", fnv(name.as_bytes()), v as u64);
        {
            let mut g = self.w.inner.borrow_mut();
            if let Some(n) = g.nodes.get_mut(&self.node) {
                n.gauges.insert(name, v);
            }
        }
        // The oracle for "cancellation reaches every node": at the instant the coordinator declares the
        // cancellation complete, every worker that is up must have seen it.
        if name == "cancel_done" && v == 1 {
            let missing: Vec<NodeId> = {
                let g = self.w.inner.borrow();
                g.nodes
                    .iter()
                    .filter(|(id, n)| {
                        **id != COORD
                            && n.alive
                            && !n.main.as_ref().is_some_and(|c| c.finished()) // a node that already exited has nothing to cancel
                            && n.gauges.get("cancel_seen").copied().unwrap_or(0) != 1
                    })
                    .map(|(id, _)| *id)
                    .collect()
            };
            for id in missing {
                self.violation(format!("cancellation did not reach node {id}"));
            }
        }
    }

    fn violation(&self, msg: String) {
        let mut g = self.w.inner.borrow_mut();
        if g.violations.len() < 16 {
            g.violations.push(msg.clone());
        }
        drop(g);
        self.w.ev(self.node, "violation", fnv(msg.as_bytes()), 0);
    }
}
