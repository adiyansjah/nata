//! One exchange stream over TCP: credit-based flow control, cancellation, ordering, and liveness.
//!
//! Sender (a worker producing batches) and receiver (the consumer) are connected by one TCP connection.
//! * Flow control: the receiver grants `credit_bytes` at the start and gives back a batch's bytes when the CONSUMER takes the batch off
//!   the queue. The sender sends only while it has credit, so at most `credit_bytes` are ever in flight or buffered, however slow the
//!   consumer is. TCP's own window is not relied on (it bounds bytes in the kernel, not in the receiver's queue).
//! * Ordering: Data frames carry 0, 1, 2, ... ; a gap, a repeat or a reorder is an error, not a silently different result.
//! * Completeness: the stream ends with Eos{count}; a connection that closes without it is a lost peer, never a finished stream.
//! * Cancellation: the receiver sends Cancel; the sender's next (or current, blocked) `send_batch` returns `Cancelled`.
//! * Completion is a handshake, not a one-sided close: after Eos the sender half-closes and KEEPS the connection until the receiver's
//!   consumer has taken the whole stream and closed its side. A process that exits right after writing its last frame would close a
//!   socket that still holds unread credit frames; Linux then sends RST and drops the data it has not transmitted yet, the Eos included
//!   (found by the network-namespace runs, 10 of 25 short runs failed on a 10 ms path; loopback almost never shows it).
//! * Liveness: every received byte counts as a sign of life; a side that has heard nothing for `dead_after` shuts the socket and reports
//!   the peer lost. Both sides ping when they have sent nothing for `ping_every`. Writes time out after `dead_after` too.

use std::collections::VecDeque;
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::wire::{Frame, WireError, read_frame, write_frame};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetError {
    Cancelled,
    PeerLost(String),
    Protocol(String),
    TooBig { len: usize, credit: u64 },
    Io(String),
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetError::Cancelled => write!(f, "cancelled"),
            NetError::PeerLost(s) => write!(f, "peer lost: {s}"),
            NetError::Protocol(s) => write!(f, "protocol error: {s}"),
            NetError::TooBig { len, credit } => write!(f, "batch of {len} bytes can never fit the credit of {credit} bytes"),
            NetError::Io(s) => write!(f, "io error: {s}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub credit_bytes: u64,
    pub dead_after: Duration,
    pub ping_every: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Config { credit_bytes: 4 << 20, dead_after: Duration::from_secs(5), ping_every: Duration::from_millis(500) }
    }
}

fn wire_to_net(e: WireError) -> NetError {
    match e {
        WireError::Io(e) => NetError::PeerLost(format!("io: {e}")),
        WireError::Truncated => NetError::PeerLost("connection closed in the middle of a frame".into()),
        other => NetError::Protocol(other.to_string()),
    }
}

/// Time of the last byte received and the last frame sent, as nanoseconds since `base`.
struct Activity {
    base: Instant,
    last_rx: AtomicU64,
    last_tx: AtomicU64,
}

impl Activity {
    fn new() -> Arc<Activity> {
        Arc::new(Activity { base: Instant::now(), last_rx: AtomicU64::new(0), last_tx: AtomicU64::new(0) })
    }
    fn now(&self) -> u64 {
        self.base.elapsed().as_nanos() as u64
    }
    fn rx(&self) {
        self.last_rx.store(self.now(), Ordering::Relaxed);
    }
    fn tx(&self) {
        self.last_tx.store(self.now(), Ordering::Relaxed);
    }
    fn idle_rx(&self) -> Duration {
        Duration::from_nanos(self.now().saturating_sub(self.last_rx.load(Ordering::Relaxed)))
    }
    fn idle_tx(&self) -> Duration {
        Duration::from_nanos(self.now().saturating_sub(self.last_tx.load(Ordering::Relaxed)))
    }
}

/// A reader that records every successful read as a sign of life (a large frame arriving slowly is alive).
struct ActiveRead {
    inner: TcpStream,
    act: Arc<Activity>,
}

impl std::io::Read for ActiveRead {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.act.rx();
        }
        Ok(n)
    }
}

type Writer = Arc<Mutex<TcpStream>>;

fn send_frame(w: &Writer, act: &Activity, f: &Frame) -> Result<(), NetError> {
    let mut g = w.lock().unwrap();
    write_frame(&mut *g, f).map_err(|e| NetError::PeerLost(format!("write failed: {e}")))?;
    act.tx();
    Ok(())
}

