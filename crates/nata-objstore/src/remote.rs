//! The thin wrapper of ours around `object_store` (ADR 0011): one place where every request is admitted, validated, retried, bounded, counted and
//! cancelled. `object_store`'s own retries are OFF (`max_retries: 0`), so exactly one layer owns the attempt count.
//!
//! What it does, per the ADR: at most `query_inflight` / `node_inflight` active attempts (a retry does not hold a permit while it sleeps); response
//! bytes reserved against the query's memory grant (ADR 0009's service) BEFORE the request is issued; every range request pinned to the identity
//! (version/generation, else ETag + length) with If-Match and a check of the response; the body checked chunk by chunk against the requested length
//! (never an unbounded `bytes()`); transient classes retried with equal-jitter exponential backoff, at most `max_attempts` attempts, a per-attempt
//! timeout over headers and body, an operation deadline over everything; permanent classes fail at once; cancellation interrupts grant and permit waits,
//! requests and sleeps. Errors carry a class and counts, never the dependency's text (it can hold an endpoint, a key or a body).

use std::error::Error as StdError;
use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::StreamExt;
use nata_sched::memory::{Grant, MemoryError, MemoryManager, Reservation, Wake};
use object_store::client::{
    HttpClient, HttpConnector, HttpError, HttpErrorKind, HttpRequest, HttpResponse,
    HttpResponseBody, HttpService, ReqwestConnector,
};
use object_store::path::Path;
use object_store::{ClientOptions, Error as OsError, GetOptions, GetRange, ObjectStore};
use tokio::sync::{Notify, Semaphore};

use crate::policy::{Config, Mutants};
use crate::util::{Rng, lock};

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
    /// A response would not fit the query's memory grant at all.
    GrantTooSmall,
    /// A LIST that returned more objects than the bound allows.
    TooManyKeys,
    Cancelled,
    DeadlineExceeded,
    AttemptsExhausted,
    Other,
}

impl Class {
    pub fn retryable(self) -> bool {
        matches!(
            self,
            Class::Throttled
                | Class::Server5xx
                | Class::Timeout
                | Class::Transport
                | Class::Truncated
        )
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
        write!(
            f,
            "object read failed: class={:?} attempts={} elapsed_ms={}",
            self.class,
            self.attempts,
            self.elapsed.as_millis()
        )
    }
}
impl StdError for ReadError {}

#[derive(Debug)]
struct Fail {
    class: Class,
    retry_after: Option<Duration>,
}

