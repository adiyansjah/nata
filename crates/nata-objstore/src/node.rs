//! The node: one I/O runtime, the per-node attempt cap, the stores, the footer cache; and the query handles the Parquet scan calls from its CPU workers.
//!
//! ADR 0002's split stays: the HTTP work, the retries and the waits for grants run on this module's own Tokio runtime (two threads); decode runs on the
//! scan's worker threads. Parallel scans submit owned requests and return their
//! worker to the scheduler until completion. The synchronous planning/cursor API
//! remains a compatibility path; it never runs a future on a CPU worker.

use std::future::Future;
use std::ops::Range;
use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

use bytes::Bytes;
use futures_util::FutureExt;
use nata_parquet::remote::{
    RemoteBytes, RemoteError, RemoteErrorKind, RemoteFile, RemoteIo, RemoteQuery, Request,
};
use nata_sched::memory::{Grant, MemoryManager, Reservation};
use std::collections::HashMap;
use std::sync::Mutex;

use crate::cache::{Handle, MetaCache, MetaService};
use crate::plan::{Budget, plan_merged};
use crate::policy::{CacheMutants, Config, Mutants};
use crate::remote::{Class, Identity, Kind, Query, ReadError, Remote, Target};
use crate::store::{GcsOptions, Location, S3Options, StoreEntry, Stores};

/// How often a waiting worker looks at its cancel flag.
const POLL: Duration = Duration::from_millis(10);

/// What a node is made of (the environment is captured once, at construction).
pub struct NodeOptions {
    pub cfg: Config,
    pub mutants: Mutants,
    pub cache_mutants: CacheMutants,
    pub s3: S3Options,
    pub gcs: GcsOptions,
    /// The footer cache; `false` is the no-cache control (every open reads its footer).
    pub cache: bool,
    pub seed: u64,
    /// Upstream query admission bound; fixed equal response shares, never borrowing.
    pub max_queries: usize,
}

impl NodeOptions {
    /// ADR 0011/0012 defaults and the credentials the process environment gives.
    pub fn from_env() -> NodeOptions {
        NodeOptions {
            cfg: Config::adr(),
            mutants: Mutants::default(),
            cache_mutants: CacheMutants::default(),
            s3: S3Options::from_env(),
            gcs: GcsOptions::from_env(),
            cache: true,
            seed: 20_261_107,
            max_queries: 1,
        }
    }
}

pub struct ObjectNode {
    rt: Option<tokio::runtime::Runtime>,
    pub remote: Arc<Remote>,
    pub cache: Option<Arc<MetaCache>>,
    stores: Stores,
    cfg: Config,
    memory: Arc<MemoryManager>,
    _execution_allowance: Grant,
    pub node_pool: Arc<MemoryManager>,
    max_queries: usize,
}

impl Drop for ObjectNode {
    fn drop(&mut self) {
        if let Some(rt) = self.rt.take() {
            // An aborted request may release the final node reference on an I/O
            // worker. Tokio forbids joining a runtime there. Normal CPU-side
            // teardown still drops/joins it; this rare path requests shutdown
            // without trying to join the current thread.
            if tokio::runtime::Handle::try_current().is_ok() {
                rt.shutdown_background();
            } else {
                drop(rt);
            }
        }
    }
}

impl std::fmt::Debug for ObjectNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectNode").finish_non_exhaustive()
    }
}

/// A readable class name for a failed read; the text never carries the dependency's message.
fn describe(e: &ReadError) -> String {
    if let Class::Redirect(code) = e.class {
        return format!(
            "the store answered with a redirect (HTTP {code}), which is never followed"
        );
    }
    let what = match e.class {
        Class::Throttled => "the store throttled the request",
        Class::Server5xx => "the store answered with a server error",
        Class::Timeout => "the request timed out",
        Class::Transport => "the connection failed",
        Class::Truncated => "the response ended early",
        Class::Auth => "access was denied or the credentials were refused",
        Class::NotFound => "the object does not exist",
        Class::IdentityChanged => "the object changed while it was being read",
        Class::BadLength => "the response length disagrees with the range asked for",
        Class::BadRange => "the response is not the range asked for",
        Class::BadResponse => "the response is malformed",
        Class::Redirect(_) => "the store answered with a redirect",
        Class::NoValidator => {
            "the object has neither a version nor an ETag, so it cannot be pinned"
        }
        Class::FooterTooLarge => "the Parquet footer is larger than the 16 MiB bound",
        Class::BadFooter => "the Parquet footer is damaged",
        Class::GrantTooSmall => "the read does not fit the query's memory budget",
        Class::TooManyKeys => "the pattern matches more objects than the bound allows",
        Class::Cancelled => "the read was cancelled",
        Class::DeadlineExceeded => "the read did not finish within its deadline",
        Class::AttemptsExhausted => "the read failed after the attempt limit",
        Class::Other => "the read failed",
    };
    if e.attempts > 1 {
        format!("{what} (after {} attempts)", e.attempts)
    } else {
        what.to_string()
    }
}