/// Watches one connection: pings when idle, and shuts the socket down when nothing was heard for `dead_after`.
fn spawn_watchdog(sock: TcpStream, w: Writer, act: Arc<Activity>, cfg: Config, done: Arc<AtomicBool>, on_lost: impl Fn(String) + Send + 'static) {
    thread::spawn(move || {
        let mut ping_n = 0u64;
        while !done.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(20));
            if done.load(Ordering::Relaxed) {
                break;
            }
            #[cfg(not(feature = "bug-no-liveness"))]
            if act.idle_rx() > cfg.dead_after {
                on_lost(format!("silent for {} ms", act.idle_rx().as_millis()));
                let _ = sock.shutdown(Shutdown::Both);
                break;
            }
            if act.idle_tx() > cfg.ping_every {
                // try_lock: a writer blocked on a full send buffer must not block the watchdog (it would never get to shut the socket down)
                if let Ok(mut g) = w.try_lock() {
                    ping_n += 1;
                    if write_frame(&mut *g, &Frame::Ping { n: ping_n }).is_ok() {
                        act.tx();
                    }
                }
            }
        }
    });
}

// ------------------------------------------------------------------------------------------------------------------ sender

#[derive(Default)]
struct SendState {
    credit: u64,
    sent: u64,
    /// Credit given back by the consumer (the receiver's initial grant is not "given back": it is the window).
    returned: u64,
    grants: u64,
    peak_inflight: u64,
    blocked_waits: u64,
    cancelled: bool,
    lost: Option<String>,
    /// The reader thread has ended: the receiver closed the connection (or it broke).
    closed: bool,
}

struct SendShared {
    st: Mutex<SendState>,
    cv: Condvar,
}

#[derive(Debug, Clone, Default)]
pub struct SendStats {
    pub batches: u64,
    pub bytes: u64,
    /// Largest number of bytes sent and not yet given back as credit.
    pub peak_inflight: u64,
    /// How many `send_batch` calls had to wait for credit at least once.
    pub blocked_waits: u64,
}

pub struct Sender {
    sh: Arc<SendShared>,
    sock: TcpStream,
    w: Writer,
    act: Arc<Activity>,
    done: Arc<AtomicBool>,
    cfg: Config,
    next_seq: u64,
    batches: u64,
}

impl Sender {
    /// Connects, sends Hello, and starts the threads that read credit and cancellation. The caller retries `TcpStream::connect` itself if
    /// the peer may not be listening yet; here the stream is already connected.
    pub fn start(sock: TcpStream, src: u32, query: u64, cfg: &Config) -> Result<Sender, NetError> {
        let _ = sock.set_nodelay(true);
        let _ = sock.set_write_timeout(Some(cfg.dead_after));
        let io = |e: std::io::Error| NetError::Io(e.to_string());
        let act = Activity::new();
        act.rx();
        let w: Writer = Arc::new(Mutex::new(sock.try_clone().map_err(io)?));
        let sh = Arc::new(SendShared { st: Mutex::new(SendState::default()), cv: Condvar::new() });
        let done = Arc::new(AtomicBool::new(false));
        let closed = Arc::new(AtomicBool::new(false));
        send_frame(&w, &act, &Frame::Hello { src, query })?;
        // reader: Credit, Cancel, Ping/Pong
        {
            let (sh, w, act, done, closed) = (sh.clone(), w.clone(), act.clone(), done.clone(), closed.clone());
            let rd = sock.try_clone().map_err(io)?;
            thread::spawn(move || {
                let mut rd = ActiveRead { inner: rd, act: act.clone() };
                let lost = |why: String| {
                    let mut st = sh.st.lock().unwrap();
                    if st.lost.is_none() && !st.cancelled {
                        st.lost = Some(why);
                    }
                    sh.cv.notify_all();
                };
                loop {
                    match read_frame(&mut rd) {
                        Ok(Some(Frame::Credit { bytes })) => {
                            let mut st = sh.st.lock().unwrap();
                            st.credit = st.credit.saturating_add(bytes);
                            st.grants += 1;
                            if st.grants > 1 {
                                st.returned = st.returned.saturating_add(bytes);
                            }
                            sh.cv.notify_all();
                        }
                        Ok(Some(Frame::Cancel)) => {
                            #[cfg(not(feature = "bug-no-cancel"))]
                            {
                                let mut st = sh.st.lock().unwrap();
                                st.cancelled = true;
                                sh.cv.notify_all();
                            }
                        }
                        Ok(Some(Frame::Ping { n })) => {
                            let _ = send_frame(&w, &act, &Frame::Pong { n });
                        }
                        Ok(Some(Frame::Pong { .. })) => {}
                        Ok(Some(other)) => {
                            lost(format!("unexpected frame from the receiver: {other:?}"));
                            break;
                        }
                        Ok(None) => {
                            if !done.load(Ordering::Relaxed) {
                                lost("receiver closed the connection".into());
                            }
                            break;
                        }
                        Err(e) => {
                            if !done.load(Ordering::Relaxed) {
                                lost(wire_to_net(e).to_string());
                            }
                            break;
                        }
                    }
                }
                closed.store(true, Ordering::Relaxed);
                let mut st = sh.st.lock().unwrap();
                st.closed = true;
                sh.cv.notify_all();
            });
        }
        {
            let sh2 = sh.clone();
            // the watchdog runs until the connection is closed, also after Eos: a receiver that is stuck must not make `wait_closed` hang
            spawn_watchdog(sock.try_clone().map_err(io)?, w.clone(), act.clone(), cfg.clone(), closed.clone(), move |why| {
                let mut st = sh2.st.lock().unwrap();
                if st.lost.is_none() {
                    st.lost = Some(why);
                }
                sh2.cv.notify_all();
            });
        }
        Ok(Sender { sh, sock, w, act, done, cfg: cfg.clone(), next_seq: 0, batches: 0 })
    }

