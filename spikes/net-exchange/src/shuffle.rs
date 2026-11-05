//! An all-to-all shuffle between N nodes over the real exchange (`flow.rs`): every node generates rows, hash-partitions them by key, and
//! sends partition j to node j; every node sums what it receives. The expected result of every node is a pure function of the
//! parameters (`expected`), so a wrong, lost, duplicated or reordered row shows up as a different row count or checksum.
//! One node = one process in the network-namespace runs (`netx node`), one thread in the unit tests (`run_node`).

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::flow::{Config, NetError, Receiver, RecvStats, SendStats, Sender};

pub fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

pub const ROW_BYTES: usize = 16;

/// Row `idx` of node `src`: (key, value). Deterministic.
pub fn gen_row(seed: u64, src: u32, idx: u64) -> (u64, u64) {
    let key = mix(seed ^ ((src as u64) << 40) ^ idx);
    (key, mix(key ^ 0xC0FF_EE))
}

pub fn row_hash(key: u64, value: u64) -> u64 {
    mix(key ^ value.rotate_left(17))
}

pub fn partition(key: u64, nodes: u32) -> u32 {
    (key % nodes as u64) as u32
}

#[derive(Clone, Debug)]
pub struct Params {
    pub nodes: u32,
    pub rows_per_node: u64,
    /// Rows generated per round (split over the N destinations).
    pub batch_rows: usize,
    pub seed: u64,
    pub query: u64,
    pub cfg: Config,
    /// Pause after every batch a consumer takes: a slow consumer.
    pub consumer_delay: Duration,
    pub connect_timeout: Duration,
    /// (node, round): that node stops dead at that round without closing anything properly (a crash).
    pub die_at: Option<(u32, u64)>,
    /// (node, batches): that node's consumers cancel their streams after this many batches in total.
    pub cancel_at: Option<(u32, u64)>,
}

impl Params {
    pub fn new(nodes: u32, rows_per_node: u64) -> Params {
        Params {
            nodes,
            rows_per_node,
            batch_rows: 4096 * nodes as usize,
            seed: 42,
            query: 1,
            cfg: Config::default(),
            consumer_delay: Duration::ZERO,
            connect_timeout: Duration::from_secs(20),
            die_at: None,
            cancel_at: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NodeResult {
    pub me: u32,
    pub rows_in: u64,
    pub checksum: u64,
    pub bytes_sent: u64,
    pub bytes_recv: u64,
    /// Largest queue of any inbound stream, in bytes (must be <= credit_bytes).
    pub peak_buffered: u64,
    /// Largest in-flight of any outbound stream, in bytes (must be <= credit_bytes).
    pub peak_inflight: u64,
    pub blocked_waits: u64,
    pub secs: f64,
}

#[derive(Debug, Clone)]
pub enum NodeError {
    Net(NetError),
    /// The simulated crash of `Params::die_at`.
    Died,
    Setup(String),
}

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeError::Net(e) => write!(f, "{e}"),
            NodeError::Died => write!(f, "died (simulated crash)"),
            NodeError::Setup(s) => write!(f, "setup: {s}"),
        }
    }
}

/// What node `dst` must end up with: (rows, checksum). A pure function of the parameters.
pub fn expected(dst: u32, p: &Params) -> (u64, u64) {
    let (mut rows, mut sum) = (0u64, 0u64);
    for src in 0..p.nodes {
        for idx in 0..p.rows_per_node {
            let (k, v) = gen_row(p.seed, src, idx);
            if partition(k, p.nodes) == dst {
                rows += 1;
                sum = sum.wrapping_add(row_hash(k, v));
            }
        }
    }
    (rows, sum)
}

