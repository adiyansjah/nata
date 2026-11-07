//! The thin wrapper of ours around `object_store` (ADR 0011): one place where every request is admitted, validated, retried, bounded,
//! counted and cancelled. `object_store`'s own retries are OFF (`max_retries: 0`), so exactly one layer owns the attempt count.
//!
//! What it does, per the ADR: at most `query_inflight` / `node_inflight` active attempts (a retry does not hold a permit while it sleeps);
//! response bytes reserved against a grant BEFORE the request is issued; every range request pinned to the identity (version/generation,
//! else ETag + length) with If-Match and a check of the response; the body checked chunk by chunk against the requested length (never an
//! unbounded `bytes()`); transient classes retried with equal-jitter exponential backoff, at most `max_attempts` attempts, a per-attempt
//! timeout over headers and body, an operation deadline over everything; permanent classes fail at once; cancellation interrupts permit
//! waits, requests and sleeps. Errors carry a class and counts, never the dependency's text (it can hold an endpoint, a key or a body).

use std::error::Error as StdError;
use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures::StreamExt;
use object_store::client::{HttpClient, HttpConnector, HttpError, HttpErrorKind, HttpRequest, HttpResponse, HttpResponseBody, HttpService, ReqwestConnector};
use object_store::path::Path;
use object_store::{ClientOptions, Error as OsError, GetOptions, GetRange, ObjectStore};
use tokio::sync::{Notify, Semaphore};

use crate::policy::{Config, Mutants};
use crate::util::Rng;

const GRANT_UNIT: u64 = 4096;

// ------------------------------------------------------------------------------------------------ errors

/// What went wrong, as a class. `retryable()` is the ADR 0011 table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// 429 / SlowDown.
    Throttled,
    /// 500..=599.
    Server5xx,
    /// Per-attempt timeout (stalled headers or body), or a transport timeout.
    Timeout,
    /// Connect failure, reset, closed connection.
    Transport,
    /// The transport ended the body early (incomplete message).
    Truncated,
    /// 401/403.
    Auth,
    NotFound,
    /// 412, or a response whose size/ETag/version is not the pinned one.
    IdentityChanged,
    /// A length that disagrees: body longer/shorter than the range, Content-Length vs Content-Range.
    BadLength,
    /// The response is not the range asked for (Range ignored, wrong Content-Range, 416).
    BadRange,
    /// Anything else malformed (undecodable headers or body framing, unexpected status).
    BadResponse,
    /// The object has neither a version nor an ETag: it cannot be pinned.
    NoValidator,
    FooterTooLarge,
    BadFooter,
    /// A response would not fit the grant at all.
    GrantTooSmall,
    Cancelled,
    DeadlineExceeded,
    AttemptsExhausted,
    Other,
}

impl Class {
    pub fn retryable(self) -> bool {
        matches!(self, Class::Throttled | Class::Server5xx | Class::Timeout | Class::Transport | Class::Truncated)
    }
}

#[derive(Clone, Debug)]
pub struct ReadError {
    pub class: Class,
    pub attempts: u32,
    pub elapsed: Duration,
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "object read failed: class={:?} attempts={} elapsed_ms={}", self.class, self.attempts, self.elapsed.as_millis())
    }
}
impl StdError for ReadError {}

#[derive(Debug)]
struct Fail {
    class: Class,
    retry_after: Option<Duration>,
}

fn fail(class: Class) -> Fail {
    Fail { class, retry_after: None }
}

fn classify(e: &OsError) -> Fail {
    match e {
        OsError::NotFound { .. } => fail(Class::NotFound),
        OsError::Precondition { .. } => fail(Class::IdentityChanged),
        OsError::PermissionDenied { .. } | OsError::Unauthenticated { .. } => fail(Class::Auth),
        OsError::Generic { source, .. } => classify_chain(source.as_ref()),
        _ => fail(Class::Other),
    }
}