    /// Sends one batch, waiting for credit if the receiver's consumer is slow. Errors: `Cancelled` (the consumer cancelled), `PeerLost`.
    pub fn send_batch(&mut self, payload: &[u8]) -> Result<(), NetError> {
        let len = payload.len() as u64;
        if len > self.cfg.credit_bytes {
            return Err(NetError::TooBig { len: payload.len(), credit: self.cfg.credit_bytes });
        }
        {
            let mut st = self.sh.st.lock().unwrap();
            let mut waited = false;
            loop {
                if st.cancelled {
                    return Err(NetError::Cancelled);
                }
                if let Some(l) = &st.lost {
                    return Err(NetError::PeerLost(l.clone()));
                }
                #[cfg(feature = "bug-no-credit")]
                break;
                #[cfg(not(feature = "bug-no-credit"))]
                if st.credit >= len {
                    break;
                }
                if !waited {
                    waited = true;
                    st.blocked_waits += 1;
                }
                st = self.sh.cv.wait(st).unwrap();
            }
            st.credit = st.credit.saturating_sub(len);
            st.sent += len;
            let inflight = st.sent - st.returned.min(st.sent);
            st.peak_inflight = st.peak_inflight.max(inflight);
        }
        let seq = self.next_seq;
        let mut g = self.w.lock().unwrap();
        write_frame(&mut *g, &Frame::Data { seq, payload: payload.to_vec() }).map_err(|e| {
            let why = format!("write failed: {e}");
            let mut st = self.sh.st.lock().unwrap();
            st.lost.get_or_insert(why.clone());
            NetError::PeerLost(why)
        })?;
        self.act.tx();
        self.next_seq += 1;
        self.batches += 1;
        Ok(())
    }

    /// Closes the connection at once, with no Eos and no other frame: what a crashed process looks like to the receiver.
    pub fn kill(self) {
        self.done.store(true, Ordering::Relaxed);
        let _ = self.sock.shutdown(Shutdown::Both);
    }

    pub fn stats(&self) -> SendStats {
        let st = self.sh.st.lock().unwrap();
        SendStats { batches: self.batches, bytes: st.sent, peak_inflight: st.peak_inflight, blocked_waits: st.blocked_waits }
    }

    /// Phase 1 of the end: Eos with the frame count, then the write side is shut down (the FIN arrives after the data). The connection
    /// stays open for reading: the receiver may still be returning credit. Call `wait_closed` before the process may exit.
    pub fn finish_send(&mut self) -> Result<SendStats, NetError> {
        {
            let st = self.sh.st.lock().unwrap();
            if st.cancelled {
                return Err(NetError::Cancelled);
            }
            if let Some(l) = &st.lost {
                return Err(NetError::PeerLost(l.clone()));
            }
        }
        send_frame(&self.w, &self.act, &Frame::Eos { count: self.next_seq })?;
        let stats = self.stats();
        let _ = self.w.lock().unwrap().shutdown(Shutdown::Write);
        self.done.store(true, Ordering::Relaxed); // errors from now on are not "lost": the stream is complete from the sender's side
        Ok(stats)
    }

