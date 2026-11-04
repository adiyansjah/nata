//! The REAL runtime: the same protocol code (`protocol.rs`) on wall-clock time, one OS thread per node, an
//! in-process network (cross-thread mailboxes) and a real object store (files in a directory, written
//! atomically with rename). Used to show that the `Env` abstraction has a second, real implementation; it
//! injects no network faults, but a node can be killed (its futures dropped) and restarted.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::env::*;
use crate::exec::{Executor, Mailbox, RecvTimeout, Sleep};
use crate::frame::encode_i64s;
use crate::protocol::{self, COORD, Config, MS};
use crate::rng::{Rng, mix};
use crate::scenario::{Failure, verify_result};

pub struct FsStore {
    dir: PathBuf,
}

impl FsStore {
    pub fn new(dir: PathBuf) -> std::io::Result<FsStore> {
        std::fs::create_dir_all(&dir)?;
        Ok(FsStore { dir })
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(key.replace('/', "__"))
    }

    pub fn get(&self, key: &str) -> Result<Vec<u8>, StoreError> {
        match std::fs::read(self.path(key)) {
            Ok(v) => Ok(v),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(StoreError::NotFound),
            Err(_) => Err(StoreError::Unavailable),
        }
    }

    pub fn put(&self, key: &str, val: &[u8]) -> Result<(), StoreError> {
        let tmp = self.dir.join(format!(".tmp-{}", key.replace('/', "__")));
        std::fs::write(&tmp, val).map_err(|_| StoreError::Unavailable)?;
        std::fs::rename(&tmp, self.path(key)).map_err(|_| StoreError::Unavailable)
    }
}

pub struct RealWorld {
    mailboxes: Vec<Arc<Mailbox>>,
    up: Vec<AtomicBool>,
    kill: Vec<AtomicBool>,
    restart: Vec<AtomicBool>,
    stop: AtomicBool,
    store: FsStore,
    buffers: Arc<AtomicI64>,
    violations: Mutex<Vec<String>>,
    gauges: Mutex<BTreeMap<(NodeId, &'static str), i64>>,
    start: Instant,
    seed: u64,
}

impl RealWorld {
    fn gauge(&self, node: NodeId, name: &'static str) -> i64 {
        self.gauges.lock().unwrap_or_else(|e| e.into_inner()).get(&(node, name)).copied().unwrap_or(0)
    }
}

#[derive(Clone)]
pub struct RealEnv {
    w: Arc<RealWorld>,
    node: NodeId,
    exec: Executor,
    rng: Rc<RefCell<Rng>>,
}

impl Env for RealEnv {
    fn me(&self) -> NodeId {
        self.node
    }

    fn now(&self) -> Nanos {
        self.exec.now()
    }

    fn rand(&self) -> u64 {
        self.rng.borrow_mut().next()
    }

    fn sleep(&self, d: Nanos) -> impl Future<Output = ()> {
        Sleep::new(self.exec.clone(), d)
    }

    fn send(&self, to: NodeId, bytes: Vec<u8>) {
        let (me, to) = (self.node as usize, to as usize);
        if to < self.w.mailboxes.len() && self.w.up[me].load(SeqCst) && self.w.up[to].load(SeqCst) {
            self.w.mailboxes[to].deliver(self.node, bytes);
        }
    }

    fn recv_timeout(&self, d: Nanos) -> impl Future<Output = Option<(NodeId, Vec<u8>)>> {
        RecvTimeout::new(self.exec.clone(), Arc::clone(&self.w.mailboxes[self.node as usize]), d)
    }

    fn spawn(&self, f: impl Future<Output = ()> + 'static) -> TaskCtl {
        self.exec.spawn(self.node, f)
    }

    // The file system calls block the node's thread inside the poll. That is acceptable in this runtime
    // (one thread per node, nothing else to run meanwhile); a production runtime would use async I/O.
    fn store_get(&self, key: &str) -> impl Future<Output = Result<Vec<u8>, StoreError>> {
        let r = self.w.store.get(key);
        async move { r }
    }

    fn store_put(&self, key: &str, val: Vec<u8>) -> impl Future<Output = Result<(), StoreError>> {
        let r = self.w.store.put(key, &val);
        async move { r }
    }

