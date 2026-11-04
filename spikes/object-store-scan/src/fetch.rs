//! The client side of the read path: one place where every request is made, counted, timed, limited, retried and
//! cancelled. Everything the measurements report about requests (count, bytes, waste, first-byte and total latency,
//! requests in flight, retries, what happened after a cancel) is recorded here, on the CLIENT side.
//!
//! Retries are done here, not by `object_store` (its own retries are switched off by `StoreSpec::build(None)`), so that
//! the retry logic is ours, is counted and can be shown to fail when it is wrong (see [`Mutant`]).

use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use object_store::path::Path;
use object_store::{GetOptions, GetRange, ObjectStore};
use tokio::sync::{Notify, Semaphore};

use crate::store::Redactor;
use crate::util::Rng;

// ------------------------------------------------------------------ memory gauges

/// Live bytes and their peak. A [`Guard`] adds on creation and subtracts on drop, so a value is counted from the
/// moment it is created until the last owner lets go of it, wherever it is (being built, queued, held by a consumer).
#[derive(Default)]
pub struct Gauge {
    cur: AtomicUsize,
    peak: AtomicUsize,
}

pub struct Guard {
    g: Arc<Gauge>,
    n: usize,
}

impl Gauge {
    pub fn new() -> Arc<Gauge> {
        Arc::new(Gauge::default())
    }
    pub fn track(self: &Arc<Self>, n: usize) -> Guard {
        let c = self.cur.fetch_add(n, SeqCst) + n;
        self.peak.fetch_max(c, SeqCst);
        Guard { g: Arc::clone(self), n }
    }
    pub fn cur(&self) -> usize {
        self.cur.load(SeqCst)
    }
    pub fn peak(&self) -> usize {
        self.peak.load(SeqCst)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.g.cur.fetch_sub(self.n, SeqCst);
    }
}

struct Owned {
    b: Bytes,
    _g: Guard,
}

impl AsRef<[u8]> for Owned {
    fn as_ref(&self) -> &[u8] {
        &self.b
    }
}

/// Wrap `b` so that it is counted in `g` until the last clone or slice of it is dropped.
pub fn tracked_bytes(g: &Arc<Gauge>, b: Bytes) -> Bytes {
    let guard = g.track(b.len());
    Bytes::from_owner(Owned { b, _g: guard })
}

// ------------------------------------------------------------------ cancellation

/// Set by the consumer side (`Source::finish`) when the query ends before the producers did; awaited by the producers.
pub struct CancelToken {
    flag: AtomicBool,
    notify: Notify,
    at: Mutex<Option<Instant>>,
}

impl CancelToken {
    pub fn new() -> Arc<CancelToken> {
        Arc::new(CancelToken {
            flag: AtomicBool::new(false),
            notify: Notify::new(),
            at: Mutex::new(None),
        })
    }
    pub fn cancel(&self) {
        {
            let mut at = self.at.lock().unwrap_or_else(|e| e.into_inner());
            if at.is_none() {
                *at = Some(Instant::now());
            }
        }
        self.flag.store(true, SeqCst);
        self.notify.notify_waiters();
    }
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(SeqCst)
    }
    pub fn at(&self) -> Option<Instant> {
        *self.at.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub async fn cancelled(&self) {
        loop {
            let n = self.notify.notified();
            tokio::pin!(n);
            n.as_mut().enable();
            if self.flag.load(SeqCst) {
                return;
            }
            n.await;
        }
    }
}

// ------------------------------------------------------------------ statistics

#[derive(Clone, Debug)]
pub struct ReqRec {
    /// Seconds since the query started.
    pub t_start: f64,
    /// Until the response headers arrived (first byte), ms. 0 when the attempt failed before that.
    pub ttfb_ms: f64,
    /// Until the whole body arrived, ms.
    pub total_ms: f64,
    pub bytes: u64,
    pub ok: bool,
    pub footer: bool,
    pub attempt: u32,
}

#[derive(Clone, Debug, Default)]
pub struct CancelSnap {
    /// Seconds since the query started.
    pub t: f64,
    pub inflight: usize,
    pub started: u64,
    pub completed_ok: u64,
    pub bytes_ok: u64,
}

