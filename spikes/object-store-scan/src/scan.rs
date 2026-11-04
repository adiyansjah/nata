//! The scan: plan from the footers, fetch under one of the policies, decode (on the I/O runtime or on the CPU pool),
//! and hand the result to the CPU pool through the real bridge channel (`chan.rs`, ADR 0002).
//!
//! Fetch policies (a [`Policy`] is the answer to four questions):
//!   * how many requests may be in flight (over the whole query)?            `concurrency`
//!   * are nearby ranges merged into one request, and across what gap?       `gap`
//!   * how many consecutive row groups does one unit of work cover?          `span`
//!   * is the next unit fetched while the current one is decoded and sent?   `prefetch`
//! `A` = 1 request in flight, no merging; `B` = N in flight; `C` = B + merging; `D` = C + prefetch; `E` = C over 8
//! row groups at a time. The parquet async reader (`ParquetRecordBatchStreamBuilder`, `AsyncFileReader`) decodes
//! the bytes; the bytes themselves are fetched by [`crate::fetch::Fetcher`] so that one place counts every request.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures::future::BoxFuture;
use futures::{FutureExt, StreamExt};
use object_store::ObjectStore;
use object_store::path::Path;
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{ArrowReaderMetadata, ArrowReaderOptions};
use parquet::arrow::async_reader::{AsyncFileReader, MetadataFetch, MetadataSuffixFetch, ParquetRecordBatchStreamBuilder};
use parquet::errors::ParquetError;
use parquet::file::metadata::{ParquetMetaData, ParquetMetaDataReader};
use parquet::file::reader::ChunkReader;

use crate::api::Pipeline;
use crate::chan::{Sender, channel};
use crate::exec::*;
use crate::fetch::*;
use crate::sched::Query;
use crate::store::Redactor;
use crate::util::{proc_cpu_seconds, sorted, percentile};

// ------------------------------------------------------------------ configuration

#[derive(Clone, Debug)]
pub struct Policy {
    pub name: String,
    pub concurrency: usize,
    pub gap: Option<u64>,
    pub span: usize,
    pub prefetch: bool,
    pub producers: usize,
}

const MIB: u64 = 1024 * 1024;
/// A merged request is never larger than this.
const MAX_REQUEST: u64 = 64 * MIB;