fn classify_chain(src: &(dyn StdError + 'static)) -> Fail {
    // transport failures first: the chain holds an HttpError with a kind
    let mut cur: Option<&(dyn StdError + 'static)> = Some(src);
    while let Some(e) = cur {
        if let Some(h) = e.downcast_ref::<HttpError>() {
            return classify_http(h);
        }
        cur = e.source();
    }
    // a status error: `RetryError` is not exported by object_store 0.14, so the status comes from the text. Our transport (BoundedConnector)
    // puts "nata-status=<code>;ra=<seconds>" in the bounded error body; the plain-transport mutant falls back to the crate's own wording.
    parse_status(&src.to_string())
}

fn digits_after(text: &str, marker: &str) -> Option<u64> {
    let i = text.find(marker)? + marker.len();
    let d: String = text[i..].chars().take_while(|c| c.is_ascii_digit()).collect();
    d.parse().ok()
}

fn parse_status(text: &str) -> Fail {
    let code = digits_after(text, "nata-status=").or_else(|| digits_after(text, "status code: ")).unwrap_or(0);
    let retry_after = digits_after(text, ";ra=").map(Duration::from_secs);
    let class = match code {
        429 => Class::Throttled,
        408 => Class::Timeout,
        500..=599 => Class::Server5xx,
        401 | 403 => Class::Auth,
        404 => Class::NotFound,
        412 => Class::IdentityChanged,
        416 => Class::BadRange,
        _ => Class::BadResponse,
    };
    Fail { class, retry_after }
}

fn classify_http(h: &HttpError) -> Fail {
    match h.kind() {
        HttpErrorKind::Timeout => fail(Class::Timeout),
        HttpErrorKind::Connect | HttpErrorKind::Interrupted => fail(Class::Transport),
        HttpErrorKind::Request => fail(Class::Truncated),
        _ => fail(Class::BadResponse),
    }
}

// ------------------------------------------------------------------------------------------------ transport bound

/// Wraps the reqwest transport of `object_store`: a non-success response keeps at most `cap` bytes of its body (the client crate would
/// otherwise buffer the whole error body, unbounded, to put it in an error message), and the body is replaced by a short marker that
/// carries only a valid Retry-After. Success bodies stay streams; the wrapper bounds them chunk by chunk.
#[derive(Debug)]
pub struct BoundedConnector {
    pub cap: usize,
}

impl HttpConnector for BoundedConnector {
    fn connect(&self, options: &ClientOptions) -> object_store::Result<HttpClient> {
        let inner = ReqwestConnector::default().connect(options)?;
        Ok(HttpClient::new(BoundedService { inner, cap: self.cap }))
    }
}

#[derive(Debug)]
struct BoundedService {
    inner: HttpClient,
    cap: usize,
}

impl HttpService for BoundedService {
    // The desugared form of `#[async_trait] async fn call(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>` (no extra crate).
    fn call<'a, 'b>(&'a self, req: HttpRequest) -> Pin<Box<dyn Future<Output = Result<HttpResponse, HttpError>> + Send + 'b>>
    where
        'a: 'b,
        Self: 'b,
    {
        Box::pin(async move {
            let resp = self.inner.execute(req).await?;
            if resp.status().is_success() {
                return Ok(resp);
            }
            let (parts, body) = resp.into_parts();
            let mut seen = 0usize;
            let mut stream = body.bytes_stream();
            while seen < self.cap {
                match stream.next().await {
                    Some(Ok(c)) => seen += c.len(),
                    _ => break,
                }
            }
            drop(stream); // the rest of the body is never read
            let ra = parts.headers.get("retry-after").and_then(|v| v.to_str().ok()).and_then(|s| s.trim().parse::<u64>().ok());
            let marker = match ra {
                Some(s) => format!("nata-status={};ra={s}", parts.status.as_u16()),
                None => format!("nata-status={}", parts.status.as_u16()),
            };
            Ok(HttpResponse::from_parts(parts, HttpResponseBody::from(marker)))
        })
    }
}

// ------------------------------------------------------------------------------------------------ store construction

#[derive(Clone, Debug)]
pub struct S3Spec {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub key: Option<String>,
    pub secret: Option<String>,
    pub allow_http: bool,
}

fn client_options(cfg: &Config, m: &Mutants) -> ClientOptions {
    let mut co = ClientOptions::new().with_pool_max_idle_per_host(64).with_connect_timeout(Duration::from_secs(5));
    // The per-attempt timeout is ours (it must be able to fail as a mutant); the client's own timeout is only a far backstop.
    co = if m.no_attempt_timeout { co.with_timeout_disabled() } else { co.with_timeout(cfg.op_deadline + Duration::from_secs(10)) };
    co
}

pub fn build_s3(spec: &S3Spec, cfg: &Config, m: &Mutants) -> Result<Arc<dyn ObjectStore>, String> {
    use object_store::aws::AmazonS3Builder;
    let mut co = client_options(cfg, m).with_allow_http(spec.allow_http);
    if spec.allow_http {
        co = co.with_http1_only();
    }
    let mut b = AmazonS3Builder::new()
        .with_endpoint(&spec.endpoint)
        .with_bucket_name(&spec.bucket)
        .with_region(&spec.region)
        .with_virtual_hosted_style_request(false)
        .with_allow_http(spec.allow_http)
        .with_client_options(co)
        .with_retry(no_retry());
    match (&spec.key, &spec.secret) {
        (Some(k), Some(s)) => b = b.with_access_key_id(k).with_secret_access_key(s),
        _ => b = b.with_skip_signature(true),
    }
    if !m.no_transport_bound {
        b = b.with_http_connector(BoundedConnector { cap: cfg.error_body_cap });
    }
    b.build().map(|s| Arc::new(s) as Arc<dyn ObjectStore>).map_err(|_| "cannot build the S3 store (details withheld)".to_string())
}

pub fn build_gcs(bucket: &str, cfg: &Config, m: &Mutants) -> Result<Arc<dyn ObjectStore>, String> {
    use object_store::gcp::GoogleCloudStorageBuilder;
    let mut b = GoogleCloudStorageBuilder::new().with_bucket_name(bucket).with_client_options(client_options(cfg, m)).with_retry(no_retry());
    if !m.no_transport_bound {
        b = b.with_http_connector(BoundedConnector { cap: cfg.error_body_cap });
    }
    b.build().map(|s| Arc::new(s) as Arc<dyn ObjectStore>).map_err(|_| "cannot build the GCS store (details withheld)".to_string())
}

fn no_retry() -> object_store::RetryConfig {
    object_store::RetryConfig { backoff: object_store::BackoffConfig::default(), max_retries: 0, retry_timeout: Duration::from_secs(1) }
}

// ------------------------------------------------------------------------------------------------ counters

#[derive(Default, Debug)]
pub struct Gauge {
    cur: AtomicUsize,
    peak: AtomicUsize,
}

impl Gauge {
    fn up(&self) {
        let c = self.cur.fetch_add(1, SeqCst) + 1;
        self.peak.fetch_max(c, SeqCst);
    }
    fn down(&self) {
        self.cur.fetch_sub(1, SeqCst);
    }
    pub fn cur(&self) -> usize {
        self.cur.load(SeqCst)
    }
    pub fn peak(&self) -> usize {
        self.peak.load(SeqCst)
    }
}

struct GaugeGuard<'a>(&'a Gauge);
impl Drop for GaugeGuard<'_> {
    fn drop(&mut self) {
        self.0.down();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Head = 0,
    Footer = 1,
    Data = 2,
}

#[derive(Default, Debug)]
pub struct Stats {
    /// Attempts (every try counts, retries included), by kind.
    pub requests: [AtomicU64; 3],
    /// Body bytes received on the wire (failed attempts included), by kind.
    pub wire_bytes: [AtomicU64; 3],
    /// Bytes handed to callers (successful attempts), by kind.
    pub ok_bytes: [AtomicU64; 3],
    pub retries: AtomicU64,
    pub inflight: Gauge,
    pub grant_in_use: Gauge, // in GRANT_UNIT units; peak is what matters
    /// (first-byte ms, total ms, kind, ok) per attempt.
    pub recs: Mutex<Vec<(f64, f64, u8, bool)>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snap {
    pub requests: [u64; 3],
    pub wire_bytes: [u64; 3],
    pub ok_bytes: [u64; 3],
    pub retries: u64,
}

impl Stats {
    pub fn snap(&self) -> Snap {
        let l = |a: &[AtomicU64; 3]| [a[0].load(SeqCst), a[1].load(SeqCst), a[2].load(SeqCst)];
        Snap { requests: l(&self.requests), wire_bytes: l(&self.wire_bytes), ok_bytes: l(&self.ok_bytes), retries: self.retries.load(SeqCst) }
    }
}

impl Snap {
    pub fn minus(&self, o: &Snap) -> Snap {
        let d = |a: [u64; 3], b: [u64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        Snap { requests: d(self.requests, o.requests), wire_bytes: d(self.wire_bytes, o.wire_bytes), ok_bytes: d(self.ok_bytes, o.ok_bytes), retries: self.retries - o.retries }
    }
}

// ------------------------------------------------------------------------------------------------ cancellation

#[derive(Default, Debug)]
pub struct CancelToken {
    flag: AtomicBool,
    notify: Notify,
}

impl CancelToken {
    pub fn new() -> Arc<CancelToken> {
        Arc::new(CancelToken::default())
    }
    pub fn cancel(&self) {
        self.flag.store(true, SeqCst);
        self.notify.notify_waiters();
    }
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(SeqCst)
    }
    pub async fn cancelled(&self) {
        loop {
            let n = self.notify.notified(); // created before the check: no lost wake
            if self.is_cancelled() {
                return;
            }
            n.await;
        }
    }
}

// ------------------------------------------------------------------------------------------------ the wrapper

/// The pinned identity of an object: version/generation preferred, else ETag; the length always.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub size: u64,
    pub etag: Option<String>,
    pub version: Option<String>,
}

impl Identity {
    /// Cache-key part: the version, or the opaque ETag and the length.
    pub fn token(&self) -> String {
        match (&self.version, &self.etag) {
            (Some(v), _) => format!("v:{v}:{}", self.size),
            (None, Some(e)) => format!("e:{e}:{}", self.size),
            (None, None) => String::new(),
        }
    }
}

pub struct Remote {
    pub store: Arc<dyn ObjectStore>,
    pub cfg: Config,
    pub mutants: Mutants,
    pub stats: Stats,
    node_sem: Semaphore,
    grant: Semaphore,
    grant_units_total: u64,
    seed: u64,
    ops: AtomicU64,
}

impl Remote {
    pub fn new(store: Arc<dyn ObjectStore>, cfg: Config, mutants: Mutants, seed: u64) -> Arc<Remote> {
        let units = cfg.grant_bytes / GRANT_UNIT;
        Arc::new(Remote {
            store,
            node_sem: Semaphore::new(cfg.node_inflight),
            grant: Semaphore::new(units as usize),
            grant_units_total: units,
            cfg,
            mutants,
            stats: Stats::default(),
            seed,
            ops: AtomicU64::new(0),
        })
    }

    /// A query: its own permit pool (fair FIFO), cancel token and optional deadline.
    pub fn query(self: &Arc<Self>) -> Arc<Query> {
        self.query_with_deadline(None)
    }

    pub fn query_with_deadline(self: &Arc<Self>, deadline: Option<Instant>) -> Arc<Query> {
        Arc::new(Query { remote: self.clone(), sem: Semaphore::new(self.cfg.query_inflight), cancel: CancelToken::new(), deadline })
    }
}

pub struct Query {
    pub remote: Arc<Remote>,
    sem: Semaphore,
    pub cancel: Arc<CancelToken>,
    deadline: Option<Instant>,
}

impl Query {
    fn cfg(&self) -> &Config {
        &self.remote.cfg
    }

    /// Run `attempt` under the retry policy. `attempt(n)` makes try number n (1-based) and must acquire its own permits.
    async fn run<T, F, Fut>(&self, mut attempt: F) -> Result<T, ReadError>
    where
        F: FnMut(u32) -> Fut,
        Fut: Future<Output = Result<T, Fail>>,
    {
        let m = &self.remote.mutants;
        let cfg = self.cfg();
        let start = Instant::now();
        let mut deadline = start + cfg.op_deadline;
        if let Some(q) = self.deadline {
            deadline = deadline.min(q);
        }
        let attempts = Arc::new(AtomicU32::new(0));
        let mut rng = Rng::new(self.remote.seed ^ self.remote.ops.fetch_add(1, SeqCst).wrapping_mul(0x9E37_79B9));
        let err = |class: Class, attempts: &AtomicU32| ReadError { class, attempts: attempts.load(SeqCst), elapsed: start.elapsed() };

        let body = async {
            loop {
                let n = attempts.fetch_add(1, SeqCst) + 1;
                let one = attempt(n);
                let r: Result<T, Fail> = if m.no_attempt_timeout {
                    tokio::select! { r = one => r, _ = self.cancel.cancelled() => Err(fail(Class::Cancelled)) }
                } else {
                    tokio::select! {
                        r = tokio::time::timeout(cfg.attempt_timeout, one) => r.unwrap_or_else(|_| Err(fail(Class::Timeout))),
                        _ = self.cancel.cancelled() => Err(fail(Class::Cancelled)),
                    }
                };
                let f = match r {
                    Ok(v) => return Ok(v),
                    Err(f) => f,
                };
                if f.class == Class::Cancelled {
                    return Err(err(Class::Cancelled, &attempts));
                }
                let retry = f.class.retryable() || m.retry_permanent && f.class != Class::Cancelled;
                if !retry {
                    return Err(err(f.class, &attempts));
                }
                if n >= cfg.max_attempts && !m.no_attempt_cap {
                    return Err(err(f.class, &attempts));
                }
                // backoff: equal jitter over the nominal interval; a valid Retry-After replaces it when it fits the deadline
                let k = (n - 1).min(20);
                let nominal = cfg.backoff_initial.saturating_mul(1u32 << k).min(cfg.backoff_max);
                let mut delay = nominal / 2 + nominal.mul_f64(0.5 * rng.unit());
                if let (Some(ra), false) = (f.retry_after, m.ignore_retry_after) {
                    if Instant::now() + ra >= deadline {
                        return Err(err(f.class, &attempts)); // the server asks for longer than we may wait: no further attempt
                    }
                    delay = delay.max(ra);
                }
                if !m.no_deadline && Instant::now() + delay >= deadline {
                    return Err(err(Class::DeadlineExceeded, &attempts));
                }
                self.remote.stats.retries.fetch_add(1, SeqCst);
                tokio::select! { _ = tokio::time::sleep(delay) => {}, _ = self.cancel.cancelled() => return Err(err(Class::Cancelled, &attempts)) }
            }
        };
        if m.no_deadline {
            body.await
        } else {
            match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), body).await {
                Ok(r) => r,
                Err(_) => Err(err(Class::DeadlineExceeded, &attempts)),
            }
        }
    }

    async fn acquire<'a>(&self, sem: &'a Semaphore, n: u32) -> Result<tokio::sync::SemaphorePermit<'a>, Fail> {
        tokio::select! {
            p = sem.acquire_many(n) => p.map_err(|_| fail(Class::Other)),
            _ = self.cancel.cancelled() => Err(fail(Class::Cancelled)),
        }
    }

    /// The identity lookup: one HEAD (counted as Kind::Head).
    pub async fn head(&self, path: &Path) -> Result<Identity, ReadError> {
        let r = self
            .run(|_| async {
                let _q = self.acquire(&self.sem, 1).await?;
                let _n = self.acquire(&self.remote.node_sem, 1).await?;
                self.remote.stats.inflight.up();
                let _g = GaugeGuard(&self.remote.stats.inflight);
                let t0 = Instant::now();
                self.remote.stats.requests[Kind::Head as usize].fetch_add(1, SeqCst);
                let res = self.remote.store.get_opts(path, GetOptions { head: true, ..Default::default() }).await;
                let ms = t0.elapsed().as_secs_f64() * 1e3;
                self.remote.stats.recs.lock().unwrap().push((ms, ms, Kind::Head as u8, res.is_ok()));
                let meta = res.map_err(|e| classify(&e))?.meta;
                if meta.e_tag.is_none() && meta.version.is_none() && !self.remote.mutants.no_version_check {
                    return Err(fail(Class::NoValidator));
                }
                Ok(Identity { size: meta.size, etag: meta.e_tag, version: meta.version })
            })
            .await;
        match r {
            Err(_) if self.remote.mutants.swallow_error => Ok(Identity { size: 0, etag: None, version: None }),
            r => r,
        }
    }

    /// One logical range (at most `merge_max` bytes), pinned to `id`, validated, retried per the policy.
    pub async fn fetch_range(&self, path: &Path, id: &Identity, range: Range<u64>, kind: Kind) -> Result<Bytes, ReadError> {
        let r = self.fetch_range_inner(path, id, range.clone(), kind).await;
        match r {
            Err(_) if self.remote.mutants.swallow_error => Ok(Bytes::from(vec![0u8; (range.end - range.start) as usize])),
            r => r,
        }
    }

    async fn fetch_range_inner(&self, path: &Path, id: &Identity, range: Range<u64>, kind: Kind) -> Result<Bytes, ReadError> {
        let t0 = Instant::now();
        let cfg = self.cfg().clone();
        let bad = |class: Class| ReadError { class, attempts: 0, elapsed: t0.elapsed() };
        if range.start >= range.end || range.end > id.size {
            return Err(bad(Class::BadRange)); // checked half-open range within the pinned object length
        }
        let len = range.end - range.start;
        if len > cfg.merge_max {
            return Err(bad(Class::BadRange));
        }
        let units = len.div_ceil(GRANT_UNIT);
        if units > self.remote.grant_units_total {
            return Err(bad(Class::GrantTooSmall)); // the minimum cannot fit: a bounded error, not a hang
        }
        let m = self.remote.mutants.clone();
        // keep the bytes of earlier failed attempts only for the append_on_retry mutant
        let carried: Mutex<Vec<u8>> = Mutex::new(Vec::new());
        self.run(|_attempt| {
            let carried = &carried;
            let m = m.clone();
            let range = range.clone();
            async move {
                // 1. grant first (before any request permit: a permit holder never waits for a grant, so there is no cycle)
                let _grant = self.acquire(&self.remote.grant, units as u32).await?;
                // 2. permits; none is held while the retry loop sleeps
                let _q = self.acquire(&self.sem, 1).await?;
                let _n = self.acquire(&self.remote.node_sem, 1).await?;
                self.remote.stats.inflight.up();
                let _gg = GaugeGuard(&self.remote.stats.inflight);
                let ki = kind as usize;
                self.remote.stats.requests[ki].fetch_add(1, SeqCst);
                let t0 = Instant::now();
                let mut opts = GetOptions { range: Some(GetRange::Bounded(range.clone())), ..Default::default() };
                if !m.no_version_check {
                    opts.if_match = id.etag.clone();
                    opts.version = id.version.clone();
                }
                let rec = |fb: f64, ok: bool| self.remote.stats.recs.lock().unwrap().push((fb, t0.elapsed().as_secs_f64() * 1e3, ki as u8, ok));
                let res = match self.remote.store.get_opts(path, opts).await {
                    Ok(r) => r,
                    Err(e) => {
                        rec(t0.elapsed().as_secs_f64() * 1e3, false);
                        return Err(classify(&e));
                    }
                };
                let first_byte = t0.elapsed().as_secs_f64() * 1e3;
                if !m.no_version_check {
                    let meta = &res.meta;
                    if meta.size != id.size
                        || id.version.is_some() && meta.version.is_some() && meta.version != id.version
                        || id.etag.is_some() && meta.e_tag != id.etag
                    {
                        rec(first_byte, false);
                        return Err(fail(Class::IdentityChanged));
                    }
                }
                if !m.no_length_check && res.range != range {
                    rec(first_byte, false);
                    return Err(fail(Class::BadRange));
                }
                let mut stream = res.into_stream();
                let mut buf: Vec<u8> = if m.append_on_retry { std::mem::take(&mut *carried.lock().unwrap()) } else { Vec::new() };
                let base = buf.len();
                if m.no_length_check {
                    // the known-bad version grows with whatever the server sends
                } else {
                    buf.reserve_exact(len as usize);
                }
                while let Some(chunk) = stream.next().await {
                    let chunk = match chunk {
                        Ok(c) => c,
                        Err(e) => {
                            self.remote.stats.wire_bytes[ki].fetch_add((buf.len() - base) as u64, SeqCst);
                            if m.append_on_retry {
                                *carried.lock().unwrap() = buf;
                            }
                            rec(first_byte, false);
                            return Err(classify_body(&e));
                        }
                    };
                    if !m.no_length_check && (buf.len() - base) as u64 + chunk.len() as u64 > len {
                        self.remote.stats.wire_bytes[ki].fetch_add((buf.len() - base + chunk.len()) as u64, SeqCst);
                        rec(first_byte, false);
                        return Err(fail(Class::BadLength));
                    }
                    buf.extend_from_slice(&chunk);
                }
                self.remote.stats.wire_bytes[ki].fetch_add((buf.len() - base) as u64, SeqCst);
                if !m.no_length_check && (buf.len() - base) as u64 != len {
                    rec(first_byte, false);
                    return Err(fail(Class::BadLength)); // a clean end of body that is short: the server's framing agrees, its Content-Range does not
                }
                rec(first_byte, true);
                self.remote.stats.ok_bytes[ki].fetch_add(len, SeqCst);
                Ok(Bytes::from(buf))
            }
        })
        .await
    }

    /// Several needed ranges of one object: planned (merged or not), fetched concurrently under the permits, sliced back per needed range.
    pub async fn read_ranges(&self, path: &Path, id: &Identity, needed: &[Range<u64>], plan: &[crate::plan::Planned], kind: Kind) -> Result<Vec<Bytes>, ReadError> {
        let fetched: Vec<Result<Bytes, ReadError>> = futures::future::join_all(plan.iter().map(|p| self.fetch_range(path, id, p.fetch.clone(), kind))).await;
        let mut got = Vec::with_capacity(fetched.len());
        let mut first_err: Option<ReadError> = None;
        for f in fetched {
            match f {
                Ok(b) => got.push(b),
                Err(e) => {
                    // report the first failure (a deadline or cancel on a sibling is not the root cause)
                    if first_err.as_ref().is_none_or(|x| x.class == Class::Cancelled && e.class != Class::Cancelled) {
                        first_err = Some(e);
                    }
                    got.push(Bytes::new());
                }
            }
        }
        if let Some(e) = first_err {
            return Err(e);
        }
        let mut out = Vec::with_capacity(needed.len());
        for (i, r) in needed.iter().enumerate() {
            if r.start >= r.end {
                out.push(Bytes::new());
                continue;
            }
            let mut pieces: Vec<(usize, &Range<u64>)> = plan.iter().enumerate().filter(|(_, p)| p.covers.contains(&i)).map(|(j, p)| (j, &p.fetch)).collect();
            pieces.sort_by_key(|(_, f)| f.start);
            if let Some((j, f)) = pieces.iter().find(|(_, f)| f.start <= r.start && f.end >= r.end) {
                out.push(got[*j].slice((r.start - f.start) as usize..(r.end - f.start) as usize));
                continue;
            }
            // spans several fetches (a range larger than the response cap): assemble
            let mut v = Vec::with_capacity((r.end - r.start) as usize);
            let mut at = r.start;
            for (j, f) in pieces {
                if f.start <= at && f.end > at {
                    let to = f.end.min(r.end);
                    v.extend_from_slice(&got[j][(at - f.start) as usize..(to - f.start) as usize]);
                    at = to;
                }
            }
            if at != r.end {
                return Err(ReadError { class: Class::BadRange, attempts: 0, elapsed: Duration::ZERO });
            }
            out.push(Bytes::from(v));
        }
        Ok(out)
    }

    /// The Parquet footer metadata bytes (without the 8-byte tail): one suffix read, one more only if the footer is longer than the first
    /// read. The length is checked against the cap BEFORE any further allocation.
    pub async fn footer(&self, path: &Path, id: &Identity) -> Result<Bytes, ReadError> {
        let bad = |class: Class| ReadError { class, attempts: 0, elapsed: Duration::ZERO };
        if id.size < 12 {
            return Err(bad(Class::BadFooter));
        }
        let want = id.size.min(self.cfg().footer_prefetch);
        let tail = self.fetch_range(path, id, id.size - want..id.size, Kind::Footer).await?;
        let n = tail.len();
        let mut last8 = [0u8; 8];
        last8.copy_from_slice(&tail[n - 8..]);
        let mlen = parquet::file::metadata::FooterTail::try_new(&last8).map_err(|_| bad(Class::BadFooter))?.metadata_length() as u64;
        if mlen > self.cfg().footer_cap {
            return Err(bad(Class::FooterTooLarge));
        }
        if mlen + 8 > id.size {
            return Err(bad(Class::BadFooter));
        }
        if mlen + 8 <= n as u64 {
            return Ok(tail.slice(n - 8 - mlen as usize..n - 8));
        }
        let front = self.fetch_range(path, id, id.size - 8 - mlen..id.size - want, Kind::Footer).await?;
        let mut v = Vec::with_capacity(mlen as usize);
        v.extend_from_slice(&front);
        v.extend_from_slice(&tail[..n - 8]);
        Ok(Bytes::from(v))
    }
}