pub struct FetchStats {
    pub t0: Instant,
    recs: Mutex<Vec<ReqRec>>,
    pub inflight: AtomicUsize,
    pub peak_inflight: AtomicUsize,
    /// Attempts started (a retry is a new attempt).
    pub started: AtomicU64,
    pub completed_ok: AtomicU64,
    pub failed: AtomicU64,
    /// Attempts whose future was dropped before it finished (cancelled in flight).
    pub dropped: AtomicU64,
    pub retries: AtomicU64,
    pub bytes_ok_data: AtomicU64,
    pub bytes_ok_footer: AtomicU64,
    snap: Mutex<Option<CancelSnap>>,
}

impl FetchStats {
    pub fn new() -> Arc<FetchStats> {
        Arc::new(FetchStats {
            t0: Instant::now(),
            recs: Mutex::new(Vec::new()),
            inflight: AtomicUsize::new(0),
            peak_inflight: AtomicUsize::new(0),
            started: AtomicU64::new(0),
            completed_ok: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            retries: AtomicU64::new(0),
            bytes_ok_data: AtomicU64::new(0),
            bytes_ok_footer: AtomicU64::new(0),
            snap: Mutex::new(None),
        })
    }

    pub fn records(&self) -> Vec<ReqRec> {
        self.recs.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn bytes_ok(&self) -> u64 {
        self.bytes_ok_data.load(SeqCst) + self.bytes_ok_footer.load(SeqCst)
    }

    /// Record the state at the moment the consumer cancels (once). Everything counted later happened AFTER the cancel.
    pub fn mark_cancel(&self) {
        let mut s = self.snap.lock().unwrap_or_else(|e| e.into_inner());
        if s.is_none() {
            *s = Some(CancelSnap {
                t: self.t0.elapsed().as_secs_f64(),
                inflight: self.inflight.load(SeqCst),
                started: self.started.load(SeqCst),
                completed_ok: self.completed_ok.load(SeqCst),
                bytes_ok: self.bytes_ok(),
            });
        }
    }

    pub fn cancel_snap(&self) -> Option<CancelSnap> {
        self.snap.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

struct AttemptGuard<'a> {
    s: &'a FetchStats,
    finished: bool,
}

impl<'a> AttemptGuard<'a> {
    fn new(s: &'a FetchStats) -> Self {
        s.started.fetch_add(1, SeqCst);
        let now = s.inflight.fetch_add(1, SeqCst) + 1;
        s.peak_inflight.fetch_max(now, SeqCst);
        AttemptGuard { s, finished: false }
    }
}

impl Drop for AttemptGuard<'_> {
    fn drop(&mut self) {
        self.s.inflight.fetch_sub(1, SeqCst);
        if !self.finished {
            self.s.dropped.fetch_add(1, SeqCst);
        }
    }
}

// ------------------------------------------------------------------ retry policy and the known-bad variants

/// Deliberately wrong behaviours, to show that the checks can fail. `None` is the real implementation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mutant {
    None,
    /// Retry without an attempt limit.
    RetryForever,
    /// Retry immediately, with no backoff.
    RetryNoBackoff,
    /// Retry errors that cannot succeed (403, 404).
    RetryPermanent,
    /// A producer that hits a fetch error drops that unit and carries on (the query then returns partial data).
    SwallowError,
    /// Producers notice a cancelled query only when they next try to send.
    CancelSendOnly,
    /// Producers ignore a closed channel and a cancel, and keep fetching.
    CancelIgnore,
    /// The bridge channel is given a huge capacity (no backpressure).
    Unbounded,
}

impl Mutant {
    pub fn label(&self) -> &'static str {
        match self {
            Mutant::None => "none",
            Mutant::RetryForever => "retry-forever",
            Mutant::RetryNoBackoff => "retry-no-backoff",
            Mutant::RetryPermanent => "retry-permanent-errors",
            Mutant::SwallowError => "swallow-error",
            Mutant::CancelSendOnly => "cancel-notices-only-on-send",
            Mutant::CancelIgnore => "cancel-ignored",
            Mutant::Unbounded => "unbounded-channel",
        }
    }
}

