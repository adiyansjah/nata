//! The CPU-pool side: what travels through the bridge, the queries, and the operators that run on the pool.
//!
//! The pipeline is `ScanSource -> [FilterLe] -> AggSink`, written against the operator protocol of ADR 0001
//! (`api.rs`, included from `spikes/operator-trait`) and fed by the bridge channel of `spikes/async-bridge`.
//! The batch type of that protocol is `Vec<Vec<i64>>`, so Arrow columns are converted to `i64` on the pool: integers
//! and dates as they are, decimals as their unscaled value, one-character strings as their byte. That conversion is
//! CPU work done on the pool, as the ADR wants.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arrow::array::{Array, ArrayRef, AsArray};
use arrow::datatypes::{DataType, Date32Type, Decimal128Type, Int32Type, Int64Type};
use arrow::record_batch::RecordBatch;
use bytes::{Buf, Bytes};
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{ArrowReaderMetadata, ParquetRecordBatchReader, ParquetRecordBatchReaderBuilder};
use parquet::errors::ParquetError;
use parquet::file::reader::{ChunkReader, Length};

use crate::api::*;
use crate::batch::Batch;
use crate::chan::{Recv, Receiver};
use crate::fetch::{CancelToken, FetchStats, Gauge, Guard};
use crate::sched::lock;

// ------------------------------------------------------------------ what travels through the bridge

/// A value counted in a [`Gauge`] from creation until it is dropped.
pub struct Tracked<T> {
    pub v: T,
    _g: Guard,
}

impl<T> Tracked<T> {
    pub fn new(v: T, bytes: usize, gauge: &Arc<Gauge>) -> Self {
        Tracked { v, _g: gauge.track(bytes) }
    }
}

/// The fetched bytes of one unit (one or more row groups of one file) and what is needed to decode them.
pub struct UnitData {
    pub file_size: u64,
    pub pieces: Arc<Vec<(Range<u64>, Bytes)>>,
    pub meta: ArrowReaderMetadata,
    pub rgs: Vec<usize>,
    pub mask: ProjectionMask,
    pub batch_rows: usize,
}

pub enum Item {
    /// Decoded on the I/O runtime (`DecodeMode::Tokio`).
    Batch(Tracked<RecordBatch>),
    /// Raw bytes, decoded on the CPU pool (`DecodeMode::Pool`, what ADR 0002 prescribes).
    Unit(Tracked<UnitData>),
    /// The producer failed; the query must fail.
    Err(String),
    /// The producer finished everything it was given. A channel that closes WITHOUT this is an error, never a short result.
    Done,
}

// ------------------------------------------------------------------ in-memory file for the synchronous decoder

pub struct MemFile {
    pub size: u64,
    pub pieces: Arc<Vec<(Range<u64>, Bytes)>>,
}

impl MemFile {
    fn piece(&self, start: u64, len: u64) -> Option<&(Range<u64>, Bytes)> {
        let i = self.pieces.partition_point(|p| p.0.start <= start);
        let p = self.pieces.get(i.checked_sub(1)?)?;
        (p.0.end >= start + len).then_some(p)
    }
}

impl Length for MemFile {
    fn len(&self) -> u64 {
        self.size
    }
}

impl ChunkReader for MemFile {
    type T = bytes::buf::Reader<Bytes>;

    fn get_read(&self, start: u64) -> parquet::errors::Result<Self::T> {
        let p = self
            .piece(start, 0)
            .ok_or_else(|| ParquetError::General(format!("offset {start} was not fetched")))?;
        let off = (start - p.0.start) as usize;
        Ok(p.1.slice(off..).reader())
    }

    fn get_bytes(&self, start: u64, length: usize) -> parquet::errors::Result<Bytes> {
        let p = self
            .piece(start, length as u64)
            .ok_or_else(|| ParquetError::General(format!("range {start}+{length} was not fetched")))?;
        let off = (start - p.0.start) as usize;
        Ok(p.1.slice(off..off + length))
    }
}