    fn fault_point(&self, _name: &'static str) {}

    fn track(&self, _what: &'static str) -> Tracked {
        Tracked::new(&self.w.buffers)
    }

    fn event(&self, _kind: &'static str, _a: u64, _b: u64) {}

    fn gauge_set(&self, name: &'static str, v: i64) {
        self.w.gauges.lock().unwrap_or_else(|e| e.into_inner()).insert((self.node, name), v);
    }

    fn violation(&self, msg: String) {
        self.w.violations.lock().unwrap_or_else(|e| e.into_inner()).push(msg);
    }
}

fn node_thread(w: Arc<RealWorld>, node: NodeId, cfg: Config, deadline: Nanos) {
    let mut incarnation = 0u64;
    loop {
        incarnation += 1;
        let exec = Executor::new_real_at(w.start);
        let rng = Rc::new(RefCell::new(Rng::stream(w.seed, mix(node as u64, incarnation))));
        let env = RealEnv { w: Arc::clone(&w), node, exec: exec.clone(), rng };
        w.up[node as usize].store(true, SeqCst);
        let main = if node == COORD {
            exec.spawn(node, protocol::coordinator(env.clone(), cfg.clone()))
        } else {
            exec.spawn(node, protocol::worker(env.clone(), cfg.clone()))
        };
        let w2 = Arc::clone(&w);
        exec.run(deadline, u64::MAX, move || main.finished() || w2.kill[node as usize].load(SeqCst));
        if !w.kill[node as usize].load(SeqCst) {
            break; // the node's main task ended (or the deadline passed)
        }
        // Killed: the process "dies" (all its futures are dropped) and may be restarted later.
        w.up[node as usize].store(false, SeqCst);
        w.mailboxes[node as usize].clear();
        w.gauges.lock().unwrap_or_else(|e| e.into_inner()).retain(|(n, _), _| *n != node);
        exec.clear();
        drop(env);
        loop {
            if w.stop.load(SeqCst) {
                return;
            }
            if w.restart[node as usize].swap(false, SeqCst) {
                w.kill[node as usize].store(false, SeqCst);
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

pub struct RealPlan {
    /// (node, kill after this many ms, restart after this many further ms)
    pub kill: Option<(NodeId, u64, Option<u64>)>,
}

pub struct RealOutcome {
    pub fail: Option<Failure>,
    pub status: &'static str,
    pub elapsed_ms: u64,
}

/// Run the toy protocol on real threads, real time and a real directory; verify the committed result.
pub fn run_real(seed: u64, cfg: &Config, files: &[Vec<i64>], plan: &RealPlan, deadline_ms: u64) -> RealOutcome {
    let n = cfg.workers as usize + 1;
    let dir = std::env::temp_dir().join(format!("nata-sim-real-{}-{seed}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let store = FsStore::new(dir.clone()).expect("create store dir");
    for (f, ids) in files.iter().enumerate() {
        store.put(&format!("input/{f}"), &encode_i64s(ids)).expect("write input");
    }
    let start = Instant::now();
    let w = Arc::new(RealWorld {
        mailboxes: (0..n).map(|_| Mailbox::new()).collect(),
        up: (0..n).map(|_| AtomicBool::new(false)).collect(),
        kill: (0..n).map(|_| AtomicBool::new(false)).collect(),
        restart: (0..n).map(|_| AtomicBool::new(false)).collect(),
        stop: AtomicBool::new(false),
        store,
        buffers: Arc::new(AtomicI64::new(0)),
        violations: Mutex::new(Vec::new()),
        gauges: Mutex::new(BTreeMap::new()),
        start,
        seed,
    });
    let deadline = deadline_ms * MS;
    let mut handles = Vec::new();
    for node in 0..n as NodeId {
        let (w, cfg) = (Arc::clone(&w), cfg.clone());
        handles.push(std::thread::spawn(move || node_thread(w, node, cfg, deadline)));
    }
    let killer = plan.kill.map(|(node, at_ms, restart_ms)| {
        let w = Arc::clone(&w);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(at_ms));
            w.kill[node as usize].store(true, SeqCst);
            if let Some(r) = restart_ms {
                std::thread::sleep(Duration::from_millis(r));
                w.restart[node as usize].store(true, SeqCst);
            }
        })
    });
    let mut handles = handles.into_iter();
    let coordinator = handles.next().expect("coordinator thread");
    let _ = coordinator.join();
    w.stop.store(true, SeqCst);
    for h in handles {
        let _ = h.join();
    }
    if let Some(k) = killer {
        let _ = k.join();
    }
    let elapsed_ms = start.elapsed().as_millis() as u64;
    let outcome = w.gauge(COORD, "outcome");
    let status = match outcome {
        1 => "completed",
        2 => "cancelled",
        _ => "none",
    };
    let mut expected: Vec<i64> = files.iter().flat_map(|f| crate::engine::reference(f)).collect();
    expected.sort();
    let violations = w.violations.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut fail = if let Some(v) = violations.first() {
        Some(Failure { kind: "violation", msg: v.clone() })
    } else {
        verify_result(outcome, w.store.get("result/rows").ok(), w.store.get("result/manifest").ok(), &expected)
    };
    let live = w.buffers.load(SeqCst);
    if fail.is_none() && live != 0 {
        fail = Some(Failure { kind: "leak", msg: format!("{live} buffer(s) still allocated after every node ended") });
    }
    let _ = std::fs::remove_dir_all(&dir);
    RealOutcome { fail, status, elapsed_ms }
}
