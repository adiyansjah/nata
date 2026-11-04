//! SIMULATED object store: a small HTTP/1.1 server that speaks the part of the S3 API a read path uses
//! (path-style `GET /bucket/key` with `Range`, and `HEAD`), serves files from a directory, and adds
//!   * first-byte latency drawn from a heavy-tailed distribution (log-normal body, Pareto tail),
//!   * a per-connection transfer cost (ms per MiB) and a global bandwidth cap,
//!   * random failures (5xx, connection reset, hang) and SCRIPTED failures ("fail the first K attempts of every
//!     request matching a path, permanently or K times"), so retry tests are deterministic,
//!   * counters and a per-request log for the tests (`/__stats`, `/__log`) and a control endpoint (`/__ctl`).
//!
//! Everything measured against it is labelled SIMULATED. It runs on loopback, so it has no TCP slow start, no TLS,
//! no cross-zone path and one shared kernel; it can say whether the client logic is right, not what GCS does.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::util::Rng;

// ------------------------------------------------------------------ latency profile

#[derive(Clone, Debug)]
pub struct Profile {
    pub name: String,
    /// Median first-byte latency of the log-normal body, ms.
    pub median_ms: f64,
    /// Log-normal sigma (0 = constant latency).
    pub sigma: f64,
    /// Probability that a request falls in the heavy tail instead.
    pub tail_prob: f64,
    /// Pareto tail: minimum, shape, cap (ms).
    pub tail_min_ms: f64,
    pub tail_alpha: f64,
    pub tail_cap_ms: f64,
    /// Per-connection transfer cost, ms per MiB (12 ms/MiB is about 83 MiB/s per connection).
    pub per_mb_ms: f64,
    /// Global bandwidth cap over all connections, MiB/s (0 = unlimited).
    pub bandwidth_mb_s: f64,
    /// Random failures, as probabilities per request.
    pub p_5xx: f64,
    pub p_reset: f64,
    pub p_hang: f64,
    pub hang_ms: u64,
}

impl Profile {
    pub fn zero() -> Self {
        Profile {
            name: "zero".into(),
            median_ms: 0.0,
            sigma: 0.0,
            tail_prob: 0.0,
            tail_min_ms: 0.0,
            tail_alpha: 1.5,
            tail_cap_ms: 0.0,
            per_mb_ms: 0.0,
            bandwidth_mb_s: 0.0,
            p_5xx: 0.0,
            p_reset: 0.0,
            p_hang: 0.0,
            hang_ms: 3000,
        }
    }

    /// Resembles a same-region object store seen from a VM: ~25 ms median first byte, a 1 percent heavy tail
    /// (120 ms and up, shape 1.5, capped at 2 s), about 83 MiB/s per connection, 1200 MiB/s in total.
    /// These numbers are an ASSUMPTION written down here, not a measurement of any real store.
    pub fn gcs() -> Self {
        Profile {
            name: "gcs".into(),
            median_ms: 25.0,
            sigma: 0.35,
            tail_prob: 0.01,
            tail_min_ms: 120.0,
            tail_alpha: 1.5,
            tail_cap_ms: 2000.0,
            per_mb_ms: 12.0,
            bandwidth_mb_s: 1200.0,
            ..Profile::zero()
        }
    }

    /// Slow transfers (10 MiB/s per connection): long-running requests for the cancellation and memory tests.
    pub fn slow() -> Self {
        Profile {
            name: "slow".into(),
            median_ms: 20.0,
            per_mb_ms: 100.0,
            ..Profile::zero()
        }
    }

    /// Exactly 50 ms before the first byte, nothing else: the harness self-test injects this known latency.
    pub fn const50() -> Self {
        Profile {
            name: "const50".into(),
            median_ms: 50.0,
            ..Profile::zero()
        }
    }

    /// Zero latency, 50 MiB/s in total: the harness self-test injects this known bandwidth cap.
    pub fn bw50() -> Self {
        Profile {
            name: "bw50".into(),
            bandwidth_mb_s: 50.0,
            ..Profile::zero()
        }
    }

