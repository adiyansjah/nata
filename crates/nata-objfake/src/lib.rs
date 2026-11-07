//! A small, hostile, scriptable S3-style endpoint (path style, HTTP/1.1, GET/HEAD with Range and If-Match) over raw sockets, so that it can
//! say things a real HTTP library refuses to: a Content-Length that lies, a body that stalls or stops, a Content-Range that claims more than
//! was asked, an endless body. Every fault is scheduled by request ordinal or by a seed, so a run replays exactly (SEED + plan = same bytes).
//! It is a test fixture: no TLS, loopback only, and it checks WHICH access key signed a request (the credential scope of an `Authorization` header), not the
//! signature. It also lists objects (S3 `list-type=2` XML, which the GCS XML API shares), and `FakeImds` is the EC2 instance-metadata service.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// SplitMix64, seeded: every fault schedule replays from its seed.
#[derive(Clone, Debug)]
pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }
}

/// `Mutex::lock` that survives a poisoned lock (a handler that panicked): the fixture's state is plain data.
trait LockExt<T> {
    fn lock_(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> LockExt<T> for Mutex<T> {
    fn lock_(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[derive(Clone, Debug)]
pub enum Fault {
    /// An error status with `body` bytes of body (and an optional Retry-After). `endless`: the body never ends (a hostile error body).
    Status {
        code: u16,
        retry_after: Option<u64>,
        body: usize,
        endless: bool,
    },
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
    /// A SUCCESS answer (200) whose Content-Length is `claim` and whose body is endless zeros: for a LIST, a token or a credential document.
    EndlessSuccess { claim: u64 },
    /// A SUCCESS answer with `body` as its exact content (a hostile or malformed XML/JSON document).
    Document { body: Vec<u8> },
    /// These exact bytes instead of a response, then the connection closes (garbage, a half-valid status line, headers without an end).
    Raw { bytes: Vec<u8> },
    /// A redirect (`code` 3xx) to `location` (the request's key is appended: `<location>/<key>`), or without a `Location` header. For GET, HEAD and LIST.
    Redirect { code: u16, location: Option<String> },
}

#[derive(Clone, Debug, Default)]
pub struct Plan {
    /// Fault for the i-th GET (0-based, since the last `set_plan`); GETs beyond the script are normal.
    pub script: Vec<Option<Fault>>,
    /// Applied to every GET not covered by the script.
    pub sticky: Option<Fault>,
    /// Applied to every HEAD.
    pub head: Option<Fault>,
    /// Applied to every LIST (a GET of the bucket with `list-type=2`), instead of the ordinal script.
    pub list: Option<Fault>,
    /// Applied to every GET and HEAD of the named key, whatever the ordinal.
    pub by_key: Vec<(String, Fault)>,
    /// Seeded random faults on GETs not covered above: (seed, per-mille probability, faults to choose from).
    pub seeded: Option<(u64, u32, Vec<Fault>)>,
    /// Before serving the i-th GET OF THAT KEY (0-based; the footer read is the first), replace `key` (a version change between two reads).
    pub swap_before_get: Vec<(usize, String, Arc<Vec<u8>>)>,
}

impl Plan {
    pub fn script(v: Vec<Option<Fault>>) -> Plan {
        Plan {
            script: v,
            ..Default::default()
        }
    }
    pub fn always(f: Fault) -> Plan {
        Plan {
            sticky: Some(f),
            ..Default::default()
        }
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
    /// The access key id of the request's `Authorization` credential scope (never the signature), and whether a session token header was sent.
    pub auth_key: Option<String>,
    pub session_token: bool,
    /// Whether the request carried an `Authorization: Bearer ...` header (the GCS style).
    pub bearer: bool,
    /// The raw query string (`list-type=2&prefix=...`).
    pub query: String,
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
    /// When set, a request not signed by this access key id is answered 403.
    require_key: Mutex<Option<String>>,
    /// Objects per LIST page.
    list_page: AtomicUsize,
}

/// A second loopback listener that records what reaches it: the target a hostile endpoint redirects to. A test asserts it saw nothing.
pub struct FakeSink {
    pub addr: String,
    seen: Arc<Mutex<SinkSeen>>,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Default)]
struct SinkSeen {
    connections: usize,
    /// The head (request line and headers, lower-cased) of every request that arrived.
    heads: Vec<String>,
}

impl Drop for FakeSink {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeSink {
    pub async fn start() -> FakeSink {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = format!("http://{}", listener.local_addr().expect("addr"));
        let seen = Arc::new(Mutex::new(SinkSeen::default()));
        let s2 = seen.clone();
        let task = tokio::spawn(async move {
            let mut conns = tokio::task::JoinSet::new();
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                s2.lock_().connections += 1;
                let s3 = s2.clone();
                conns.spawn(async move {
                    let mut buf = vec![0u8; 16 << 10];
                    let mut n = 0;
                    while n < buf.len() {
                        match sock.read(&mut buf[n..]).await {
                            Ok(0) | Err(_) => break,
                            Ok(k) => n += k,
                        }
                        if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    if n > 0 {
                        s3.lock_()
                            .heads
                            .push(String::from_utf8_lossy(&buf[..n]).to_lowercase());
                        let _ = sock
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                            .await;
                    }
                });
            }
        });
        FakeSink { addr, seen, task }
    }
    /// Connections accepted (a connection that sent nothing counts).
    pub fn connections(&self) -> usize {
        self.seen.lock_().connections
    }
    /// Requests that arrived, as lower-cased heads.
    pub fn requests(&self) -> Vec<String> {
        self.seen.lock_().heads.clone()
    }
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
        let addr = format!("http://{}", listener.local_addr().expect("addr"));
        let shared = Arc::new(Shared {
            objects: Mutex::new(HashMap::new()),
            plan: Mutex::new(Plan::default()),
            gets: AtomicUsize::new(0),
            key_gets: Mutex::new(HashMap::new()),
            log: Mutex::new(Vec::new()),
            t0: Instant::now(),
            require_key: Mutex::new(None),
            list_page: AtomicUsize::new(1000),
        });
        let sh = shared.clone();
        let task = tokio::spawn(async move {
            let mut conns = tokio::task::JoinSet::new(); // dropped with this task: every connection handler is aborted
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    break;
                };
                let _ = sock.set_nodelay(true); // head and body are separate writes: without this, Nagle + delayed ACK add ~40 ms per request
                let sh = sh.clone();
                conns.spawn(async move {
                    let _ = serve(sock, sh).await;
                });
            }
        });
        FakeS3 {
            addr,
            bucket: bucket.to_string(),
            shared,
            task,
        }
    }

    pub fn put(&self, key: &str, bytes: Vec<u8>) {
        let etag = etag_of(&bytes);
        self.shared.objects.lock_().insert(
            key.to_string(),
            Obj {
                bytes: Arc::new(bytes),
                etag,
                version: None,
            },
        );
    }
    pub fn put_versioned(&self, key: &str, bytes: Vec<u8>, version: &str) {
        let etag = etag_of(&bytes);
        self.shared.objects.lock_().insert(
            key.to_string(),
            Obj {
                bytes: Arc::new(bytes),
                etag,
                version: Some(version.to_string()),
            },
        );
    }
    pub fn set_plan(&self, plan: Plan) {
        *self.shared.plan.lock_() = plan;
        self.shared.gets.store(0, SeqCst);
        self.shared.key_gets.lock_().clear();
        self.shared.log.lock_().clear();
    }
    /// Answer 403 to every request not signed by this access key id.
    pub fn require_access_key(&self, key: &str) {
        *self.shared.require_key.lock_() = Some(key.to_string());
    }
    /// Objects per LIST page (default 1000): small values exercise continuation tokens.
    pub fn set_list_page(&self, n: usize) {
        self.shared.list_page.store(n.max(1), SeqCst);
    }
    pub fn remove(&self, key: &str) {
        self.shared.objects.lock_().remove(key);
    }
    pub fn clear_log(&self) {
        self.shared.log.lock_().clear();
    }
    pub fn log(&self) -> Vec<ReqLog> {
        self.shared.log.lock_().clone()
    }
    pub fn gets(&self) -> usize {
        self.shared
            .log
            .lock_()
            .iter()
            .filter(|r| r.method == "GET")
            .count()
    }
    pub fn heads(&self) -> usize {
        self.shared
            .log
            .lock_()
            .iter()
            .filter(|r| r.method == "HEAD")
            .count()
    }
    pub fn etag(&self, key: &str) -> Option<String> {
        self.shared.objects.lock_().get(key).map(|o| o.etag.clone())
    }
}

struct Req {
    method: String,
    key: String,
    range: Option<(u64, u64)>,
    if_match: Option<String>,
    version_id: Option<String>,
    query: String,
    auth_key: Option<String>,
    session_token: bool,
    bearer: bool,
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 3 <= b.len()
            && let Some(v) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        kv.strip_prefix(name)
            .and_then(|r| r.strip_prefix('='))
            .map(percent_decode)
    })
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
            let (path, query) = target
                .split_once('?')
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .unwrap_or((target.clone(), String::new()));
            let key = percent_decode(
                path.trim_start_matches('/')
                    .split_once('/')
                    .map(|(_, k)| k)
                    .unwrap_or(""),
            );
            let mut range = None;
            let mut if_match = None;
            let mut auth_key = None;
            let mut session_token = false;
            let mut bearer = false;
            for l in lines {
                let Some((k, v)) = l.split_once(':') else {
                    continue;
                };
                let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
                if k == "range" {
                    if let Some(r) = v.strip_prefix("bytes=")
                        && let Some((a, b)) = r.split_once('-')
                        && let (Ok(a), Ok(b)) = (a.parse::<u64>(), b.parse::<u64>())
                    {
                        range = Some((a, b));
                    }
                } else if k == "if-match" {
                    if_match = Some(v.to_string());
                } else if k == "authorization" {
                    bearer = v.starts_with("Bearer ");
                    auth_key = v
                        .split("Credential=")
                        .nth(1)
                        .and_then(|c| c.split('/').next())
                        .map(str::to_string);
                } else if k == "x-amz-security-token" {
                    session_token = true;
                }
            }
            let version_id =
                query_param(&query, "versionId").or_else(|| query_param(&query, "generation"));
            return Ok(Some(Req {
                method,
                key,
                range,
                if_match,
                version_id,
                query,
                auth_key,
                session_token,
                bearer,
            }));
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
        301 => "301 Moved Permanently",
        302 => "302 Found",
        307 => "307 Temporary Redirect",
        308 => "308 Permanent Redirect",
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
        let Some(req) = read_head(&mut sock, &mut buf).await? else {
            return Ok(());
        };
        let is_get = req.method == "GET";
        let ordinal = if is_get {
            sh.gets.fetch_add(1, SeqCst)
        } else {
            0
        };
        let key_ordinal = if is_get {
            let mut g = sh.key_gets.lock_();
            let c = g.entry(req.key.clone()).or_insert(0);
            *c += 1;
            *c - 1
        } else {
            0
        };
        // the plan, and any object swap scheduled before this GET
        let (fault, swaps) = {
            let plan = sh.plan.lock_();
            let swaps: Vec<_> = plan
                .swap_before_get
                .iter()
                .filter(|(i, k, _)| is_get && *i == key_ordinal && *k == req.key)
                .cloned()
                .collect();
            let is_list = is_get && req.key.is_empty() && req.query.contains("list-type=2");
            let keyed = plan
                .by_key
                .iter()
                .find(|(k, _)| *k == req.key)
                .map(|(_, f)| f.clone());
            let f = if keyed.is_some() {
                keyed
            } else if !is_get {
                plan.head.clone()
            } else if is_list && plan.list.is_some() {
                plan.list.clone()
            } else if let Some(Some(f)) = plan.script.get(ordinal) {
                Some(f.clone())
            } else if ordinal < plan.script.len() {
                None
            } else if let Some(f) = &plan.sticky {
                Some(f.clone())
            } else if let Some((seed, permille, faults)) = &plan.seeded {
                let mut r = Rng::new(seed ^ (ordinal as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                if !faults.is_empty() && r.below(1000) < *permille as u64 {
                    Some(faults[r.below(faults.len() as u64) as usize].clone())
                } else {
                    None
                }
            } else {
                None
            };
            (f, swaps)
        };
        for (_, key, bytes) in swaps {
            let etag = etag_of(&bytes);
            sh.objects.lock_().insert(
                key,
                Obj {
                    bytes: Arc::new((*bytes).clone()),
                    etag,
                    version: None,
                },
            );
        }
        sh.log.lock_().push(ReqLog {
            at: sh.t0.elapsed(),
            method: req.method.clone(),
            key: req.key.clone(),
            range: req.range,
            if_match: req.if_match.clone(),
            fault: fault.as_ref().map(|f| format!("{f:?}")).unwrap_or_default(),
            ordinal,
            auth_key: req.auth_key.clone(),
            session_token: req.session_token,
            bearer: req.bearer,
            query: req.query.clone(),
        });
        let keep = respond(&mut sock, &sh, &req, fault).await?;
        if !keep {
            return Ok(());
        }
    }
}

/// Returns whether the connection may be reused.
async fn respond(
    sock: &mut TcpStream,
    sh: &Shared,
    req: &Req,
    fault: Option<Fault>,
) -> std::io::Result<bool> {
    let required = sh.require_key.lock_().clone();
    if let Some(want) = required
        && req.auth_key.as_deref() != Some(want.as_str())
    {
        sock.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
            .await?;
        return Ok(true);
    }
    if let Some(Fault::Redirect { code, location }) = &fault {
        let loc = location
            .as_ref()
            .map(|l| format!("Location: {l}/{}?{}\r\n", req.key, req.query))
            .unwrap_or_default();
        sock.write_all(
            format!(
                "HTTP/1.1 {}\r\n{loc}Content-Length: 0\r\n\r\n",
                status_line(*code)
            )
            .as_bytes(),
        )
        .await?;
        return Ok(true);
    }
    if req.method == "GET" && req.key.is_empty() && req.query.contains("list-type=2") {
        return respond_list(sock, sh, req, fault).await;
    }
    let obj = {
        let g = sh.objects.lock_();
        g.get(&req.key)
            .map(|o| (o.bytes.clone(), o.etag.clone(), o.version.clone()))
    };
    if let Some(Fault::Delay { ms }) = &fault {
        tokio::time::sleep(Duration::from_millis(*ms)).await;
    }
    match &fault {
        Some(Fault::Reset) => return Ok(false),
        Some(Fault::Raw { bytes }) => {
            let _ = sock.write_all(bytes).await;
            return Ok(false);
        }
        Some(Fault::StallHeaders) => {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            return Ok(false);
        }
        Some(Fault::Status {
            code,
            retry_after,
            body,
            endless,
        }) => {
            let ra = retry_after
                .map(|s| format!("Retry-After: {s}\r\n"))
                .unwrap_or_default();
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
            let _ = sock
                .write_all(
                    format!("HTTP/1.1 200 OK\r\nX-Junk: {junk}\r\nContent-Length: 0\r\n\r\n")
                        .as_bytes(),
                )
                .await;
            return Ok(false);
        }
        _ => {}
    }
    let Some((bytes, etag, version)) = obj else {
        sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
            .await?;
        return Ok(true);
    };
    if let (Some(want), None) = (&req.version_id, &fault)
        && version.as_deref() != Some(want.as_str())
    {
        sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
            .await?;
        return Ok(true);
    }
    let size = bytes.len() as u64;
    let vh = version
        .as_ref()
        .map(|v| format!("x-amz-version-id: {v}\r\nx-goog-generation: {v}\r\n"))
        .unwrap_or_default();
    let base = format!("ETag: {etag}\r\nLast-Modified: {HDATE}\r\nAccept-Ranges: bytes\r\n{vh}");
    if req.method == "HEAD" {
        sock.write_all(
            format!("HTTP/1.1 200 OK\r\n{base}Content-Length: {size}\r\n\r\n").as_bytes(),
        )
        .await?;
        return Ok(true);
    }
    if let Some(m) = &req.if_match
        && !matches!(fault, Some(Fault::IgnoreIfMatch))
        && m != "*"
        && !m.split(',').any(|x| x.trim() == etag)
    {
        sock.write_all(b"HTTP/1.1 412 Precondition Failed\r\nContent-Length: 0\r\n\r\n")
            .await?;
        return Ok(true);
    }
    let Some((a, b)) = req.range else {
        sock.write_all(
            format!("HTTP/1.1 200 OK\r\n{base}Content-Length: {size}\r\n\r\n").as_bytes(),
        )
        .await?;
        sock.write_all(&bytes).await?;
        return Ok(true);
    };
    let b = b.min(size.saturating_sub(1));
    if a > b {
        sock.write_all(format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{size}\r\nContent-Length: 0\r\n\r\n").as_bytes()).await?;
        return Ok(true);
    }
    let want = &bytes[a as usize..=b as usize];
    let partial = |s: u64, e: u64, len: u64| {
        format!(
            "HTTP/1.1 206 Partial Content\r\n{base}Content-Range: bytes {s}-{e}/{size}\r\nContent-Length: {len}\r\n\r\n"
        )
    };
    match fault {
        Some(Fault::IgnoreRange) => {
            sock.write_all(
                format!("HTTP/1.1 200 OK\r\n{base}Content-Length: {size}\r\n\r\n").as_bytes(),
            )
            .await?;
            sock.write_all(&bytes).await?;
            Ok(true)
        }
        Some(Fault::RangeClaim {
            start_delta,
            end_delta,
        }) => {
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
            sock.write_all(partial(a, b, want.len() as u64).as_bytes())
                .await?;
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
            sock.write_all(partial(a, b, want.len() as u64).as_bytes())
                .await?;
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
            sock.write_all(partial(a, b, want.len() as u64).as_bytes())
                .await?;
            sock.write_all(want).await?;
            Ok(true)
        }
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// `GET /bucket?list-type=2&prefix=..&continuation-token=..&max-keys=..` (the S3 ListObjectsV2 XML, which the GCS XML API shares).
async fn respond_list(
    sock: &mut TcpStream,
    sh: &Shared,
    req: &Req,
    fault: Option<Fault>,
) -> std::io::Result<bool> {
    match &fault {
        Some(Fault::EndlessSuccess { claim }) => {
            sock.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: {claim}\r\n\r\n").as_bytes()).await?;
            endless_body(sock).await;
            return Ok(false);
        }
        Some(Fault::Document { body }) => {
            sock.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes()).await?;
            sock.write_all(body).await?;
            return Ok(true);
        }
        Some(Fault::Status {
            code,
            retry_after,
            body,
            endless,
        }) => {
            let ra = retry_after
                .map(|s| format!("Retry-After: {s}\r\n"))
                .unwrap_or_default();
            let cl = if *endless { u64::MAX / 4 } else { *body as u64 };
            sock.write_all(
                format!(
                    "HTTP/1.1 {}\r\n{ra}Content-Length: {cl}\r\n\r\n",
                    status_line(*code)
                )
                .as_bytes(),
            )
            .await?;
            if *endless {
                endless_body(sock).await;
                return Ok(false);
            }
            sock.write_all(&vec![b'x'; *body]).await?;
            return Ok(true);
        }
        Some(Fault::Reset) => return Ok(false),
        Some(Fault::Raw { bytes }) => {
            let _ = sock.write_all(bytes).await;
            return Ok(false);
        }
        _ => {}
    }
    let prefix = query_param(&req.query, "prefix").unwrap_or_default();
    let after = query_param(&req.query, "continuation-token")
        .and_then(|t| t.parse::<usize>().ok())
        .unwrap_or(0);
    let page = sh.list_page.load(SeqCst).min(
        query_param(&req.query, "max-keys")
            .and_then(|m| m.parse().ok())
            .unwrap_or(usize::MAX),
    );
    let mut keys: Vec<(String, usize, String)> = sh
        .objects
        .lock_()
        .iter()
        .filter(|(k, _)| k.starts_with(&prefix))
        .map(|(k, o)| (k.clone(), o.bytes.len(), o.etag.clone()))
        .collect();
    keys.sort();
    let slice: Vec<_> = keys.iter().skip(after).take(page).collect();
    let truncated = after + slice.len() < keys.len();
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><Name>b</Name>",
    );
    xml += &format!(
        "<Prefix>{}</Prefix><KeyCount>{}</KeyCount><MaxKeys>{page}</MaxKeys><IsTruncated>{truncated}</IsTruncated>",
        xml_escape(&prefix),
        slice.len()
    );
    for (k, size, etag) in &slice {
        xml += &format!(
            "<Contents><Key>{}</Key><LastModified>2015-10-21T07:28:00.000Z</LastModified><ETag>{}</ETag><Size>{size}</Size><StorageClass>STANDARD</StorageClass></Contents>",
            xml_escape(k),
            xml_escape(etag)
        );
    }
    if truncated {
        xml += &format!(
            "<NextContinuationToken>{}</NextContinuationToken>",
            after + slice.len()
        );
    }
    xml += "</ListBucketResult>";
    sock.write_all(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: {}\r\n\r\n",
            xml.len()
        )
        .as_bytes(),
    )
    .await?;
    sock.write_all(xml.as_bytes()).await?;
    Ok(true)
}

/// Zeros as fast as the peer reads, until it closes.
async fn endless_body(sock: &mut TcpStream) {
    let chunk = vec![0u8; 1 << 20];
    while sock.write_all(&chunk).await.is_ok() {}
}

// ------------------------------------------------------------------------------------------------ instance metadata

/// What the fake metadata service hands out.
#[derive(Clone, Debug)]
pub struct ImdsCreds {
    pub key_id: String,
    pub secret: String,
    pub token: Option<String>,
}

/// Which answer of the metadata service is hostile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImdsFault {
    /// `PUT /latest/api/token` answers 200 with an endless body.
    EndlessToken,
    /// The role-name listing answers 200 with an endless body.
    EndlessRoleList,
    /// The credentials JSON answers 200 with an endless body and a huge Content-Length.
    EndlessCredentials,
    /// The credentials JSON is this exact document.
    Credentials(Vec<u8>),
}