fn to_remote(e: &ReadError) -> RemoteError {
    let kind = match e.class {
        Class::BadFooter | Class::FooterTooLarge | Class::BadLength | Class::BadRange => {
            RemoteErrorKind::Corrupt
        }
        Class::GrantTooSmall | Class::TooManyKeys => RemoteErrorKind::Resource,
        Class::Cancelled => RemoteErrorKind::Cancelled,
        _ => RemoteErrorKind::Io,
    };
    RemoteError::new(kind, describe(e))
}

impl ObjectNode {
    pub fn new(o: NodeOptions) -> Result<Arc<ObjectNode>, String> {
        let memory =
            MemoryManager::new(o.cfg.grant_bytes, 1, 1).map_err(|_| "invalid node memory pool")?;
        let cache = (o.cache && o.cfg.cache_bytes > 0 && o.cfg.cache_entries > 0)
            .then(|| MetaCache::from_config(&o.cfg, o.cache_mutants));
        if let Some(c) = &cache {
            let n = usize::try_from(o.cfg.cache_bytes).map_err(|_| "cache allowance overflow")?;
            let grant = match MemoryManager::request(&memory, 0, n) {
                Ok(Reservation::Granted(g)) => g,
                _ => return Err("footer cache allowance does not fit the node memory pool".into()),
            };
            c.reserve(grant);
        }
        let execution_bytes = o
            .cfg
            .grant_bytes
            .checked_sub(
                1 + if cache.is_some() {
                    o.cfg.cache_bytes as usize
                } else {
                    0
                },
            )
            .ok_or("node allowance overflow")?;
        let execution_allowance = match MemoryManager::request(&memory, 0, execution_bytes) {
            Ok(Reservation::Granted(g)) => g,
            _ => return Err("execution allowance does not fit node memory pool".into()),
        };
        let execution_memory =
            MemoryManager::new(execution_bytes, (execution_bytes / 8).clamp(1, 1 << 20), 1)
                .map_err(|_| "invalid execution allowance")?;
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("nata-io")
            .enable_all()
            .build()
            .map_err(|_| "cannot start the object-store I/O runtime".to_string())?;
        Ok(Arc::new(ObjectNode {
            rt: Some(rt),
            remote: Remote::new(o.cfg.clone(), o.mutants.clone(), o.seed),
            cache,
            stores: Stores::new(o.s3, o.gcs, o.cfg.clone(), o.mutants),
            cfg: o.cfg,
            memory: execution_memory,
            _execution_allowance: execution_allowance,
            node_pool: memory,
            max_queries: o.max_queries.max(1),
        }))
    }

    pub fn cfg(&self) -> &Config {
        &self.cfg
    }

