//! A small, hostile, scriptable S3-style endpoint (path style, HTTP/1.1, GET/HEAD with Range and If-Match) over raw sockets, so that it can
//! say things a real HTTP library refuses to: a Content-Length that lies, a body that stalls or stops, a Content-Range that claims more than
//! was asked, an endless body. Every fault is scheduled by request ordinal or by a seed, so a run replays exactly (SEED + plan = same bytes).
//! It is a test fixture: no TLS, loopback, no authentication.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::util::Rng;

#[derive(Clone, Debug)]
pub enum Fault {
    /// An error status with `body` bytes of body (and an optional Retry-After). `endless`: the body never ends (a hostile error body).
    Status { code: u16, retry_after: Option<u64>, body: usize, endless: bool },
    /// Wait, then answer normally.
    Delay { ms: u64 },
    /// Accept the request and never answer.
    StallHeaders,
    /// Send the headers and `after` body bytes, then go silent with the connection open.
    StallBody { after: usize },
    /// Correct headers (Content-Length = the range), `after` bytes, then close.
    TruncateAfter { after: usize },
    /// Chunked body, `after` bytes in one chunk, then close without the final chunk.
    TruncateChunked { after: usize },
    /// Close the connection without a response.
    Reset,
    /// 206 whose Content-Range is the requested range shifted by (start, end); the body matches the CLAIMED range (consistent framing).
    RangeClaim { start_delta: i64, end_delta: i64 },
    /// 206, Content-Range correct, Content-Length (and body) = the range length + `delta` (consistent framing, disagreeing with the range).
    LengthMismatch { delta: i64 },
    /// 200 with the whole object, as if Range were ignored.
    IgnoreRange,
    /// 206, Content-Range correct, Content-Length = `claim` (huge) and an endless stream of zeros.
    Endless { claim: u64 },
    /// A single response header line of this many bytes.
    HugeHeader { bytes: usize },
    /// Serve the CURRENT object ignoring If-Match (a lying/legacy endpoint), with its current ETag.
    IgnoreIfMatch,
}

#[derive(Clone, Debug, Default)]
pub struct Plan {
    /// Fault for the i-th GET (0-based, since the last `set_plan`); GETs beyond the script are normal.
    pub script: Vec<Option<Fault>>,
    /// Applied to every GET not covered by the script.
    pub sticky: Option<Fault>,
    /// Applied to every HEAD.
    pub head: Option<Fault>,
    /// Seeded random faults on GETs not covered above: (seed, per-mille probability, faults to choose from).
    pub seeded: Option<(u64, u32, Vec<Fault>)>,
    /// Before serving the i-th GET OF THAT KEY (0-based; the footer read is the first), replace `key` (a version change between two reads).
    pub swap_before_get: Vec<(usize, String, Arc<Vec<u8>>)>,
}

impl Plan {
    pub fn script(v: Vec<Option<Fault>>) -> Plan {
        Plan { script: v, ..Default::default() }
    }
    pub fn always(f: Fault) -> Plan {
        Plan { sticky: Some(f), ..Default::default() }
    }
}

#[derive(Clone, Debug)]
pub struct ReqLog {
    pub at: Duration,
    pub method: String,
    pub key: String,
    pub range: Option<(u64, u64)>,
    pub if_match: Option<String>,
    pub fault: String,
    pub ordinal: usize,
}

struct Obj {
    bytes: Arc<Vec<u8>>,
    etag: String,
    version: Option<String>,
}

struct Shared {
    objects: Mutex<HashMap<String, Obj>>,
    plan: Mutex<Plan>,
    gets: AtomicUsize,
    key_gets: Mutex<HashMap<String, usize>>,
    log: Mutex<Vec<ReqLog>>,
    t0: Instant,
}