impl Policy {
    fn mk(name: &str, concurrency: usize, gap: Option<u64>, span: usize, prefetch: bool) -> Policy {
        Policy {
            name: name.into(),
            concurrency,
            gap,
            span,
            prefetch,
            producers: 4,
        }
    }
    /// One request per column chunk, one request in flight at any time, nothing merged, no prefetch.
    pub fn a() -> Policy {
        Policy::mk("A", 1, None, 1, false)
    }
    /// One request per column chunk, up to 16 in flight.
    pub fn b() -> Policy {
        Policy::mk("B", 16, None, 1, false)
    }
    /// B, plus adjacent ranges merged when the gap between them is at most 1 MiB (the `object_store` default).
    pub fn c() -> Policy {
        Policy::mk("C", 16, Some(MIB), 1, false)
    }
    /// C, plus: the next row group of each producer is fetched while the current one is decoded and sent.
    pub fn d() -> Policy {
        Policy::mk("D", 16, Some(MIB), 1, true)
    }
    /// C over 8 consecutive row groups per unit, so ranges of neighbouring row groups can merge.
    pub fn e() -> Policy {
        Policy::mk("E", 16, Some(MIB), 8, false)
    }
    pub fn named(n: &str) -> Option<Policy> {
        Some(match n {
            "A" => Policy::a(),
            "B" => Policy::b(),
            "C" => Policy::c(),
            "D" => Policy::d(),
            "E" => Policy::e(),
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeMode {
    /// The parquet async stream decodes on the I/O runtime and RecordBatches cross the bridge.
    Tokio,
    /// Raw bytes cross the bridge and the CPU pool decodes (ADR 0002's prescription).
    Pool,
}

#[derive(Clone, Copy, Debug)]
pub enum CancelTrigger {
    /// Cancel the query from outside this many ms after it started.
    AfterMs(u64),
    /// Cancel after this many requests have completed.
    AfterRequests(u64),
}

#[derive(Clone, Debug)]
pub struct ScanConfig {
    pub policy: Policy,
    pub decode: DecodeMode,
    /// Capacity of each bridge channel, in items.
    pub cap: usize,
    pub pool_threads: usize,
    pub batch_rows: usize,
    pub retry: RetryPolicy,
    pub mutant: Mutant,
    /// Slept by the consumer per batch (a slow consumer).
    pub consumer_delay: Duration,
    pub footer_hint: usize,
    pub cancel_after: Option<CancelTrigger>,
    /// After this long the `RetryForever` mutant is stopped by a watchdog (and any other run is left alone).
    pub hard_stop_after: Option<Duration>,
}

impl ScanConfig {
    pub fn new(policy: Policy, decode: DecodeMode) -> ScanConfig {
        ScanConfig {
            policy,
            decode,
            cap: 4,
            pool_threads: crate::util::nproc().max(2),
            batch_rows: 16_384,
            retry: RetryPolicy::standard(),
            mutant: Mutant::None,
            consumer_delay: Duration::ZERO,
            footer_hint: 64 * 1024,
            cancel_after: None,
            hard_stop_after: None,
        }
    }
}

pub struct Dataset {
    pub store: Arc<dyn ObjectStore>,
    pub files: Vec<Path>,
    pub redact: Redactor,
}

// ------------------------------------------------------------------ plan

struct FilePlan {
    path: Path,
    size: u64,
    meta: Arc<ParquetMetaData>,
    arrow_meta: ArrowReaderMetadata,
    mask: ProjectionMask,
}

struct Unit {
    file: usize,
    rgs: Vec<usize>,
    /// Column-chunk byte ranges the query needs, sorted.
    ranges: Vec<Range<u64>>,
    needed: u64,
}

struct Plan {
    files: Vec<FilePlan>,
    units: Vec<Unit>,
    row_groups: usize,
}

struct FooterFetch {
    f: Arc<Fetcher>,
    path: Path,
    size: Arc<AtomicUsize>,
}

fn to_pq(e: FetchError) -> ParquetError {
    ParquetError::External(Box::new(e))
}

impl MetadataFetch for &mut FooterFetch {
    fn fetch(&mut self, range: Range<u64>) -> BoxFuture<'_, parquet::errors::Result<Bytes>> {
        let (f, p) = (Arc::clone(&self.f), self.path.clone());
        async move { f.get_range(&p, range, true).await.map_err(to_pq) }.boxed()
    }
}

impl MetadataSuffixFetch for &mut FooterFetch {
    fn fetch_suffix(&mut self, suffix: usize) -> BoxFuture<'_, parquet::errors::Result<Bytes>> {
        let (f, p, size) = (Arc::clone(&self.f), self.path.clone(), Arc::clone(&self.size));
        async move {
            let (b, total) = f.get_suffix(&p, suffix as u64).await.map_err(to_pq)?;
            size.store(total as usize, SeqCst);
            Ok(b)
        }
        .boxed()
    }
}

async fn load_meta(f: &Arc<Fetcher>, path: &Path, hint: usize) -> Result<(Arc<ParquetMetaData>, u64), String> {
    let size = Arc::new(AtomicUsize::new(0));
    let mut ff = FooterFetch {
        f: Arc::clone(f),
        path: path.clone(),
        size: Arc::clone(&size),
    };
    let md = ParquetMetaDataReader::new()
        .with_prefetch_hint(Some(hint))
        .load_via_suffix_and_finish(&mut ff)
        .await
        .map_err(|e| format!("reading the footer failed: {e}"))?;
    Ok((Arc::new(md), size.load(SeqCst) as u64))
}

async fn build_plan(fetcher: &Arc<Fetcher>, ds: &Dataset, spec: &'static QuerySpec, cfg: &ScanConfig) -> Result<Plan, String> {
    let n = cfg.policy.concurrency.max(1);
    let hint = cfg.footer_hint;
    let metas: Vec<Result<(Path, Arc<ParquetMetaData>, u64), String>> = futures::stream::iter(ds.files.iter().cloned())
        .map(|p| {
            let f = Arc::clone(fetcher);
            async move {
                let (m, size) = load_meta(&f, &p, hint).await?;
                Ok((p, m, size))
            }
        })
        .buffered(n)
        .collect()
        .await;
    let mut files = Vec::new();
    let mut units = Vec::new();
    let mut row_groups = 0;
    for (fi, m) in metas.into_iter().enumerate() {
        let (path, meta, size) = m?;
        let descr = meta.file_metadata().schema_descr();
        let mut idx = Vec::new();
        for name in spec.columns {
            let i = descr
                .columns()
                .iter()
                .position(|c| c.name() == *name)
                .ok_or_else(|| format!("column {name} is not in the file"))?;
            idx.push(i);
        }
        let mask = ProjectionMask::roots(descr, idx.iter().copied());
        let arrow_meta = ArrowReaderMetadata::try_new(Arc::clone(&meta), ArrowReaderOptions::new())
            .map_err(|e| format!("schema: {e}"))?;
        let nrg = meta.num_row_groups();
        row_groups += nrg;
        let all: Vec<usize> = (0..nrg).collect();
        for chunk in all.chunks(cfg.policy.span.max(1)) {
            let mut ranges = Vec::new();
            let mut needed = 0u64;
            for &rg in chunk {
                for &ci in &idx {
                    let (s, l) = meta.row_group(rg).column(ci).byte_range();
                    ranges.push(s..s + l);
                    needed += l;
                }
            }
            ranges.sort_by_key(|r| r.start);
            units.push(Unit {
                file: fi,
                rgs: chunk.to_vec(),
                ranges,
                needed,
            });
        }
        files.push(FilePlan {
            path,
            size,
            meta,
            arrow_meta,
            mask,
        });
    }
    Ok(Plan { files, units, row_groups })
}

/// Merge sorted ranges whose gap is at most `gap` (no merged request larger than `MAX_REQUEST`).
pub fn plan_requests(ranges: &[Range<u64>], gap: Option<u64>) -> Vec<Range<u64>> {
    let mut out: Vec<Range<u64>> = Vec::new();
    for r in ranges {
        if let (Some(g), Some(last)) = (gap, out.last_mut()) {
            if r.start <= last.end + g && r.end.max(last.end) - last.start <= MAX_REQUEST {
                last.end = last.end.max(r.end);
                continue;
            }
        }
        out.push(r.clone());
    }
    out
}

// ------------------------------------------------------------------ the async reader over fetched bytes

struct PreloadedReader {
    mf: MemFile,
    meta: Arc<ParquetMetaData>,
}

impl PreloadedReader {
    fn slice(&self, r: Range<u64>) -> parquet::errors::Result<Bytes> {
        self.mf.get_bytes(r.start, (r.end - r.start) as usize)
    }
}

impl AsyncFileReader for PreloadedReader {
    fn get_bytes(&mut self, range: Range<u64>) -> BoxFuture<'_, parquet::errors::Result<Bytes>> {
        let r = self.slice(range);
        async move { r }.boxed()
    }

    fn get_byte_ranges(&mut self, ranges: Vec<Range<u64>>) -> BoxFuture<'_, parquet::errors::Result<Vec<Bytes>>> {
        let r: parquet::errors::Result<Vec<Bytes>> = ranges.into_iter().map(|r| self.slice(r)).collect();
        async move { r }.boxed()
    }