// ------------------------------------------------------------------ queries

/// A query of the shape the spike supports: scan some columns, optional `col <= value` filter, group by 0-2
/// one-character columns, `count(*)` and sums of integer or decimal columns; or `LIMIT n`.
pub struct QuerySpec {
    pub name: &'static str,
    /// The DuckDB statement; `{T}` is replaced by the table expression.
    pub duck_sql: &'static str,
    pub columns: &'static [&'static str],
    /// Decimal scale of each column (0 for integers, dates and characters); checked against the file.
    pub scales: &'static [u32],
    pub filter_le: Option<(usize, i64)>,
    pub group: &'static [usize],
    pub sums: &'static [usize],
    pub limit: Option<usize>,
}

/// `SELECT count(*), sum(l_orderkey), sum(l_extendedprice)`: two columns, no filter, no groups.
pub const Q_SUM: QuerySpec = QuerySpec {
    name: "sum",
    duck_sql: "SELECT count(*), sum(l_orderkey), sum(l_extendedprice) FROM {T}",
    columns: &["l_orderkey", "l_extendedprice"],
    scales: &[0, 2],
    filter_le: None,
    group: &[],
    sums: &[0, 1],
    limit: None,
};

/// A TPC-H Q1-shaped group-by: five columns, a date filter, two group keys.
pub const Q_GRP: QuerySpec = QuerySpec {
    name: "group",
    duck_sql: "SELECT l_returnflag, l_linestatus, count(*), sum(l_quantity), sum(l_extendedprice) FROM {T} \
               WHERE l_shipdate <= DATE '1998-09-02' GROUP BY l_returnflag, l_linestatus",
    columns: &["l_shipdate", "l_returnflag", "l_linestatus", "l_quantity", "l_extendedprice"],
    scales: &[0, 0, 0, 2, 2],
    filter_le: Some((0, 10471)), // 1998-09-02 as days since 1970-01-01; a unit test checks the constant
    group: &[1, 2],
    sums: &[3, 4],
    limit: None,
};

/// `SELECT l_orderkey FROM t LIMIT 1`: stops the whole scan after one row.
pub const Q_LIMIT1: QuerySpec = QuerySpec {
    name: "limit1",
    duck_sql: "SELECT l_orderkey FROM {T} LIMIT 1",
    columns: &["l_orderkey"],
    scales: &[0],
    filter_le: None,
    group: &[],
    sums: &[],
    limit: Some(1),
};

pub fn query(name: &str) -> Option<&'static QuerySpec> {
    [&Q_SUM, &Q_GRP, &Q_LIMIT1].into_iter().find(|q| q.name == name)
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn format_scaled(v: i128, scale: u32) -> String {
    if scale == 0 {
        return v.to_string();
    }
    let d = 10i128.pow(scale);
    format!("{}.{:0w$}", v / d, (v % d).abs(), w = scale as usize)
}

pub type Groups = HashMap<Vec<i64>, (u64, Vec<i128>)>;

/// The text both our engine and the reference produce: one line per group, `|`-separated, sorted bytewise.
pub fn render_groups(spec: &QuerySpec, groups: &Groups) -> String {
    let mut lines: Vec<String> = groups
        .iter()
        .map(|(k, (count, sums))| {
            let mut f: Vec<String> = k
                .iter()
                .map(|&c| char::from_u32(c as u32).map_or_else(|| c.to_string(), |ch| ch.to_string()))
                .collect();
            f.push(count.to_string());
            for (s, &ci) in sums.iter().zip(spec.sums) {
                f.push(format_scaled(*s, spec.scales[ci]));
            }
            f.join("|")
        })
        .collect();
    lines.sort();
    lines.join("\n") + "\n"
}

/// Sort the lines of a result text bytewise (DuckDB's output order is not defined without ORDER BY).
pub fn normalize(text: &str) -> String {
    let mut l: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
    l.sort_unstable();
    l.join("\n") + "\n"
}

// ------------------------------------------------------------------ conversion of Arrow columns (on the pool)