fn consume(payload: &[u8]) -> Result<(u64, u64), NetError> {
    if payload.len() % ROW_BYTES != 0 {
        return Err(NetError::Protocol(format!("batch of {} bytes is not a whole number of rows", payload.len())));
    }
    let (mut rows, mut sum) = (0u64, 0u64);
    for r in payload.chunks_exact(ROW_BYTES) {
        let k = u64::from_le_bytes(r[..8].try_into().unwrap());
        let v = u64::from_le_bytes(r[8..].try_into().unwrap());
        rows += 1;
        sum = sum.wrapping_add(row_hash(k, v));
    }
    Ok((rows, sum))
}

fn connect_retry(addr: SocketAddr, timeout: Duration) -> Result<TcpStream, NodeError> {
    let t0 = Instant::now();
    loop {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
            Ok(s) => return Ok(s),
            Err(e) => {
                if t0.elapsed() > timeout {
                    return Err(NodeError::Net(NetError::PeerLost(format!("could not connect to {addr}: {e}"))));
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

fn accept_deadline(l: &TcpListener, timeout: Duration) -> Result<TcpStream, NodeError> {
    l.set_nonblocking(true).map_err(|e| NodeError::Setup(e.to_string()))?;
    let t0 = Instant::now();
    loop {
        match l.accept() {
            Ok((s, _)) => {
                s.set_nonblocking(false).map_err(|e| NodeError::Setup(e.to_string()))?;
                return Ok(s);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if t0.elapsed() > timeout {
                    return Err(NodeError::Net(NetError::PeerLost("a peer never connected".into())));
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(e) => return Err(NodeError::Setup(e.to_string())),
        }
    }
}

struct Shared {
    aborted: AtomicBool,
    first_err: Mutex<Option<NetError>>,
    inbound: Mutex<Vec<Arc<Receiver>>>,
}

impl Shared {
    fn abort(&self, e: NetError) {
        {
            let mut g = self.first_err.lock().unwrap();
            if g.is_none() {
                *g = Some(e);
            }
        }
        if !self.aborted.swap(true, Ordering::SeqCst) {
            // tell every sender of this node to stop
            for rx in self.inbound.lock().unwrap().iter() {
                rx.cancel();
            }
        }
    }
}

/// Runs node `me` of the shuffle to the end. `listener` is already bound to `addrs[me]`.
pub fn run_node(me: u32, listener: TcpListener, addrs: &[SocketAddr], p: &Params) -> Result<NodeResult, NodeError> {
    let n = p.nodes;
    let t0 = Instant::now();
    let shared = Arc::new(Shared { aborted: AtomicBool::new(false), first_err: Mutex::new(None), inbound: Mutex::new(Vec::new()) });

    // 1. connect to every other node (the kernel queues the connection until that node accepts; a peer that is not listening yet is retried)
    let mut senders: Vec<Option<Sender>> = Vec::new();
    for dst in 0..n {
        if dst == me {
            senders.push(None);
            continue;
        }
        let sock = connect_retry(addrs[dst as usize], p.connect_timeout)?;
        senders.push(Some(Sender::start(sock, me, p.query, &p.cfg).map_err(NodeError::Net)?));
    }

    // 2. accept the N-1 inbound streams; one consumer thread per stream
    let received = Arc::new(AtomicU64Pair::default());
    let mut consumers = Vec::new();
    let cancel_after = p.cancel_at.and_then(|(node, k)| (node == me).then_some(k));
    for _ in 0..n - 1 {
        let sock = accept_deadline(&listener, p.connect_timeout)?;
        let rx = Arc::new(Receiver::start(sock, &p.cfg).map_err(NodeError::Net)?);
        shared.inbound.lock().unwrap().push(rx.clone());
        let (sh, delay, recv) = (shared.clone(), p.consumer_delay, received.clone());
        consumers.push(thread::spawn(move || -> Result<RecvStats, NetError> {
            let mut mine = (0u64, 0u64);
            loop {
                match rx.next() {
                    Ok(Some(b)) => {
                        let (r, s) = consume(&b).inspect_err(|e| sh.abort(e.clone()))?;
                        mine.0 += r;
                        mine.1 = mine.1.wrapping_add(s);
                        let total = recv.batches.fetch_add(1, Ordering::SeqCst) + 1;
                        if let Some(k) = cancel_after {
                            if total >= k {
                                sh.abort(NetError::Cancelled);
                                recv.add(mine);
                                return Err(NetError::Cancelled);
                            }
                        }
                        if !delay.is_zero() {
                            thread::sleep(delay);
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        sh.abort(e.clone());
                        return Err(e);
                    }
                }
            }
            recv.add(mine);
            Ok(rx.stats())
        }));
    }

    // 3. generate, partition, send
    let rounds = p.rows_per_node.div_ceil(p.batch_rows as u64);
    let (mut local_rows, mut local_sum) = (0u64, 0u64);
    let mut send_err: Option<NetError> = None;
    'rounds: for round in 0..rounds {
        if p.die_at == Some((me, round)) {
            // a crash: nothing is closed politely; every socket just goes away
            for rx in shared.inbound.lock().unwrap().iter() {
                rx.kill();
            }
            for s in senders.into_iter().flatten() {
                s.kill();
            }
            return Err(NodeError::Died);
        }
        if shared.aborted.load(Ordering::SeqCst) {
            break;
        }
        let lo = round * p.batch_rows as u64;
        let hi = (lo + p.batch_rows as u64).min(p.rows_per_node);
        let mut bufs: Vec<Vec<u8>> = (0..n).map(|_| Vec::with_capacity(p.batch_rows / n as usize * ROW_BYTES + 64)).collect();
        for idx in lo..hi {
            let (k, v) = gen_row(p.seed, me, idx);
            let d = partition(k, n);
            if d == me {
                local_rows += 1;
                local_sum = local_sum.wrapping_add(row_hash(k, v));
            } else {
                bufs[d as usize].extend_from_slice(&k.to_le_bytes());
                bufs[d as usize].extend_from_slice(&v.to_le_bytes());
            }
        }
        for dst in 0..n {
            if dst == me || bufs[dst as usize].is_empty() {
                continue;
            }
            if let Err(e) = senders[dst as usize].as_mut().unwrap().send_batch(&bufs[dst as usize]) {
                send_err = Some(e);
                break 'rounds;
            }
        }
    }
    if let Some(e) = &send_err {
        shared.abort(e.clone());
    }

    // 4. finish the outbound streams in two phases, then wait for the inbound ones. Phase 1 sends Eos on EVERY stream before phase 2 waits for
    // any receiver to close, so no node waits for a close that depends on an Eos this node has not sent yet.
    let mut send_stats = SendStats::default();
    let mut finish_err = None;
    let mut lingering: Vec<Sender> = Vec::new();
    for mut s in senders.into_iter().flatten() {
        let st = s.stats();
        send_stats.bytes += st.bytes;
        send_stats.blocked_waits += st.blocked_waits;
        send_stats.peak_inflight = send_stats.peak_inflight.max(st.peak_inflight);
        if send_err.is_none() && !shared.aborted.load(Ordering::SeqCst) {
            match s.finish_send() {
                Ok(_) => lingering.push(s),
                Err(e) => finish_err = Some(e),
            }
        } else {
            // the query is failing: do not pretend this stream ended well
            s.kill();
        }
    }
    if let Some(e) = &finish_err {
        shared.abort(e.clone());
    }
    for s in &lingering {
        if let Err(e) = s.wait_closed() {
            shared.abort(e);
        }
    }
    let mut recv_stats = RecvStats::default();
    for c in consumers {
        match c.join() {
            Ok(Ok(st)) => {
                recv_stats.bytes += st.bytes;
                recv_stats.peak_buffered = recv_stats.peak_buffered.max(st.peak_buffered);
            }
            Ok(Err(_)) => {}
            Err(_) => shared.abort(NetError::Protocol("a consumer thread panicked".into())),
        }
    }
    if let Some(e) = shared.first_err.lock().unwrap().clone() {
        return Err(NodeError::Net(e));
    }
    let (rows_in, sum) = received.get();
    Ok(NodeResult {
        me,
        rows_in: rows_in + local_rows,
        checksum: sum.wrapping_add(local_sum),
        bytes_sent: send_stats.bytes,
        bytes_recv: recv_stats.bytes,
        peak_buffered: recv_stats.peak_buffered,
        peak_inflight: send_stats.peak_inflight,
        blocked_waits: send_stats.blocked_waits,
        secs: t0.elapsed().as_secs_f64(),
    })
}

#[derive(Default)]
struct AtomicU64Pair {
    batches: std::sync::atomic::AtomicU64,
    acc: Mutex<(u64, u64)>,
}

impl AtomicU64Pair {
    fn add(&self, (r, s): (u64, u64)) {
        let mut g = self.acc.lock().unwrap();
        g.0 += r;
        g.1 = g.1.wrapping_add(s);
    }
    fn get(&self) -> (u64, u64) {
        *self.acc.lock().unwrap()
    }
}

/// The same shuffle between threads of ONE process with bounded in-memory channels (no sockets, no framing): the baseline that the network
/// exchange is compared with. Returns (seconds, total bytes moved between different nodes).
pub fn run_inproc(p: &Params) -> (f64, u64) {
    use std::sync::mpsc::sync_channel;
    let n = p.nodes as usize;
    let t0 = Instant::now();
    let avg = (p.batch_rows / n.max(1) * ROW_BYTES).max(1) as u64;
    let cap = (p.cfg.credit_bytes / avg).max(1) as usize;
    // channel[src][dst]
    let mut txs: Vec<Vec<Option<std::sync::mpsc::SyncSender<Vec<u8>>>>> = (0..n).map(|_| (0..n).map(|_| None).collect()).collect();
    let mut rxs: Vec<Vec<std::sync::mpsc::Receiver<Vec<u8>>>> = (0..n).map(|_| Vec::new()).collect();
    for src in 0..n {
        for dst in 0..n {
            if src != dst {
                let (t, r) = sync_channel::<Vec<u8>>(cap);
                txs[src][dst] = Some(t);
                rxs[dst].push(r);
            }
        }
    }
    let mut handles = Vec::new();
    for (dst, rs) in rxs.into_iter().enumerate() {
        for r in rs {
            let _ = dst;
            handles.push(thread::spawn(move || {
                let mut bytes = 0u64;
                let mut sum = 0u64;
                while let Ok(b) = r.recv() {
                    bytes += b.len() as u64;
                    sum = sum.wrapping_add(consume(&b).map(|x| x.1).unwrap_or(0));
                }
                (bytes, sum)
            }));
        }
    }
    let mut producers = Vec::new();
    for (me, row) in txs.into_iter().enumerate() {
        let p = p.clone();
        producers.push(thread::spawn(move || {
            let rounds = p.rows_per_node.div_ceil(p.batch_rows as u64);
            for round in 0..rounds {
                let lo = round * p.batch_rows as u64;
                let hi = (lo + p.batch_rows as u64).min(p.rows_per_node);
                let mut bufs: Vec<Vec<u8>> = (0..p.nodes).map(|_| Vec::new()).collect();
                for idx in lo..hi {
                    let (k, v) = gen_row(p.seed, me as u32, idx);
                    let d = partition(k, p.nodes) as usize;
                    if d != me {
                        bufs[d].extend_from_slice(&k.to_le_bytes());
                        bufs[d].extend_from_slice(&v.to_le_bytes());
                    }
                }
                for (d, b) in bufs.into_iter().enumerate() {
                    if let (Some(tx), false) = (&row[d], b.is_empty()) {
                        let _ = tx.send(b);
                    }
                }
            }
        }));
    }
    for h in producers {
        let _ = h.join();
    }
    let mut total = 0u64;
    for h in handles {
        if let Ok((b, _)) = h.join() {
            total += b;
        }
    }
    (t0.elapsed().as_secs_f64(), total)
}