    pub fn named(n: &str) -> Option<Profile> {
        Some(match n {
            "zero" => Profile::zero(),
            "gcs" => Profile::gcs(),
            "slow" => Profile::slow(),
            "const50" => Profile::const50(),
            "bw50" => Profile::bw50(),
            _ => return None,
        })
    }

    /// One first-byte latency draw, ms.
    pub fn first_byte_ms(&self, rng: &mut Rng) -> f64 {
        if self.tail_prob > 0.0 && rng.f64() < self.tail_prob {
            let u = rng.f64();
            return (self.tail_min_ms * u.powf(-1.0 / self.tail_alpha)).min(self.tail_cap_ms.max(self.tail_min_ms));
        }
        if self.sigma == 0.0 {
            return self.median_ms;
        }
        self.median_ms * (self.sigma * rng.normal()).exp()
    }

    fn set(&mut self, key: &str, v: &str) -> Result<(), String> {
        let f: f64 = v.parse().map_err(|_| format!("bad number {v:?} for {key}"))?;
        match key {
            "median_ms" => self.median_ms = f,
            "sigma" => self.sigma = f,
            "tail_prob" => self.tail_prob = f,
            "tail_min_ms" => self.tail_min_ms = f,
            "tail_alpha" => self.tail_alpha = f,
            "tail_cap_ms" => self.tail_cap_ms = f,
            "per_mb_ms" => self.per_mb_ms = f,
            "bandwidth_mb_s" => self.bandwidth_mb_s = f,
            "p_5xx" => self.p_5xx = f,
            "p_reset" => self.p_reset = f,
            "p_hang" => self.p_hang = f,
            "hang_ms" => self.hang_ms = f as u64,
            _ => return Err(format!("unknown profile field {key}")),
        }
        Ok(())
    }
}

// ------------------------------------------------------------------ scripted faults

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// Answer with this HTTP status and a small error body.
    Status(u16),
    /// Send the headers and about half of the body, then reset the connection.
    ResetMid,
    /// Reset the connection before any response.
    ResetEarly,
    /// Accept the request and do not answer for `hang_ms`, then drop the connection.
    Hang,
}

