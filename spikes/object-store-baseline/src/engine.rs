//! A small scan engine to time the wrapper: footer (cache or not) -> row-group units -> ranges (merged or not) -> fetch under the policy ->
//! decode on a CPU executor -> partial aggregate. The CPU executor is either the split design of ADR 0002 (N plain threads fed through a
//! bounded queue, tokio only for I/O) or the open alternative (a second tokio runtime with N workers). The query set matches DuckDB's SQL
//! for the same four shapes, so results are checked against it before anything is timed.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arrow::array::{Array, AsArray};
use arrow::datatypes::{DataType, Int64Type};
use bytes::{Buf, Bytes};
use futures::{StreamExt, TryStreamExt, stream};
use object_store::path::Path;
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{ArrowReaderMetadata, ArrowReaderOptions, ParquetRecordBatchReaderBuilder};
use parquet::errors::ParquetError;
use parquet::file::metadata::ParquetMetaData;
use parquet::file::reader::{ChunkReader, Length};
use parquet::file::statistics::Statistics;
use tokio::sync::Semaphore;

use crate::cache::MetaService;
use crate::plan::{Budget, Planned, plan_merged, plan_unmerged};
use crate::remote::{Class, Identity, Kind, Query, ReadError};

pub const LINEITEM: [&str; 16] = [
    "l_orderkey", "l_partkey", "l_suppkey", "l_linenumber", "l_quantity", "l_extendedprice", "l_discount", "l_tax", "l_returnflag", "l_linestatus",
    "l_shipdate", "l_commitdate", "l_receiptdate", "l_shipinstruct", "l_shipmode", "l_comment",
];

// ------------------------------------------------------------------------------------------------ queries

#[derive(Clone, Debug, PartialEq)]
pub enum QueryKind {
    /// Every column decoded: `count(*)` and one sum per column (a sum cannot be answered from statistics).
    Full,
    /// Four BIGINT columns summed.
    Proj,
    /// `l_orderkey BETWEEN lo AND hi`: row groups pruned by statistics, `count(*)`, `sum(l_partkey)`.
    Selective { lo: i64, hi: i64 },
    /// Footers only: `count(*)`.
    Meta,
}