fn fail(class: Class) -> Fail {
    Fail {
        class,
        retry_after: None,
    }
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
    let d: String = text[i..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    d.parse().ok()
}

fn parse_status(text: &str) -> Fail {
    let code = digits_after(text, "nata-status=")
        .or_else(|| digits_after(text, "status code: "))
        .unwrap_or(0);
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

/// Wraps the reqwest transport of `object_store`:
/// * a non-success response keeps at most `cap` bytes of its body (the client crate would otherwise buffer the whole error body, unbounded, to put it in
///   an error message), and the body is replaced by a short marker that carries only a valid Retry-After;
/// * a SUCCESS response to a request that is not a ranged object read (a LIST page, a credential or token JSON, an STS answer) is collected here up to
///   `meta_cap` bytes and fails beyond it: the library reads those bodies whole (`bytes()`, `json()`, `text()`) and would allocate from a hostile
///   Content-Length or an endless body. Ranged reads stay streams; the wrapper bounds them chunk by chunk against the requested range.
#[derive(Debug)]
pub struct BoundedConnector {
    pub cap: usize,
    pub meta_cap: usize,
    pub bound_success: bool,
}

impl HttpConnector for BoundedConnector {
    fn connect(&self, options: &ClientOptions) -> object_store::Result<HttpClient> {
        let inner = ReqwestConnector::default().connect(options)?;
        Ok(HttpClient::new(BoundedService {
            inner,
            cap: self.cap,
            meta_cap: self.meta_cap,
            bound_success: self.bound_success,
        }))
    }
}

#[derive(Debug)]
struct BoundedService {
    inner: HttpClient,
    cap: usize,
    meta_cap: usize,
    bound_success: bool,
}

impl HttpService for BoundedService {
    // The desugared form of `#[async_trait] async fn call(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>` (no extra crate).
    fn call<'a, 'b>(
        &'a self,
        req: HttpRequest,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, HttpError>> + Send + 'b>>
    where
        'a: 'b,
        Self: 'b,
    {
        Box::pin(async move {
            let ranged = req.headers().contains_key("range");
            let resp = self.inner.execute(req).await?;
            if resp.status().is_success() {
                if ranged || !self.bound_success {
                    return Ok(resp);
                }
                let (parts, body) = resp.into_parts();
                let mut stream = body.bytes_stream();
                let mut buf: Vec<u8> = Vec::new();
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk?;
                    if buf.len().saturating_add(chunk.len()) > self.meta_cap {
                        return Err(HttpError::new(
                            HttpErrorKind::Decode,
                            std::io::Error::other("a response body beyond the bound"),
                        ));
                    }
                    buf.extend_from_slice(&chunk);
                }
                return Ok(HttpResponse::from_parts(
                    parts,
                    HttpResponseBody::from(Bytes::from(buf)),
                ));
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
            let ra = parts
                .headers
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<u64>().ok());
            let marker = match ra {
                Some(s) => format!("nata-status={};ra={s}", parts.status.as_u16()),
                None => format!("nata-status={}", parts.status.as_u16()),
            };
            Ok(HttpResponse::from_parts(
                parts,
                HttpResponseBody::from(marker),
            ))
        })
    }
}

// ------------------------------------------------------------------------------------------------ counters

#[derive(Default, Debug)]
pub struct Gauge {
    cur: AtomicUsize,
    peak: AtomicUsize,
}

impl Gauge {
    pub(crate) fn up(&self) {
        let c = self.cur.fetch_add(1, SeqCst) + 1;
        self.peak.fetch_max(c, SeqCst);
    }
    pub(crate) fn down(&self) {
        self.cur.fetch_sub(1, SeqCst);
    }
    pub fn cur(&self) -> usize {
        self.cur.load(SeqCst)
    }
    pub fn peak(&self) -> usize {
        self.peak.load(SeqCst)
    }
}

pub(crate) struct GaugeGuard<'a>(pub(crate) &'a Gauge);
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
        Snap {
            requests: l(&self.requests),
            wire_bytes: l(&self.wire_bytes),
            ok_bytes: l(&self.ok_bytes),
            retries: self.retries.load(SeqCst),
        }
    }
}

impl Snap {
    pub fn minus(&self, o: &Snap) -> Snap {
        let d = |a: [u64; 3], b: [u64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        Snap {
            requests: d(self.requests, o.requests),
            wire_bytes: d(self.wire_bytes, o.wire_bytes),
            ok_bytes: d(self.ok_bytes, o.ok_bytes),
            retries: self.retries - o.retries,
        }
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

/// One object: the store it lives in and its path there.
#[derive(Clone, Debug)]
pub struct Target {
    pub store: Arc<dyn ObjectStore>,
    pub path: Path,
}

/// Bytes handed to a caller, with the grant that keeps them charged until the last reader drops them.
#[derive(Debug)]
pub struct Fetched<T> {
    pub data: T,
    pub grant: Option<Grant>,
}

/// The node-wide half of the policy: the per-node attempt cap, the counters, the mutants and the jitter seed.
pub struct Remote {
    pub cfg: Config,
    pub mutants: Mutants,
    pub stats: Stats,
    node_sem: Semaphore,
    seed: u64,
    ops: AtomicU64,
}

impl Remote {
    pub fn new(cfg: Config, mutants: Mutants, seed: u64) -> Arc<Remote> {
        Arc::new(Remote {
            node_sem: Semaphore::new(cfg.node_inflight),
            cfg,
            mutants,
            stats: Stats::default(),
            seed,
            ops: AtomicU64::new(0),
        })
    }

    /// A query: its own permit pool (fair FIFO), cancel token, optional deadline, and the memory service its response bytes are granted from.
    pub fn query(self: &Arc<Self>, memory: Arc<MemoryManager>) -> Arc<Query> {
        self.query_with_deadline(memory, None)
    }

    pub fn query_with_deadline(
        self: &Arc<Self>,
        memory: Arc<MemoryManager>,
        deadline: Option<Instant>,
    ) -> Arc<Query> {
        Arc::new(Query {
            remote: self.clone(),
            sem: Semaphore::new(self.cfg.query_inflight),
            cancel: CancelToken::new(),
            deadline,
            memory,
            gate: tokio::sync::Mutex::new(()),
            wake: Arc::new(Notify::new()),
        })
    }
}

/// The memory service wakes a parked reservation through this: a sticky `Notify` permit, so a release between the check and the wait is never lost.
struct NotifyWake(Arc<Notify>);

impl Wake for NotifyWake {
    fn wake(&self) {
        self.0.notify_one();
    }
}

/// Removes this reader's place in the memory service's queue however its wait ends (a grant, a cancel, a dropped future).
struct QueueSlot<'a>(&'a MemoryManager);

impl Drop for QueueSlot<'_> {
    fn drop(&mut self) {
        let _ = self.0.finish(OPERATOR);
    }
}

/// The one operator the object-store reads register with the query's memory service.
pub const OPERATOR: usize = 0;

pub struct Query {
    pub remote: Arc<Remote>,
    sem: Semaphore,
    pub cancel: Arc<CancelToken>,
    deadline: Option<Instant>,
    memory: Arc<MemoryManager>,
    /// The memory service keeps ONE outstanding request per operator: reads that need a grant take turns here, in arrival order.
    gate: tokio::sync::Mutex<()>,
    wake: Arc<Notify>,
}

impl Query {
    fn cfg(&self) -> &Config {
        &self.remote.cfg
    }

