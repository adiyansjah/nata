//! The node: one I/O runtime, the per-node attempt cap, the stores, the footer cache; and the query handles the Parquet scan calls from its CPU workers.
//!
//! ADR 0002's split stays: the HTTP work, the retries and the waits for grants run on this module's own Tokio runtime (two threads); decode runs on the
//! scan's worker threads. A worker that needs bytes asks and WAITS for the answer on a plain channel, polling its cancel every few milliseconds, then
//! decodes from memory. (This skeleton blocks the worker while it waits; ADR 0011's parked, sticky-wake bridge is the next step of slice 11. The wait
//! never runs a future on the worker: no `block_on`.)

use std::future::Future;
use std::ops::Range;
use std::sync::Arc;
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

use bytes::Bytes;
use nata_parquet::remote::{
    RemoteBytes, RemoteError, RemoteErrorKind, RemoteFile, RemoteIo, RemoteQuery,
};
use nata_sched::memory::MemoryManager;
use parquet::file::metadata::ParquetMetaData;

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
        }
    }
}

pub struct ObjectNode {
    rt: tokio::runtime::Runtime,
    pub remote: Arc<Remote>,
    pub cache: Option<Arc<MetaCache>>,
    stores: Stores,
    cfg: Config,
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
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("nata-io")
            .enable_all()
            .build()
            .map_err(|_| "cannot start the object-store I/O runtime".to_string())?;
        let cache = o
            .cache
            .then(|| MetaCache::from_config(&o.cfg, o.cache_mutants));
        Ok(Arc::new(ObjectNode {
            rt,
            remote: Remote::new(o.cfg.clone(), o.mutants.clone(), o.seed),
            cache,
            stores: Stores::new(o.s3, o.gcs, o.cfg.clone(), o.mutants),
            cfg: o.cfg,
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
        let task = self.rt.spawn(async move {
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

    /// A query's memory service: budget `B`, a small progress reserve (ADR 0009), one operator (the object-store reads).
    pub fn memory(&self) -> Result<Arc<MemoryManager>, RemoteError> {
        let budget = self.cfg.grant_bytes;
        let progress = (1usize << 20).min(budget / 8).max(1);
        MemoryManager::new(budget, progress, 1).map_err(|_| {
            RemoteError::new(
                RemoteErrorKind::Resource,
                "the object-store memory budget is too small",
            )
        })
    }

    pub fn query(self: &Arc<Self>) -> Result<Arc<ObjectQuery>, RemoteError> {
        let q = self.remote.query(self.memory()?);
        Ok(Arc::new(ObjectQuery {
            node: Arc::clone(self),
            q,
            budget: Budget::new(),
        }))
    }
}

/// A query's handle on the node: the scan calls `open` and `fetch` from its workers.
pub struct ObjectQuery {
    node: Arc<ObjectNode>,
    pub q: Arc<Query>,
    budget: Budget,
}

impl std::fmt::Debug for ObjectQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectQuery").finish_non_exhaustive()
    }
}

impl Drop for ObjectQuery {
    fn drop(&mut self) {
        self.q.cancel_all();
    }
}

/// What `RemoteFile::pinned` holds: the object, its pinned identity, and the cache pin of its footer.
struct Pinned {
    target: Target,
    identity: Identity,
    _handle: Arc<Handle>,
}

impl RemoteQuery for ObjectQuery {
    fn open(&self, location: &str, cancel: &dyn Fn() -> bool) -> Result<RemoteFile, RemoteError> {
        let loc = Location::parse(location)
            .map_err(|m| RemoteError::new(RemoteErrorKind::NotSupported, m))?;
        let (entry, target) = self.node.entry(&loc)?;
        let svc = self.node.service(&entry, &loc);
        let (q, t) = (Arc::clone(&self.q), target.clone());
        let key = format!("{}/{}", loc.bucket, loc.key);
        let opened = self
            .node
            .call(cancel, async move { svc.open(&q, &t, &key).await })
            .map_err(|e| to_remote(&e))?;
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

    fn fetch(
        &self,
        file: &RemoteFile,
        ranges: &[Range<u64>],
        cancel: &dyn Fn() -> bool,
    ) -> Result<RemoteBytes, RemoteError> {
        let pinned = Arc::clone(&file.pinned).downcast::<Pinned>().map_err(|_| {
            RemoteError::new(RemoteErrorKind::Io, "a file opened by another reader")
        })?;
        // one row group's column chunks: planned together (merged only within the allowance of ADR 0011, never across row groups), granted as one unit
        let mut needed: Vec<Range<u64>> = ranges.to_vec();
        needed.sort_by_key(|r| (r.start, r.end));
        let plan = plan_merged(&needed, self.node.cfg(), &self.budget);
        let (q, p, needed2) = (Arc::clone(&self.q), Arc::clone(&pinned), needed.clone());
        let got = self
            .node
            .call(cancel, async move {
                q.read_ranges(&p.target, &p.identity, &needed2, &plan, Kind::Data)
                    .await
            })
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

    fn footer(
        &self,
        location: &str,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Arc<ParquetMetaData>, RemoteError> {
        let q = self.0.query()?;
        Ok(q.open(location, cancel)?.metadata)
    }
}

/// The object-store reader of a process, started on first use: a process that only reads local files never starts the I/O runtime or reads the
/// credential environment. `options` runs once, when the first remote location is touched.
pub struct LazyObjectIo {
    options: Box<dyn Fn() -> NodeOptions + Send + Sync>,
    node: std::sync::OnceLock<Result<ObjectHandle, String>>,
}

impl std::fmt::Debug for LazyObjectIo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LazyObjectIo")
            .field("started", &self.node.get().is_some())
            .finish()
    }
}

impl LazyObjectIo {
    pub fn new(options: impl Fn() -> NodeOptions + Send + Sync + 'static) -> LazyObjectIo {
        LazyObjectIo {
            options: Box::new(options),
            node: std::sync::OnceLock::new(),
        }
    }

    /// ADR 0011/0012 defaults with the credentials of the process environment and the given per-query memory budget in bytes.
    pub fn from_env(query_memory_bytes: usize) -> LazyObjectIo {
        LazyObjectIo::new(move || {
            let mut o = NodeOptions::from_env();
            o.cfg.grant_bytes = query_memory_bytes;
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
    fn begin(&self) -> Result<Arc<dyn RemoteQuery>, RemoteError> {
        self.node()?.begin()
    }

    fn expand(&self, pattern: &str, cancel: &dyn Fn() -> bool) -> Result<Vec<String>, RemoteError> {
        self.node()?.expand(pattern, cancel)
    }

    fn footer(
        &self,
        location: &str,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Arc<ParquetMetaData>, RemoteError> {
        self.node()?.footer(location, cancel)
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
