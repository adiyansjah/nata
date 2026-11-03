//! Spike for ADR 0001: pull (static partitions) versus push (morsel-driven).
//!
//! Query (TPC-H Q1 shape): scan lineitem -> filter -> project -> hash aggregate.
//! The operator logic (filter, project, aggregate update) is shared; only the
//! driver differs:
//!   pull: each thread owns a fixed range of row groups and pulls batches
//!         through a chain of `Box<dyn PullOp>` (Volcano / DataFusion style).
//!   push: threads take the next row group (morsel) from a shared counter and
//!         push each batch through a chain of transforms into a thread-local
//!         aggregate (DuckDB / HyPer style).
//!
//! What this can and cannot show: it measures per-batch overhead and scaling
//! on a uniform scan. It cannot show skew handling, spill, or window behaviour.
//! Those arguments are made in the ADR from design reasoning, not from this run.

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, BuildHasherDefault, Hasher};
use std::fs::File;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use arrow::array::{Array, AsArray, Date32Array, Float64Array, RecordBatch, Scalar};
use arrow::compute::kernels::{cmp::lt_eq, numeric};
use arrow::compute::{cast, filter_record_batch};
use arrow::datatypes::{DataType, Decimal128Type, Float64Type};
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{
    ArrowReaderMetadata, ArrowReaderOptions, ParquetRecordBatchReader,
    ParquetRecordBatchReaderBuilder,
};

const COLUMNS: [&str; 6] = [
    "l_returnflag",
    "l_linestatus",
    "l_quantity",
    "l_extendedprice",
    "l_discount",
    "l_shipdate",
];
const SHIPDATE_CUTOFF: i32 = 10471; // 1998-09-02
const BATCH_SIZE: usize = 8192;
/// 0 = use `BATCH_SIZE`; the pull-versus-push report sets other sizes to magnify per-batch costs.
static BATCH_OVERRIDE: AtomicUsize = AtomicUsize::new(0);

// ---------- shared operator logic ----------

fn filter(b: &RecordBatch) -> Option<RecordBatch> {
    let d = b.column_by_name("l_shipdate").expect("l_shipdate");
    let cutoff = Scalar::new(Date32Array::from(vec![SHIPDATE_CUTOFF]));
    let pred = lt_eq(d, &cutoff).expect("compare");
    let out = filter_record_batch(b, &pred).expect("filter");
    (out.num_rows() > 0).then_some(out)
}

fn project(b: &RecordBatch) -> RecordBatch {
    let col = |n: &str, t: &DataType| cast(b.column_by_name(n).expect("column"), t).expect("cast");
    let qty = col("l_quantity", &DataType::Float64);
    let price = col("l_extendedprice", &DataType::Float64);
    let disc = col("l_discount", &DataType::Float64);
    let one = Scalar::new(Float64Array::from(vec![1.0]));
    let one_minus_disc = numeric::sub(&one, &disc).expect("sub");
    let disc_price = numeric::mul(&price, &one_minus_disc).expect("mul");
    RecordBatch::try_from_iter([
        ("rf", col("l_returnflag", &DataType::Utf8)),
        ("ls", col("l_linestatus", &DataType::Utf8)),
        ("qty", qty),
        ("price", price),
        ("disc_price", disc_price),
    ])
    .expect("batch")
}

/// One pass over the three decimal columns: no intermediate arrays. Assumes
/// no NULLs (true for TPC-H); a real kernel must handle them.
fn project_fused(b: &RecordBatch) -> RecordBatch {
    let dec = |n: &str| {
        let a = b.column_by_name(n).expect("column").as_primitive::<Decimal128Type>();
        let scale = match a.data_type() {
            DataType::Decimal128(_, s) => 10f64.powi(i32::from(*s)),
            _ => unreachable!("decimal column"),
        };
        (a.values(), scale)
    };
    let ((qv, qs), (pv, ps), (dv, ds)) =
        (dec("l_quantity"), dec("l_extendedprice"), dec("l_discount"));
    let n = b.num_rows();
    let (mut q, mut p, mut dp) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
    for i in 0..n {
        // i128 -> f64 has no hardware instruction; TPC-H values fit in i64.
        let price = pv[i] as i64 as f64 / ps;
        let disc = dv[i] as i64 as f64 / ds;
        q.push(qv[i] as i64 as f64 / qs);
        p.push(price);
        dp.push(price * (1.0 - disc));
    }
    RecordBatch::try_from_iter([
        ("rf", b.column_by_name("l_returnflag").expect("rf").clone()),
        ("ls", b.column_by_name("l_linestatus").expect("ls").clone()),
        ("qty", std::sync::Arc::new(Float64Array::from(q)) as arrow::array::ArrayRef),
        ("price", std::sync::Arc::new(Float64Array::from(p))),
        ("disc_price", std::sync::Arc::new(Float64Array::from(dp))),
    ])
    .expect("batch")
}

#[derive(Default)]
struct AggG<S>(HashMap<(u8, u8), [f64; 4], S>);

type Agg = AggG<RandomState>;

/// First byte of a string key per row, from either a plain string array or a
/// dictionary array (looked up through a small per-batch table).
enum KeyCol<'a> {
    Str(&'a arrow::array::StringArray),
    Dict(&'a arrow::array::Int8Array, Vec<u8>),
}

impl<'a> KeyCol<'a> {
    fn new(a: &'a arrow::array::ArrayRef) -> Self {
        match a.data_type() {
            DataType::Dictionary(..) => {
                let d = a.as_dictionary::<arrow::datatypes::Int8Type>();
                let vals = d.values().as_string::<i32>();
                let table = (0..vals.len()).map(|i| vals.value(i).as_bytes()[0]).collect();
                KeyCol::Dict(d.keys(), table)
            }
            _ => KeyCol::Str(a.as_string::<i32>()),
        }
    }

    #[inline]
    fn get(&self, i: usize) -> u8 {
        match self {
            KeyCol::Str(a) => a.value(i).as_bytes()[0],
            KeyCol::Dict(keys, table) => table[keys.value(i) as usize],
        }
    }
}

/// Minimal multiply-rotate hasher (the "Fx" scheme), instead of SipHash.
#[derive(Default)]
struct Fx(u64);
impl Hasher for Fx {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.write_u64(u64::from(*b));
        }
    }
    fn write_u8(&mut self, i: u8) {
        self.write_u64(u64::from(i));
    }
    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}
type FastAgg = AggG<BuildHasherDefault<Fx>>;

impl<S: BuildHasher + Default> AggG<S> {
    fn update(&mut self, b: &RecordBatch) {
        let rf = KeyCol::new(b.column(0));
        let ls = KeyCol::new(b.column(1));
        let qty = b.column(2).as_primitive::<Float64Type>();
        let price = b.column(3).as_primitive::<Float64Type>();
        let dp = b.column(4).as_primitive::<Float64Type>();
        for i in 0..b.num_rows() {
            let key = (rf.get(i), ls.get(i));
            let e = self.0.entry(key).or_insert([0.0; 4]);
            e[0] += qty.value(i);
            e[1] += price.value(i);
            e[2] += dp.value(i);
            e[3] += 1.0;
        }
    }

    fn merge(&mut self, other: Self) {
        for (k, v) in other.0 {
            let e = self.0.entry(k).or_insert([0.0; 4]);
            for j in 0..4 {
                e[j] += v[j];
            }
        }
    }

    /// Counts must match exactly; f64 sums within 1e-9 relative, because the
    /// summation order depends on thread count.
    fn agrees<T: BuildHasher>(&self, other: &AggG<T>) -> bool {
        self.0.len() == other.0.len()
            && self.0.iter().all(|(k, a)| {
                other.0.get(k).is_some_and(|b| {
                    a[3] == b[3]
                        && (0..3).all(|j| (a[j] - b[j]).abs() <= 1e-9 * a[j].abs().max(1.0))
                })
            })
    }