pub struct FakeS3 {
    pub addr: String,
    pub bucket: String,
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeS3 {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn fnv(b: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &x in b {
        h ^= x as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn etag_of(b: &[u8]) -> String {
    format!("\"{:016x}{:08x}\"", fnv(b), b.len())
}

impl FakeS3 {
    pub async fn start(bucket: &str) -> FakeS3 {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = format!("http://{}", listener.local_addr().unwrap());
        let shared = Arc::new(Shared { objects: Mutex::new(HashMap::new()), plan: Mutex::new(Plan::default()), gets: AtomicUsize::new(0), key_gets: Mutex::new(HashMap::new()), log: Mutex::new(Vec::new()), t0: Instant::now() });
        let sh = shared.clone();
        let task = tokio::spawn(async move {
            let mut conns = tokio::task::JoinSet::new(); // dropped with this task: every connection handler is aborted
            loop {
                let Ok((sock, _)) = listener.accept().await else { break };
                let _ = sock.set_nodelay(true); // head and body are separate writes: without this, Nagle + delayed ACK add ~40 ms per request
                let sh = sh.clone();
                conns.spawn(async move {
                    let _ = serve(sock, sh).await;
                });
            }
        });
        FakeS3 { addr, bucket: bucket.to_string(), shared, task }
    }

    pub fn put(&self, key: &str, bytes: Vec<u8>) {
        let etag = etag_of(&bytes);
        self.shared.objects.lock().unwrap().insert(key.to_string(), Obj { bytes: Arc::new(bytes), etag, version: None });
    }
    pub fn put_versioned(&self, key: &str, bytes: Vec<u8>, version: &str) {
        let etag = etag_of(&bytes);
        self.shared.objects.lock().unwrap().insert(key.to_string(), Obj { bytes: Arc::new(bytes), etag, version: Some(version.to_string()) });
    }
    pub fn set_plan(&self, plan: Plan) {
        *self.shared.plan.lock().unwrap() = plan;
        self.shared.gets.store(0, SeqCst);
        self.shared.key_gets.lock().unwrap().clear();
        self.shared.log.lock().unwrap().clear();
    }
    pub fn clear_log(&self) {
        self.shared.log.lock().unwrap().clear();
    }
    pub fn log(&self) -> Vec<ReqLog> {
        self.shared.log.lock().unwrap().clone()
    }
    pub fn gets(&self) -> usize {
        self.shared.log.lock().unwrap().iter().filter(|r| r.method == "GET").count()
    }
    pub fn heads(&self) -> usize {
        self.shared.log.lock().unwrap().iter().filter(|r| r.method == "HEAD").count()
    }
    pub fn etag(&self, key: &str) -> Option<String> {
        self.shared.objects.lock().unwrap().get(key).map(|o| o.etag.clone())
    }
}

struct Req {
    method: String,
    key: String,
    range: Option<(u64, u64)>,
    if_match: Option<String>,
    version_id: Option<String>,
}

async fn read_head(sock: &mut TcpStream, buf: &mut Vec<u8>) -> std::io::Result<Option<Req>> {
    loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..p]).into_owned();
            buf.drain(..p + 4);
            let mut lines = head.split("\r\n");
            let first = lines.next().unwrap_or("");
            let mut it = first.split(' ');
            let method = it.next().unwrap_or("").to_string();
            let target = it.next().unwrap_or("").to_string();
            let (path, query) = target.split_once('?').map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((target.clone(), String::new()));
            let key = path.trim_start_matches('/').split_once('/').map(|(_, k)| k.to_string()).unwrap_or_default();
            let mut range = None;
            let mut if_match = None;
            for l in lines {
                let Some((k, v)) = l.split_once(':') else { continue };
                let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
                if k == "range" {
                    if let Some(r) = v.strip_prefix("bytes=") {
                        if let Some((a, b)) = r.split_once('-') {
                            if let (Ok(a), Ok(b)) = (a.parse::<u64>(), b.parse::<u64>()) {
                                range = Some((a, b));
                            }
                        }
                    }
                } else if k == "if-match" {
                    if_match = Some(v.to_string());
                }
            }
            let version_id = query.split('&').find_map(|kv| kv.strip_prefix("versionId=")).map(|s| s.to_string());
            return Ok(Some(Req { method, key, range, if_match, version_id }));
        }
        if buf.len() > 64 * 1024 {
            return Ok(None);
        }
        let mut tmp = [0u8; 4096];
        let n = sock.read(&mut tmp).await?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&tmp[..n]);
    }
}

const HDATE: &str = "Wed, 21 Oct 2015 07:28:00 GMT";

fn status_line(code: u16) -> &'static str {
    match code {
        200 => "200 OK",
        206 => "206 Partial Content",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        412 => "412 Precondition Failed",
        416 => "416 Range Not Satisfiable",
        429 => "429 Too Many Requests",
        500 => "500 Internal Server Error",
        502 => "502 Bad Gateway",
        503 => "503 Slow Down",
        504 => "504 Gateway Timeout",
        _ => "500 Internal Server Error",
    }
}