    /// Run `fut` on the I/O runtime and wait for it on this thread, polling `cancel`. On cancel the task is aborted (its permits and grants drop with it).
    pub fn call<T: Send + 'static>(
        &self,
        cancel: &dyn Fn() -> bool,
        fut: impl Future<Output = Result<T, ReadError>> + Send + 'static,
    ) -> Result<T, ReadError> {
        let (tx, rx) = channel();
        let task = self.rt.as_ref().expect("I/O runtime").spawn(async move {
            let _ = tx.send(fut.await);
        });
        loop {
            match rx.recv_timeout(POLL) {
                Ok(r) => return r,
                Err(RecvTimeoutError::Timeout) => {
                    if cancel() {
                        task.abort();
                        return Err(ReadError {
                            class: Class::Cancelled,
                            attempts: 0,
                            elapsed: Duration::ZERO,
                        });
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(ReadError {
                        class: Class::Other,
                        attempts: 0,
                        elapsed: Duration::ZERO,
                    });
                }
            }
        }
    }

    fn entry(&self, loc: &Location) -> Result<(StoreEntry, Target), RemoteError> {
        let path = loc
            .path()
            .map_err(|m| RemoteError::new(RemoteErrorKind::NotSupported, m))?;
        let entry = self
            .stores
            .get(loc)
            .map_err(|m| RemoteError::new(RemoteErrorKind::Io, m))?;
        let target = Target {
            store: entry.store.clone(),
            path,
        };
        Ok((entry, target))
    }

    fn service(&self, entry: &StoreEntry, loc: &Location) -> Arc<MetaService> {
        Arc::new(MetaService::new(
            self.cache.clone(),
            loc.scheme.name(),
            &format!("{}/{}", entry.scope, loc.bucket),
        ))
    }

    /// Shared node response/staging service, after withholding the cache allowance.
    pub fn memory(&self) -> Result<Arc<MemoryManager>, RemoteError> {
        Ok(self.memory.clone())
    }

    pub fn query(self: &Arc<Self>) -> Result<Arc<ObjectQuery>, RemoteError> {
        let node = self.memory.snapshot();
        let budget = (node.budget - node.progress) / self.max_queries;
        let memory =
            MemoryManager::new(budget, (budget / 8).clamp(1, 1 << 20), 1).map_err(|_| {
                RemoteError::new(RemoteErrorKind::Resource, "invalid query memory budget")
            })?;
        let q = self.remote.query_with_node(memory, self.memory.clone());
        Ok(Arc::new(ObjectQuery {
            node: Arc::clone(self),
            q,
            budget: Arc::new(Budget::new()),
            bound: Arc::new(Mutex::new(HashMap::new())),
        }))
    }
}

/// A query's handle on the node: the scan calls `open` and `fetch` from its workers.
pub struct ObjectQuery {
    node: Arc<ObjectNode>,
    pub q: Arc<Query>,
    budget: Arc<Budget>,
    bound: Arc<Mutex<HashMap<String, RemoteFile>>>,
}

impl std::fmt::Debug for ObjectQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectQuery").finish_non_exhaustive()
    }
}

/// What `RemoteFile::pinned` holds: the object, its pinned identity, and the cache pin of its footer.
struct Pinned {
    target: Target,
    identity: Identity,
    _handle: Arc<Handle>,
}

impl ObjectQuery {
    async fn open_async(&self, location: &str) -> Result<RemoteFile, RemoteError> {
        let loc = Location::parse(location)
            .map_err(|m| RemoteError::new(RemoteErrorKind::NotSupported, m))?;
        let (entry, target) = self.node.entry(&loc)?;
        let bound = crate::util::lock(&self.bound).remove(location);
        if let Some(file) = bound {
            let id = self.q.head(&target).await.map_err(|e| to_remote(&e))?;
            let pin = file
                .pinned
                .clone()
                .downcast::<Pinned>()
                .map_err(|_| RemoteError::new(RemoteErrorKind::Io, "invalid footer handle"))?;
            if id.token() != pin.identity.token() {
                return Err(RemoteError::new(
                    RemoteErrorKind::Io,
                    "the object changed after binding",
                ));
            }
            return Ok(file);
        }
        let svc = self.node.service(&entry, &loc);
        let (q, t) = (Arc::clone(&self.q), target.clone());
        let key = format!("{}/{}", loc.bucket, loc.key);
        let opened = svc.open(&q, &t, &key).await.map_err(|e| to_remote(&e))?;
        Ok(RemoteFile {
            metadata: Arc::clone(&opened.handle.meta),
            length: opened.identity.size,
            pinned: Arc::new(Pinned {
                target,
                identity: opened.identity,
                _handle: opened.handle,
            }),
        })
    }

    async fn fetch_async(
        &self,
        file: &RemoteFile,
        ranges: &[Range<u64>],
    ) -> Result<RemoteBytes, RemoteError> {
        let pinned = Arc::clone(&file.pinned).downcast::<Pinned>().map_err(|_| {
            RemoteError::new(RemoteErrorKind::Io, "a file opened by another reader")
        })?;
        // one row group's column chunks: planned together (merged only within the allowance of ADR 0011, never across row groups), granted as one unit
        let mut needed: Vec<Range<u64>> = ranges.to_vec();
        needed.sort_by_key(|r| (r.start, r.end));
        let plan = plan_merged(&needed, self.node.cfg(), &self.budget);
        let (q, p, needed2) = (Arc::clone(&self.q), Arc::clone(&pinned), needed.clone());
        let got = q
            .read_ranges(&p.target, &p.identity, &needed2, &plan, Kind::Data)
            .await
            .map_err(|e| to_remote(&e))?;
        let pieces: Vec<(u64, Bytes)> = needed
            .iter()
            .zip(got.data)
            .filter(|(r, _)| r.start < r.end)
            .map(|(r, b)| (r.start, b))
            .collect();
        Ok(RemoteBytes {
            pieces,
            hold: Box::new(got.grant),
        })
    }
}