/// An error while the body is streaming: any transport-level failure is a truncation (retryable, bounded by the attempt cap), whatever kind
/// the client crate gave it (a chunked body cut off mid-way arrives as a decode-kind error); a status or identity error keeps its class.
fn classify_body(e: &OsError) -> Fail {
    let f = classify(e);
    if f.class == Class::BadResponse { fail(Class::Truncated) } else { f }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_token_wakes_waiters() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let t = CancelToken::new();
            let t2 = t.clone();
            let h = tokio::spawn(async move { t2.cancelled().await });
            tokio::task::yield_now().await;
            t.cancel();
            tokio::time::timeout(Duration::from_secs(1), h).await.unwrap().unwrap();
            t.cancelled().await; // already cancelled: returns at once
        });
    }

    #[test]
    fn retryable_table_matches_the_adr() {
        for c in [Class::Throttled, Class::Server5xx, Class::Timeout, Class::Transport, Class::Truncated] {
            assert!(c.retryable(), "{c:?}");
        }
        for c in [Class::Auth, Class::NotFound, Class::IdentityChanged, Class::BadLength, Class::BadRange, Class::BadResponse, Class::NoValidator, Class::Cancelled, Class::DeadlineExceeded] {
            assert!(!c.retryable(), "{c:?}");
        }
    }
}