impl Fault {
    pub fn parse(s: &str) -> Option<Fault> {
        Some(match s {
            "reset" | "reset_mid" => Fault::ResetMid,
            "reset_early" => Fault::ResetEarly,
            "hang" => Fault::Hang,
            n => Fault::Status(n.parse().ok()?),
        })
    }
    fn label(&self) -> String {
        match self {
            Fault::Status(c) => format!("status{c}"),
            Fault::ResetMid => "reset_mid".into(),
            Fault::ResetEarly => "reset_early".into(),
            Fault::Hang => "hang".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Any,
    /// Ranged reads that are not a suffix read.
    Data,
    /// Suffix reads (`Range: bytes=-N`), which is how the footer is fetched.
    Footer,
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub path_contains: String,
    pub fault: Fault,
    /// The fault applies to the first `first` attempts of every distinct (path, range) key; `u32::MAX` = always.
    pub first: u32,
    pub target: Target,
}

// ------------------------------------------------------------------ state, stats, log

#[derive(Default)]
pub struct SimStats {
    pub requests: AtomicU64,
    pub bytes_sent: AtomicU64,
    pub inflight: AtomicU64,
    pub peak_inflight: AtomicU64,
    /// A write failed because the client went away (a cancelled or timed-out request).
    pub aborted: AtomicU64,
    pub faults: AtomicU64,
    pub s2xx: AtomicU64,
    pub s4xx: AtomicU64,
    pub s5xx: AtomicU64,
    pub connections: AtomicU64,
}

impl SimStats {
    fn reset(&self) {
        for a in [
            &self.requests,
            &self.bytes_sent,
            &self.aborted,
            &self.faults,
            &self.s2xx,
            &self.s4xx,
            &self.s5xx,
            &self.connections,
        ] {
            a.store(0, Relaxed);
        }
        self.peak_inflight.store(self.inflight.load(Relaxed), Relaxed);
    }

    pub fn render(&self) -> String {
        format!(
            "requests={}\nbytes_sent={}\ninflight={}\npeak_inflight={}\naborted={}\nfaults={}\ns2xx={}\ns4xx={}\ns5xx={}\nconnections={}\n",
            self.requests.load(Relaxed),
            self.bytes_sent.load(Relaxed),
            self.inflight.load(Relaxed),
            self.peak_inflight.load(Relaxed),
            self.aborted.load(Relaxed),
            self.faults.load(Relaxed),
            self.s2xx.load(Relaxed),
            self.s4xx.load(Relaxed),
            self.s5xx.load(Relaxed),
            self.connections.load(Relaxed),
        )
    }
}

#[derive(Clone, Debug)]
pub struct LogEntry {
    pub id: u64,
    pub t_start_ms: f64,
    pub t_end_ms: f64,
    pub path: String,
    pub range: String,
    pub attempt: u32,
    pub status: u16,
    pub fault: String,
    pub bytes: u64,
}

pub struct SimState {
    pub root: PathBuf,
    pub bucket: String,
    start: Instant,
    profile: Mutex<Profile>,
    rules: Mutex<Vec<Rule>>,
    attempts: Mutex<HashMap<String, u32>>,
    pub stats: SimStats,
    log: Mutex<Vec<LogEntry>>,
    logging: AtomicBool,
    link: Mutex<Instant>,
    seq: AtomicU64,
    seed: u64,
    pub shutdown: AtomicBool,
}

impl SimState {
    pub fn new(root: PathBuf, bucket: &str, profile: Profile, seed: u64) -> Arc<SimState> {
        Arc::new(SimState {
            root,
            bucket: bucket.to_string(),
            start: Instant::now(),
            profile: Mutex::new(profile),
            rules: Mutex::new(Vec::new()),
            attempts: Mutex::new(HashMap::new()),
            stats: SimStats::default(),
            log: Mutex::new(Vec::new()),
            logging: AtomicBool::new(true),
            link: Mutex::new(Instant::now()),
            seq: AtomicU64::new(0),
            seed,
            shutdown: AtomicBool::new(false),
        })
    }

    fn profile(&self) -> Profile {
        self.profile.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Apply one control command (also used in-process). Commands:
    /// `profile NAME`, `set FIELD VALUE`, `rule PATH_CONTAINS KIND FIRST|perm TARGET`, `clear_rules`, `reset`,
    /// `logging on|off`, `shutdown`.
    pub fn command(&self, line: &str) -> Result<(), String> {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.as_slice() {
            [] => Ok(()),
            ["profile", n] => {
                *self.profile.lock().unwrap_or_else(|e| e.into_inner()) =
                    Profile::named(n).ok_or_else(|| format!("unknown profile {n}"))?;
                Ok(())
            }
            ["set", k, v] => self.profile.lock().unwrap_or_else(|e| e.into_inner()).set(k, v),
            ["rule", p, kind, first, target] => {
                let fault = Fault::parse(kind).ok_or_else(|| format!("bad fault kind {kind}"))?;
                let first = if *first == "perm" {
                    u32::MAX
                } else {
                    first.parse().map_err(|_| format!("bad count {first}"))?
                };
                let target = match *target {
                    "any" => Target::Any,
                    "data" => Target::Data,
                    "footer" => Target::Footer,
                    t => return Err(format!("bad target {t}")),
                };
                // "-" stands for "every path"
                let path_contains = if *p == "-" { String::new() } else { (*p).to_string() };
                self.rules.lock().unwrap_or_else(|e| e.into_inner()).push(Rule {
                    path_contains,
                    fault,
                    first,
                    target,
                });
                Ok(())
            }
            ["clear_rules"] => {
                self.rules.lock().unwrap_or_else(|e| e.into_inner()).clear();
                self.attempts.lock().unwrap_or_else(|e| e.into_inner()).clear();
                Ok(())
            }
            ["reset"] => {
                self.stats.reset();
                self.log.lock().unwrap_or_else(|e| e.into_inner()).clear();
                self.attempts.lock().unwrap_or_else(|e| e.into_inner()).clear();
                Ok(())
            }
            ["logging", v] => {
                self.logging.store(*v == "on", Relaxed);
                Ok(())
            }
            ["shutdown"] => {
                self.shutdown.store(true, Relaxed);
                Ok(())
            }
            _ => Err(format!("unknown command {line:?}")),
        }
    }

    pub fn log_snapshot(&self) -> Vec<LogEntry> {
        self.log.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn render_log(&self) -> String {
        let mut s = String::new();
        for e in self.log_snapshot() {
            s.push_str(&format!(
                "{}\t{:.3}\t{:.3}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                e.id, e.t_start_ms, e.t_end_ms, e.path, e.range, e.attempt, e.status, e.fault, e.bytes
            ));
        }
        s
    }

    /// Reserve `n` bytes of the shared link; returns when they may be sent.
    fn link_deadline(&self, n: usize, mb_s: f64) -> Option<Instant> {
        if mb_s <= 0.0 {
            return None;
        }
        let mut g = self.link.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let start = if *g > now { *g } else { now };
        let end = start + Duration::from_secs_f64(n as f64 / (mb_s * 1024.0 * 1024.0));
        *g = end;
        Some(end)
    }
}

/// Parse the log text of `/__log` (used by the control client).
pub fn parse_log(text: &str) -> Vec<LogEntry> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 9 {
                return None;
            }
            Some(LogEntry {
                id: f[0].parse().ok()?,
                t_start_ms: f[1].parse().ok()?,
                t_end_ms: f[2].parse().ok()?,
                path: f[3].to_string(),
                range: f[4].to_string(),
                attempt: f[5].parse().ok()?,
                status: f[6].parse().ok()?,
                fault: f[7].to_string(),
                bytes: f[8].parse().ok()?,
            })
        })
        .collect()
}

// ------------------------------------------------------------------ the server

pub struct SimServer {
    pub addr: SocketAddr,
    pub state: Arc<SimState>,
    task: tokio::task::JoinHandle<()>,
}

impl SimServer {
    /// Bind `127.0.0.1:port` (0 = any free port) and serve `root` as bucket `bucket`.
    pub async fn start(root: PathBuf, bucket: &str, profile: Profile, seed: u64, port: u16) -> std::io::Result<SimServer> {
        let listener = TcpListener::bind(("127.0.0.1", port)).await?;
        let addr = listener.local_addr()?;
        let state = SimState::new(root, bucket, profile, seed);
        let st = Arc::clone(&state);
        let task = tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else { break };
                let _ = sock.set_nodelay(true);
                st.stats.connections.fetch_add(1, Relaxed);
                let st2 = Arc::clone(&st);
                tokio::spawn(async move {
                    let _ = handle_conn(st2, sock).await;
                });
            }
        });
        Ok(SimServer { addr, state, task })
    }
}