    fn render(&self) -> String {
        let mut rows: Vec<_> = self.0.iter().collect();
        rows.sort_by_key(|(k, _)| **k);
        rows.iter()
            .map(|((rf, ls), v)| {
                format!(
                    "{},{},{:.2},{:.2},{:.2},{}",
                    *rf as char, *ls as char, v[0], v[1], v[2], v[3] as u64
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// ---------- parquet access ----------

struct Source {
    bytes: Option<bytes::Bytes>,
    path: String,
    meta: ArrowReaderMetadata,
    mask: ProjectionMask,
    row_groups: usize,
}

impl Source {
    fn open(path: &str) -> Self {
        Self::open_as(path, |s| s)
    }

    /// Open with a rewritten Arrow schema (the reader casts while decoding).
    fn open_as(path: &str, f: impl Fn(arrow::datatypes::SchemaRef) -> arrow::datatypes::SchemaRef) -> Self {
        let file = File::open(path).expect("open");
        let plain = ArrowReaderMetadata::load(&file, ArrowReaderOptions::new()).expect("meta");
        let wanted = f(plain.schema().clone());
        let meta = ArrowReaderMetadata::load(&file, ArrowReaderOptions::new().with_schema(wanted)).expect("meta");
        let schema = meta.metadata().file_metadata().schema_descr_ptr();
        let mask = ProjectionMask::columns(&schema, COLUMNS);
        let row_groups = meta.metadata().num_row_groups();
        Source { bytes: None, path: path.to_string(), meta, mask, row_groups }
    }

    fn reader(&self, rg: usize) -> ParquetRecordBatchReader {
        let bs = match BATCH_OVERRIDE.load(Ordering::Relaxed) {
            0 => BATCH_SIZE,
            n => n,
        };
        self.reader_with(rg, self.mask.clone(), bs)
    }

    /// Hold the whole file in memory (as an engine with a local data cache would).
    fn in_memory(mut self) -> Self {
        self.bytes = Some(bytes::Bytes::from(std::fs::read(&self.path).expect("read file")));
        self
    }

    fn reader_with(&self, rg: usize, mask: ProjectionMask, batch_size: usize) -> ParquetRecordBatchReader {
        fn build<T: parquet::file::reader::ChunkReader + 'static>(
            input: T,
            meta: ArrowReaderMetadata,
            mask: ProjectionMask,
            rg: usize,
            batch_size: usize,
        ) -> ParquetRecordBatchReader {
            ParquetRecordBatchReaderBuilder::new_with_metadata(input, meta)
                .with_projection(mask)
                .with_row_groups(vec![rg])
                .with_batch_size(batch_size)
                .build()
                .expect("reader")
        }
        match &self.bytes {
            Some(b) => build(b.clone(), self.meta.clone(), mask, rg, batch_size),
            // Own handle per reader: a cloned fd shares the seek offset and
            // concurrent readers would corrupt each other's reads.
            None => build(File::open(&self.path).expect("open"), self.meta.clone(), mask, rg, batch_size),
        }
    }
}

// ---------- design A: pull, static partitions ----------

trait PullOp {
    fn next(&mut self) -> Option<RecordBatch>;
}

struct Scan<'a> {
    src: &'a Source,
    pending: Vec<usize>,
    cur: Option<ParquetRecordBatchReader>,
}

impl PullOp for Scan<'_> {
    fn next(&mut self) -> Option<RecordBatch> {
        loop {
            if let Some(r) = &mut self.cur {
                match r.next() {
                    Some(b) => return Some(b.expect("decode")),
                    None => self.cur = None,
                }
            }
            let rg = self.pending.pop()?;
            self.cur = Some(self.src.reader(rg));
        }
    }
}

struct Filter<'a>(Box<dyn PullOp + 'a>);
impl PullOp for Filter<'_> {
    fn next(&mut self) -> Option<RecordBatch> {
        loop {
            let b = self.0.next()?;
            if let Some(out) = filter(&b) {
                return Some(out);
            }
        }
    }
}

struct Project<'a>(Box<dyn PullOp + 'a>);
impl PullOp for Project<'_> {
    fn next(&mut self) -> Option<RecordBatch> {
        self.0.next().map(|b| project(&b))
    }
}

fn run_pull(src: &Source, threads: usize) -> Agg {
    let groups: Vec<usize> = (0..src.row_groups).collect();
    let chunk = groups.len().div_ceil(threads);
    std::thread::scope(|s| {
        let handles: Vec<_> = groups
            .chunks(chunk)
            .map(|part| {
                let mut part = part.to_vec();
                part.reverse(); // Scan pops from the back
                s.spawn(move || {
                    let scan = Scan { src, pending: part, cur: None };
                    let mut op: Box<dyn PullOp + '_> =
                        Box::new(Project(Box::new(Filter(Box::new(scan)))));
                    let mut agg = Agg::default();
                    while let Some(b) = op.next() {
                        agg.update(&b);
                    }
                    agg
                })
            })
            .collect();
        let mut total = Agg::default();
        for h in handles {
            total.merge(h.join().expect("thread"));
        }
        total
    })
}

// ---------- v2: exact integer arithmetic, fused filter+project+aggregate ----------

/// Per group: sum(qty) and sum(price) at scale 2, sum(price*(1-disc)) at
/// scale 4, count. i128 accumulators, so sums are exact and independent of
/// thread count.
#[derive(Default)]
struct ExactAgg(HashMap<(u8, u8), [i128; 4], BuildHasherDefault<Fx>>);

impl ExactAgg {
    fn merge(&mut self, other: ExactAgg) {
        for (k, v) in other.0 {
            let e = self.0.entry(k).or_insert([0; 4]);
            for j in 0..4 {
                e[j] += v[j];
            }
        }
    }

    fn to_agg(&self) -> FastAgg {
        let mut a = FastAgg::default();
        for (k, v) in &self.0 {
            a.0.insert(*k, [v[0] as f64 / 100.0, v[1] as f64 / 100.0, v[2] as f64 / 10_000.0, v[3] as f64]);
        }
        a
    }