    fn get_metadata<'a>(&'a mut self, _o: Option<&'a ArrowReaderOptions>) -> BoxFuture<'a, parquet::errors::Result<Arc<ParquetMetaData>>> {
        let m = Arc::clone(&self.meta);
        async move { Ok(m) }.boxed()
    }
}

// ------------------------------------------------------------------ producers

struct Shared {
    fetcher: Arc<Fetcher>,
    plan: Plan,
    cancel: Arc<CancelToken>,
    cfg: ScanConfig,
    bridge: Arc<Gauge>,
    alive: AtomicUsize,
    max_item: AtomicUsize,
    max_unit_fetch: AtomicUsize,
}

struct Alive(Arc<Shared>);
impl Alive {
    fn new(s: &Arc<Shared>) -> Alive {
        s.alive.fetch_add(1, SeqCst);
        Alive(Arc::clone(s))
    }
}
impl Drop for Alive {
    fn drop(&mut self) {
        self.0.alive.fetch_sub(1, SeqCst);
    }
}

struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);
impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct Fetched {
    pieces: Arc<Vec<(Range<u64>, Bytes)>>,
    bytes: u64,
}

async fn fetch_unit(sh: &Arc<Shared>, ui: usize) -> Result<Fetched, FetchError> {
    let unit = &sh.plan.units[ui];
    let file = &sh.plan.files[unit.file];
    let reqs = plan_requests(&unit.ranges, sh.cfg.policy.gap);
    let futs = reqs.into_iter().map(|r| {
        let f = Arc::clone(&sh.fetcher);
        let p = file.path.clone();
        async move {
            let b = f.get_range(&p, r.clone(), false).await?;
            Ok::<_, FetchError>((r, b))
        }
    });
    let pieces = futures::future::try_join_all(futs).await?;
    let bytes: u64 = pieces.iter().map(|p| p.1.len() as u64).sum();
    sh.max_unit_fetch.fetch_max(bytes as usize, SeqCst);
    Ok(Fetched {
        pieces: Arc::new(pieces),
        bytes,
    })
}