struct ImdsShared {
    creds: ImdsCreds,
    fault: Mutex<Option<ImdsFault>>,
    log: Mutex<Vec<String>>,
}

/// The EC2 instance metadata service (IMDSv2: a token PUT, the role listing, the role credentials), on loopback.
pub struct FakeImds {
    pub addr: String,
    shared: Arc<ImdsShared>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeImds {
    fn drop(&mut self) {
        self.task.abort();
    }
}

const IMDS_ROLE: &str = "fake-role";
const IMDS_TOKEN: &str = "fake-imds-session-token";

impl FakeImds {
    pub async fn start(creds: ImdsCreds) -> FakeImds {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = format!("http://{}", listener.local_addr().expect("addr"));
        let shared = Arc::new(ImdsShared {
            creds,
            fault: Mutex::new(None),
            log: Mutex::new(Vec::new()),
        });
        let sh = shared.clone();
        let task = tokio::spawn(async move {
            let mut conns = tokio::task::JoinSet::new();
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    break;
                };
                let _ = sock.set_nodelay(true);
                let sh = sh.clone();
                conns.spawn(async move {
                    let _ = serve_imds(sock, sh).await;
                });
            }
        });
        FakeImds { addr, shared, task }
    }

    pub fn set_fault(&self, f: Option<ImdsFault>) {
        *self.shared.fault.lock_() = f;
    }

    /// `METHOD path` of every request, in order.
    pub fn requests(&self) -> Vec<String> {
        self.shared.log.lock_().clone()
    }
}