    fn render(&self) -> String {
        fn dec(v: i128, scale: u32) -> String {
            let p = 10i128.pow(scale);
            format!("{}.{:0w$}", v / p, v % p, w = scale as usize)
        }
        let mut rows: Vec<_> = self.0.iter().collect();
        rows.sort_by_key(|(k, _)| **k);
        rows.iter()
            .map(|((rf, ls), v)| {
                format!("{},{},{},{},{},{}", *rf as char, *ls as char, dec(v[0], 2), dec(v[1], 2), dec(v[2], 4), v[3])
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Filter, project and aggregate one decoded batch in a single loop: no
/// filtered copy, no intermediate arrays, no float conversion, no hashing per
/// row (groups are dense indexes built from the two dictionary keys).
/// Assumes: decimals of scale 2 and precision <= 18, dictionary strings with
/// Int8 keys, no NULLs. A real kernel must handle all three.
fn accumulate<T>(b: &RecordBatch, out: &mut ExactAgg)
where
    T: arrow::array::ArrowPrimitiveType,
    T::Native: Into<i128>,
{
    use arrow::datatypes::{Date32Type, Int8Type};
    let rf = b.column_by_name("l_returnflag").expect("rf").as_dictionary::<Int8Type>();
    let ls = b.column_by_name("l_linestatus").expect("ls").as_dictionary::<Int8Type>();
    let qty = b.column_by_name("l_quantity").expect("qty").as_primitive::<T>().values();
    let price = b.column_by_name("l_extendedprice").expect("price").as_primitive::<T>().values();
    let disc = b.column_by_name("l_discount").expect("disc").as_primitive::<T>().values();
    let date = b.column_by_name("l_shipdate").expect("date").as_primitive::<Date32Type>().values();
    let (rk, lk) = (rf.keys().values(), ls.keys().values());

    let first_byte = |d: &arrow::array::DictionaryArray<Int8Type>| -> Vec<u8> {
        let v = d.values().as_string::<i32>();
        (0..v.len()).map(|i| v.value(i).as_bytes()[0]).collect()
    };
    let (rtab, ltab) = (first_byte(rf), first_byte(ls));
    let nl = ltab.len();
    // 4 copies of every group's accumulators; with V2_IL=1 consecutive rows use
    // different copies, so a run of rows in one group does not serialize on one
    // memory location (store-to-load forwarding).
    static IL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let mask = if *IL.get_or_init(|| std::env::var("V2_IL").is_ok()) { 3 } else { 0 };
    let mut acc = vec![[0i128; 4]; rtab.len() * nl * 4];

    for i in 0..b.num_rows() {
        if date[i] <= SHIPDATE_CUTOFF {
            let a = &mut acc[(rk[i] as usize * nl + lk[i] as usize) * 4 + (i & mask)];
            let (q, p, d): (i128, i128, i128) = (qty[i].into(), price[i].into(), disc[i].into());
            // 1 - disc at scale 2 is (100 - d). Spike-only shortcut: the i64 product
            // falls back to i128 if it would overflow, but `p as i64` truncates for
            // wider values and the i128 `+=` below wraps silently in release builds.
            // Safe only under the precision <= 18, TPC-H-sized assumption; a real
            // kernel must check overflow (or prove a bound from column statistics).
            let m = (p as i64).checked_mul(100 - d as i64).map_or_else(|| p * (100 - d), i128::from);
            a[0] += q;
            a[1] += p;
            a[2] += m;
            a[3] += 1;
        }
    }
    for (ri, &rbyte) in rtab.iter().enumerate() {
        for (li, &lbyte) in ltab.iter().enumerate() {
            let mut a = [0i128; 4];
            for c in 0..4 {
                for j in 0..4 {
                    a[j] += acc[(ri * nl + li) * 4 + c][j];
                }
            }
            if a[3] > 0 {
                let e = out.0.entry((rbyte, lbyte)).or_insert([0; 4]);
                for j in 0..4 {
                    e[j] += a[j];
                }
            }
        }
    }
}

fn run_push_v2<T>(src: &Source, threads: usize) -> ExactAgg
where
    T: arrow::array::ArrowPrimitiveType,
    T::Native: Into<i128>,
{
    let next_morsel = AtomicUsize::new(0);
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut sink = ExactAgg::default();
                    loop {
                        let rg = next_morsel.fetch_add(1, Ordering::Relaxed);
                        if rg >= src.row_groups {
                            break;
                        }
                        for b in src.reader(rg) {
                            accumulate::<T>(&b.expect("decode"), &mut sink);
                        }
                    }
                    sink
                })
            })
            .collect();
        let mut total = ExactAgg::default();
        for h in handles {
            total.merge(h.join().expect("thread"));
        }
        total
    })
}

fn dec64_source(path: &str) -> Source {
    static DEC64: std::sync::LazyLock<Vec<(&'static str, DataType)>> = std::sync::LazyLock::new(|| {
        let dict = DataType::Dictionary(Box::new(DataType::Int8), Box::new(DataType::Utf8));
        let d = DataType::Decimal64(15, 2);
        vec![
            ("l_returnflag", dict.clone()),
            ("l_linestatus", dict),
            ("l_quantity", d.clone()),
            ("l_extendedprice", d.clone()),
            ("l_discount", d),
        ]
    });
    Source::open_as(path, retype(DEC64.as_slice().to_vec().leak()))
}

fn v2_report(path: &str, runs: usize) {
    use arrow::datatypes::Decimal64Type;
    static DEC64: std::sync::LazyLock<Vec<(&'static str, DataType)>> = std::sync::LazyLock::new(|| {
        let dict = DataType::Dictionary(Box::new(DataType::Int8), Box::new(DataType::Utf8));
        let d = DataType::Decimal64(15, 2);
        vec![
            ("l_returnflag", dict.clone()),
            ("l_linestatus", dict),
            ("l_quantity", d.clone()),
            ("l_extendedprice", d.clone()),
            ("l_discount", d),
        ]
    });

    let dict = dict_source(path);
    let reference = run_push_with::<BuildHasherDefault<Fx>>(&dict, 8, true);
    let dict_mem = dict_source(path).in_memory();
    // Decimal64: does the reader support it? Probe without aborting the report.
    let dec64 = std::panic::catch_unwind(|| {
        let s = Source::open_as(path, retype(DEC64.as_slice().to_vec().leak()));
        let n: usize = s.reader(0).map(|b| b.expect("decode").num_rows()).sum();
        (s, n)
    });

    println!("end to end, push/morsel, {runs} timed runs (+1 warm-up); min / median ms\n");
    println!("| variant | 1 thread | 2 | 4 | 8 |\n|---|---:|---:|---:|---:|");
    let mut last = None;
    let mut row = |name: &str, f: &dyn Fn(usize) -> ExactAgg| {
        let mut cells = Vec::new();
        for threads in [1usize, 2, 4, 8] {
            let mut times = Vec::new();
            for i in 0..=runs {
                let t = Instant::now();
                let out = f(threads);
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                assert!(reference.agrees(&out.to_agg()), "{name}: result disagrees");
                if i > 0 {
                    times.push(ms);
                }
                last = Some(out);
            }
            times.sort_by(f64::total_cmp);
            cells.push(format!("{:.0} / {:.0}", times[0], times[times.len() / 2]));
        }
        println!("| {name} | {} |", cells.join(" | "));
    };
    row("v2: fused filter+aggregate, dense groups, exact ints (Decimal128)", &|t| run_push_v2::<arrow::datatypes::Decimal128Type>(&dict, t));
    match &dec64 {
        Ok((s, _)) => row("v2 + reader returns Decimal64", &|t| run_push_v2::<Decimal64Type>(s, t)),
        Err(_) => println!("| v2 + reader returns Decimal64 | not supported by this parquet version |"),
    }
    row("v2 + file held in memory (Decimal128)", &|t| run_push_v2::<arrow::datatypes::Decimal128Type>(&dict_mem, t));
    if let Ok((s, _)) = &dec64 {
        let mem = Source::open_as(path, retype(DEC64.as_slice().to_vec().leak())).in_memory();
        let _ = s;
        row("v2 + Decimal64 + file in memory", &|t| run_push_v2::<Decimal64Type>(&mem, t));
    }
    println!("\nresult (exact):\n{}", last.expect("ran").render());
}

// ---------- design A': pull, but with the same morsel queue as push ----------

/// Pulls from a shared morsel counter, exactly like the push workers do. The only difference
/// from `run_push_with` is the dispatch style: nested `next()` calls instead of a transform list.
struct MorselScan<'a> {
    src: &'a Source,
    next: &'a AtomicUsize,
    cur: Option<ParquetRecordBatchReader>,
}

impl PullOp for MorselScan<'_> {
    fn next(&mut self) -> Option<RecordBatch> {
        loop {
            if let Some(r) = &mut self.cur {
                match r.next() {
                    Some(b) => return Some(b.expect("decode")),
                    None => self.cur = None,
                }
            }
            let rg = self.next.fetch_add(1, Ordering::Relaxed);
            if rg >= self.src.row_groups {
                return None;
            }
            self.cur = Some(self.src.reader(rg));
        }
    }
}