    pub fn memory(&self) -> &Arc<MemoryManager> {
        &self.memory
    }

    /// Cancel everything this query has in flight: waits for grants and permits, requests, sleeps.
    pub fn cancel_all(&self) {
        self.cancel.cancel();
        self.memory.cancel();
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
        let mut rng = Rng::new(
            self.remote.seed
                ^ self
                    .remote
                    .ops
                    .fetch_add(1, SeqCst)
                    .wrapping_mul(0x9E37_79B9),
        );
        let err = |class: Class, attempts: &AtomicU32| ReadError {
            class,
            attempts: attempts.load(SeqCst),
            elapsed: start.elapsed(),
        };

        let body = async {
            loop {
                let n = attempts.fetch_add(1, SeqCst) + 1;
                let one = attempt(n);
                let r: Result<T, Fail> = if m.no_attempt_timeout() {
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
                let retry =
                    f.class.retryable() || m.retry_permanent() && f.class != Class::Cancelled;
                if !retry {
                    return Err(err(f.class, &attempts));
                }
                if n >= cfg.max_attempts && !m.no_attempt_cap() {
                    return Err(err(f.class, &attempts));
                }
                // backoff: equal jitter over the nominal interval; a valid Retry-After replaces it when it fits the deadline
                let k = (n - 1).min(20);
                let nominal = cfg
                    .backoff_initial
                    .saturating_mul(1u32 << k)
                    .min(cfg.backoff_max);
                let mut delay = nominal / 2 + nominal.mul_f64(0.5 * rng.unit());
                if let (Some(ra), false) = (f.retry_after, m.ignore_retry_after()) {
                    if Instant::now() + ra >= deadline {
                        return Err(err(f.class, &attempts)); // the server asks for longer than we may wait: no further attempt
                    }
                    delay = delay.max(ra);
                }
                if !m.no_deadline() && Instant::now() + delay >= deadline {
                    return Err(err(Class::DeadlineExceeded, &attempts));
                }
                self.remote.stats.retries.fetch_add(1, SeqCst);
                tokio::select! { _ = tokio::time::sleep(delay) => {}, _ = self.cancel.cancelled() => return Err(err(Class::Cancelled, &attempts)) }
            }
        };
        if m.no_deadline() {
            body.await
        } else {
            match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), body).await {
                Ok(r) => r,
                Err(_) => Err(err(Class::DeadlineExceeded, &attempts)),
            }
        }
    }