enum Finish {
    Completed,
    /// The consumer went away.
    Closed,
}

async fn producer_inner(sh: &Arc<Shared>, units: &[usize], tx: &Sender<Item>) -> Result<Finish, String> {
    let ignore_close = sh.cfg.mutant == Mutant::CancelIgnore;
    let mut pre: Option<AbortOnDrop<Result<Fetched, FetchError>>> = None;
    for (k, &ui) in units.iter().enumerate() {
        let fetched = match pre.take() {
            Some(mut h) => match (&mut h.0).await {
                Ok(r) => r,
                Err(e) => Err(FetchError {
                    retryable: false,
                    msg: format!("prefetch task failed: {e}"),
                    attempts: 0,
                }),
            },
            None => fetch_unit(sh, ui).await,
        };
        let fetched = match fetched {
            Ok(f) => f,
            Err(e) => {
                if sh.cfg.mutant == Mutant::SwallowError {
                    continue; // the known-bad version: drop the unit and carry on
                }
                return Err(e.to_string());
            }
        };
        if sh.cfg.policy.prefetch {
            if let Some(&nu) = units.get(k + 1) {
                let sh2 = Arc::clone(sh);
                pre = Some(AbortOnDrop(tokio::spawn(async move {
                    let _a = Alive::new(&sh2);
                    fetch_unit(&sh2, nu).await
                })));
            }
        }
        let unit = &sh.plan.units[ui];
        let file = &sh.plan.files[unit.file];
        match sh.cfg.decode {
            DecodeMode::Pool => {
                let ud = UnitData {
                    file_size: file.size,
                    pieces: Arc::clone(&fetched.pieces),
                    meta: file.arrow_meta.clone(),
                    rgs: unit.rgs.clone(),
                    mask: file.mask.clone(),
                    batch_rows: sh.cfg.batch_rows,
                };
                sh.max_item.fetch_max(fetched.bytes as usize, SeqCst);
                let item = Item::Unit(Tracked::new(ud, fetched.bytes as usize, &sh.bridge));
                if tx.send(item).await.is_err() && !ignore_close {
                    return Ok(Finish::Closed);
                }
            }
            DecodeMode::Tokio => {
                let reader = PreloadedReader {
                    mf: MemFile {
                        size: file.size,
                        pieces: Arc::clone(&fetched.pieces),
                    },
                    meta: Arc::clone(&file.meta),
                };
                let mut stream = ParquetRecordBatchStreamBuilder::new_with_metadata(reader, file.arrow_meta.clone())
                    .with_row_groups(unit.rgs.clone())
                    .with_projection(file.mask.clone())
                    .with_batch_size(sh.cfg.batch_rows)
                    .build()
                    .map_err(|e| format!("decode setup: {e}"))?;
                while let Some(rb) = stream.next().await {
                    let rb = rb.map_err(|e| format!("decode error: {e}"))?;
                    let bytes = rb.get_array_memory_size();
                    sh.max_item.fetch_max(bytes, SeqCst);
                    let item = Item::Batch(Tracked::new(rb, bytes, &sh.bridge));
                    if tx.send(item).await.is_err() && !ignore_close {
                        return Ok(Finish::Closed);
                    }
                }
            }
        }
    }
    Ok(Finish::Completed)
}