struct ProjectFused<'a>(Box<dyn PullOp + 'a>);
impl PullOp for ProjectFused<'_> {
    fn next(&mut self) -> Option<RecordBatch> {
        self.0.next().map(|b| project_fused(&b))
    }
}

/// Exactly `threads` workers (unlike `run_pull`, which rounded the partition size up).
fn run_pull_morsel<S: BuildHasher + Default + Send>(src: &Source, threads: usize) -> AggG<S> {
    let counter = AtomicUsize::new(0);
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let scan = MorselScan { src, next: &counter, cur: None };
                    let mut op: Box<dyn PullOp + '_> =
                        Box::new(ProjectFused(Box::new(Filter(Box::new(scan)))));
                    let mut agg = AggG::<S>::default();
                    while let Some(b) = op.next() {
                        agg.update(&b);
                    }
                    agg
                })
            })
            .collect();
        let mut total = AggG::<S>::default();
        for h in handles {
            total.merge(h.join().expect("thread"));
        }
        total
    })
}

fn quantile(v: &mut [f64], q: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * q).round() as usize]
}

/// Pull (morsel queue) against push, interleaved, with a pull-against-pull control arm.
fn pullpush_report(path: &str, rounds: usize) {
    type Fx = BuildHasherDefault<self::Fx>;
    let src = dict_source(path);
    let reference = run_push_with::<Fx>(&src, 4, true);
    let arms: [(&str, fn(&Source, usize) -> bool); 3] = [
        ("pull", |s, t| true_if(s, t, 0)),
        ("push", |s, t| true_if(s, t, 1)),
        ("pull2", |s, t| true_if(s, t, 2)),
    ];
    fn true_if(s: &Source, t: usize, arm: usize) -> bool {
        let r = match arm {
            1 => run_push_with::<Fx>(s, t, true),
            _ => run_pull_morsel::<Fx>(s, t),
        };
        let _ = &r;
        REF.with(|x| x.borrow().as_ref().is_none_or(|e| e.agrees(&r)))
    }
    thread_local! { static REF: std::cell::RefCell<Option<FastAgg>> = const { std::cell::RefCell::new(None) }; }
    REF.with(|x| *x.borrow_mut() = Some(reference));

    println!("rounds: {rounds} (+1 warm-up), each cell runs pull, push and a second pull in a rotating order\n");
    println!("| batch rows | threads | pull ms min / median | push ms min / median | push / pull, median of paired ratios [Q1, Q3] | control: pull2 / pull [Q1, Q3] |");
    println!("|---:|---:|---|---|---|---|");
    for bs in [8192usize, 1024, 256] {
        BATCH_OVERRIDE.store(bs, Ordering::Relaxed);
        for threads in [1usize, 2, 4, 8] {
            let mut t: [Vec<f64>; 3] = Default::default();
            for round in 0..=rounds {
                let mut ms = [0.0f64; 3];
                for k in 0..3 {
                    let arm = (round + k) % 3; // rotate the order so no arm is always first
                    let start = Instant::now();
                    assert!((arms[arm].1)(&src, threads), "{} disagrees with the reference", arms[arm].0);
                    ms[arm] = start.elapsed().as_secs_f64() * 1000.0;
                }
                if round > 0 {
                    for a in 0..3 {
                        t[a].push(ms[a]);
                    }
                }
            }
            let mut push_over_pull: Vec<f64> = t[1].iter().zip(&t[0]).map(|(p, q)| p / q).collect();
            let mut control: Vec<f64> = t[2].iter().zip(&t[0]).map(|(p, q)| p / q).collect();
            let fmt = |v: &mut Vec<f64>| {
                format!("{:.3} [{:.3}, {:.3}]", quantile(v, 0.5), quantile(v, 0.25), quantile(v, 0.75))
            };
            let mm = |v: &mut Vec<f64>| format!("{:.0} / {:.0}", quantile(v, 0.0), quantile(v, 0.5));
            println!(
                "| {bs} | {threads} | {} | {} | {} | {} |",
                mm(&mut t[0]), mm(&mut t[1]), fmt(&mut push_over_pull), fmt(&mut control)
            );
        }
    }
    BATCH_OVERRIDE.store(0, Ordering::Relaxed);
}

// ---------- design C: push, morsel-driven ----------

trait Transform {
    fn push(&mut self, b: RecordBatch) -> Option<RecordBatch>;
}
struct FilterT;
impl Transform for FilterT {
    fn push(&mut self, b: RecordBatch) -> Option<RecordBatch> {
        filter(&b)
    }
}
struct ProjectT {
    fused: bool,
}
impl Transform for ProjectT {
    fn push(&mut self, b: RecordBatch) -> Option<RecordBatch> {
        Some(if self.fused { project_fused(&b) } else { project(&b) })
    }
}

fn run_push(src: &Source, threads: usize) -> Agg {
    run_push_with::<RandomState>(src, threads, false)
}

fn run_push_with<S: BuildHasher + Default + Send>(src: &Source, threads: usize, fused: bool) -> AggG<S> {
    let next_morsel = AtomicUsize::new(0);
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut chain: Vec<Box<dyn Transform>> = vec![Box::new(FilterT), Box::new(ProjectT { fused })];
                    let mut sink = AggG::<S>::default();
                    loop {
                        let rg = next_morsel.fetch_add(1, Ordering::Relaxed);
                        if rg >= src.row_groups {
                            break;
                        }
                        for b in src.reader(rg) {
                            let mut cur = Some(b.expect("decode"));
                            for t in chain.iter_mut() {
                                cur = cur.and_then(|b| t.push(b));
                            }
                            if let Some(out) = cur {
                                sink.update(&out);
                            }
                        }
                    }
                    sink
                })
            })
            .collect();
        let mut total = AggG::<S>::default();
        for h in handles {
            total.merge(h.join().expect("thread"));
        }
        total
    })
}

// ---------- harness ----------

/// Single-thread time per stage: [decode, filter, project, aggregate] in ms.
fn stages<S: BuildHasher + Default>(src: &Source, fused: bool) -> [f64; 4] {
    let mut t = [std::time::Duration::ZERO; 4];
    let mut agg = AggG::<S>::default();
    for rg in 0..src.row_groups {
        let mut r = src.reader(rg);
        loop {
            let st = Instant::now();
            let Some(b) = r.next() else { break };
            let b = b.expect("decode");
            t[0] += st.elapsed();
            let st = Instant::now();
            let f = filter(&b);
            t[1] += st.elapsed();
            let Some(f) = f else { continue };
            let st = Instant::now();
            let p = if fused { project_fused(&f) } else { project(&f) };
            t[2] += st.elapsed();
            let st = Instant::now();
            agg.update(&p);
            t[3] += st.elapsed();
        }
    }
    t.map(|d| d.as_secs_f64() * 1000.0)
}

fn best_stages<S: BuildHasher + Default>(src: &Source, fused: bool) -> [f64; 4] {
    (0..8)
        .map(|_| stages::<S>(src, fused))
        .min_by(|a, b| a.iter().sum::<f64>().total_cmp(&b.iter().sum::<f64>()))
        .expect("runs")
}