pub fn col_to_i64(arr: &ArrayRef, expected_scale: u32) -> std::result::Result<Vec<i64>, String> {
    if arr.null_count() > 0 {
        return Err("not supported yet: NULL values in a scanned column".into());
    }
    match arr.data_type() {
        DataType::Int64 => Ok(arr.as_primitive::<Int64Type>().values().to_vec()),
        DataType::Int32 => Ok(arr.as_primitive::<Int32Type>().values().iter().map(|&v| i64::from(v)).collect()),
        DataType::Date32 => Ok(arr.as_primitive::<Date32Type>().values().iter().map(|&v| i64::from(v)).collect()),
        DataType::Decimal128(_, s) => {
            if *s < 0 || *s as u32 != expected_scale {
                return Err(format!("decimal scale {s} in the file, {expected_scale} expected"));
            }
            arr.as_primitive::<Decimal128Type>()
                .values()
                .iter()
                .map(|&v| i64::try_from(v).map_err(|_| "decimal does not fit in 64 bits: not supported yet".to_string()))
                .collect()
        }
        DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View | DataType::Dictionary(_, _) => {
            let s = arrow::compute::cast(arr, &DataType::Utf8).map_err(|e| e.to_string())?;
            s.as_string::<i32>()
                .iter()
                .map(|v| match v {
                    Some(x) if x.len() == 1 => Ok(i64::from(x.as_bytes()[0])),
                    _ => Err("not supported yet: string column with values longer than one byte".to_string()),
                })
                .collect()
        }
        t => Err(format!("not supported yet: column type {t:?}")),
    }
}

pub fn to_batch(rb: &RecordBatch, spec: &QuerySpec) -> std::result::Result<Batch, String> {
    let mut cols = Vec::with_capacity(spec.columns.len());
    for (i, name) in spec.columns.iter().enumerate() {
        let a = rb
            .column_by_name(name)
            .ok_or_else(|| format!("column {name} missing from the decoded batch"))?;
        cols.push(col_to_i64(a, spec.scales[i])?);
    }
    Ok(Batch::from_cols(cols))
}

// ------------------------------------------------------------------ the source

/// What the source tells the producers when the query ends before they do.
pub struct ScanCtl {
    pub cancel: Arc<CancelToken>,
    pub stats: Arc<FetchStats>,
}

struct Decoder {
    rd: ParquetRecordBatchReader,
    _unit: Tracked<UnitData>,
}

pub struct ScanSource {
    rx: Receiver<Item>,
    ctl: Arc<ScanCtl>,
    spec: &'static QuerySpec,
    dec: Option<Decoder>,
    held: Option<Tracked<RecordBatch>>,
    done: bool,
}

impl Source for ScanSource {
    fn next(&mut self, waker: &Waker) -> Result<SourceResult> {
        self.held = None;
        loop {
            if let Some(d) = self.dec.as_mut() {
                match d.rd.next() {
                    Some(Ok(rb)) => {
                        let b = to_batch(&rb, self.spec).map_err(QueryError)?;
                        return Ok(SourceResult::Batch(b));
                    }
                    Some(Err(e)) => return err(format!("decode error: {e}")),
                    None => self.dec = None,
                }
            }
            match self.rx.recv(waker) {
                Recv::Item(Item::Batch(tb)) => {
                    let b = to_batch(&tb.v, self.spec).map_err(QueryError)?;
                    self.held = Some(tb);
                    return Ok(SourceResult::Batch(b));
                }
                Recv::Item(Item::Unit(u)) => {
                    let mf = MemFile {
                        size: u.v.file_size,
                        pieces: Arc::clone(&u.v.pieces),
                    };
                    let rd = ParquetRecordBatchReaderBuilder::new_with_metadata(mf, u.v.meta.clone())
                        .with_row_groups(u.v.rgs.clone())
                        .with_projection(u.v.mask.clone())
                        .with_batch_size(u.v.batch_rows)
                        .build()
                        .map_err(|e| QueryError(format!("decode setup: {e}")))?;
                    self.dec = Some(Decoder { rd, _unit: u });
                }
                Recv::Item(Item::Err(m)) => return err(m),
                Recv::Item(Item::Done) => {
                    self.done = true;
                    return Ok(SourceResult::Finished);
                }
                Recv::Blocked => return Ok(SourceResult::Blocked),
                Recv::Closed => {
                    return if self.done {
                        Ok(SourceResult::Finished)
                    } else {
                        err("a producer ended without completing: the scan would have been incomplete")
                    };
                }
            }
        }
    }