    /// Phase 2: waits until the receiver has taken the whole stream and closed its side (or the connection broke). Waiting is bounded by the
    /// liveness timer: a receiver that stops answering is shut down by the watchdog after `dead_after`. Returns Ok also if the connection broke
    /// after Eos was sent: the sender has nothing left to deliver, and a receiver that died is the receiver's query's problem.
    pub fn wait_closed(&self) -> Result<(), NetError> {
        #[cfg(feature = "bug-no-linger")]
        return Ok(());
        #[allow(unreachable_code)]
        let mut st = self.sh.st.lock().unwrap();
        while !st.closed {
            st = self.sh.cv.wait(st).unwrap();
        }
        Ok(())
    }

    /// `finish_send` then `wait_closed`.
    pub fn finish(mut self) -> Result<SendStats, NetError> {
        let stats = self.finish_send()?;
        self.wait_closed()?;
        Ok(stats)
    }
}

impl Drop for Sender {
    /// Dropping without `finish` is how a crash looks to the peer: the connection closes with no Eos.
    fn drop(&mut self) {
        self.done.store(true, Ordering::Relaxed);
    }
}

// ----------------------------------------------------------------------------------------------------------------- receiver

#[derive(Default)]
struct RecvState {
    q: VecDeque<Vec<u8>>,
    buffered: u64,
    peak_buffered: u64,
    received: u64,
    returned: u64,
    expected_seq: u64,
    eos: bool,
    err: Option<NetError>,
    cancelled: bool,
    batches: u64,
}

struct RecvShared {
    st: Mutex<RecvState>,
    cv: Condvar,
}

#[derive(Debug, Clone, Default)]
pub struct RecvStats {
    pub batches: u64,
    pub bytes: u64,
    /// Largest number of bytes sitting in the queue waiting for the consumer. Must never exceed `credit_bytes`.
    pub peak_buffered: u64,
}

pub struct Receiver {
    sh: Arc<RecvShared>,
    w: Writer,
    act: Arc<Activity>,
    done: Arc<AtomicBool>,
    sock: TcpStream,
    pub src: u32,
    pub query: u64,
}

impl Receiver {
    /// Accepts ONE connection on `listener`, reads its Hello, grants the initial credit, and starts reading.
    pub fn accept(listener: &TcpListener, cfg: &Config) -> Result<Receiver, NetError> {
        let (sock, _peer) = listener.accept().map_err(|e| NetError::Io(e.to_string()))?;
        Self::start(sock, cfg)
    }