async fn producer(sh: Arc<Shared>, units: Vec<usize>, tx: Sender<Item>) {
    let _alive = Alive::new(&sh);
    let work = producer_inner(&sh, &units, &tx);
    let outcome = if matches!(sh.cfg.mutant, Mutant::CancelSendOnly | Mutant::CancelIgnore) {
        work.await
    } else {
        tokio::select! {
            biased;
            _ = sh.cancel.cancelled() => return, // dropping `work` drops every request in flight and aborts the prefetch
            r = work => r,
        }
    };
    match outcome {
        Ok(Finish::Completed) => {
            let _ = tx.send(Item::Done).await;
        }
        Ok(Finish::Closed) => {}
        Err(m) => {
            let _ = tx.send(Item::Err(m)).await;
        }
    }
}

// ------------------------------------------------------------------ running a scan

#[derive(Clone, Debug, Default)]
pub struct ScanOutcome {
    /// The result text when the query succeeded.
    pub text: Option<String>,
    /// The error message when it failed (already redacted).
    pub error: Option<String>,
    pub wall: Duration,
    pub plan_time: Duration,
    pub first_batch: Option<Duration>,
    pub files: usize,
    pub row_groups: usize,
    pub units: usize,
    pub channels: usize,
    /// Bytes of the column chunks the query needs, over the whole plan.
    pub needed_bytes: u64,
    pub data_bytes: u64,
    pub footer_bytes: u64,
    /// Attempts started (a retry is a new attempt).
    pub attempts: u64,
    pub data_requests: u64,
    pub footer_requests: u64,
    pub retries: u64,
    pub failed_attempts: u64,
    pub dropped_attempts: u64,
    pub peak_inflight: usize,
    /// Per-request client-side latency of the successful attempts, ms: until the headers (first byte) and in total.
    pub req_ttfb_ms: Vec<f64>,
    pub req_total_ms: Vec<f64>,
    pub cpu_s: f64,
    // cancellation
    pub cancel_at_s: Option<f64>,
    pub inflight_at_cancel: usize,
    pub started_after_cancel: u64,
    pub completed_after_cancel: u64,
    /// Completed MORE than 50 ms after the cancel: a request that was left running (the race window of a prompt cancel is far shorter).
    pub late_completions_after_cancel: u64,
    pub bytes_after_cancel: u64,
    pub quiesce_ms: Option<f64>,
    pub producers_joined: bool,
    pub alive_after: usize,
    pub inflight_after: usize,
    // memory
    pub bridge_peak: usize,
    pub fetch_peak: usize,
    pub max_item_bytes: usize,
    pub max_unit_fetch_bytes: usize,
    pub cap: usize,
}

impl ScanOutcome {
    pub fn ok(&self) -> bool {
        self.error.is_none()
    }
    pub fn waste_pct(&self) -> f64 {
        if self.needed_bytes == 0 {
            return 0.0;
        }
        100.0 * (self.data_bytes as f64 - self.needed_bytes as f64) / self.needed_bytes as f64
    }
    pub fn total_ms_pct(&self, p: f64) -> f64 {
        percentile(&sorted(self.req_total_ms.clone()), p)
    }
    pub fn ttfb_ms_pct(&self, p: f64) -> f64 {
        percentile(&sorted(self.req_ttfb_ms.clone()), p)
    }
    /// Stated bound on the bytes queued in the bridge: every channel holds at most `cap` items, plus the item its
    /// producer is trying to send and the item its consumer task holds (ADR 0002: capacity + producers + 1).
    pub fn bridge_bound(&self) -> usize {
        self.channels * (self.cap + 2) * self.max_item_bytes
    }
    /// Stated bound on the fetched bytes alive at once: per producer, the unit it is working on, the prefetched one,
    /// and (when raw units cross the bridge) the units queued in its channel.
    pub fn fetch_bound(&self, prefetch: bool, pool_decode: bool) -> usize {
        let per = 1 + usize::from(prefetch) + if pool_decode { self.cap + 1 } else { 0 };
        self.channels * per * self.max_unit_fetch_bytes
    }
}