struct Pending<T> {
    done: Arc<nata_sched::bridge::Completion<Result<T, RemoteError>>>,
    task: tokio::task::JoinHandle<()>,
}
impl<T: Send> Request<T> for Pending<T> {
    fn poll(&mut self, wake: &nata_sched::bridge::Waker) -> Option<Result<T, RemoteError>> {
        self.done.poll(wake)
    }
}
impl<T> Drop for Pending<T> {
    fn drop(&mut self) {
        self.done.abandon();
        self.task.abort();
    }
}
impl ObjectQuery {
    fn owned(&self) -> Self {
        Self {
            node: self.node.clone(),
            q: self.q.clone(),
            budget: self.budget.clone(),
            bound: self.bound.clone(),
        }
    }
    fn start<T: Send + 'static>(
        &self,
        fut: impl Future<Output = Result<T, RemoteError>> + Send + 'static,
    ) -> Box<dyn Request<T>> {
        let done = nata_sched::bridge::Completion::new();
        let complete = done.clone();
        let task = self
            .node
            .rt
            .as_ref()
            .expect("I/O runtime")
            .spawn(async move {
                let result = std::panic::AssertUnwindSafe(fut)
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| {
                        Err(RemoteError::new(
                            RemoteErrorKind::Corrupt,
                            "remote request failed while parsing",
                        ))
                    });
                complete.complete(result);
            });
        Box::new(Pending { done, task })
    }
}
impl RemoteQuery for ObjectQuery {
    fn open(&self, location: &str, cancel: &dyn Fn() -> bool) -> Result<RemoteFile, RemoteError> {
        let this = self.owned();
        let location = location.to_string();
        self.node
            .call(cancel, async move { Ok(this.open_async(&location).await) })
            .map_err(|e| to_remote(&e))?
    }
    fn fetch(
        &self,
        file: &RemoteFile,
        ranges: &[Range<u64>],
        cancel: &dyn Fn() -> bool,
    ) -> Result<RemoteBytes, RemoteError> {
        let this = self.owned();
        let file = file.clone();
        let ranges = ranges.to_vec();
        self.node
            .call(
                cancel,
                async move { Ok(this.fetch_async(&file, &ranges).await) },
            )
            .map_err(|e| to_remote(&e))?
    }
    fn start_open(&self, location: &str) -> Box<dyn Request<RemoteFile>> {
        let this = self.owned();
        let location = location.to_string();
        self.start(async move { this.open_async(&location).await })
    }
    fn start_fetch(
        &self,
        file: &RemoteFile,
        ranges: &[Range<u64>],
    ) -> Box<dyn Request<RemoteBytes>> {
        let this = self.owned();
        let file = file.clone();
        let ranges = ranges.to_vec();
        self.start(async move { this.fetch_async(&file, &ranges).await })
    }
    fn metrics(&self) -> Option<String> {
        use std::sync::atomic::Ordering::SeqCst;
        let s = self.node.remote.stats.snap();
        let (hits, misses, evictions, retained) =
            self.node.cache.as_ref().map_or((0, 0, 0, 0), |c| {
                (
                    c.stats.hits.load(SeqCst),
                    c.stats.misses.load(SeqCst),
                    c.stats.evictions.load(SeqCst),
                    c.retained_bytes(),
                )
            });
        Some(format!(
            "object_store: scope=node-cumulative cache_hits={hits} cache_misses={misses} cache_evictions={evictions} cache_bytes={retained} identity_checks={} footer_gets={} data_gets={} fetched_bytes={} successful_bytes={} retries={} peak_inflight={} peak_grants={} cache_reserved={}",
            s.requests[0],
            s.requests[1],
            s.requests[2],
            s.wire_bytes.iter().sum::<u64>(),
            s.ok_bytes.iter().sum::<u64>(),
            s.retries,
            self.node.remote.stats.inflight.peak(),
            self.node.memory.snapshot().peak,
            self.node.cache.as_ref().map_or(0, |c| c.cap_bytes())
        ))
    }
    fn cancel(&self) {
        self.q.cancel_all();
    }
}

/// `Arc<ObjectNode>` as the scan's [`RemoteIo`].
#[derive(Clone)]
pub struct ObjectHandle(pub Arc<ObjectNode>);

impl std::fmt::Debug for ObjectHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectHandle").finish_non_exhaustive()
    }
}

