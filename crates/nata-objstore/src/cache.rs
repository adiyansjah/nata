//! The bounded, identity-validated metadata cache of ADR 0012, and the service that uses it.
//!
//! Key: provider + scope (store/bucket alias + an opaque credential-scope id) + object path + identity token (version/generation, else
//! ETag + length) + reader version. A warm lookup still does the identity lookup (a HEAD, counted as `Kind::Head`) and issues NO footer GET.
//! Charge: parsed metadata size (`ParquetMetaData::memory_size`) + key bytes + bookkeeping, not the footer wire length. Cap: bytes and entries,
//! LRU among UNPINNED entries; a pinned entry (a live `Arc<Handle>`) stays charged even after eviction, until the last handle drops. When room
//! cannot be made, the entry is BYPASSED (served to the query uncached), never admitted over the cap. Concurrent misses share a bounded fill; cancelling the last waiter aborts it.

use futures_util::FutureExt;
use nata_sched::bridge::{Broadcast, Waker};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Mutex, Weak};

use nata_sched::memory::Grant;
use parquet::file::metadata::{ParquetMetaData, ParquetMetaDataReader};

use crate::policy::CacheMutants;
use crate::remote::{Class, Identity, Query, ReadError, Target};
use crate::util::lock;

pub const READER_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MetaKey {
    pub provider: String,
    pub scope: String,
    pub path: String,
    pub identity: String,
    pub reader: u32,
}

impl MetaKey {
    fn bytes(&self) -> u64 {
        (self.provider.capacity()
            + self.scope.capacity()
            + self.path.capacity()
            + self.identity.capacity()
            + 4) as u64
    }
}

/// A parsed footer and the charge it holds in the cache. Dropping the last `Arc<Handle>` returns the charge.
pub struct Handle {
    pub meta: Arc<ParquetMetaData>,
    charge: u64,
    retained: Arc<AtomicU64>,
    /// A bypassed footer (not admitted to the cache) stays charged to the QUERY'S grant for as long as the scan holds it (ADR 0012: hold the parsed
    /// footer under the query grant instead of the cache).
    _hold: Option<Grant>,
    _reservation: Option<Arc<Grant>>,
}

impl Handle {
    /// Bytes this handle holds in the cache's retained total (0 for a bypassed or uncached handle).
    pub fn charge(&self) -> u64 {
        self.charge
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // saturating: the uncharged_pins mutant has already subtracted
        let _ = self
            .retained
            .try_update(SeqCst, SeqCst, |x| Some(x.saturating_sub(self.charge)));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Hit,
    Miss,
    /// Parsed but not admitted (over the entry cap, or no room without evicting a pinned entry).
    Bypass,
}

#[derive(Default, Debug)]
pub struct CacheStats {
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    pub bypasses: AtomicU64,
    pub evictions: AtomicU64,
}

struct Entry {
    handle: Arc<Handle>,
    last: u64,
}

pub struct MetaCache {
    cap_bytes: u64,
    cap_entries: usize,
    max_entry: u64,
    inner: Mutex<(HashMap<MetaKey, Entry>, u64)>,
    /// Bytes charged to live handles, cached or evicted-but-pinned.
    retained: Arc<AtomicU64>,
    pub stats: CacheStats,
    pub mutants: CacheMutants,
    fills: Mutex<HashMap<MetaKey, Weak<Fill>>>,
    reservation: Mutex<Option<Arc<Grant>>>,
}

impl MetaCache {
    /// ADR 0012 targets: 64 MiB, 1,024 entries, 16 MiB per entry.
    pub fn adr(mutants: CacheMutants) -> Arc<MetaCache> {
        MetaCache::new(64 << 20, 1024, 16 << 20, mutants)
    }
    pub fn from_config(cfg: &crate::policy::Config, mutants: CacheMutants) -> Arc<MetaCache> {
        MetaCache::new(
            cfg.cache_bytes,
            cfg.cache_entries,
            cfg.cache_entry_max,
            mutants,
        )
    }
    pub fn new(
        cap_bytes: u64,
        cap_entries: usize,
        max_entry: u64,
        mutants: CacheMutants,
    ) -> Arc<MetaCache> {
        Arc::new(MetaCache {
            cap_bytes,
            cap_entries,
            max_entry,
            inner: Mutex::new((HashMap::new(), 0)),
            retained: Arc::new(AtomicU64::new(0)),
            stats: CacheStats::default(),
            mutants,
            fills: Mutex::new(HashMap::new()),
            reservation: Mutex::new(None),
        })
    }
    pub fn reserve(&self, grant: Grant) {
        *lock(&self.reservation) = Some(Arc::new(grant));
    }
    /// Live subscribers holding shared fills (bounded diagnostic, no keys).
    pub fn fill_waiters(&self) -> usize {
        lock(&self.fills).values().map(Weak::strong_count).sum()
    }
    pub fn retained_bytes(&self) -> u64 {
        self.retained.load(SeqCst)
    }
    pub fn entries(&self) -> usize {
        lock(&self.inner).0.len()
    }
    pub fn cap_bytes(&self) -> u64 {
        self.cap_bytes
    }