fn stages_report(src: &Source, dict: &Source) {
    println!("single thread, best of 8 runs (ms): decode / filter / project / aggregate / total\n");
    println!("| variant | decode | filter | project | aggregate | total |\n|---|---:|---:|---:|---:|---:|");
    for (name, s) in [
        ("baseline (SipHash, 5-pass project)", best_stages::<RandomState>(src, false)),
        ("fast hasher", best_stages::<BuildHasherDefault<Fx>>(src, false)),
        ("fused project", best_stages::<RandomState>(src, true)),
        ("fast hasher + fused project", best_stages::<BuildHasherDefault<Fx>>(src, true)),
        ("... + dictionary strings", best_stages::<BuildHasherDefault<Fx>>(dict, true)),
    ] {
        println!(
            "| {name} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} |",
            s[0], s[1], s[2], s[3], s.iter().sum::<f64>()
        );
    }
}

fn fast_report(src: &Source, dict: &Source, runs: usize) {
    println!("end to end, push/morsel, {runs} timed runs (+1 warm-up)\n");
    println!("| variant | threads | min ms | median ms |\n|---|---:|---:|---:|");
    let reference = run_push(src, 8);
    for variant in 0..3 {
        for threads in [1usize, 2, 4, 8] {
            let mut times = Vec::new();
            for i in 0..=runs {
                let t = Instant::now();
                let ok = match variant {
                    0 => reference.agrees(&run_push(src, threads)),
                    1 => reference.agrees(&run_push_with::<BuildHasherDefault<Fx>>(src, threads, true)),
                    _ => reference.agrees(&run_push_with::<BuildHasherDefault<Fx>>(dict, threads, true)),
                };
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                assert!(ok, "result disagrees");
                if i > 0 {
                    times.push(ms);
                }
            }
            times.sort_by(f64::total_cmp);
            println!(
                "| {} | {threads} | {:.0} | {:.0} |",
                ["baseline", "fast hasher + fused project", "... + dictionary strings"][variant],
                times[0],
                times[times.len() / 2]
            );
        }
    }
}

/// Decode-only time (single thread, no operators) for a set of columns.
fn decode_ms(src: &Source, cols: &[&str], batch_size: usize) -> f64 {
    let schema = src.meta.metadata().file_metadata().schema_descr_ptr();
    let mask = ProjectionMask::columns(&schema, cols.iter().copied());
    (0..6)
        .map(|_| {
            let t = Instant::now();
            let mut rows = 0;
            for rg in 0..src.row_groups {
                for b in src.reader_with(rg, mask.clone(), batch_size) {
                    rows += b.expect("decode").num_rows();
                }
            }
            assert_eq!(rows, 6_001_215);
            t.elapsed().as_secs_f64() * 1000.0
        })
        .fold(f64::MAX, f64::min)
}

fn retype(types: &'static [(&'static str, DataType)]) -> impl Fn(arrow::datatypes::SchemaRef) -> arrow::datatypes::SchemaRef {
    move |schema| {
        let fields: Vec<arrow::datatypes::FieldRef> = schema
            .fields()
            .iter()
            .map(|f| match types.iter().find(|(n, _)| n == f.name()) {
                Some((_, t)) => std::sync::Arc::new(arrow::datatypes::Field::new(f.name(), t.clone(), f.is_nullable())),
                None => f.clone(),
            })
            .collect();
        std::sync::Arc::new(arrow::datatypes::Schema::new(fields))
    }
}