impl ObjectHandle {
    pub fn new(o: NodeOptions) -> Result<ObjectHandle, String> {
        Ok(ObjectHandle(ObjectNode::new(o)?))
    }
}

impl RemoteIo for ObjectHandle {
    fn scoped(&self) -> Option<Arc<dyn RemoteIo>> {
        let io = self.clone();
        Some(Arc::new(ScopedObjectIo::new(
            self.reserved_bytes(),
            move || Ok(io.clone()),
        )))
    }
    fn reserved_bytes(&self) -> usize {
        self.0.cfg.grant_bytes
    }
    fn begin(&self) -> Result<Arc<dyn RemoteQuery>, RemoteError> {
        Ok(self.0.query()?)
    }

    fn expand(&self, pattern: &str, cancel: &dyn Fn() -> bool) -> Result<Vec<String>, RemoteError> {
        let loc = Location::parse(pattern)
            .map_err(|m| RemoteError::new(RemoteErrorKind::NotSupported, m))?;
        if !has_glob(&loc.key) {
            return Ok(vec![pattern.to_string()]);
        }
        let entry = self
            .0
            .stores
            .get(&loc)
            .map_err(|m| RemoteError::new(RemoteErrorKind::Io, m))?;
        let (prefix, segs) = split_glob(&loc.key);
        let prefix_path =
            object_store::path::Path::parse(prefix.trim_end_matches('/')).map_err(|_| {
                RemoteError::new(
                    RemoteErrorKind::NotSupported,
                    "the glob prefix has an empty or relative segment",
                )
            })?;
        let q = self.0.query()?;
        let (q2, store) = (Arc::clone(&q.q), entry.store.clone());
        let keys = self
            .0
            .call(cancel, async move { q2.list(&store, &prefix_path).await })
            .map_err(|e| to_remote(&e))?;
        let mut out: Vec<String> = keys
            .into_iter()
            .filter(|k| key_matches(&segs, k))
            .map(|k| format!("{}://{}/{k}", loc.scheme.name(), loc.bucket))
            .collect();
        out.sort();
        out.dedup();
        Ok(out)
    }

    fn footer(&self, location: &str, cancel: &dyn Fn() -> bool) -> Result<RemoteFile, RemoteError> {
        self.0.query()?.open(location, cancel)
    }
}

/// The object-store reader of a process, started on first use: a process that only reads local files never starts the I/O runtime or reads the
/// credential environment. `options` runs once, when the first remote location is touched.
#[derive(Clone)]
pub struct LazyObjectIo {
    options: Arc<dyn Fn() -> NodeOptions + Send + Sync>,
    node: Arc<std::sync::OnceLock<Result<ObjectHandle, String>>>,
    allowance: usize,
}

impl std::fmt::Debug for LazyObjectIo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LazyObjectIo")
            .field("started", &self.node.get().is_some())
            .finish()
    }
}

impl LazyObjectIo {
    pub fn new(
        allowance: usize,
        options: impl Fn() -> NodeOptions + Send + Sync + 'static,
    ) -> LazyObjectIo {
        LazyObjectIo {
            options: Arc::new(options),
            node: Arc::new(std::sync::OnceLock::new()),
            allowance,
        }
    }

    /// ADR defaults with a bounded node object-service allowance and per-query cap.
    pub fn from_env(query_memory_bytes: usize) -> LazyObjectIo {
        Self::from_env_with_queries(query_memory_bytes, 1)
    }
    pub fn from_env_with_queries(query_memory_bytes: usize, max_queries: usize) -> LazyObjectIo {
        LazyObjectIo::new(query_memory_bytes, move || {
            let mut o = NodeOptions::from_env();
            o.cfg.grant_bytes = query_memory_bytes;
            o.max_queries = max_queries.max(1);
            o
        })
    }

    pub fn node(&self) -> Result<&ObjectHandle, RemoteError> {
        self.node
            .get_or_init(|| ObjectHandle::new((self.options)()))
            .as_ref()
            .map_err(|m| RemoteError::new(RemoteErrorKind::Io, m.clone()))
    }
}

impl RemoteIo for LazyObjectIo {
    fn scoped(&self) -> Option<Arc<dyn RemoteIo>> {
        let io = self.clone();
        Some(Arc::new(ScopedObjectIo::new(self.allowance, move || {
            io.node().cloned()
        })))
    }
    fn reserved_bytes(&self) -> usize {
        self.allowance
    }
    fn begin(&self) -> Result<Arc<dyn RemoteQuery>, RemoteError> {
        self.node()?.begin()
    }