    fn norm(&self, k: &MetaKey) -> MetaKey {
        if self.mutants.key_without_identity() {
            MetaKey {
                identity: String::new(),
                ..k.clone()
            }
        } else {
            k.clone()
        }
    }

    pub fn get(&self, key: &MetaKey) -> Option<Arc<Handle>> {
        let key = self.norm(key);
        let mut g = lock(&self.inner);
        g.1 += 1;
        let tick = g.1;
        g.0.get_mut(&key).map(|e| {
            e.last = tick;
            e.handle.clone()
        })
    }

    /// Wrap parsed metadata: admit it when it fits (evicting unpinned LRU entries), otherwise return it as a bypassed, uncharged handle.
    pub fn admit(
        &self,
        key: &MetaKey,
        meta: ParquetMetaData,
        hold: Option<Grant>,
    ) -> (Arc<Handle>, Outcome) {
        let key = self.norm(key);
        let charge = meta.memory_size() as u64 + key.bytes() + 160;
        let mut g = lock(&self.inner);
        g.1 += 1;
        let tick = g.1;
        if let Some(e) = g.0.get_mut(&key) {
            // another query filled it first: keep that one
            e.last = tick;
            return (e.handle.clone(), Outcome::Hit);
        }
        let mut hold = hold;
        let bypass = |meta: ParquetMetaData, s: &MetaCache, hold: Option<Grant>| {
            s.stats.bypasses.fetch_add(1, SeqCst);
            (
                Arc::new(Handle {
                    meta: Arc::new(meta),
                    charge: 0,
                    retained: s.retained.clone(),
                    _hold: hold,
                    _reservation: None,
                }),
                Outcome::Bypass,
            )
        };
        if !self.mutants.no_cap() {
            if charge > self.max_entry || charge > self.cap_bytes {
                return bypass(meta, self, hold.take());
            }
            // make room among unpinned entries, least recently used first
            loop {
                let over_bytes = self.retained.load(SeqCst) + charge > self.cap_bytes;
                let over_entries = g.0.len() >= self.cap_entries;
                if !over_bytes && !over_entries {
                    break;
                }
                let victim =
                    g.0.iter()
                        .filter(|(_, e)| Arc::strong_count(&e.handle) == 1)
                        .min_by_key(|(_, e)| e.last)
                        .map(|(k, _)| k.clone());
                match victim {
                    Some(k) => {
                        g.0.remove(&k);
                        self.stats.evictions.fetch_add(1, SeqCst);
                    }
                    None if self.mutants.uncharged_pins() => {
                        // known-bad: evict the LRU entry although it is pinned, and stop charging it at once
                        let k =
                            g.0.iter()
                                .min_by_key(|(_, e)| e.last)
                                .map(|(k, _)| k.clone());
                        match k.and_then(|k| g.0.remove(&k)) {
                            Some(e) => {
                                self.retained.fetch_sub(
                                    e.handle.charge.min(self.retained.load(SeqCst)),
                                    SeqCst,
                                );
                            }
                            None => break,
                        }
                    }
                    None => return bypass(meta, self, hold.take()), // everything left is pinned
                }
            }
        }
        self.retained.fetch_add(charge, SeqCst);
        let h = Arc::new(Handle {
            meta: Arc::new(meta),
            charge,
            retained: self.retained.clone(),
            _hold: None,
            _reservation: lock(&self.reservation).clone(),
        });
        g.0.insert(
            key,
            Entry {
                handle: h.clone(),
                last: tick,
            },
        );
        (h, Outcome::Miss)
    }
}

type FillResult = Result<(Arc<Handle>, Option<Outcome>), ReadError>;
struct Fill {
    done: Arc<Broadcast<FillResult>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fill {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Footer service: identity lookup, cache lookup, fetch and parse on a miss.
#[derive(Clone)]
pub struct MetaService {
    pub cache: Option<Arc<MetaCache>>,
    pub provider: String,
    pub scope: String,
}

pub struct Opened {
    pub identity: Identity,
    pub handle: Arc<Handle>,
    pub outcome: Option<Outcome>,
}

/// Parsed thrift is far larger than its wire form (measured 3.8-5.5x on honest footers, ADR 0012 amendment); a damaged footer can claim much more.
/// The staging charge taken BEFORE parsing is the bound the local reader already uses for the same reason (`reader.rs`, aggregate footer metadata):
/// 128 bytes per wire byte plus 4 KiB. A parse that would exceed the staging grant cannot be accounted, so it is refused, not published.
pub fn staging_bytes(wire_len: usize) -> Option<u64> {
    (wire_len as u64).checked_mul(128)?.checked_add(4096)
}

impl MetaService {
    pub fn new(cache: Option<Arc<MetaCache>>, provider: &str, scope: &str) -> MetaService {
        MetaService {
            cache,
            provider: provider.to_string(),
            scope: scope.to_string(),
        }
    }

    fn key(&self, location: &str, id: &Identity) -> MetaKey {
        MetaKey {
            provider: self.provider.clone(),
            scope: self.scope.clone(),
            path: location.to_string(),
            identity: id.token(),
            reader: READER_VERSION,
        }
    }

    /// Open `t` (named `location` in the key): pin its identity (HEAD), then serve the footer from the cache or fetch and parse it.
    pub async fn open(&self, q: &Query, t: &Target, location: &str) -> Result<Opened, ReadError> {
        let operation = q.for_read();
        let q = operation.as_ref();
        if let Some(c) = self.cache.as_ref().filter(|c| c.mutants.no_validation()) {
            // known-bad: trust the cache by path, no identity lookup (a replaced object is served stale)
            let found = lock(&c.inner)
                .0
                .iter()
                .filter(|(k, _)| k.path == location && k.scope == self.scope)
                .last()
                .map(|(_, e)| e.handle.clone());
            if let Some(h) = found {
                c.stats.hits.fetch_add(1, SeqCst);
                return Ok(Opened {
                    identity: Identity {
                        size: 0,
                        etag: None,
                        version: None,
                    },
                    handle: h,
                    outcome: Some(Outcome::Hit),
                });
            }
        }
        let id = q.head(t).await?;
        let key = self.key(location, &id);
        if let Some(c) = &self.cache {
            if let Some(h) = c.get(&key) {
                c.stats.hits.fetch_add(1, SeqCst);
                return Ok(Opened {
                    identity: id,
                    handle: h,
                    outcome: Some(Outcome::Hit),
                });
            }
            c.stats.misses.fetch_add(1, SeqCst);
        }
        let (handle, outcome) = if let Some(c) = &self.cache {
            let fill = {
                let mut fills = lock(&c.fills);
                fills.retain(|_, f| f.strong_count() > 0);
                if let Some(fill) = fills.get(&key).and_then(Weak::upgrade) {
                    fill
                } else {
                    // A previous fill may have published between get() and
                    // acquiring the fill table. Do not start another GET.
                    if let Some(handle) = c.get(&key) {
                        c.stats.hits.fetch_add(1, SeqCst);
                        return Ok(Opened {
                            identity: id,
                            handle,
                            outcome: Some(Outcome::Hit),
                        });
                    }
                    if fills.len() >= 32 {
                        return Err(ReadError {
                            class: Class::GrantTooSmall,
                            attempts: 0,
                            elapsed: Default::default(),
                        });
                    }
                    let done = Broadcast::new();
                    let result = done.clone();
                    let svc = self.clone();
                    let q = q.fill_query();
                    let t = t.clone();
                    let id = id.clone();
                    let key2 = key.clone();
                    let task = tokio::spawn(async move {
                        let loaded = std::panic::AssertUnwindSafe(svc.load(&q, &t, &id, &key2))
                            .catch_unwind()
                            .await
                            .unwrap_or_else(|_| {
                                Err(ReadError {
                                    class: Class::BadFooter,
                                    attempts: 0,
                                    elapsed: Default::default(),
                                })
                            });
                        result.complete(loaded);
                    });
                    let fill = Arc::new(Fill { done, task });
                    fills.insert(key.clone(), Arc::downgrade(&fill));
                    fill
                }
            };
            let sub = Broadcast::subscribe(&fill.done, 32).ok_or(ReadError {
                class: Class::GrantTooSmall,
                attempts: 0,
                elapsed: Default::default(),
            })?;
            let result = tokio::select! {
                r = futures_util::future::poll_fn(|cx| {
                    let w = cx.waker().clone();
                    match sub.poll(&Waker::new(move || w.wake_by_ref())) { Some(r) => std::task::Poll::Ready(r), None => std::task::Poll::Pending }
                }) => r,
                _ = q.cancelled() => Err(ReadError { class: Class::Cancelled, attempts: 0, elapsed: Default::default() }),
            };
            drop(sub);
            drop(fill);
            result?
        } else {
            self.load(q, t, &id, &key).await?
        };
        Ok(Opened {
            identity: id,
            handle,
            outcome,
        })
    }
    async fn load(&self, q: &Query, t: &Target, id: &Identity, key: &MetaKey) -> FillResult {
        let bad = |class: Class| ReadError {
            class,
            attempts: 0,
            elapsed: Default::default(),
        };
        let footer = q.footer(t, id).await?;
        let stage = staging_bytes(footer.data.len()).ok_or_else(|| bad(Class::GrantTooSmall))?;
        let mut hold = q.grant(stage).await?;
        let meta = ParquetMetaDataReader::decode_metadata(&footer.data)
            .map_err(|_| bad(Class::BadFooter))?;
        drop(footer); // the wire bytes and their grant are done; the parsed footer is charged by the staging grant or the cache
        if meta.memory_size() as u64 > stage {
            return Err(bad(Class::BadFooter)); // more than the staging bound: refused, never published
        }
        let retained = (meta.memory_size() as u64)
            .checked_add(key.bytes())
            .and_then(|n| n.checked_add(160))
            .filter(|n| *n <= stage)
            .ok_or_else(|| bad(Class::BadFooter))?;
        if let Some(g) = &mut hold {
            g.shrink_to(usize::try_from(retained).map_err(|_| bad(Class::GrantTooSmall))?)
                .map_err(|_| bad(Class::GrantTooSmall))?;
        }
        let result = match &self.cache {
            Some(c) => {
                let (h, o) = c.admit(key, meta, hold);
                (h, Some(o))
            }
            None => (
                Arc::new(Handle {
                    meta: Arc::new(meta),
                    charge: 0,
                    retained: Arc::new(AtomicU64::new(0)),
                    _hold: hold,
                    _reservation: None,
                }),
                None,
            ),
        };
        Ok(result)
    }
}