async fn serve(mut sock: TcpStream, sh: Arc<Shared>) -> std::io::Result<()> {
    let mut buf = Vec::new();
    loop {
        let Some(req) = read_head(&mut sock, &mut buf).await? else { return Ok(()) };
        let is_get = req.method == "GET";
        let ordinal = if is_get { sh.gets.fetch_add(1, SeqCst) } else { 0 };
        let key_ordinal = if is_get {
            let mut g = sh.key_gets.lock().unwrap();
            let c = g.entry(req.key.clone()).or_insert(0);
            *c += 1;
            *c - 1
        } else {
            0
        };
        // the plan, and any object swap scheduled before this GET
        let (fault, swaps) = {
            let plan = sh.plan.lock().unwrap();
            let swaps: Vec<_> = plan.swap_before_get.iter().filter(|(i, k, _)| is_get && *i == key_ordinal && *k == req.key).cloned().collect();
            let f = if !is_get {
                plan.head.clone()
            } else if let Some(Some(f)) = plan.script.get(ordinal) {
                Some(f.clone())
            } else if ordinal < plan.script.len() {
                None
            } else if let Some(f) = &plan.sticky {
                Some(f.clone())
            } else if let Some((seed, permille, faults)) = &plan.seeded {
                let mut r = Rng::new(seed ^ (ordinal as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                if !faults.is_empty() && r.below(1000) < *permille as u64 { Some(faults[r.below(faults.len() as u64) as usize].clone()) } else { None }
            } else {
                None
            };
            (f, swaps)
        };
        for (_, key, bytes) in swaps {
            let etag = etag_of(&bytes);
            sh.objects.lock().unwrap().insert(key, Obj { bytes: Arc::new((*bytes).clone()), etag, version: None });
        }
        sh.log.lock().unwrap().push(ReqLog { at: sh.t0.elapsed(), method: req.method.clone(), key: req.key.clone(), range: req.range, if_match: req.if_match.clone(), fault: fault.as_ref().map(|f| format!("{f:?}")).unwrap_or_default(), ordinal });
        let keep = respond(&mut sock, &sh, &req, fault).await?;
        if !keep {
            return Ok(());
        }
    }
}

/// Returns whether the connection may be reused.
async fn respond(sock: &mut TcpStream, sh: &Shared, req: &Req, fault: Option<Fault>) -> std::io::Result<bool> {
    let obj = {
        let g = sh.objects.lock().unwrap();
        g.get(&req.key).map(|o| (o.bytes.clone(), o.etag.clone(), o.version.clone()))
    };
    if let Some(Fault::Delay { ms }) = &fault {
        tokio::time::sleep(Duration::from_millis(*ms)).await;
    }
    match &fault {
        Some(Fault::Reset) => return Ok(false),
        Some(Fault::StallHeaders) => {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            return Ok(false);
        }
        Some(Fault::Status { code, retry_after, body, endless }) => {
            let ra = retry_after.map(|s| format!("Retry-After: {s}\r\n")).unwrap_or_default();
            let cl = if *endless { u64::MAX / 4 } else { *body as u64 };
            sock.write_all(format!("HTTP/1.1 {}\r\n{ra}Content-Type: application/xml\r\nContent-Length: {cl}\r\n\r\n", status_line(*code)).as_bytes()).await?;
            if *endless {
                endless_body(sock).await;
                return Ok(false);
            }
            sock.write_all(&vec![b'x'; *body]).await?;
            return Ok(true);
        }
        Some(Fault::HugeHeader { bytes }) => {
            let junk = "a".repeat(*bytes);
            let _ = sock.write_all(format!("HTTP/1.1 200 OK\r\nX-Junk: {junk}\r\nContent-Length: 0\r\n\r\n").as_bytes()).await;
            return Ok(false);
        }
        _ => {}
    }
    let Some((bytes, etag, version)) = obj else {
        sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await?;
        return Ok(true);
    };
    if let (Some(want), None) = (&req.version_id, &fault) {
        if version.as_deref() != Some(want.as_str()) {
            sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n").await?;
            return Ok(true);
        }
    }
    let size = bytes.len() as u64;
    let vh = version.as_ref().map(|v| format!("x-amz-version-id: {v}\r\n")).unwrap_or_default();
    let base = format!("ETag: {etag}\r\nLast-Modified: {HDATE}\r\nAccept-Ranges: bytes\r\n{vh}");
    if req.method == "HEAD" {
        sock.write_all(format!("HTTP/1.1 200 OK\r\n{base}Content-Length: {size}\r\n\r\n").as_bytes()).await?;
        return Ok(true);
    }
    if let Some(m) = &req.if_match {
        if !matches!(fault, Some(Fault::IgnoreIfMatch)) && m != "*" && !m.split(',').any(|x| x.trim() == etag) {
            sock.write_all(b"HTTP/1.1 412 Precondition Failed\r\nContent-Length: 0\r\n\r\n").await?;
            return Ok(true);
        }
    }
    let Some((a, b)) = req.range else {
        sock.write_all(format!("HTTP/1.1 200 OK\r\n{base}Content-Length: {size}\r\n\r\n").as_bytes()).await?;
        sock.write_all(&bytes).await?;
        return Ok(true);
    };
    let b = b.min(size.saturating_sub(1));
    if a > b {
        sock.write_all(format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{size}\r\nContent-Length: 0\r\n\r\n").as_bytes()).await?;
        return Ok(true);
    }
    let want = &bytes[a as usize..=b as usize];
    let partial = |s: u64, e: u64, len: u64| format!("HTTP/1.1 206 Partial Content\r\n{base}Content-Range: bytes {s}-{e}/{size}\r\nContent-Length: {len}\r\n\r\n");
    match fault {
        Some(Fault::IgnoreRange) => {
            sock.write_all(format!("HTTP/1.1 200 OK\r\n{base}Content-Length: {size}\r\n\r\n").as_bytes()).await?;
            sock.write_all(&bytes).await?;
            Ok(true)
        }
        Some(Fault::RangeClaim { start_delta, end_delta }) => {
            let s = (a as i64 + start_delta).clamp(0, size as i64 - 1) as u64;
            let e = (b as i64 + end_delta).clamp(s as i64, size as i64 - 1) as u64;
            sock.write_all(partial(s, e, e - s + 1).as_bytes()).await?;
            sock.write_all(&bytes[s as usize..=e as usize]).await?;
            Ok(true)
        }
        Some(Fault::LengthMismatch { delta }) => {
            let len = (want.len() as i64 + delta).max(0) as usize;
            sock.write_all(partial(a, b, len as u64).as_bytes()).await?;
            let mut body = want.to_vec();
            body.resize(len, 0xAB);
            sock.write_all(&body).await?;
            Ok(true)
        }
        Some(Fault::TruncateAfter { after }) => {
            sock.write_all(partial(a, b, want.len() as u64).as_bytes()).await?;
            sock.write_all(&want[..after.min(want.len())]).await?;
            Ok(false)
        }
        Some(Fault::TruncateChunked { after }) => {
            sock.write_all(format!("HTTP/1.1 206 Partial Content\r\n{base}Content-Range: bytes {a}-{b}/{size}\r\nTransfer-Encoding: chunked\r\n\r\n").as_bytes()).await?;
            let n = after.min(want.len());
            sock.write_all(format!("{n:x}\r\n").as_bytes()).await?;
            sock.write_all(&want[..n]).await?;
            sock.write_all(b"\r\n").await?;
            Ok(false)
        }
        Some(Fault::StallBody { after }) => {
            sock.write_all(partial(a, b, want.len() as u64).as_bytes()).await?;
            sock.write_all(&want[..after.min(want.len())]).await?;
            sock.flush().await?;
            tokio::time::sleep(Duration::from_secs(3600)).await;
            Ok(false)
        }
        Some(Fault::Endless { claim }) => {
            sock.write_all(partial(a, b, claim).as_bytes()).await?;
            endless_body(sock).await;
            Ok(false)
        }
        _ => {
            sock.write_all(partial(a, b, want.len() as u64).as_bytes()).await?;
            sock.write_all(want).await?;
            Ok(true)
        }
    }
}

/// Zeros as fast as the peer reads, until it closes.
async fn endless_body(sock: &mut TcpStream) {
    let chunk = vec![0u8; 1 << 20];
    while sock.write_all(&chunk).await.is_ok() {}
}