async fn serve_imds(mut sock: TcpStream, sh: Arc<ImdsShared>) -> std::io::Result<()> {
    let mut buf = Vec::new();
    loop {
        // reuse the S3 request reader: the target is the path (its first segment is dropped as a "bucket" there, so read the line ourselves)
        let end = loop {
            if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break p;
            }
            if buf.len() > 64 * 1024 {
                return Ok(());
            }
            let mut tmp = [0u8; 4096];
            let n = sock.read(&mut tmp).await?;
            if n == 0 {
                return Ok(());
            }
            buf.extend_from_slice(&tmp[..n]);
        };
        let head = String::from_utf8_lossy(&buf[..end]).into_owned();
        buf.drain(..end + 4);
        let mut lines = head.split("\r\n");
        let mut first = lines.next().unwrap_or("").split(' ');
        let (method, path) = (
            first.next().unwrap_or("").to_string(),
            first.next().unwrap_or("").to_string(),
        );
        let token_ok = lines.any(|l| {
            l.to_ascii_lowercase()
                .starts_with("x-aws-ec2-metadata-token:")
                && l.ends_with(IMDS_TOKEN)
        });
        sh.log.lock_().push(format!("{method} {path}"));
        let fault = sh.fault.lock_().clone();
        let json = format!(
            "{{\"Code\":\"Success\",\"AccessKeyId\":\"{}\",\"SecretAccessKey\":\"{}\",{}\"Expiration\":\"2999-01-01T00:00:00Z\"}}",
            sh.creds.key_id,
            sh.creds.secret,
            sh.creds
                .token
                .as_ref()
                .map(|t| format!("\"Token\":\"{t}\","))
                .unwrap_or_default()
        );
        let (status, body): (&str, Option<Vec<u8>>) = match (method.as_str(), path.as_str()) {
            ("PUT", "/latest/api/token") => ("200 OK", Some(IMDS_TOKEN.as_bytes().to_vec())),
            (
                "GET",
                "/latest/meta-data/iam/security-credentials/"
                | "/latest/meta-data/iam/security-credentials",
            ) if token_ok => ("200 OK", Some(IMDS_ROLE.as_bytes().to_vec())),
            ("GET", p)
                if token_ok
                    && p == format!("/latest/meta-data/iam/security-credentials/{IMDS_ROLE}") =>
            {
                ("200 OK", Some(json.into_bytes()))
            }
            _ => ("401 Unauthorized", Some(Vec::new())),
        };
        let endless = matches!(
            (&fault, method.as_str(), path.as_str()),
            (Some(ImdsFault::EndlessToken), "PUT", _)
                | (
                    Some(ImdsFault::EndlessRoleList),
                    "GET",
                    "/latest/meta-data/iam/security-credentials/"
                )
                | (
                    Some(ImdsFault::EndlessCredentials),
                    "GET",
                    "/latest/meta-data/iam/security-credentials/fake-role"
                )
        );
        if endless {
            sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1099511627776\r\n\r\n")
                .await?;
            endless_body(&mut sock).await;
            return Ok(());
        }
        let body = match (&fault, path.as_str()) {
            (Some(ImdsFault::Credentials(doc)), p) if p.ends_with(IMDS_ROLE) => Some(doc.clone()),
            _ => body,
        };
        let body = body.unwrap_or_default();
        sock.write_all(
            format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await?;
        sock.write_all(&body).await?;
    }
}