/// Do arrow-rs kernels give strict semantics, and what do they cost per 8192-row batch?
fn kernels_report() {
    use arrow::array::{Decimal128Array, Int32Array, Int64Array, StringArray};
    use arrow::compute::{CastOptions, cast_with_options, sum, sum_checked};
    use arrow::compute::kernels::numeric::{add, add_wrapping, div, mul, mul_wrapping, sub, sub_wrapping};
    use std::hint::black_box;

    println!("## Semantics (arrow-rs {})\n", "60.1");
    let show = |name: &str, r: Result<String, arrow::error::ArrowError>| match r {
        Ok(v) => println!("- {name}: ok -> {v}"),
        Err(e) => println!("- {name}: ERROR -> {e}"),
    };
    let max = Int32Array::from(vec![i32::MAX]);
    let one = Scalar::new(Int32Array::from(vec![1]));
    show("i32::MAX + 1, add", add(&max, &one).map(|a| format!("{:?}", a.as_primitive::<arrow::datatypes::Int32Type>().values())));
    show("i32::MAX + 1, add_wrapping", add_wrapping(&max, &one).map(|a| format!("{:?}", a.as_primitive::<arrow::datatypes::Int32Type>().values())));
    let big = Int64Array::from(vec![5_000_000_000i64]);
    let fmt = |a: arrow::array::ArrayRef| format!("{:?} (nulls: {})", a.as_primitive::<arrow::datatypes::Int32Type>().iter().collect::<Vec<_>>(), a.null_count());
    show("cast i64 5e9 -> i32, default options", cast_with_options(&big, &DataType::Int32, &CastOptions::default()).map(fmt));
    show("cast i64 5e9 -> i32, safe=false", cast_with_options(&big, &DataType::Int32, &CastOptions { safe: false, ..Default::default() }).map(fmt));
    let txt = StringArray::from(vec!["abc"]);
    show("cast 'abc' -> i32, default options", cast_with_options(&txt, &DataType::Int32, &CastOptions::default()).map(fmt));
    show("cast 'abc' -> i32, safe=false", cast_with_options(&txt, &DataType::Int32, &CastOptions { safe: false, ..Default::default() }).map(fmt));
    let zero = Scalar::new(Int32Array::from(vec![0]));
    show("1 / 0 (i32)", div(&Int32Array::from(vec![1]), &zero).map(|a| format!("{:?}", a.as_primitive::<arrow::datatypes::Int32Type>().values())));
    let bigs = Int64Array::from(vec![i64::MAX, 1]);
    show("sum([i64::MAX, 1]), sum", Ok(format!("{:?}", sum(&bigs))));
    show("sum([i64::MAX, 1]), sum_checked", sum_checked(&bigs).map(|v| format!("{v:?}")));
    let d = |v: i128, p: u8, sc: i8| Decimal128Array::from(vec![v]).with_precision_and_scale(p, sc).expect("dec");
    show("dec(15,2) 99999.99 * dec(15,2) 99999.99", mul(&d(9_999_999, 15, 2), &d(9_999_999, 15, 2)).map(|a| format!("{:?} -> {}", a.data_type(), a.as_primitive::<arrow::datatypes::Decimal128Type>().value(0))));
    show("dec(38,0) 10^37 * dec(38,0) 100", mul(&d(10i128.pow(37), 38, 0), &d(100, 38, 0)).map(|a| format!("{:?}", a.data_type())));

    println!("\n## Cost: p * (100 - d) over 6,001,215 rows in 8192-row batches\n");
    let n = 6_001_215usize;
    let mut seed = 0x9E3779B97F4A7C15u64;
    let mut next = || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; seed };
    let pv: Vec<i64> = (0..n).map(|_| (next() % 10_500_000) as i64).collect();
    let dv: Vec<i64> = (0..n).map(|_| (next() % 11) as i64).collect();
    let p = Int64Array::from(pv.clone());
    let dd = Int64Array::from(dv.clone());
    let p128 = Decimal128Array::from(pv.iter().map(|v| *v as i128).collect::<Vec<_>>()).with_precision_and_scale(15, 2).expect("dec");
    let d128 = Decimal128Array::from(dv.iter().map(|v| *v as i128).collect::<Vec<_>>()).with_precision_and_scale(15, 2).expect("dec");
    let hundred = Scalar::new(Int64Array::from(vec![100]));
    let hundred128 = Scalar::new(Decimal128Array::from(vec![100i128]).with_precision_and_scale(15, 2).expect("dec"));
    const B: usize = 8192;
    let mut out_a = vec![0i64; B];
    let mut out_b = vec![0i64; B];

    let arrow_i64 = |checked: bool| {
        let mut acc = 0usize;
        for off in (0..n).step_by(B) {
            let len = B.min(n - off);
            let (ps, ds) = (p.slice(off, len), dd.slice(off, len));
            let r = if checked { mul(&ps, &sub(&hundred, &ds).expect("sub")).expect("mul") } else { mul_wrapping(&ps, &sub_wrapping(&hundred, &ds).expect("sub")).expect("mul") };
            acc += black_box(r).len();
        }
        acc
    };
    let hand = |checked: bool, out: &mut Vec<i64>| {
        let mut acc = 0usize;
        for off in (0..n).step_by(B) {
            let len = B.min(n - off);
            let (ps, ds) = (&pv[off..off + len], &dv[off..off + len]);
            if checked {
                for i in 0..len {
                    out[i] = ps[i].checked_mul(100i64.checked_sub(ds[i]).expect("overflow")).expect("overflow");
                }
            } else {
                for i in 0..len {
                    out[i] = ps[i].wrapping_mul(100i64.wrapping_sub(ds[i]));
                }
            }
            acc += black_box(&out[..len]).len();
        }
        acc
    };
    let hand_fused_sum = || {
        let mut total = 0i128;
        for off in (0..n).step_by(B) {
            let len = B.min(n - off);
            let (ps, ds) = (&pv[off..off + len], &dv[off..off + len]);
            let mut s = 0i128;
            for i in 0..len {
                s += i128::from(ps[i].checked_mul(100 - ds[i]).expect("overflow"));
            }
            total += s;
        }
        total
    };
    let arrow_dec = || {
        let mut acc = 0usize;
        for off in (0..n).step_by(B) {
            let len = B.min(n - off);
            let (ps, ds) = (p128.slice(off, len), d128.slice(off, len));
            let r = mul(&ps, &sub(&hundred128, &ds).expect("sub")).expect("mul");
            acc += black_box(r).len();
        }
        acc
    };

    // 10% NULLs in each input (independently), same values elsewhere.
    let mut nulls_rng = 0x2545F4914F6CDD1Du64;
    let mut nnext = || { nulls_rng ^= nulls_rng << 13; nulls_rng ^= nulls_rng >> 7; nulls_rng ^= nulls_rng << 17; nulls_rng };
    let pn = Int64Array::from_iter(pv.iter().map(|v| if nnext() % 10 == 0 { None } else { Some(*v) }));
    let dn = Int64Array::from_iter(dv.iter().map(|v| if nnext() % 10 == 0 { None } else { Some(*v) }));
    let arrow_i64_nulls = || {
        let mut acc = 0usize;
        for off in (0..n).step_by(B) {
            let len = B.min(n - off);
            let (ps, ds) = (pn.slice(off, len), dn.slice(off, len));
            let r = mul(&ps, &sub(&hundred, &ds).expect("sub")).expect("mul");
            acc += black_box(r).len();
        }
        acc
    };
    let mut out_n = vec![0i64; B];
    let hand_nulls = |out: &mut Vec<i64>| {
        let mut acc = 0usize;
        for off in (0..n).step_by(B) {
            let len = B.min(n - off);
            let (ps, ds) = (pn.slice(off, len), dn.slice(off, len));
            let valid = arrow::buffer::NullBuffer::union(ps.nulls(), ds.nulls());
            let (pvals, dvals) = (ps.values(), ds.values());
            match &valid {
                Some(v) => {
                    for i in 0..len {
                        // NULL slots hold garbage; skip them so garbage cannot trigger an overflow error.
                        out[i] = if v.is_valid(i) { pvals[i].checked_mul(100 - dvals[i]).expect("overflow") } else { 0 };
                    }
                }
                None => {
                    for i in 0..len {
                        out[i] = pvals[i].checked_mul(100 - dvals[i]).expect("overflow");
                    }
                }
            }
            acc += black_box((&out[..len], &valid)).0.len();
        }
        acc
    };

    let mut out_nb = vec![0i64; B];
    // Branchless NULL handling: compute every slot, and raise the overflow flag
    // only where the slot is valid (NULL slots hold arbitrary values).
    let hand_nulls_branchless = |out: &mut Vec<i64>| {
        let mut acc = 0usize;
        for off in (0..n).step_by(B) {
            let len = B.min(n - off);
            let (ps, ds) = (pn.slice(off, len), dn.slice(off, len));
            let valid = arrow::buffer::NullBuffer::union(ps.nulls(), ds.nulls()).expect("has nulls");
            let (pvals, dvals) = (ps.values(), ds.values());
            let mut bad = false;
            for i in 0..len {
                let (t, o1) = 100i64.overflowing_sub(dvals[i]);
                let (r, o2) = pvals[i].overflowing_mul(t);
                out[i] = r;
                bad |= (o1 | o2) & valid.is_valid(i);
            }
            assert!(!bad, "overflow");
            acc += black_box((&out[..len], &valid)).0.len();
        }
        acc
    };

    // Same expression on the Decimal128 storage (i128 values), by hand.
    let (p128v, d128v) = (p128.values(), d128.values());
    let mut out128 = vec![0i128; B];
    let hand128 = |out: &mut Vec<i128>| {
        let mut acc = 0usize;
        for off in (0..n).step_by(B) {
            let len = B.min(n - off);
            let (ps, ds) = (&p128v[off..off + len], &d128v[off..off + len]);
            for i in 0..len {
                out[i] = ps[i].checked_mul(100i128.checked_sub(ds[i]).expect("overflow")).expect("overflow");
            }
            acc += black_box(&out[..len]).len();
        }
        acc
    };
    // Narrow to i64 first (valid when precision <= 18), multiply in i64, widen back.
    let mut out128n = vec![0i128; B];
    let hand128_narrow = |out: &mut Vec<i128>| {
        let mut acc = 0usize;
        for off in (0..n).step_by(B) {
            let len = B.min(n - off);
            let (ps, ds) = (&p128v[off..off + len], &d128v[off..off + len]);
            for i in 0..len {
                out[i] = i128::from((ps[i] as i64).checked_mul(100 - ds[i] as i64).expect("overflow"));
            }
            acc += black_box(&out[..len]).len();
        }
        acc
    };

    // Two primitive passes (as a vectorized interpreter would run `100 - d` then `p * t`), checked,
    // with a reused temporary and output: isolates "own kernels" from "fused expression".
    let mut tmp2 = vec![0i64; B];
    let mut out2 = vec![0i64; B];
    let hand_two_pass = |tmp: &mut Vec<i64>, out: &mut Vec<i64>| {
        let mut acc = 0usize;
        for off in (0..n).step_by(B) {
            let len = B.min(n - off);
            let (ps, ds) = (&pv[off..off + len], &dv[off..off + len]);
            for i in 0..len {
                tmp[i] = 100i64.checked_sub(ds[i]).expect("overflow");
            }
            for i in 0..len {
                out[i] = ps[i].checked_mul(tmp[i]).expect("overflow");
            }
            acc += black_box(&out[..len]).len();
        }
        acc
    };
    // Same, with branch-free overflow flags (one check per pass, not per row).
    let mut tmp3 = vec![0i64; B];
    let mut out3 = vec![0i64; B];
    let hand_two_pass_flags = |tmp: &mut Vec<i64>, out: &mut Vec<i64>| {
        let mut acc = 0usize;
        for off in (0..n).step_by(B) {
            let len = B.min(n - off);
            let (ps, ds) = (&pv[off..off + len], &dv[off..off + len]);
            let mut bad = false;
            for i in 0..len {
                let (t, o) = 100i64.overflowing_sub(ds[i]);
                tmp[i] = t;
                bad |= o;
            }
            for i in 0..len {
                let (r, o) = ps[i].overflowing_mul(tmp[i]);
                out[i] = r;
                bad |= o;
            }
            assert!(!bad, "overflow");
            acc += black_box(&out[..len]).len();
        }
        acc
    };

    // Fair-allocation variants: the hand kernels build a fresh output array per batch, as arrow does.
    let hand_i64_alloc = || {
        let mut acc = 0usize;
        for off in (0..n).step_by(B) {
            let len = B.min(n - off);
            let (ps, ds) = (&pv[off..off + len], &dv[off..off + len]);
            let v: Vec<i64> = (0..len).map(|i| ps[i].checked_mul(100 - ds[i]).expect("overflow")).collect();
            acc += black_box(Int64Array::from(v)).len();
        }
        acc
    };
    let hand_narrow_alloc = || {
        let mut acc = 0usize;
        for off in (0..n).step_by(B) {
            let len = B.min(n - off);
            let (ps, ds) = (&p128v[off..off + len], &d128v[off..off + len]);
            let v: Vec<i128> = (0..len)
                .map(|i| i128::from((ps[i] as i64).checked_mul(100 - ds[i] as i64).expect("overflow")))
                .collect();
            let a = Decimal128Array::from(v).with_precision_and_scale(31, 4).expect("dec");
            acc += black_box(a).len();
        }
        acc
    };
    // Arrow Decimal64 kernels: does a (15,2) * (16,2) multiply even produce a (31,4) result?
    use arrow::array::Decimal64Array;
    let p64 = Decimal64Array::from(pv.clone()).with_precision_and_scale(15, 2).expect("dec");
    let d64 = Decimal64Array::from(dv.clone()).with_precision_and_scale(15, 2).expect("dec");
    let hundred64 = Scalar::new(Decimal64Array::from(vec![100i64]).with_precision_and_scale(15, 2).expect("dec"));
    let probe = sub(&hundred64, &d64.slice(0, 8)).and_then(|t| mul(&p64.slice(0, 8), &t));
    match &probe {
        Ok(a) => println!("\narrow Decimal64 (15,2) * (16,2): result type {:?}", a.data_type()),
        Err(e) => println!("\narrow Decimal64 (15,2) * (16,2): ERROR -> {e}"),
    }
    let arrow_dec64 = || {
        let mut acc = 0usize;
        for off in (0..n).step_by(B) {
            let len = B.min(n - off);
            let (ps, ds) = (p64.slice(off, len), d64.slice(off, len));
            let r = mul(&ps, &sub(&hundred64, &ds).expect("sub")).expect("mul");
            acc += black_box(r).len();
        }
        acc
    };

    type Bench<'a> = (&'a str, Box<dyn FnMut() + 'a>);
    let mut benches: Vec<Bench> = vec![
        ("arrow i64, checked (2 kernels, 2 temp arrays)", Box::new(|| { black_box(arrow_i64(true)); })),
        ("arrow i64, wrapping", Box::new(|| { black_box(arrow_i64(false)); })),
        ("hand loop i64, checked, one pass, reused output", Box::new(|| { black_box(hand(true, &mut out_a)); })),
        ("hand loop i64, wrapping, one pass, reused output", Box::new(|| { black_box(hand(false, &mut out_b)); })),
        ("hand loop i64, checked, fused with sum (no output array)", Box::new(|| { black_box(hand_fused_sum()); })),
        ("arrow i64, checked, 10% NULLs per input", Box::new(|| { black_box(arrow_i64_nulls()); })),
        ("hand loop i64, checked, 10% NULLs per input (validity union + per-row test)", Box::new(|| { black_box(hand_nulls(&mut out_n)); })),
        ("hand loop i64, checked, 10% NULLs, branchless (overflow flag masked by validity)", Box::new(|| { black_box(hand_nulls_branchless(&mut out_nb)); })),
        ("hand i64, TWO passes (sub, then mul), checked per row, reused temp and output", Box::new(|| { black_box(hand_two_pass(&mut tmp2, &mut out2)); })),
        ("hand i64, TWO passes, overflow flag per pass (branch-free)", Box::new(|| { black_box(hand_two_pass_flags(&mut tmp3, &mut out3)); })),
        ("hand loop i64, checked, allocates an Int64Array per batch", Box::new(|| { black_box(hand_i64_alloc()); })),
        ("hand loop Decimal128 narrowed to i64, allocates a Decimal128Array(31,4) per batch", Box::new(|| { black_box(hand_narrow_alloc()); })),
        ("arrow Decimal128(15,2), checked (2 kernels)", Box::new(|| { black_box(arrow_dec()); })),
        ("arrow Decimal64(15,2), checked (2 kernels), only if the multiply is accepted", Box::new(|| { if probe.is_ok() { black_box(arrow_dec64()); } })),
        ("hand loop on Decimal128 storage, i128 checked, one pass", Box::new(|| { black_box(hand128(&mut out128)); })),
        ("hand loop on Decimal128 storage, narrowed to i64, checked", Box::new(|| { black_box(hand128_narrow(&mut out128n)); })),
    ];
    let mut times: Vec<Vec<f64>> = vec![Vec::new(); benches.len()];
    for round in 0..16 {
        for (i, (_, f)) in benches.iter_mut().enumerate() {
            let t = Instant::now();
            f();
            if round > 0 {
                times[i].push(t.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }
    println!("15 timed rounds after 1 warm-up, variants alternating in each round; ms for all 6M rows\n\n| variant | min | median | ns/row (min) |\n|---|---:|---:|---:|");
    for ((name, _), mut t) in benches.iter().zip(times) {
        t.sort_by(f64::total_cmp);
        println!("| {name} | {:.1} | {:.1} | {:.2} |", t[0], t[t.len() / 2], t[0] * 1e6 / n as f64);
    }
}

fn dict_source(path: &str) -> Source {
    static DICT8: std::sync::LazyLock<Vec<(&'static str, DataType)>> = std::sync::LazyLock::new(|| {
        let t = DataType::Dictionary(Box::new(DataType::Int8), Box::new(DataType::Utf8));
        vec![("l_returnflag", t.clone()), ("l_linestatus", t)]
    });
    Source::open_as(path, retype(DICT8.as_slice().to_vec().leak()))
}

fn variants_report(path: &str) {
    use std::sync::LazyLock;
    static DICT: LazyLock<Vec<(&'static str, DataType)>> = LazyLock::new(|| {
        let t = DataType::Dictionary(Box::new(DataType::Int8), Box::new(DataType::Utf8));
        vec![("l_returnflag", t.clone()), ("l_linestatus", t)]
    });
    static VIEW: LazyLock<Vec<(&'static str, DataType)>> =
        LazyLock::new(|| vec![("l_returnflag", DataType::Utf8View), ("l_linestatus", DataType::Utf8View)]);
    let strs = ["l_returnflag", "l_linestatus"];

    println!("string columns only (2 cols), decode, best of 6 (ms)\n\n| arrow type | ms |\n|---|---:|");
    let base = Source::open(path);
    println!("| Utf8 (what the spike uses) | {:.0} |", decode_ms(&base, &strs, BATCH_SIZE));
    let view = Source::open_as(path, retype(VIEW.as_slice().to_vec().leak()));
    println!("| Utf8View | {:.0} |", decode_ms(&view, &strs, BATCH_SIZE));
    let dict = Source::open_as(path, retype(DICT.as_slice().to_vec().leak()));
    println!("| Dictionary(Int8, Utf8) | {:.0} |", decode_ms(&dict, &strs, BATCH_SIZE));
    static DICT32: LazyLock<Vec<(&'static str, DataType)>> = LazyLock::new(|| {
        let t = DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8));
        vec![("l_returnflag", t.clone()), ("l_linestatus", t)]
    });
    let dict32 = Source::open_as(path, retype(DICT32.as_slice().to_vec().leak()));
    println!("| Dictionary(Int32, Utf8) | {:.0} |", decode_ms(&dict32, &strs, BATCH_SIZE));
    println!("\nall 6 columns, strings as Dictionary: {:.0} ms (Utf8: {:.0} ms)", decode_ms(&dict, &COLUMNS, BATCH_SIZE), decode_ms(&base, &COLUMNS, BATCH_SIZE));
}