impl Drop for SimServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn find_head_end(b: &[u8]) -> Option<usize> {
    b.windows(4).position(|w| w == b"\r\n\r\n")
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

async fn handle_conn(st: Arc<SimState>, mut sock: TcpStream) -> std::io::Result<()> {
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut tmp = [0u8; 4096];
    loop {
        // read one request head
        let head_end = loop {
            if let Some(p) = find_head_end(&buf) {
                break p;
            }
            let n = sock.read(&mut tmp).await?;
            if n == 0 {
                return Ok(());
            }
            buf.extend_from_slice(&tmp[..n]);
            if buf.len() > 64 * 1024 {
                return Ok(());
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
        buf.drain(..head_end + 4);
        let mut lines = head.split("\r\n");
        let reqline = lines.next().unwrap_or("");
        let mut parts = reqline.split(' ');
        let method = parts.next().unwrap_or("").to_string();
        let target = parts.next().unwrap_or("").to_string();
        let mut range_hdr = String::new();
        let mut content_len = 0usize;
        let mut close = false;
        for l in lines {
            if let Some((k, v)) = l.split_once(':') {
                let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
                match k.as_str() {
                    "range" => range_hdr = v.to_string(),
                    "content-length" => content_len = v.parse().unwrap_or(0),
                    "connection" => close = v.eq_ignore_ascii_case("close"),
                    _ => {}
                }
            }
        }
        // request body (control commands only)
        while buf.len() < content_len {
            let n = sock.read(&mut tmp).await?;
            if n == 0 {
                return Ok(());
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        let body: Vec<u8> = buf.drain(..content_len).collect();
        let path_part = target.split('?').next().unwrap_or("").to_string();

        if path_part.starts_with("/__") {
            let (code, text) = control(&st, &method, &path_part, &body);
            let resp = format!(
                "HTTP/1.1 {code} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            );
            sock.write_all(resp.as_bytes()).await?;
            return Ok(());
        }

        match serve_object(&st, &mut sock, &method, &percent_decode(&path_part), &range_hdr).await {
            Ok(keep) => {
                if !keep || close {
                    return Ok(());
                }
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::ConnectionReset {
                    reset_conn(sock);
                }
                return Err(e);
            }
        }
    }
}

fn control(st: &Arc<SimState>, method: &str, path: &str, body: &[u8]) -> (u16, String) {
    match (method, path) {
        ("GET", "/__stats") => (200, st.stats.render()),
        ("GET", "/__log") => (200, st.render_log()),
        ("POST", "/__ctl") => {
            let text = String::from_utf8_lossy(body);
            for line in text.lines() {
                if let Err(e) = st.command(line) {
                    return (400, format!("error: {e}\n"));
                }
            }
            (200, "ok\n".into())
        }
        _ => (404, "unknown control endpoint\n".into()),
    }
}

struct Inflight<'a>(&'a SimStats);
impl<'a> Inflight<'a> {
    fn new(s: &'a SimStats) -> Self {
        let now = s.inflight.fetch_add(1, Relaxed) + 1;
        s.peak_inflight.fetch_max(now, Relaxed);
        Inflight(s)
    }
}
impl Drop for Inflight<'_> {
    fn drop(&mut self) {
        self.0.inflight.fetch_sub(1, Relaxed);
    }
}

#[derive(Default)]
struct ReqOut {
    status: u16,
    bytes: u64,
    fault: String,
}

fn reset_conn(sock: TcpStream) {
    // linger 0 turns the close into an RST, which is what a "connection reset by peer" is
    #[allow(deprecated)]
    let _ = sock.set_linger(Some(Duration::ZERO));
    drop(sock);
}

/// Returns whether the connection may be reused.
async fn serve_object(
    st: &Arc<SimState>,
    sock: &mut TcpStream,
    method: &str,
    path: &str,
    range_hdr: &str,
) -> std::io::Result<bool> {
    let id = st.seq.fetch_add(1, Relaxed);
    st.stats.requests.fetch_add(1, Relaxed);
    let _g = Inflight::new(&st.stats);
    let t_start = st.start.elapsed().as_secs_f64() * 1e3;
    let key = format!("{path}|{range_hdr}");
    let attempt = {
        let mut m = st.attempts.lock().unwrap_or_else(|e| e.into_inner());
        let c = m.entry(key).or_insert(0);
        *c += 1;
        *c
    };
    let mut out = ReqOut::default();
    let prof = st.profile();
    let mut rng = Rng::new(st.seed ^ id.wrapping_mul(0x2545_F491_4F6C_DD1D));
    let is_footer = range_hdr.starts_with("bytes=-");

    // the fault for this request: scripted rules first, then random
    let mut fault: Option<Fault> = None;
    {
        let rules = st.rules.lock().unwrap_or_else(|e| e.into_inner());
        for r in rules.iter() {
            let t_ok = match r.target {
                Target::Any => true,
                Target::Data => !is_footer,
                Target::Footer => is_footer,
            };
            if t_ok && path.contains(&r.path_contains) && attempt <= r.first {
                fault = Some(r.fault);
                break;
            }
        }
    }
    if fault.is_none() && (prof.p_5xx + prof.p_reset + prof.p_hang) > 0.0 {
        let u = rng.f64();
        if u < prof.p_5xx {
            fault = Some(Fault::Status(503));
        } else if u < prof.p_5xx + prof.p_reset {
            fault = Some(Fault::ResetMid);
        } else if u < prof.p_5xx + prof.p_reset + prof.p_hang {
            fault = Some(Fault::Hang);
        }
    }

    // A hung request is logged when it starts (it would otherwise only appear after `hang_ms`).
    let hang_logged = fault == Some(Fault::Hang) && st.logging.load(Relaxed);
    if hang_logged {
        st.log.lock().unwrap_or_else(|e| e.into_inner()).push(LogEntry {
            id,
            t_start_ms: t_start,
            t_end_ms: t_start,
            path: path.to_string(),
            range: range_hdr.to_string(),
            attempt,
            status: 0,
            fault: Fault::Hang.label(),
            bytes: 0,
        });
    }
    let result = serve_inner(st, sock, method, path, range_hdr, &prof, &mut rng, fault, &mut out).await;
    let t_end = st.start.elapsed().as_secs_f64() * 1e3;
    if result.is_err() && fault.is_none() {
        st.stats.aborted.fetch_add(1, Relaxed);
    }
    match out.status {
        200..=299 => st.stats.s2xx.fetch_add(1, Relaxed),
        400..=499 => st.stats.s4xx.fetch_add(1, Relaxed),
        500..=599 => st.stats.s5xx.fetch_add(1, Relaxed),
        _ => 0,
    };
    if st.logging.load(Relaxed) && !hang_logged {
        st.log.lock().unwrap_or_else(|e| e.into_inner()).push(LogEntry {
            id,
            t_start_ms: t_start,
            t_end_ms: t_end,
            path: path.to_string(),
            range: range_hdr.to_string(),
            attempt,
            status: out.status,
            fault: out.fault.clone(),
            bytes: out.bytes,
        });
    }
    result
}

fn error_response(code: u16, msg: &str) -> Vec<u8> {
    let body = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>{msg}</Code><Message>{msg}</Message></Error>");
    format!(
        "HTTP/1.1 {code} X\r\nContent-Type: application/xml\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

#[allow(clippy::too_many_arguments)]
async fn serve_inner(
    st: &Arc<SimState>,
    sock: &mut TcpStream,
    method: &str,
    path: &str,
    range_hdr: &str,
    prof: &Profile,
    rng: &mut Rng,
    fault: Option<Fault>,
    out: &mut ReqOut,
) -> std::io::Result<bool> {
    if let Some(f) = fault {
        st.stats.faults.fetch_add(1, Relaxed);
        out.fault = f.label();
        match f {
            Fault::Status(code) => {
                out.status = code;
                sock.write_all(&error_response(code, "InjectedFault")).await?;
                return Ok(true);
            }
            Fault::ResetEarly => {
                // never reuse this socket: hand it to a closer by shutting down hard
                return Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "injected reset (early)"));
            }
            Fault::Hang => {
                tokio::time::sleep(Duration::from_millis(prof.hang_ms)).await;
                return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "injected hang"));
            }
            Fault::ResetMid => {} // handled while sending the body
        }
    }
    if method != "GET" && method != "HEAD" {
        out.status = 405;
        sock.write_all(&error_response(405, "MethodNotAllowed")).await?;
        return Ok(true);
    }
    let prefix = format!("/{}/", st.bucket);
    let Some(key) = path.strip_prefix(&prefix) else {
        out.status = 404;
        sock.write_all(&error_response(404, "NoSuchBucket")).await?;
        return Ok(true);
    };
    if key.contains("..") {
        out.status = 400;
        sock.write_all(&error_response(400, "BadKey")).await?;
        return Ok(true);
    }
    let file = st.root.join(key);
    let Ok(meta) = tokio::fs::metadata(&file).await else {
        out.status = 404;
        sock.write_all(&error_response(404, "NoSuchKey")).await?;
        return Ok(true);
    };
    if !meta.is_file() {
        out.status = 404;
        sock.write_all(&error_response(404, "NoSuchKey")).await?;
        return Ok(true);
    }
    let size = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let etag = format!("\"{size:x}-{mtime:x}\"");

    // range
    let (status, start, end_incl) = if range_hdr.is_empty() {
        (200u16, 0u64, size.saturating_sub(1))
    } else {
        let spec = range_hdr.strip_prefix("bytes=").unwrap_or("");
        let (a, b) = spec.split_once('-').unwrap_or(("", ""));
        let r = if a.is_empty() {
            let n: u64 = b.parse().unwrap_or(0);
            if n == 0 || size == 0 {
                None
            } else {
                Some((size.saturating_sub(n), size - 1))
            }
        } else {
            let s: u64 = a.parse().unwrap_or(u64::MAX);
            let e: u64 = if b.is_empty() { size.saturating_sub(1) } else { b.parse().unwrap_or(0) };
            if s >= size || e < s { None } else { Some((s, e.min(size - 1))) }
        };
        match r {
            Some((s, e)) => (206, s, e),
            None => {
                out.status = 416;
                let body = "<Error><Code>InvalidRange</Code></Error>";
                let resp = format!(
                    "HTTP/1.1 416 X\r\nContent-Range: bytes */{size}\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(resp.as_bytes()).await?;
                return Ok(true);
            }
        }
    };
    let len = if size == 0 { 0 } else { end_incl - start + 1 };

    // first-byte latency
    let fb = prof.first_byte_ms(rng);
    if fb > 0.0 {
        tokio::time::sleep(Duration::from_secs_f64(fb / 1e3)).await;
    }

    out.status = status;
    let mut head = String::new();
    head.push_str(if status == 206 {
        "HTTP/1.1 206 Partial Content\r\n"
    } else {
        "HTTP/1.1 200 OK\r\n"
    });
    head.push_str(&format!("Content-Length: {len}\r\nAccept-Ranges: bytes\r\nETag: {etag}\r\n"));
    head.push_str("Last-Modified: Thu, 01 Jan 2026 00:00:00 GMT\r\nContent-Type: application/octet-stream\r\n");
    if status == 206 {
        head.push_str(&format!("Content-Range: bytes {start}-{end_incl}/{size}\r\n"));
    }
    head.push_str("\r\n");
    sock.write_all(head.as_bytes()).await?;
    if method == "HEAD" || len == 0 {
        return Ok(true);
    }

    let mut f = tokio::fs::File::open(&file).await?;
    f.seek(std::io::SeekFrom::Start(start)).await?;
    // Read 1 MiB at a time, send 64 KiB at a time. Pacing uses ABSOLUTE deadlines (a per-connection one and the shared
    // link's), because a timer sleeps in whole milliseconds and a 64 KiB chunk at 80 MiB/s is under one.
    const BLOCK: usize = 1024 * 1024;
    const CHUNK: usize = 64 * 1024;
    let mut block = vec![0u8; BLOCK];
    let mut left = len;
    let reset_after = if fault == Some(Fault::ResetMid) { len / 2 } else { u64::MAX };
    let mut sent_total = 0u64;
    let mut conn_t = Instant::now();
    while left > 0 {
        let nb = (left as usize).min(BLOCK);
        f.read_exact(&mut block[..nb]).await?;
        let mut off = 0;
        while off < nb {
            let n = (nb - off).min(CHUNK);
            if prof.per_mb_ms > 0.0 {
                conn_t += Duration::from_secs_f64(prof.per_mb_ms * n as f64 / (1024.0 * 1024.0) / 1e3);
            }
            let mut until = conn_t;
            if let Some(t) = st.link_deadline(n, prof.bandwidth_mb_s) {
                until = until.max(t);
            }
            if until > Instant::now() {
                tokio::time::sleep_until(tokio::time::Instant::from_std(until)).await;
            }
            if sent_total >= reset_after {
                return Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "injected reset (mid-body)"));
            }
            sock.write_all(&block[off..off + n]).await?;
            st.stats.bytes_sent.fetch_add(n as u64, Relaxed);
            out.bytes += n as u64;
            sent_total += n as u64;
            off += n;
        }
        left -= nb as u64;
    }
    Ok(true)
}

// ------------------------------------------------------------------ control client (blocking, std only)

/// Talks to a running simulated store (`objscan serve`, or an in-process `SimServer`) over its HTTP control endpoints.
#[derive(Clone, Debug)]
pub struct SimCtl {
    pub addr: String,
}

impl SimCtl {
    pub fn new(addr: impl Into<String>) -> Self {
        SimCtl { addr: addr.into() }
    }