/// Run one query over `ds` and return everything that was measured. Blocking: call it from a plain thread.
pub fn run_scan(rt: &tokio::runtime::Runtime, ds: &Dataset, spec: &'static QuerySpec, cfg: &ScanConfig) -> ScanOutcome {
    let t0 = Instant::now();
    let cpu0 = proc_cpu_seconds();
    let stats = FetchStats::new();
    let fetcher = Fetcher::new(
        Arc::clone(&ds.store),
        Arc::clone(&stats),
        cfg.retry.clone(),
        cfg.mutant,
        cfg.policy.concurrency,
        ds.redact.clone(),
    );
    let cancel = CancelToken::new();

    // a watchdog that ends a runaway retry loop (only the RetryForever mutant ever needs it)
    let wd_done = Arc::new(AtomicBool::new(false));
    if let Some(d) = cfg.hard_stop_after {
        let (f, done) = (Arc::clone(&fetcher), Arc::clone(&wd_done));
        std::thread::spawn(move || {
            let t = Instant::now();
            while t.elapsed() < d {
                if done.load(SeqCst) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            f.hard_stop.store(true, SeqCst);
        });
    }

    let mut out = ScanOutcome {
        cap: cfg.cap,
        ..Default::default()
    };
    let fill_requests = |out: &mut ScanOutcome| {
        out.attempts = stats.started.load(SeqCst);
        out.retries = stats.retries.load(SeqCst);
        out.failed_attempts = stats.failed.load(SeqCst);
        out.dropped_attempts = stats.dropped.load(SeqCst);
        out.peak_inflight = stats.peak_inflight.load(SeqCst);
        out.data_bytes = stats.bytes_ok_data.load(SeqCst);
        out.footer_bytes = stats.bytes_ok_footer.load(SeqCst);
        out.inflight_after = stats.inflight.load(SeqCst);
        for r in stats.records().iter().filter(|r| r.ok) {
            if r.footer {
                out.footer_requests += 1;
            } else {
                out.data_requests += 1;
                out.req_ttfb_ms.push(r.ttfb_ms);
                out.req_total_ms.push(r.total_ms);
            }
        }
    };

    let plan = match rt.block_on(build_plan(&fetcher, ds, spec, cfg)) {
        Ok(p) => p,
        Err(e) => {
            wd_done.store(true, SeqCst);
            out.error = Some(ds.redact.apply(&e));
            out.wall = t0.elapsed();
            out.plan_time = out.wall;
            out.cpu_s = proc_cpu_seconds() - cpu0;
            fill_requests(&mut out);
            return out;
        }
    };
    out.plan_time = t0.elapsed();
    out.files = plan.files.len();
    out.row_groups = plan.row_groups;
    out.units = plan.units.len();
    out.needed_bytes = plan.units.iter().map(|u| u.needed).sum();

    let nch = cfg.policy.producers.min(plan.units.len()).max(1);
    out.channels = nch;
    let cap = if cfg.mutant == Mutant::Unbounded { 1 << 24 } else { cfg.cap };
    let mut txs = Vec::new();
    let mut rxs = Vec::new();
    for _ in 0..nch {
        let (tx, rx) = channel::<Item>(cap);
        txs.push(tx);
        rxs.push(rx);
    }
    let mut assign: Vec<Vec<usize>> = vec![Vec::new(); nch];
    for ui in 0..plan.units.len() {
        assign[ui % nch].push(ui);
    }
    let sh = Arc::new(Shared {
        fetcher: Arc::clone(&fetcher),
        plan,
        cancel: Arc::clone(&cancel),
        cfg: cfg.clone(),
        bridge: Gauge::new(),
        alive: AtomicUsize::new(0),
        max_item: AtomicUsize::new(0),
        max_unit_fetch: AtomicUsize::new(0),
    });

    let sink = AggSink::new(spec, cfg.consumer_delay, t0);
    let ctl = Arc::new(ScanCtl {
        cancel: Arc::clone(&cancel),
        stats: Arc::clone(&stats),
    });
    let ops: Vec<Arc<dyn crate::api::OperatorFactory>> = match spec.filter_le {
        Some((col, value)) => vec![Arc::new(FilterFactory { col, value })],
        None => vec![],
    };
    let pipeline = Pipeline {
        name: "scan".into(),
        source: Arc::new(ScanSources::new(rxs, ctl, spec)),
        ops,
        sink: Arc::new(AggGlobal(Arc::clone(&sink))),
        deps: vec![],
        tasks: nch,
    };
    let rq = match Query::new(vec![pipeline]).start(cfg.pool_threads) {
        Ok(rq) => rq,
        Err(e) => {
            wd_done.store(true, SeqCst);
            out.error = Some(e.0);
            out.wall = t0.elapsed();
            return out;
        }
    };

    let mut handles = Vec::new();
    for (tx, units) in txs.into_iter().zip(assign) {
        handles.push(rt.spawn(producer(Arc::clone(&sh), units, tx)));
    }
    let aborts: Vec<_> = handles.iter().map(|h| h.abort_handle()).collect();

    if let Some(trig) = cfg.cancel_after {
        match trig {
            CancelTrigger::AfterMs(ms) => std::thread::sleep(Duration::from_millis(ms)),
            CancelTrigger::AfterRequests(n) => {
                let t = Instant::now();
                while stats.completed_ok.load(SeqCst) < n && t.elapsed() < Duration::from_secs(30) {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
        rq.cancel();
    }
    let res = rq.wait();
    out.wall = t0.elapsed();
    out.first_batch = sink.first_batch();
    match res {
        Ok(()) => out.text = Some(sink.result_text()),
        Err(e) => out.error = Some(ds.redact.apply(&e.0)),
    }

    // the producers must all end by themselves: after a cancel, an error or a LIMIT nobody is reading any more
    let joined = rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(20), futures::future::join_all(handles)).await.is_ok()
    });
    let t_joined = Instant::now();
    out.producers_joined = joined;
    if !joined {
        aborts.iter().for_each(|a| a.abort());
        std::thread::sleep(Duration::from_millis(100));
    }
    wd_done.store(true, SeqCst);
    // aborted prefetch tasks are dropped by the runtime a moment after the abort: give them 300 ms to go
    let settle = Instant::now();
    while (sh.alive.load(SeqCst) > 0 || stats.inflight.load(SeqCst) > 0) && settle.elapsed() < Duration::from_millis(300) {
        std::thread::sleep(Duration::from_millis(5));
    }
    out.alive_after = sh.alive.load(SeqCst);
    out.cpu_s = proc_cpu_seconds() - cpu0;
    fill_requests(&mut out);
    if let (Some(at), Some(snap)) = (cancel.at(), stats.cancel_snap()) {
        out.cancel_at_s = Some(snap.t);
        out.inflight_at_cancel = snap.inflight;
        out.started_after_cancel = out.attempts.saturating_sub(snap.started);
        out.completed_after_cancel = stats.completed_ok.load(SeqCst).saturating_sub(snap.completed_ok);
        out.bytes_after_cancel = stats.bytes_ok().saturating_sub(snap.bytes_ok);
        out.quiesce_ms = Some(t_joined.saturating_duration_since(at).as_secs_f64() * 1e3);
        out.late_completions_after_cancel = stats
            .records()
            .iter()
            .filter(|r| r.ok && r.t_start + r.total_ms / 1e3 > snap.t + 0.05)
            .count() as u64;
    }
    out.bridge_peak = sh.bridge.peak();
    out.fetch_peak = fetcher.mem.peak();
    out.max_item_bytes = sh.max_item.load(SeqCst);
    out.max_unit_fetch_bytes = sh.max_unit_fetch.load(SeqCst);
    out
}

/// A dataset made of the files of `layout` in `spec`'s store.
pub fn dataset(store: Arc<dyn ObjectStore>, spec: &crate::store::StoreSpec, layout: &str, files: usize) -> Dataset {
    Dataset {
        store,
        files: (0..files).map(|i| spec.path(&crate::layouts::rel_key(layout, i))).collect(),
        redact: spec.redactor(),
    }
}

/// The same files read straight from a local directory (the CPU-bound control: same decode path, no network).
pub fn local_dataset(dir: &std::path::Path, layout: &str, files: usize) -> Result<Dataset, String> {
    let store = crate::store::local_store(dir)?;
    Ok(Dataset {
        store,
        files: (0..files).map(|i| Path::from(crate::layouts::rel_key(layout, i))).collect(),
        redact: Redactor::default(),
    })
}

/// Plan only (footers, no data): files, row groups, units and the bytes the query needs. Used to size the tests.
pub struct PlanStats {
    pub files: usize,
    pub row_groups: usize,
    pub units: usize,
    pub needed_bytes: u64,
}

pub fn plan_stats(rt: &tokio::runtime::Runtime, ds: &Dataset, spec: &'static QuerySpec, cfg: &ScanConfig) -> Result<PlanStats, String> {
    let fetcher = Fetcher::new(
        Arc::clone(&ds.store),
        FetchStats::new(),
        cfg.retry.clone(),
        Mutant::None,
        cfg.policy.concurrency,
        ds.redact.clone(),
    );
    let plan = rt.block_on(build_plan(&fetcher, ds, spec, cfg))?;
    Ok(PlanStats {
        files: plan.files.len(),
        row_groups: plan.row_groups,
        units: plan.units.len(),
        needed_bytes: plan.units.iter().map(|u| u.needed).sum(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_merged_across_a_gap_up_to_the_limit_and_not_beyond() {
        let r = [0..10, 20..30, 100..110];
        assert_eq!(plan_requests(&r, None), vec![0..10, 20..30, 100..110], "no gap threshold: one request per range");
        assert_eq!(plan_requests(&r, Some(9)), vec![0..10, 20..30, 100..110], "a gap of 10 is larger than 9");
        assert_eq!(plan_requests(&r, Some(10)), vec![0..30, 100..110]);
        assert_eq!(plan_requests(&r, Some(1000)), vec![0..110]);
        // the size limit stops a merge even when the gap allows it
        let big = [0..MAX_REQUEST, MAX_REQUEST + 1..MAX_REQUEST + 5];
        assert_eq!(plan_requests(&big, Some(10)).len(), 2);
        assert!(plan_requests(&[], Some(10)).is_empty());
    }

    #[test]
    fn the_five_policies_differ_in_exactly_the_stated_knobs() {
        let (a, b, c, d, e) = (Policy::a(), Policy::b(), Policy::c(), Policy::d(), Policy::e());
        assert_eq!((a.concurrency, a.gap, a.span, a.prefetch), (1, None, 1, false));
        assert_eq!((b.concurrency, b.gap, b.span, b.prefetch), (16, None, 1, false));
        assert_eq!((c.concurrency, c.gap.is_some(), c.span, c.prefetch), (16, true, 1, false));
        assert_eq!((d.concurrency, d.gap, d.span, d.prefetch), (16, c.gap, 1, true));
        assert_eq!((e.concurrency, e.gap, e.span, e.prefetch), (16, c.gap, 8, false));
        for p in [&a, &b, &c, &d, &e] {
            assert_eq!(p.producers, 4, "same producers in every policy, so only the fetch policy differs");
        }
    }

    #[test]
    fn backoff_doubles_and_is_capped() {
        let p = RetryPolicy::fast_tests();
        assert_eq!(p.nominal_backoff(1), Duration::from_millis(40));
        assert_eq!(p.nominal_backoff(2), Duration::from_millis(80));
        assert_eq!(p.nominal_backoff(3), Duration::from_millis(160));
        assert_eq!(p.nominal_backoff(10), Duration::from_millis(400));
        assert_eq!(p.min_backoff_after(2), Duration::from_millis(40));
    }
}