    async fn acquire<'a>(
        &self,
        sem: &'a Semaphore,
        n: u32,
    ) -> Result<tokio::sync::SemaphorePermit<'a>, Fail> {
        tokio::select! {
            p = sem.acquire_many(n) => p.map_err(|_| fail(Class::Other)),
            _ = self.cancel.cancelled() => Err(fail(Class::Cancelled)),
        }
    }

    /// Reserve `bytes` of response capacity from the query's memory service BEFORE any request is issued (ADR 0009 "reserve before allocation"). A grant
    /// that is not available parks this read on the service's sticky wake, never a CPU worker; a request larger than the budget is a bounded error.
    async fn reserve(&self, bytes: u64) -> Result<Option<Grant>, Fail> {
        if self.remote.mutants.no_grant() {
            return Ok(None);
        }
        let bytes = usize::try_from(bytes).map_err(|_| fail(Class::GrantTooSmall))?;
        let _turn = tokio::select! { g = self.gate.lock() => g, _ = self.cancel.cancelled() => return Err(fail(Class::Cancelled)) };
        let _slot = QueueSlot(&self.memory);
        loop {
            match MemoryManager::request(&self.memory, OPERATOR, bytes) {
                Ok(Reservation::Granted(g)) => return Ok(Some(g)),
                Ok(Reservation::Pending { .. }) => {
                    self.memory
                        .park(OPERATOR, NotifyWake(self.wake.clone()))
                        .map_err(|_| fail(Class::Other))?;
                    tokio::select! { _ = self.wake.notified() => {}, _ = self.cancel.cancelled() => return Err(fail(Class::Cancelled)) }
                }
                Err(MemoryError::MinimumExceedsBudget | MemoryError::NoReclaimableState) => {
                    return Err(fail(Class::GrantTooSmall));
                }
                Err(MemoryError::Cancelled) => return Err(fail(Class::Cancelled)),
                Err(_) => return Err(fail(Class::Other)),
            }
        }
    }

    /// The same, for a caller outside the retry loop (it has no attempts to count).
    pub async fn grant(&self, bytes: u64) -> Result<Option<Grant>, ReadError> {
        let t0 = Instant::now();
        self.reserve(bytes).await.map_err(|f| ReadError {
            class: f.class,
            attempts: 0,
            elapsed: t0.elapsed(),
        })
    }

    /// The identity lookup: one HEAD (counted as Kind::Head).
    pub async fn head(&self, t: &Target) -> Result<Identity, ReadError> {
        let r = self
            .run(|_| async {
                let _q = self.acquire(&self.sem, 1).await?;
                let _n = self.acquire(&self.remote.node_sem, 1).await?;
                self.remote.stats.inflight.up();
                let _g = GaugeGuard(&self.remote.stats.inflight);
                let t0 = Instant::now();
                self.remote.stats.requests[Kind::Head as usize].fetch_add(1, SeqCst);
                let res = t
                    .store
                    .get_opts(
                        &t.path,
                        GetOptions {
                            head: true,
                            ..Default::default()
                        },
                    )
                    .await;
                let ms = t0.elapsed().as_secs_f64() * 1e3;
                lock(&self.remote.stats.recs).push((ms, ms, Kind::Head as u8, res.is_ok()));
                let meta = res.map_err(|e| classify(&e))?.meta;
                if meta.e_tag.is_none()
                    && meta.version.is_none()
                    && !self.remote.mutants.no_version_check()
                {
                    return Err(fail(Class::NoValidator));
                }
                Ok(Identity {
                    size: meta.size,
                    etag: meta.e_tag,
                    version: meta.version,
                })
            })
            .await;
        match r {
            Err(_) if self.remote.mutants.swallow_error() => Ok(Identity {
                size: 0,
                etag: None,
                version: None,
            }),
            r => r,
        }
    }

    /// One logical range (at most `merge_max` bytes), pinned to `id`, validated, retried per the policy; its bytes are granted first.
    pub async fn fetch_range(
        &self,
        t: &Target,
        id: &Identity,
        range: Range<u64>,
        kind: Kind,
    ) -> Result<Fetched<Bytes>, ReadError> {
        let t0 = Instant::now();
        let r = async {
            self.check_range(id, &range, t0)?;
            let grant = self.grant(range.end - range.start).await?;
            let data = self.fetch_range_inner(t, id, range.clone(), kind).await?;
            Ok(Fetched { data, grant })
        }
        .await;
        match r {
            Err(_) if self.remote.mutants.swallow_error() => Ok(Fetched {
                data: Bytes::from(vec![0u8; (range.end - range.start) as usize]),
                grant: None,
            }),
            r => r,
        }
    }

    /// The checked half-open range within the pinned object length, no larger than the response cap.
    fn check_range(&self, id: &Identity, range: &Range<u64>, t0: Instant) -> Result<(), ReadError> {
        let bad = |class: Class| ReadError {
            class,
            attempts: 0,
            elapsed: t0.elapsed(),
        };
        if range.start >= range.end
            || range.end > id.size
            || range.end - range.start > self.cfg().merge_max
        {
            return Err(bad(Class::BadRange));
        }
        Ok(())
    }

    /// One range under the retry policy; the caller holds the grant for its bytes.
    async fn fetch_range_inner(
        &self,
        t: &Target,
        id: &Identity,
        range: Range<u64>,
        kind: Kind,
    ) -> Result<Bytes, ReadError> {
        let len = range.end - range.start;
        let m = self.remote.mutants.clone();
        // keep the bytes of earlier failed attempts only for the append_on_retry mutant
        let carried: Mutex<Vec<u8>> = Mutex::new(Vec::new());
        self.run(|_attempt| {
            let carried = &carried;
            let m = m.clone();
            let range = range.clone();
            async move {
                // permits; none is held while the retry loop sleeps. The grant (taken before the loop) is never waited for while holding a permit.
                let _q = if m.no_inflight_cap() {
                    None
                } else {
                    Some(self.acquire(&self.sem, 1).await?)
                };
                let _n = if m.no_inflight_cap() {
                    None
                } else {
                    Some(self.acquire(&self.remote.node_sem, 1).await?)
                };
                self.remote.stats.inflight.up();
                let _gg = GaugeGuard(&self.remote.stats.inflight);
                let ki = kind as usize;
                self.remote.stats.requests[ki].fetch_add(1, SeqCst);
                let t0 = Instant::now();
                let mut opts = GetOptions {
                    range: Some(GetRange::Bounded(range.clone())),
                    ..Default::default()
                };
                if !m.no_version_check() {
                    opts.if_match = id.etag.clone();
                    opts.version = id.version.clone();
                }
                let rec = |fb: f64, ok: bool| {
                    lock(&self.remote.stats.recs).push((
                        fb,
                        t0.elapsed().as_secs_f64() * 1e3,
                        ki as u8,
                        ok,
                    ))
                };
                let res = match t.store.get_opts(&t.path, opts).await {
                    Ok(r) => r,
                    Err(e) => {
                        rec(t0.elapsed().as_secs_f64() * 1e3, false);
                        return Err(classify(&e));
                    }
                };
                let first_byte = t0.elapsed().as_secs_f64() * 1e3;
                if !m.no_version_check() {
                    let meta = &res.meta;
                    if meta.size != id.size
                        || id.version.is_some()
                            && meta.version.is_some()
                            && meta.version != id.version
                        || id.etag.is_some() && meta.e_tag != id.etag
                    {
                        rec(first_byte, false);
                        return Err(fail(Class::IdentityChanged));
                    }
                }
                if !m.no_length_check() && res.range != range {
                    rec(first_byte, false);
                    return Err(fail(Class::BadRange));
                }
                let mut stream = res.into_stream();
                let mut buf: Vec<u8> = if m.append_on_retry() {
                    std::mem::take(&mut *lock(carried))
                } else {
                    Vec::new()
                };
                let base = buf.len();
                if !m.no_length_check() {
                    // the length was checked against the pinned object and the response cap before the request; the grant covers these bytes
                    buf.reserve_exact(len as usize);
                }
                while let Some(chunk) = stream.next().await {
                    let chunk = match chunk {
                        Ok(c) => c,
                        Err(e) => {
                            self.remote.stats.wire_bytes[ki]
                                .fetch_add((buf.len() - base) as u64, SeqCst);
                            if m.append_on_retry() {
                                *lock(carried) = buf;
                            }
                            rec(first_byte, false);
                            return Err(classify_body(&e));
                        }
                    };
                    if !m.no_length_check() && (buf.len() - base) as u64 + chunk.len() as u64 > len
                    {
                        self.remote.stats.wire_bytes[ki]
                            .fetch_add((buf.len() - base + chunk.len()) as u64, SeqCst);
                        rec(first_byte, false);
                        return Err(fail(Class::BadLength));
                    }
                    buf.extend_from_slice(&chunk);
                }
                self.remote.stats.wire_bytes[ki].fetch_add((buf.len() - base) as u64, SeqCst);
                if !m.no_length_check() && (buf.len() - base) as u64 != len {
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

    /// Several needed ranges of one object (one row group's column chunks): planned (merged within the group or not), their TOTAL reserved as one grant
    /// before the first request (a reader never holds part of a unit while waiting for the rest), fetched concurrently under the permits, and sliced
    /// back per needed range. The grant travels with the bytes.
    pub async fn read_ranges(
        &self,
        t: &Target,
        id: &Identity,
        needed: &[Range<u64>],
        plan: &[crate::plan::Planned],
        kind: Kind,
    ) -> Result<Fetched<Vec<Bytes>>, ReadError> {
        let t0 = Instant::now();
        let mut total = 0u64;
        for p in plan {
            self.check_range(id, &p.fetch, t0)?;
            total = total.saturating_add(p.fetch.end - p.fetch.start);
        }
        let grant = self.grant(total).await?;
        let fetched: Vec<Result<Bytes, ReadError>> = futures_util::future::join_all(
            plan.iter()
                .map(|p| self.fetch_range_inner(t, id, p.fetch.clone(), kind)),
        )
        .await;
        let mut got = Vec::with_capacity(fetched.len());
        let mut first_err: Option<ReadError> = None;
        for f in fetched {
            match f {
                Ok(b) => got.push(b),
                Err(e) => {
                    // report the first failure (a deadline or cancel on a sibling is not the root cause)
                    if first_err
                        .as_ref()
                        .is_none_or(|x| x.class == Class::Cancelled && e.class != Class::Cancelled)
                    {
                        first_err = Some(e);
                    }
                    got.push(Bytes::new());
                }
            }
        }
        if let Some(e) = first_err {
            return match self.remote.mutants.swallow_error() {
                true => Ok(Fetched {
                    data: needed
                        .iter()
                        .map(|r| Bytes::from(vec![0u8; (r.end - r.start) as usize]))
                        .collect(),
                    grant,
                }),
                false => Err(e),
            };
        }
        let mut out = Vec::with_capacity(needed.len());
        for (i, r) in needed.iter().enumerate() {
            if r.start >= r.end {
                out.push(Bytes::new());
                continue;
            }
            let mut pieces: Vec<(usize, &Range<u64>)> = plan
                .iter()
                .enumerate()
                .filter(|(_, p)| p.covers.contains(&i))
                .map(|(j, p)| (j, &p.fetch))
                .collect();
            pieces.sort_by_key(|(_, f)| f.start);
            if let Some((j, f)) = pieces
                .iter()
                .find(|(_, f)| f.start <= r.start && f.end >= r.end)
            {
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
                return Err(ReadError {
                    class: Class::BadRange,
                    attempts: 0,
                    elapsed: Duration::ZERO,
                });
            }
            out.push(Bytes::from(v));
        }
        Ok(Fetched { data: out, grant })
    }

    /// The Parquet footer metadata bytes (without the 8-byte tail): one suffix read, one more only if the footer is longer than the first read. The
    /// length is checked against the cap BEFORE any further allocation. The grant of the bytes returns with them.
    pub async fn footer(&self, t: &Target, id: &Identity) -> Result<Fetched<Bytes>, ReadError> {
        let bad = |class: Class| ReadError {
            class,
            attempts: 0,
            elapsed: Duration::ZERO,
        };
        if id.size < 12 {
            return Err(bad(Class::BadFooter));
        }
        let want = id.size.min(self.cfg().footer_prefetch);
        // the magic number at the start of the file: parquet-rs reads the footer only, so a file whose first four bytes are wrong would still decode
        // (the local reader refuses it the same way, `check_magic`). A file inside the first read has it already; otherwise one 4-byte read, in parallel
        // with the tail read so that it adds a request, not a round trip.
        let (tail, head_ok) = if id.size <= want {
            let tail = self
                .fetch_range(t, id, id.size - want..id.size, Kind::Footer)
                .await?;
            let ok = tail.data.starts_with(b"PAR1");
            (tail, ok)
        } else {
            let (tail, head) = futures_util::future::join(
                self.fetch_range(t, id, id.size - want..id.size, Kind::Footer),
                self.fetch_range(t, id, 0..4, Kind::Footer),
            )
            .await;
            (tail?, head?.data.as_ref() == b"PAR1")
        };
        if !head_ok {
            return Err(bad(Class::BadFooter));
        }
        let n = tail.data.len();
        let mut last8 = [0u8; 8];
        last8.copy_from_slice(&tail.data[n - 8..]);
        let mlen = parquet::file::metadata::FooterTail::try_new(&last8)
            .map_err(|_| bad(Class::BadFooter))?
            .metadata_length() as u64;
        if mlen > self.cfg().footer_cap {
            return Err(bad(Class::FooterTooLarge));
        }
        if mlen + 8 > id.size {
            return Err(bad(Class::BadFooter));
        }
        if mlen + 8 <= n as u64 {
            return Ok(Fetched {
                data: tail.data.slice(n - 8 - mlen as usize..n - 8),
                grant: tail.grant,
            });
        }
        let front = self
            .fetch_range(t, id, id.size - 8 - mlen..id.size - want, Kind::Footer)
            .await?;
        let mut v = Vec::with_capacity(mlen as usize);
        v.extend_from_slice(&front.data);
        v.extend_from_slice(&tail.data[..n - 8]);
        Ok(Fetched {
            data: Bytes::from(v),
            grant: front.grant,
        })
    }

    /// Every object under `prefix` (recursive), at most `max_list_keys`: one retried attempt over the pages. Names and sizes only; never a body.
    pub async fn list(
        &self,
        store: &Arc<dyn ObjectStore>,
        prefix: &Path,
    ) -> Result<Vec<String>, ReadError> {
        let cap = self.cfg().max_list_keys;
        self.run(|_| async {
            let _q = self.acquire(&self.sem, 1).await?;
            let _n = self.acquire(&self.remote.node_sem, 1).await?;
            self.remote.stats.inflight.up();
            let _g = GaugeGuard(&self.remote.stats.inflight);
            self.remote.stats.requests[Kind::Head as usize].fetch_add(1, SeqCst);
            let mut keys: Vec<String> = Vec::new();
            let mut stream = store.list(Some(prefix));
            while let Some(item) = stream.next().await {
                let meta = item.map_err(|e| classify_body(&e))?;
                if keys.len() >= cap {
                    return Err(fail(Class::TooManyKeys));
                }
                keys.push(meta.location.to_string());
            }
            Ok(keys)
        })
        .await
    }
}

/// An error while the body is streaming: any transport-level failure is a truncation (retryable, bounded by the attempt cap), whatever kind
/// the client crate gave it (a chunked body cut off mid-way arrives as a decode-kind error); a status or identity error keeps its class.
fn classify_body(e: &OsError) -> Fail {
    let f = classify(e);
    if f.class == Class::BadResponse {
        fail(Class::Truncated)
    } else {
        f
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_token_wakes_waiters() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        rt.block_on(async {
            let t = CancelToken::new();
            let t2 = t.clone();
            let h = tokio::spawn(async move { t2.cancelled().await });
            tokio::task::yield_now().await;
            t.cancel();
            tokio::time::timeout(Duration::from_secs(1), h)
                .await
                .expect("woken")
                .expect("joined");
            t.cancelled().await; // already cancelled: returns at once
        });
    }

    #[test]
    fn retryable_table_matches_the_adr() {
        for c in [
            Class::Throttled,
            Class::Server5xx,
            Class::Timeout,
            Class::Transport,
            Class::Truncated,
        ] {
            assert!(c.retryable(), "{c:?}");
        }
        for c in [
            Class::Auth,
            Class::NotFound,
            Class::IdentityChanged,
            Class::BadLength,
            Class::BadRange,
            Class::BadResponse,
            Class::NoValidator,
            Class::Cancelled,
            Class::DeadlineExceeded,
            Class::GrantTooSmall,
        ] {
            assert!(!c.retryable(), "{c:?}");
        }
    }
}