    fn request(&self, method: &str, path: &str, body: &str) -> Result<String, String> {
        let mut s = std::net::TcpStream::connect(&self.addr).map_err(|e| format!("ctl connect: {e}"))?;
        s.set_read_timeout(Some(Duration::from_secs(30))).ok();
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: sim\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes()).map_err(|e| format!("ctl write: {e}"))?;
        let mut resp = Vec::new();
        s.read_to_end(&mut resp).map_err(|e| format!("ctl read: {e}"))?;
        let text = String::from_utf8_lossy(&resp).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").ok_or("ctl: malformed response")?;
        if !head.starts_with("HTTP/1.1 200") {
            return Err(format!("ctl: {} {}", head.lines().next().unwrap_or(""), body.trim()));
        }
        Ok(body.to_string())
    }

    /// One or more commands separated by newlines (see [`SimState::command`]).
    pub fn command(&self, cmds: &str) -> Result<(), String> {
        self.request("POST", "/__ctl", cmds).map(|_| ())
    }

    pub fn stats(&self) -> Result<HashMap<String, u64>, String> {
        let t = self.request("GET", "/__stats", "")?;
        Ok(t.lines()
            .filter_map(|l| l.split_once('='))
            .filter_map(|(k, v)| Some((k.to_string(), v.parse().ok()?)))
            .collect())
    }