#[derive(Clone, Debug)]
pub struct RetryPolicy {
    /// Attempts per request, the first included (so `max_attempts - 1` retries).
    pub max_attempts: u32,
    pub base_backoff: Duration,
    pub max_backoff: Duration,
    /// A single attempt (headers and body) that takes longer is abandoned and retried.
    pub attempt_timeout: Duration,
}

impl RetryPolicy {
    /// What a real store gets: patient, but bounded.
    pub fn standard() -> Self {
        RetryPolicy {
            max_attempts: 5,
            base_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(5),
            attempt_timeout: Duration::from_secs(30),
        }
    }
    /// For the fault tests: the same shape, scaled down so the tests are quick.
    pub fn fast_tests() -> Self {
        RetryPolicy {
            max_attempts: 4,
            base_backoff: Duration::from_millis(40),
            max_backoff: Duration::from_millis(400),
            attempt_timeout: Duration::from_millis(800),
        }
    }
    /// The shortest wait that is allowed after attempt `k` failed (equal jitter: half of the nominal backoff).
    pub fn min_backoff_after(&self, k: u32) -> Duration {
        self.nominal_backoff(k) / 2
    }
    pub fn nominal_backoff(&self, k: u32) -> Duration {
        let f = 2u32.saturating_pow(k.saturating_sub(1)).min(1 << 20);
        (self.base_backoff * f).min(self.max_backoff)
    }
}

#[derive(Clone, Debug)]
pub struct FetchError {
    pub retryable: bool,
    pub msg: String,
    pub attempts: u32,
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (after {} attempt(s){})", self.msg, self.attempts, if self.retryable { ", retryable" } else { "" })
    }
}

impl std::error::Error for FetchError {}

/// Could a retry help? Anything that is not clearly permanent is retried (bounded by the attempt limit).
pub fn is_retryable(e: &object_store::Error) -> bool {
    use object_store::Error::*;
    !matches!(
        e,
        NotFound { .. }
            | PermissionDenied { .. }
            | Unauthenticated { .. }
            | InvalidPath { .. }
            | NotSupported { .. }
            | NotImplemented { .. }
            | Precondition { .. }
            | UnknownConfigurationKey { .. }
            | AlreadyExists { .. }
            | NotModified { .. }
    )
}

// ------------------------------------------------------------------ the fetcher

pub struct Fetcher {
    pub store: Arc<dyn ObjectStore>,
    pub stats: Arc<FetchStats>,
    pub policy: RetryPolicy,
    pub mutant: Mutant,
    sem: Semaphore,
    pub mem: Arc<Gauge>,
    /// Set by a watchdog to end a `RetryForever` mutant (which would otherwise never return).
    pub hard_stop: AtomicBool,
    redact: Redactor,
    rng: Mutex<Rng>,
}

impl Fetcher {
    /// `concurrency`: the most requests in flight at once, over the whole query.
    pub fn new(
        store: Arc<dyn ObjectStore>,
        stats: Arc<FetchStats>,
        policy: RetryPolicy,
        mutant: Mutant,
        concurrency: usize,
        redact: Redactor,
    ) -> Arc<Fetcher> {
        Arc::new(Fetcher {
            store,
            stats,
            policy,
            mutant,
            sem: Semaphore::new(concurrency.max(1)),
            mem: Gauge::new(),
            hard_stop: AtomicBool::new(false),
            redact,
            rng: Mutex::new(Rng::new(0xFE7C)),
        })
    }

    fn backoff(&self, failed_attempt: u32) -> Duration {
        let nominal = self.policy.nominal_backoff(failed_attempt);
        let u = self.rng.lock().unwrap_or_else(|e| e.into_inner()).f64();
        // equal jitter: between half and all of the nominal backoff
        nominal.mul_f64(0.5 + 0.5 * u)
    }

    /// Bytes `range` of `path`.
    pub async fn get_range(&self, path: &Path, range: Range<u64>, footer: bool) -> Result<Bytes, FetchError> {
        self.request(path, GetRange::Bounded(range), footer).await.map(|x| x.0)
    }