    pub fn start(sock: TcpStream, cfg: &Config) -> Result<Receiver, NetError> {
        let io = |e: std::io::Error| NetError::Io(e.to_string());
        let _ = sock.set_nodelay(true);
        let _ = sock.set_write_timeout(Some(cfg.dead_after));
        let _ = sock.set_read_timeout(Some(cfg.dead_after)); // only for the Hello below
        let act = Activity::new();
        act.rx();
        let mut rd = ActiveRead { inner: sock.try_clone().map_err(io)?, act: act.clone() };
        let (src, query) = match read_frame(&mut rd) {
            Ok(Some(Frame::Hello { src, query })) => (src, query),
            Ok(Some(other)) => return Err(NetError::Protocol(format!("first frame was {other:?}, not Hello"))),
            Ok(None) => return Err(NetError::PeerLost("closed before Hello".into())),
            Err(e) => return Err(wire_to_net(e)),
        };
        let _ = sock.set_read_timeout(None);
        let w: Writer = Arc::new(Mutex::new(sock.try_clone().map_err(io)?));
        let sh = Arc::new(RecvShared { st: Mutex::new(RecvState::default()), cv: Condvar::new() });
        let done = Arc::new(AtomicBool::new(false));
        send_frame(&w, &act, &Frame::Credit { bytes: cfg.credit_bytes })?;
        {
            let (sh, w, act, cfg) = (sh.clone(), w.clone(), act.clone(), cfg.clone());
            thread::spawn(move || {
                let fail = |e: NetError| {
                    let mut st = sh.st.lock().unwrap();
                    if st.err.is_none() && !st.cancelled && !st.eos {
                        st.err = Some(e);
                    }
                    sh.cv.notify_all();
                };
                loop {
                    match read_frame(&mut rd) {
                        Ok(Some(Frame::Data { seq, payload })) => {
                            let mut st = sh.st.lock().unwrap();
                            #[cfg(not(feature = "bug-no-seq-check"))]
                            if seq != st.expected_seq {
                                let e = NetError::Protocol(format!("out of order: got batch {seq}, expected {}", st.expected_seq));
                                drop(st);
                                fail(e);
                                break;
                            }
                            #[cfg(feature = "bug-no-seq-check")]
                            let _ = seq;
                            st.expected_seq += 1;
                            let len = payload.len() as u64;
                            st.received += len;
                            if st.received - st.returned > cfg.credit_bytes {
                                drop(st);
                                fail(NetError::Protocol("the sender exceeded its credit".into()));
                                break;
                            }
                            st.buffered += len;
                            st.peak_buffered = st.peak_buffered.max(st.buffered);
                            st.q.push_back(payload);
                            sh.cv.notify_all();
                        }
                        Ok(Some(Frame::Eos { count })) => {
                            let mut st = sh.st.lock().unwrap();
                            #[cfg(not(feature = "bug-eos-count-unchecked"))]
                            if count != st.expected_seq {
                                let e = NetError::Protocol(format!("Eos says {count} batches, {} arrived", st.expected_seq));
                                drop(st);
                                fail(e);
                                break;
                            }
                            #[cfg(feature = "bug-eos-count-unchecked")]
                            let _ = count;
                            st.eos = true;
                            sh.cv.notify_all();
                            // do NOT stop reading: whatever the sender still sends (a Ping) must be read, or closing this socket would reset it
                        }
                        Ok(Some(Frame::Ping { n })) => {
                            let _ = send_frame(&w, &act, &Frame::Pong { n });
                        }
                        Ok(Some(Frame::Pong { .. })) => {}
                        Ok(Some(other)) => {
                            fail(NetError::Protocol(format!("unexpected frame from the sender: {other:?}")));
                            break;
                        }
                        Ok(None) => {
                            #[cfg(feature = "bug-eof-is-eos")]
                            {
                                let mut st = sh.st.lock().unwrap();
                                st.eos = true;
                                sh.cv.notify_all();
                            }
                            #[cfg(not(feature = "bug-eof-is-eos"))]
                            fail(NetError::PeerLost("connection closed before the end of the stream".into()));
                            break;
                        }
                        Err(e) => {
                            fail(wire_to_net(e));
                            break;
                        }
                    }
                }
            });
        }
        {
            let sh2 = sh.clone();
            spawn_watchdog(sock.try_clone().map_err(io)?, w.clone(), act.clone(), cfg.clone(), done.clone(), move |why| {
                let mut st = sh2.st.lock().unwrap();
                if st.err.is_none() && !st.eos {
                    st.err = Some(NetError::PeerLost(why));
                }
                sh2.cv.notify_all();
            });
        }
        Ok(Receiver { sh, w, act, done, sock, src, query })
    }

    /// The next batch in order, `Ok(None)` after a complete stream. A lost or misbehaving sender is an error AT ONCE (buffered batches are
    /// not delivered after it: the query is going to fail anyway).
    pub fn next(&self) -> Result<Option<Vec<u8>>, NetError> {
        let mut st = self.sh.st.lock().unwrap();
        loop {
            if st.cancelled {
                return Err(NetError::Cancelled);
            }
            if let Some(e) = &st.err {
                return Err(e.clone());
            }
            if let Some(b) = st.q.pop_front() {
                let len = b.len() as u64;
                st.buffered -= len;
                st.batches += 1;
                #[cfg(not(feature = "bug-credit-not-returned"))]
                {
                    st.returned += len;
                    drop(st);
                    let _ = send_frame(&self.w, &self.act, &Frame::Credit { bytes: len });
                }
                return Ok(Some(b));
            }
            if st.eos {
                drop(st);
                // everything was taken: tell the sender so (FIN), it is waiting for this before it may exit
                let _ = self.sock.shutdown(Shutdown::Write);
                return Ok(None);
            }
            st = self.sh.cv.wait(st).unwrap();
        }
    }

    /// Tells the sender to stop. `next` returns `Cancelled` from now on.
    pub fn cancel(&self) {
        {
            let mut st = self.sh.st.lock().unwrap();
            st.cancelled = true;
            self.sh.cv.notify_all();
        }
        let _ = send_frame(&self.w, &self.act, &Frame::Cancel);
    }

    pub fn stats(&self) -> RecvStats {
        let st = self.sh.st.lock().unwrap();
        RecvStats { batches: st.batches, bytes: st.received, peak_buffered: st.peak_buffered }
    }

    /// Closes the connection at once, with no frame: what a crashed process looks like to the sender.
    pub fn kill(&self) {
        self.done.store(true, Ordering::Relaxed);
        let _ = self.sock.shutdown(Shutdown::Both);
    }

    pub fn peer_addr(&self) -> Option<SocketAddr> {
        self.sock.peer_addr().ok()
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Relaxed);
        let _ = self.sock.shutdown(Shutdown::Both);
    }
}