    fn expand(&self, pattern: &str, cancel: &dyn Fn() -> bool) -> Result<Vec<String>, RemoteError> {
        self.node()?.expand(pattern, cancel)
    }

    fn footer(&self, location: &str, cancel: &dyn Fn() -> bool) -> Result<RemoteFile, RemoteError> {
        self.node()?.footer(location, cancel)
    }
}

/// One binder/execution scope. The first footer stays pinned in its query until
/// the scan consumes it; other bind-time schema checks release their own pins.
struct ScopedObjectIo {
    get: Box<dyn Fn() -> Result<ObjectHandle, RemoteError> + Send + Sync>,
    query: std::sync::OnceLock<Result<Arc<ObjectQuery>, RemoteError>>,
    allowance: usize,
}
impl std::fmt::Debug for ScopedObjectIo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ScopedObjectIo")
    }
}
impl ScopedObjectIo {
    fn new(
        allowance: usize,
        get: impl Fn() -> Result<ObjectHandle, RemoteError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            get: Box::new(get),
            query: std::sync::OnceLock::new(),
            allowance,
        }
    }
    fn query(&self) -> Result<Arc<ObjectQuery>, RemoteError> {
        self.query
            .get_or_init(|| (self.get)()?.0.query())
            .as_ref()
            .map(Arc::clone)
            .map_err(Clone::clone)
    }
}
impl RemoteIo for ScopedObjectIo {
    fn reserved_bytes(&self) -> usize {
        self.allowance
    }
    fn begin(&self) -> Result<Arc<dyn RemoteQuery>, RemoteError> {
        Ok(self.query()?)
    }
    fn expand(&self, pattern: &str, cancel: &dyn Fn() -> bool) -> Result<Vec<String>, RemoteError> {
        (self.get)()?.expand(pattern, cancel)
    }
    fn footer(&self, location: &str, cancel: &dyn Fn() -> bool) -> Result<RemoteFile, RemoteError> {
        let q = self.query()?;
        let file = q.open(location, cancel)?;
        let mut bound = crate::util::lock(&q.bound);
        if bound.is_empty() {
            bound.insert(location.to_string(), file.clone());
        }
        Ok(file)
    }
}

fn has_glob(key: &str) -> bool {
    key.contains(['*', '?', '['])
}

/// The literal directory prefix before the first glob segment, and all the segments of the pattern.
fn split_glob(key: &str) -> (String, Vec<String>) {
    let segs: Vec<String> = key.split('/').map(str::to_string).collect();
    let first = segs
        .iter()
        .position(|s| has_glob(s) || s == "**")
        .unwrap_or(segs.len());
    (segs[..first].join("/"), segs)
}

/// Does the object name `key` match the glob segments (`*`, `?`, `[..]` inside a segment, `**` for any number of directories)?
fn key_matches(pattern: &[String], key: &str) -> bool {
    fn go(p: &[String], k: &[&str]) -> bool {
        match p.split_first() {
            None => k.is_empty(),
            Some((seg, rest)) if seg == "**" => (0..=k.len()).any(|skip| go(rest, &k[skip..])),
            Some((seg, rest)) => match k.split_first() {
                Some((name, krest)) => {
                    nata_sql::glob::segment_matches(seg, name) && go(rest, krest)
                }
                None => false,
            },
        }
    }
    let parts: Vec<&str> = key.split('/').collect();
    go(pattern, &parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &str, k: &str) -> bool {
        key_matches(&p.split('/').map(str::to_string).collect::<Vec<_>>(), k)
    }

    #[test]
    fn globs_match_segments_and_double_star() {
        assert!(m("d/*.parquet", "d/a.parquet"));
        assert!(!m("d/*.parquet", "d/x/a.parquet"));
        assert!(m("d/**/a.parquet", "d/a.parquet"));
        assert!(m("d/**/a.parquet", "d/x/y/a.parquet"));
        assert!(!m("d/**/a.parquet", "d/x/b.parquet"));
        assert!(m("part-[0-9].parquet", "part-3.parquet"));
        assert_eq!(split_glob("a/b/c*.parquet").0, "a/b");
        assert_eq!(split_glob("*.parquet").0, "");
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    #[test]
    fn invalid_admission_is_an_error_inside_an_async_caller() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("caller runtime");
        rt.block_on(async {
            let mut options = NodeOptions::from_env();
            options.cfg.grant_bytes = 1;
            assert!(ObjectNode::new(options).is_err());
        });
    }
}