impl QueryKind {
    pub fn name(&self) -> &'static str {
        match self {
            QueryKind::Full => "full",
            QueryKind::Proj => "proj",
            QueryKind::Selective { .. } => "sel",
            QueryKind::Meta => "meta",
        }
    }
    pub fn columns(&self) -> Vec<&'static str> {
        match self {
            QueryKind::Full => LINEITEM.to_vec(),
            QueryKind::Proj => vec!["l_orderkey", "l_partkey", "l_suppkey", "l_linenumber"],
            QueryKind::Selective { .. } => vec!["l_orderkey", "l_partkey"],
            QueryKind::Meta => vec![],
        }
    }
    /// The same query for DuckDB; `{T}` is the table expression.
    pub fn sql(&self) -> String {
        match self {
            QueryKind::Full => {
                // every column is decoded: sum() cannot be answered from Parquet statistics (count(col) can, which DuckDB does)
                let agg = LINEITEM
                    .iter()
                    .map(|c| match *c {
                        "l_shipdate" | "l_commitdate" | "l_receiptdate" => format!("sum(date_diff('day', DATE '1970-01-01', {c}))"),
                        "l_returnflag" | "l_linestatus" | "l_shipinstruct" | "l_shipmode" | "l_comment" => format!("sum(strlen({c}))"),
                        _ => format!("sum({c})"),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("SELECT count(*), {agg} FROM {{T}}")
            }
            QueryKind::Proj => "SELECT sum(l_orderkey), sum(l_partkey), sum(l_suppkey), sum(l_linenumber) FROM {T}".to_string(),
            QueryKind::Selective { lo, hi } => format!("SELECT count(*), sum(l_partkey) FROM {{T}} WHERE l_orderkey BETWEEN {lo} AND {hi}"),
            QueryKind::Meta => "SELECT count(*) FROM {T}".to_string(),
        }
    }
    pub fn parse(s: &str, sf: f64) -> Option<QueryKind> {
        match s {
            "full" => Some(QueryKind::Full),
            "proj" => Some(QueryKind::Proj),
            "meta" => Some(QueryKind::Meta),
            "sel" => {
                // TPC-H orderkeys run to about 6,000,000 x SF; take ~1% of the key space in the middle
                let max = (6_000_000.0 * sf) as i64;
                Some(QueryKind::Selective { lo: max * 40 / 100, hi: max * 41 / 100 })
            }
            _ => None,
        }
    }
}

/// An integer with `scale` decimal digits, as DuckDB prints a DECIMAL.
pub fn fmt_scaled(v: i128, scale: u8) -> String {
    if scale == 0 {
        return v.to_string();
    }
    let p = 10i128.pow(scale as u32);
    let (sign, a) = if v < 0 { ("-", -v) } else { ("", v) };
    format!("{sign}{}.{:0width$}", a / p, a % p, width = scale as usize)
}

/// The column sum the `full` query computes for one array, plus its decimal scale.
pub fn full_sum(a: &dyn Array) -> Result<(i128, u8), String> {
    use arrow::array::{Date32Array, Decimal128Array};
    match a.data_type() {
        DataType::Int64 | DataType::Int32 | DataType::Int16 => Ok((as_i64(a)?.iter().flatten().map(|v| v as i128).sum(), 0)),
        DataType::Decimal128(_, sc) => Ok((a.as_any().downcast_ref::<Decimal128Array>().ok_or("decimal")?.iter().flatten().sum(), *sc as u8)),
        DataType::Date32 => Ok((a.as_any().downcast_ref::<Date32Array>().ok_or("date")?.iter().flatten().map(|v| v as i128).sum(), 0)),
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View | DataType::Dictionary(_, _) => {
            let c = arrow::compute::cast(a, &DataType::Utf8).map_err(|e| e.to_string())?;
            Ok((c.as_string::<i32>().iter().flatten().map(|s| s.len() as i128).sum(), 0))
        }
        // a type the `full` query does not define must be an error, never a silently different number
        other => Err(format!("the full query has no sum for {other}")),
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Partial {
    pub rows: u64,
    /// Full: one sum per column of LINEITEM (integers and decimals as their raw sum, dates as days, strings as byte length) and the decimal scales.
    pub cols: [i128; 16],
    pub scales: [u8; 16],
    pub sums: [i128; 4],
    pub sel_count: u64,
    pub sel_sum: i128,
}

impl Partial {
    pub fn merge(&mut self, o: &Partial) {
        self.rows += o.rows;
        for i in 0..16 {
            self.cols[i] += o.cols[i];
            self.scales[i] = self.scales[i].max(o.scales[i]);
        }
        for i in 0..4 {
            self.sums[i] += o.sums[i];
        }
        self.sel_count += o.sel_count;
        self.sel_sum += o.sel_sum;
    }
    /// The text DuckDB's CLI prints for the same query (`-list -separator '|'`).
    pub fn render(&self, q: &QueryKind) -> String {
        match q {
            QueryKind::Full => {
                let mut t = format!("{}", self.rows);
                for i in 0..16 {
                    t += "|";
                    t += &fmt_scaled(self.cols[i], self.scales[i]);
                }
                t
            }
            QueryKind::Proj => format!("{}|{}|{}|{}", self.sums[0], self.sums[1], self.sums[2], self.sums[3]),
            QueryKind::Selective { .. } => {
                if self.sel_count == 0 { "0|NULL".to_string() } else { format!("{}|{}", self.sel_count, self.sel_sum) }
            }
            QueryKind::Meta => format!("{}", self.rows),
        }
    }
}

// ------------------------------------------------------------------------------------------------ decode

/// The fetched byte ranges of one unit, addressed by absolute file offset (what the Parquet reader asks for).
#[derive(Clone)]
pub struct SparseFile {
    segs: Arc<Vec<(u64, Bytes)>>,
    len: u64,
}

impl SparseFile {
    pub fn new(mut segs: Vec<(u64, Bytes)>, len: u64) -> SparseFile {
        segs.sort_by_key(|s| s.0);
        SparseFile { segs: Arc::new(segs), len }
    }
    fn find(&self, start: u64, length: u64) -> Result<Bytes, ParquetError> {
        let end = start + length;
        for (s, b) in self.segs.iter() {
            if *s <= start && s + b.len() as u64 >= end {
                return Ok(b.slice((start - s) as usize..(end - s) as usize));
            }
        }
        Err(ParquetError::General(format!("range {start}..{end} was not fetched")))
    }
}

impl Length for SparseFile {
    fn len(&self) -> u64 {
        self.len
    }
}

impl ChunkReader for SparseFile {
    type T = bytes::buf::Reader<Bytes>;
    fn get_read(&self, start: u64) -> Result<Self::T, ParquetError> {
        for (s, b) in self.segs.iter() {
            if *s <= start && s + b.len() as u64 > start {
                return Ok(b.slice((start - s) as usize..).reader());
            }
        }
        Err(ParquetError::General(format!("offset {start} was not fetched")))
    }
    fn get_bytes(&self, start: u64, length: usize) -> Result<Bytes, ParquetError> {
        self.find(start, length as u64)
    }
}

pub struct FileInfo {
    pub path: Path,
    pub identity: Identity,
    pub meta: Arc<ParquetMetaData>,
    pub arrow: ArrowReaderMetadata,
    pub cols: Vec<usize>,
    pub names: Vec<String>,
}

fn root_index(meta: &ParquetMetaData, name: &str) -> Option<usize> {
    meta.file_metadata().schema_descr().root_schema().get_fields().iter().position(|f| f.name() == name)
}

fn as_i64(a: &dyn Array) -> Result<arrow::array::Int64Array, String> {
    let c = arrow::compute::cast(a, &DataType::Int64).map_err(|e| e.to_string())?;
    Ok(c.as_primitive::<Int64Type>().clone())
}

pub fn decode_unit(fi: &FileInfo, file: SparseFile, rgs: &[usize], q: &QueryKind) -> Result<Partial, String> {
    let mut out = Partial::default();
    let mask = ProjectionMask::roots(fi.meta.file_metadata().schema_descr(), fi.cols.iter().copied());
    for &rg in rgs {
        let reader = ParquetRecordBatchReaderBuilder::new_with_metadata(file.clone(), fi.arrow.clone())
            .with_row_groups(vec![rg])
            .with_projection(mask.clone())
            .with_batch_size(32768)
            .build()
            .map_err(|e| e.to_string())?;
        for batch in reader {
            let batch = batch.map_err(|e| e.to_string())?;
            let schema = batch.schema();
            let col = |n: &str| schema.index_of(n).map(|i| batch.column(i).clone()).map_err(|e| e.to_string());
            match q {
                QueryKind::Full => {
                    out.rows += batch.num_rows() as u64;
                    for (i, n) in LINEITEM.iter().enumerate() {
                        let (v, sc) = full_sum(col(n)?.as_ref())?;
                        out.cols[i] += v;
                        out.scales[i] = sc;
                    }
                }
                QueryKind::Proj => {
                    for (i, n) in ["l_orderkey", "l_partkey", "l_suppkey", "l_linenumber"].iter().enumerate() {
                        let a = as_i64(col(n)?.as_ref())?;
                        out.sums[i] += a.iter().flatten().map(|v| v as i128).sum::<i128>();
                    }
                }
                QueryKind::Selective { lo, hi } => {
                    let k = as_i64(col("l_orderkey")?.as_ref())?;
                    let p = as_i64(col("l_partkey")?.as_ref())?;
                    for i in 0..k.len() {
                        if k.is_valid(i) && (*lo..=*hi).contains(&k.value(i)) {
                            out.sel_count += 1;
                            if p.is_valid(i) {
                                out.sel_sum += p.value(i) as i128;
                            }
                        }
                    }
                }
                QueryKind::Meta => {}
            }
        }
    }
    Ok(out)
}

/// Row groups that can contain a match (statistics only; no statistics = keep).
pub fn prune(meta: &ParquetMetaData, q: &QueryKind) -> Vec<usize> {
    let n = meta.num_row_groups();
    let QueryKind::Selective { lo, hi } = q else { return (0..n).collect() };
    let Some(ci) = root_index(meta, "l_orderkey") else { return (0..n).collect() };
    (0..n)
        .filter(|&i| match meta.row_group(i).column(ci).statistics() {
            Some(Statistics::Int64(s)) => match (s.min_opt(), s.max_opt()) {
                (Some(mn), Some(mx)) => *mx >= *lo && *mn <= *hi,
                _ => true,
            },
            Some(Statistics::Int32(s)) => match (s.min_opt(), s.max_opt()) {
                (Some(mn), Some(mx)) => *mx as i64 >= *lo && *mn as i64 <= *hi,
                _ => true,
            },
            _ => true,
        })
        .collect()
}

// ------------------------------------------------------------------------------------------------ CPU executors

type Job = Box<dyn FnOnce() + Send + 'static>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecKind {
    /// ADR 0002's choice: plain CPU worker threads fed by a bounded queue; tokio does I/O only.
    Split,
    /// The open alternative: a second tokio runtime does the decode (as async tasks that run a non-yielding morsel).
    TokioCpu,
}

pub struct Exec {
    pub kind: ExecKind,
    pub workers: usize,
    /// Bounds queued + running decode jobs (backpressure to the fetch side).
    slots: Arc<Semaphore>,
    tx: Mutex<Option<std::sync::mpsc::Sender<Job>>>,
    threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
    rt: Mutex<Option<tokio::runtime::Runtime>>,
    handle: Option<tokio::runtime::Handle>,
}

impl Exec {
    pub fn new(kind: ExecKind, workers: usize) -> Arc<Exec> {
        let workers = workers.max(1);
        let slots = Arc::new(Semaphore::new(workers * 2));
        match kind {
            ExecKind::Split => {
                let (tx, rx) = std::sync::mpsc::channel::<Job>();
                let rx = Arc::new(Mutex::new(rx));
                let threads = (0..workers)
                    .map(|i| {
                        let rx = rx.clone();
                        std::thread::Builder::new()
                            .name(format!("cpu-{i}"))
                            .spawn(move || {
                                loop {
                                    let job = { rx.lock().unwrap().recv() };
                                    match job {
                                        Ok(j) => j(),
                                        Err(_) => break,
                                    }
                                }
                            })
                            .unwrap()
                    })
                    .collect();
                Arc::new(Exec { kind, workers, slots, tx: Mutex::new(Some(tx)), threads: Mutex::new(threads), rt: Mutex::new(None), handle: None })
            }
            ExecKind::TokioCpu => {
                let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(workers).thread_name("cpu-rt").enable_all().build().unwrap();
                let handle = rt.handle().clone();
                Arc::new(Exec { kind, workers, slots, tx: Mutex::new(None), threads: Mutex::new(Vec::new()), rt: Mutex::new(Some(rt)), handle: Some(handle) })
            }
        }
    }

    /// Run `f` on the CPU side and wait for it, bounded by the slot semaphore.
    pub async fn run<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> T {
        let _slot = self.slots.acquire().await.expect("exec closed");
        match self.kind {
            ExecKind::Split => {
                let (otx, orx) = tokio::sync::oneshot::channel();
                let job: Job = Box::new(move || {
                    let _ = otx.send(f());
                });
                self.tx.lock().unwrap().as_ref().expect("exec closed").send(job).expect("workers gone");
                orx.await.expect("job dropped")
            }
            ExecKind::TokioCpu => self.handle.as_ref().unwrap().spawn(async move { f() }).await.expect("cpu task failed"),
        }
    }

    pub fn shutdown(&self) {
        self.tx.lock().unwrap().take();
        for t in self.threads.lock().unwrap().drain(..) {
            let _ = t.join();
        }
        if let Some(rt) = self.rt.lock().unwrap().take() {
            rt.shutdown_background();
        }
    }
}

impl Drop for Exec {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Keeps every CPU worker busy with 1 ms morsels (the "saturated" condition of the runtime comparison).
pub struct Burner {
    stop: Arc<AtomicBool>,
    done: Arc<AtomicU64>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

fn spin(d: Duration) {
    let t = Instant::now();
    let mut x = 0u64;
    while t.elapsed() < d {
        for i in 0..1000u64 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(i);
        }
        std::hint::black_box(x);
    }
}

impl Burner {
    pub fn start(exec: &Arc<Exec>, rt: &tokio::runtime::Handle, loops: usize) -> Burner {
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicU64::new(0));
        let tasks = (0..loops)
            .map(|_| {
                let (e, s, d) = (exec.clone(), stop.clone(), done.clone());
                rt.spawn(async move {
                    while !s.load(SeqCst) {
                        e.run(|| spin(Duration::from_millis(1))).await;
                        d.fetch_add(1, SeqCst);
                    }
                })
            })
            .collect();
        Burner { stop, done, tasks }
    }
    pub async fn stop(self) -> u64 {
        self.stop.store(true, SeqCst);
        for t in self.tasks {
            let _ = t.await;
        }
        self.done.load(SeqCst)
    }
}

// ------------------------------------------------------------------------------------------------ the scan

#[derive(Clone, Debug)]
pub enum PolicyKind {
    /// The control: one request per needed column chunk, bounded parallelism, no merging.
    Unmerged,
    /// ADR 0011: merge column chunks within the waste allowance; `rg_per_fetch` row groups are planned together (1 = the default,
    /// >1 = the optional cross-row-group merging of card rule 2).
    Merged { rg_per_fetch: usize },
}

#[derive(Debug, Default, Clone)]
pub struct ScanOut {
    pub text: String,
    pub wall: Duration,
    pub first_unit: Duration,
    pub needed_bytes: u64,
    pub fetched_bytes: u64,
    pub units: usize,
}

struct Unit {
    fi: Arc<FileInfo>,
    rgs: Vec<usize>,
}

pub struct ScanCfg {
    pub policy: PolicyKind,
    pub exec: Arc<Exec>,
    pub files_ahead: usize,
    pub units_ahead: usize,
}

fn rg_ranges(fi: &FileInfo, rg: usize) -> Vec<Range<u64>> {
    let rgm = fi.meta.row_group(rg);
    fi.cols
        .iter()
        .map(|&c| {
            let (s, l) = rgm.column(c).byte_range();
            s..s + l
        })
        .collect()
}

pub async fn scan(q: &Arc<Query>, svc: &MetaService, files: &[Path], kind: &QueryKind, sc: &ScanCfg) -> Result<ScanOut, ReadError> {
    let t0 = Instant::now();
    let cfg = q.remote.cfg.clone();
    let budget = Arc::new(Budget::new());
    let first = Arc::new(AtomicU64::new(u64::MAX));
    let needed_total = Arc::new(AtomicU64::new(0));
    let units_n = Arc::new(AtomicU64::new(0));
    let before = q.remote.stats.snap();
    let cols_names: Vec<&str> = kind.columns();

    let opened = stream::iter(files.iter().cloned()).map(|p| async move { svc.open(q, &p).await.map(|o| (p, o)) }).buffered(sc.files_ahead.max(1));
    let units = opened
        .map(|r| {
            let items: Vec<Result<Unit, ReadError>> = match r {
                Err(e) => vec![Err(e)],
                Ok((path, o)) => {
                    let meta = o.handle.meta.clone();
                    let bad = |c: Class| ReadError { class: c, attempts: 0, elapsed: Duration::ZERO };
                    let built = ArrowReaderMetadata::try_new(meta.clone(), ArrowReaderOptions::new()).map_err(|_| bad(Class::BadFooter)).and_then(|arrow| {
                        let cols: Option<Vec<usize>> = cols_names.iter().map(|n| root_index(&meta, n)).collect();
                        cols.map(|cols| (arrow, cols)).ok_or(bad(Class::BadFooter))
                    });
                    match built {
                        Err(e) => vec![Err(e)],
                        Ok((arrow, cols)) => {
                            let fi = Arc::new(FileInfo { path, identity: o.identity, meta: meta.clone(), arrow, names: cols_names.iter().map(|s| s.to_string()).collect(), cols });
                            let kept = prune(&meta, kind);
                            let per = match &sc.policy {
                                PolicyKind::Merged { rg_per_fetch } => (*rg_per_fetch).max(1),
                                PolicyKind::Unmerged => 1,
                            };
                            let mut us: Vec<Result<Unit, ReadError>> = kept.chunks(per).map(|c| Ok(Unit { fi: fi.clone(), rgs: c.to_vec() })).collect();
                            if us.is_empty() {
                                // a file with no surviving row group still counts its rows for the full/meta shapes? (none: pruned away)
                                us.clear();
                            }
                            us
                        }
                    }
                }
            };
            stream::iter(items)
        })
        .flatten();

    let mut total = Partial::default();
    if matches!(kind, QueryKind::Meta) {
        // footers only: count the rows from metadata
        let mut s = Box::pin(units);
        let mut seen = std::collections::HashSet::new();
        while let Some(u) = s.next().await {
            let u = u?;
            if seen.insert(u.fi.path.to_string()) {
                total.rows += u.fi.meta.file_metadata().num_rows() as u64;
            }
        }
        return Ok(ScanOut { text: total.render(kind), wall: t0.elapsed(), ..Default::default() });
    }

    let results = units
        .map_ok(|u| {
            let q = q.clone();
            let (budget, first, needed_total, units_n, cfg) = (budget.clone(), first.clone(), needed_total.clone(), units_n.clone(), cfg.clone());
            let policy = sc.policy.clone();
            let exec = sc.exec.clone();
            let kind = kind.clone();
            async move {
                let mut needed: Vec<Range<u64>> = Vec::new();
                for &rg in &u.rgs {
                    needed.extend(rg_ranges(&u.fi, rg));
                }
                needed_total.fetch_add(needed.iter().map(|r| r.end - r.start).sum(), SeqCst);
                let plan: Vec<Planned> = match &policy {
                    PolicyKind::Unmerged => plan_unmerged(&needed, &cfg),
                    PolicyKind::Merged { .. } => plan_merged(&needed, &cfg, &budget),
                };
                let bytes = q.read_ranges(&u.fi.path, &u.fi.identity, &needed, &plan, Kind::Data).await?;
                let segs: Vec<(u64, Bytes)> = needed.iter().map(|r| r.start).zip(bytes).collect();
                let file = SparseFile::new(segs, u.fi.identity.size);
                let rgs = u.rgs.clone();
                let fi = u.fi.clone();
                let part = exec.run(move || decode_unit(&fi, file, &rgs, &kind)).await.map_err(|_| ReadError { class: Class::BadResponse, attempts: 0, elapsed: Duration::ZERO })?;
                first.fetch_min(t0.elapsed().as_micros() as u64, SeqCst);
                units_n.fetch_add(1, SeqCst);
                Ok::<Partial, ReadError>(part)
            }
        })
        .try_buffer_unordered(sc.units_ahead.max(1));
    futures::pin_mut!(results);
    while let Some(p) = results.try_next().await? {
        total.merge(&p);
    }
    let after = q.remote.stats.snap();
    let d = after.minus(&before);
    Ok(ScanOut {
        text: total.render(kind),
        wall: t0.elapsed(),
        first_unit: Duration::from_micros(first.load(SeqCst).min(u64::MAX / 2)),
        needed_bytes: needed_total.load(SeqCst),
        fetched_bytes: d.ok_bytes[Kind::Data as usize],
        units: units_n.load(SeqCst) as usize,
    })
}