    pub fn stat(&self, name: &str) -> Result<u64, String> {
        Ok(self.stats()?.get(name).copied().unwrap_or(0))
    }

    pub fn log(&self) -> Result<Vec<LogEntry>, String> {
        Ok(parse_log(&self.request("GET", "/__log", "")?))
    }

    /// Reset counters, the log and the per-key attempt counters, remove scripted faults, and go back to the zero-latency profile.
    pub fn clean(&self) -> Result<(), String> {
        self.command("clear_rules\nreset\nprofile zero")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heavy_tail_profile_has_the_stated_shape() {
        let draw = |p: &Profile| {
            let mut rng = Rng::new(7);
            let mut v: Vec<f64> = (0..400_000).map(|_| p.first_byte_ms(&mut rng)).collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v
        };
        let share_ge = |v: &[f64], x: f64| v.iter().filter(|&&y| y >= x).count() as f64 / v.len() as f64;
        let p = Profile::gcs();
        let v = draw(&p);
        let med = v[v.len() / 2];
        assert!((med - 25.0).abs() < 2.0, "median {med}");
        // the tail: 1 percent of the requests take at least 120 ms (the body adds ~0.002 percent)
        let tail = share_ge(&v, 120.0);
        assert!((0.008..=0.012).contains(&tail), "share >= 120 ms: {tail}");
        assert!(*v.last().unwrap() <= 2000.0 + 1e-9, "capped at 2 s");
        assert!(share_ge(&v, 500.0) > 0.0005, "a heavy tail reaches far: share >= 500 ms is {}", share_ge(&v, 500.0));
        // the same profile without a tail has (almost) no such requests: the check can tell the two apart
        let q = Profile { tail_prob: 0.0, ..Profile::gcs() };
        let none = share_ge(&draw(&q), 120.0);
        assert!(none < 0.001, "no-tail share >= 120 ms: {none}");
        assert!((0.008..=0.012).contains(&tail) && !(0.008..=0.012).contains(&none));
    }

    #[test]
    fn percent_decoding_keeps_plain_paths() {
        assert_eq!(percent_decode("/a/b%20c/d.parquet"), "/a/b c/d.parquet");
        assert_eq!(percent_decode("/a/b"), "/a/b");
        assert_eq!(percent_decode("/a%"), "/a%");
    }
}