fn cols_report(src: &Source) {
    println!("decode only, single thread, best of 6 (ms)\n\n| columns | ms |\n|---|---:|");
    for c in COLUMNS {
        println!("| {c} | {:.0} |", decode_ms(src, &[c], BATCH_SIZE));
    }
    println!("| all 6 | {:.0} |", decode_ms(src, &COLUMNS, BATCH_SIZE));
    println!("\nbatch size, all 6 columns:\n\n| batch rows | ms |\n|---:|---:|");
    for bs in [1024, 4096, 8192, 32768, 131072] {
        println!("| {bs} | {:.0} |", decode_ms(src, &COLUMNS, bs));
    }
}

fn main() {
    let mode = std::env::args().nth(2);
    if matches!(mode.as_deref(), Some("stages" | "fast" | "cols" | "decode" | "variants" | "loop" | "v2" | "snapstat" | "snapreplay" | "once1" | "kernels" | "pullpush")) {
        let src = Source::open(&std::env::args().nth(1).expect("path"));
        if mode.as_deref() == Some("pullpush") {
            let rounds: usize = std::env::args().nth(3).map_or(15, |s| s.parse().expect("rounds"));
            pullpush_report(&std::env::args().nth(1).expect("path"), rounds);
        } else if mode.as_deref() == Some("kernels") {
            kernels_report();
        } else if mode.as_deref() == Some("once1") {
            // 1-thread v2 (Decimal64): min ms over N runs; for pinned / parallel-process tests.
            let src = dec64_source(&std::env::args().nth(1).expect("path"));
            let n: usize = std::env::args().nth(3).map_or(12, |s| s.parse().expect("runs"));
            let best = (0..n)
                .map(|_| {
                    let t = Instant::now();
                    std::hint::black_box(run_push_v2::<arrow::datatypes::Decimal64Type>(&src, 1));
                    t.elapsed().as_secs_f64() * 1000.0
                })
                .fold(f64::MAX, f64::min);
            println!("{best:.0}");
        } else if mode.as_deref() == Some("snapreplay") {
            // Replays dumped pages through whichever decoder SNAP_IMPL selects.
            let data = std::fs::read(std::env::args().nth(3).expect("dump file")).expect("read dump");
            let mut blocks: Vec<(&[u8], usize)> = Vec::new();
            let (mut pos, mut max_out, mut total_out) = (0usize, 0usize, 0usize);
            while pos + 8 <= data.len() {
                let n = u32::from_le_bytes(data[pos..pos + 4].try_into().expect("len")) as usize;
                let o = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().expect("len")) as usize;
                blocks.push((&data[pos + 8..pos + 8 + n], o));
                (pos, max_out, total_out) = (pos + 8 + n, max_out.max(o), total_out + o);
            }
            let mut out = vec![0u8; max_out + 64];
            let mut dec = snap::raw::Decoder::new();
            let mut best = f64::MAX;
            for _ in 0..40 {
                let t = Instant::now();
                for (b, o) in &blocks {
                    dec.decompress(b, &mut out[..*o]).expect("decompress");
                }
                best = best.min(t.elapsed().as_secs_f64() * 1000.0);
            }
            println!("{:?}: {} blocks, {:.1} MB out: best {:.1} ms = {:.2} GB/s", snap::raw::which(), blocks.len(), total_out as f64 / 1e6, best, total_out as f64 / best / 1e6);
        } else if mode.as_deref() == Some("snapstat") {
            let src = dec64_source(&std::env::args().nth(1).expect("path"));
            let schema = src.meta.metadata().file_metadata().schema_descr_ptr();
            let mask = ProjectionMask::columns(&schema, COLUMNS);
            let runs = 8;
            snap::raw::reset_stats();
            let mut best = f64::MAX;
            for _ in 0..runs {
                let t = Instant::now();
                for rg in 0..src.row_groups {
                    for b in src.reader_with(rg, mask.clone(), BATCH_SIZE) {
                        std::hint::black_box(b.expect("decode"));
                    }
                }
                best = best.min(t.elapsed().as_secs_f64() * 1000.0);
            }
            let (ns, bytes, calls) = snap::raw::stats();
            println!(
                "impl {:?}: decode-only best {:.0} ms; snappy per run {:.1} ms over {} calls, {:.1} MB out, {:.2} GB/s",
                snap::raw::which(),
                best,
                ns as f64 / 1e6 / runs as f64,
                calls / runs as u64,
                bytes as f64 / 1e6 / runs as f64,
                bytes as f64 / ns as f64
            );
        } else if mode.as_deref() == Some("v2") {
            v2_report(&std::env::args().nth(1).expect("path"), 15);
        } else if mode.as_deref() == Some("loop") {
            let threads: usize = std::env::args().nth(3).map_or(1, |s| s.parse().expect("threads"));
            let src = dec64_source(&std::env::args().nth(1).expect("path"));
            for _ in 0..60 {
                std::hint::black_box(run_push_v2::<arrow::datatypes::Decimal64Type>(&src, threads));
            }
        } else if mode.as_deref() == Some("variants") {
            variants_report(&std::env::args().nth(1).expect("path"));
        } else if mode.as_deref() == Some("decode") {
            let list = std::env::args().nth(3).expect("comma-separated columns");
            let cols: Vec<&str> = list.split(',').collect();
            println!("{:.0} ms", decode_ms(&src, &cols, BATCH_SIZE));
        } else if mode.as_deref() == Some("cols") {
            cols_report(&src);
        } else if mode.as_deref() == Some("stages") {
            stages_report(&src, &dict_source(&std::env::args().nth(1).expect("path")));
        } else {
            fast_report(&src, &dict_source(&std::env::args().nth(1).expect("path")), 15);
        }
        return;
    }
    let path = std::env::args().nth(1).expect("usage: spike <lineitem.parquet> [runs]");
    let runs: usize = std::env::args().nth(2).map_or(7, |s| s.parse().expect("runs"));
    let src = Source::open(&path);

    println!(
        "row groups: {}; batch size: {BATCH_SIZE}; timed runs: {runs} (+1 warm-up, discarded)\n",
        src.row_groups
    );
    println!("| design | threads | min ms | median ms |\n|---|---:|---:|---:|");

    let mut expected: Option<Agg> = None;
    for (name, f) in [("pull/static", run_pull as fn(&Source, usize) -> Agg), ("push/morsel", run_push)] {
        for threads in [1usize, 2, 4, 8] {
            let mut times = Vec::new();
            for i in 0..=runs {
                let t = Instant::now();
                let agg = f(&src, threads);
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                if i > 0 {
                    times.push(ms);
                }
                match &expected {
                    None => expected = Some(agg),
                    Some(e) => assert!(e.agrees(&agg), "{name} x{threads} disagrees with first result"),
                }
            }
            times.sort_by(f64::total_cmp);
            println!("| {name} | {threads} | {:.0} | {:.0} |", times[0], times[times.len() / 2]);
        }
    }
    println!("\nresult:\n{}", expected.expect("ran").render());
}