    /// The last `n` bytes of `path` and the size of the object.
    pub async fn get_suffix(&self, path: &Path, n: u64) -> Result<(Bytes, u64), FetchError> {
        self.request(path, GetRange::Suffix(n), true).await
    }

    async fn request(&self, path: &Path, range: GetRange, footer: bool) -> Result<(Bytes, u64), FetchError> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let err = match self.attempt(path, &range, footer, attempt).await {
                Ok(x) => return Ok(x),
                Err(e) => e,
            };
            let may_retry = match self.mutant {
                Mutant::RetryForever => !self.hard_stop.load(SeqCst),
                _ => attempt < self.policy.max_attempts,
            };
            let retry = (err.retryable || self.mutant == Mutant::RetryPermanent) && may_retry;
            if !retry {
                return Err(FetchError { attempts: attempt, ..err });
            }
            self.stats.retries.fetch_add(1, SeqCst);
            if self.mutant != Mutant::RetryNoBackoff {
                tokio::time::sleep(self.backoff(attempt)).await;
            }
        }
    }

    async fn attempt(&self, path: &Path, range: &GetRange, footer: bool, attempt: u32) -> Result<(Bytes, u64), FetchError> {
        // the in-flight limit is held for the attempt only, not while sleeping before a retry
        let _permit = self.sem.acquire().await.map_err(|_| FetchError {
            retryable: false,
            msg: "fetcher closed".into(),
            attempts: attempt,
        })?;
        let mut guard = AttemptGuard::new(&self.stats);
        let t_start = self.stats.t0.elapsed().as_secs_f64();
        let t0 = Instant::now();
        let mut ttfb_ms = 0.0f64;
        let opts = GetOptions {
            range: Some(range.clone()),
            ..Default::default()
        };
        let fut = async {
            let res = self.store.get_opts(path, opts).await?;
            ttfb_ms = t0.elapsed().as_secs_f64() * 1e3;
            let size = res.meta.size;
            let b = res.bytes().await?;
            Ok::<_, object_store::Error>((b, size))
        };
        let outcome = tokio::time::timeout(self.policy.attempt_timeout, fut).await;
        let total_ms = t0.elapsed().as_secs_f64() * 1e3;
        guard.finished = true;
        let result: Result<(Bytes, u64), FetchError> = match outcome {
            Err(_) => Err(FetchError {
                retryable: true,
                msg: format!("attempt timed out after {:?}", self.policy.attempt_timeout),
                attempts: attempt,
            }),
            Ok(Err(e)) => Err(FetchError {
                retryable: is_retryable(&e),
                msg: self.redact.apply(&e.to_string()),
                attempts: attempt,
            }),
            Ok(Ok((b, size))) => {
                let ok_len = match range {
                    GetRange::Bounded(r) => b.len() as u64 == r.end - r.start,
                    GetRange::Suffix(n) => b.len() as u64 <= *n,
                    GetRange::Offset(_) => true,
                };
                if ok_len {
                    Ok((b, size))
                } else {
                    Err(FetchError {
                        retryable: true,
                        msg: format!("short body: got {} bytes", b.len()),
                        attempts: attempt,
                    })
                }
            }
        };
        let (ok, bytes) = match &result {
            Ok((b, _)) => (true, b.len() as u64),
            Err(_) => (false, 0),
        };
        if ok {
            self.stats.completed_ok.fetch_add(1, SeqCst);
            if footer {
                self.stats.bytes_ok_footer.fetch_add(bytes, SeqCst);
            } else {
                self.stats.bytes_ok_data.fetch_add(bytes, SeqCst);
            }
        } else {
            self.stats.failed.fetch_add(1, SeqCst);
        }
        self.stats.recs.lock().unwrap_or_else(|e| e.into_inner()).push(ReqRec {
            t_start,
            ttfb_ms,
            total_ms,
            bytes,
            ok,
            footer,
            attempt,
        });
        // only data bytes are counted against the memory bound; a footer is parsed at once and dropped
        result.map(|(b, size)| (if footer { b } else { tracked_bytes(&self.mem, b) }, size))
    }
}