    fn finish(&mut self) {
        self.held = None;
        self.dec = None;
        self.rx.close();
        if !self.done {
            // the query ends before this producer did (LIMIT reached, cancelled, or another task failed)
            self.ctl.stats.mark_cancel();
            self.ctl.cancel.cancel();
        }
    }
}

pub struct ScanSources {
    receivers: Mutex<Vec<Option<Receiver<Item>>>>,
    ctl: Arc<ScanCtl>,
    spec: &'static QuerySpec,
}

impl ScanSources {
    pub fn new(receivers: Vec<Receiver<Item>>, ctl: Arc<ScanCtl>, spec: &'static QuerySpec) -> Self {
        ScanSources {
            receivers: Mutex::new(receivers.into_iter().map(Some).collect()),
            ctl,
            spec,
        }
    }
}

impl SourceFactory for ScanSources {
    fn make(&self) -> Box<dyn Source> {
        let rx = lock(&self.receivers)
            .iter_mut()
            .find_map(Option::take)
            .expect("more tasks than receivers");
        Box::new(ScanSource {
            rx,
            ctl: Arc::clone(&self.ctl),
            spec: self.spec,
            dec: None,
            held: None,
            done: false,
        })
    }
}

// ------------------------------------------------------------------ operators and the sink

pub struct FilterLe {
    pub col: usize,
    pub value: i64,
}

impl Operator for FilterLe {
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        let keep: Vec<usize> = (0..input.len()).filter(|&i| input.cols[self.col][i] <= self.value).collect();
        if !keep.is_empty() {
            out.cols = input.cols.iter().map(|c| keep.iter().map(|&i| c[i]).collect()).collect();
        }
        Ok(OpResult::NeedMoreInput)
    }
}

pub struct FilterFactory {
    pub col: usize,
    pub value: i64,
}

impl OperatorFactory for FilterFactory {
    fn make(&self) -> Box<dyn Operator> {
        Box::new(FilterLe { col: self.col, value: self.value })
    }
}

/// Aggregation (or LIMIT) sink. `delay` is slept per batch: a slow consumer, to exercise backpressure.
pub struct AggSink {
    spec: &'static QuerySpec,
    delay: Duration,
    t0: Instant,
    global: Mutex<Groups>,
    first_batch: Mutex<Option<Duration>>,
    limit_left: AtomicUsize,
    rows: Mutex<Vec<i64>>,
    pub batches: AtomicUsize,
}

impl AggSink {
    pub fn new(spec: &'static QuerySpec, delay: Duration, t0: Instant) -> Arc<AggSink> {
        Arc::new(AggSink {
            spec,
            delay,
            t0,
            global: Mutex::new(HashMap::new()),
            first_batch: Mutex::new(None),
            limit_left: AtomicUsize::new(spec.limit.unwrap_or(0)),
            rows: Mutex::new(Vec::new()),
            batches: AtomicUsize::new(0),
        })
    }

    pub fn first_batch(&self) -> Option<Duration> {
        *lock(&self.first_batch)
    }

    /// The result as text. For a LIMIT query: `"<n> row(s)\n"`.
    pub fn result_text(&self) -> String {
        if self.spec.limit.is_some() {
            return format!("{} row(s)\n", lock(&self.rows).len());
        }
        let g = lock(&self.global);
        if self.spec.group.is_empty() && g.is_empty() {
            // an ungrouped aggregate over no rows still has one line
            let z = (0u64, vec![0i128; self.spec.sums.len()]);
            let mut m = HashMap::new();
            m.insert(Vec::new(), z);
            return render_groups(self.spec, &m);
        }
        render_groups(self.spec, &g)
    }
}

/// `GlobalSink` for `Arc<AggSink>` (the trait is implemented on a newtype so the Arc stays usable by the caller).
pub struct AggGlobal(pub Arc<AggSink>);

impl GlobalSink for AggGlobal {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        Box::new(AggLocal {
            sink: Arc::clone(&self.0),
            local: HashMap::new(),
        })
    }
    fn finalize(&self) -> Result<()> {
        Ok(())
    }
}

struct AggLocal {
    sink: Arc<AggSink>,
    local: Groups,
}

impl LocalSink for AggLocal {
    fn push(&mut self, input: &mut Batch, _w: &Waker) -> Result<SinkResult> {
        let s = &*self.sink;
        {
            let mut fb = lock(&s.first_batch);
            if fb.is_none() {
                *fb = Some(s.t0.elapsed());
            }
        }
        s.batches.fetch_add(1, SeqCst);
        if !s.delay.is_zero() {
            std::thread::sleep(s.delay);
        }
        let n = input.len();
        if s.spec.limit.is_some() {
            let mut taken = 0usize;
            let _ = s.limit_left.try_update(SeqCst, SeqCst, |left| {
                taken = n.min(left);
                Some(left - taken)
            });
            lock(&s.rows).extend(input.cols[0].iter().take(taken));
            return Ok(if s.limit_left.load(SeqCst) == 0 {
                SinkResult::Finished
            } else {
                SinkResult::NeedMoreInput
            });
        }
        let spec = s.spec;
        for r in 0..n {
            let key: Vec<i64> = spec.group.iter().map(|&g| input.cols[g][r]).collect();
            let e = self.local.entry(key).or_insert_with(|| (0, vec![0i128; spec.sums.len()]));
            e.0 += 1;
            for (acc, &ci) in e.1.iter_mut().zip(spec.sums) {
                *acc += i128::from(input.cols[ci][r]);
            }
        }
        Ok(SinkResult::NeedMoreInput)
    }

    fn combine(self: Box<Self>) -> Result<()> {
        let mut g = lock(&self.sink.global);
        for (k, (c, sums)) in self.local {
            let e = g.entry(k).or_insert_with(|| (0, vec![0i128; sums.len()]));
            e.0 += c;
            for (a, b) in e.1.iter_mut().zip(sums) {
                *a += b;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_date_constant_is_1998_09_02() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1998, 9, 2), 10471);
        assert_eq!(Q_GRP.filter_le, Some((0, days_from_civil(1998, 9, 2))));
    }

    #[test]
    fn decimals_print_with_their_scale() {
        assert_eq!(format_scaled(123_456, 2), "1234.56");
        assert_eq!(format_scaled(5, 2), "0.05");
        assert_eq!(format_scaled(100, 2), "1.00");
        assert_eq!(format_scaled(42, 0), "42");
    }

    #[test]
    fn memfile_serves_ranges_inside_a_piece_and_refuses_the_rest() {
        let data: Vec<u8> = (0..100u8).collect();
        let mf = MemFile {
            size: 1000,
            pieces: Arc::new(vec![(10..60, Bytes::copy_from_slice(&data[10..60])), (200..300, Bytes::copy_from_slice(&data[..100]))]),
        };
        assert_eq!(mf.get_bytes(10, 5).unwrap().as_ref(), &data[10..15]);
        assert_eq!(mf.get_bytes(55, 5).unwrap().as_ref(), &data[55..60]);
        assert!(mf.get_bytes(55, 6).is_err(), "crosses the end of the piece");
        assert!(mf.get_bytes(0, 5).is_err(), "before any piece");
        assert!(mf.get_bytes(100, 5).is_err(), "between pieces");
        assert_eq!(mf.get_bytes(250, 2).unwrap().as_ref(), &data[50..52]);
    }
}
