//! A Parquet scan the way the engine would do it with parquet-rs: projection, one morsel per task, dictionary strings and Decimal64 as the engine wants them
//! (ADR 0003), and a cheap consumer ("fold") that touches every value so that decoding cannot be skipped. Plus the DuckDB reference. See Cargo.toml for the questions.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::ops::Range;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use arrow::array::{Array, AsArray, RecordBatch};
use arrow::datatypes::{DataType, Date32Type, Decimal64Type, Decimal128Type, Field, Int32Type, Int64Type, Schema, SchemaRef};
use parquet::arrow::ProjectionMask;
use parquet::file::metadata::PageIndexPolicy;
use parquet::arrow::arrow_reader::{ArrowReaderMetadata, ArrowReaderOptions, ParquetRecordBatchReader, ParquetRecordBatchReaderBuilder, RowSelection};

pub const BATCH_ROWS: usize = 8192;

/// How columns are handed out: `Plain` is parquet-rs's default (strings expanded, Decimal128); `Engine` is what ADR 0003 wants (dictionary strings with Int32 keys, Decimal64(15,2)).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Types {
    Plain,
    Engine,
}

fn engine_type(name: &str) -> Option<DataType> {
    match name {
        "l_returnflag" | "l_linestatus" => Some(DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))),
        "l_quantity" | "l_extendedprice" | "l_discount" | "l_tax" => Some(DataType::Decimal64(15, 2)),
        _ => None,
    }
}

pub struct Src {
    path: String,
    meta: ArrowReaderMetadata,
    mask: ProjectionMask,
    pub row_group_rows: Vec<usize>,
    pub has_page_index: bool,
}

impl Src {
    pub fn open(path: &str, columns: &[&str], types: Types) -> Src {
        let file = File::open(path).unwrap_or_else(|e| panic!("open {path}: {e}"));
        // the page index is loaded when the file has one (Optional policy); a file without one simply has none
        let base = ArrowReaderOptions::new().with_page_index_policy(PageIndexPolicy::Optional);
        let plain = ArrowReaderMetadata::load(&file, base.clone()).expect("metadata");
        let meta = if types == Types::Engine {
            let fields: Vec<_> = plain
                .schema()
                .fields()
                .iter()
                .map(|f| match engine_type(f.name()) {
                    Some(t) => std::sync::Arc::new(Field::new(f.name(), t, f.is_nullable())),
                    None => f.clone(),
                })
                .collect();
            let schema: SchemaRef = std::sync::Arc::new(Schema::new(fields));
            ArrowReaderMetadata::load(&file, base.with_schema(schema)).expect("metadata")
        } else {
            plain
        };
        let md = meta.metadata().clone();
        let descr = md.file_metadata().schema_descr_ptr();
        let mask = ProjectionMask::columns(&descr, columns.iter().copied());
        let row_group_rows = (0..md.num_row_groups()).map(|i| md.row_group(i).num_rows() as usize).collect();
        let has_page_index = (0..md.num_row_groups()).all(|g| md.row_group(g).columns().iter().all(|c| c.offset_index_offset().is_some()));
        Src { path: path.to_string(), meta, mask, row_group_rows, has_page_index }
    }

    /// A fresh file handle per reader (cloned handles share the seek offset). `rows`: a part of the row group.
    pub fn reader(&self, rg: usize, rows: Option<Range<usize>>) -> ParquetRecordBatchReader {
        let mut b = ParquetRecordBatchReaderBuilder::new_with_metadata(File::open(&self.path).expect("open"), self.meta.clone())
            .with_projection(self.mask.clone())
            .with_row_groups(vec![rg])
            .with_batch_size(BATCH_ROWS);
        if let Some(r) = rows {
            b = b.with_row_selection(RowSelection::from_consecutive_ranges(std::iter::once(r), self.row_group_rows[rg]));
        }
        b.build().expect("reader")
    }

    pub fn total_rows(&self) -> usize {
        self.row_group_rows.iter().sum()
    }
}

// ------------------------------------------------------------------------------------------------------------------------------ the consumer

/// What was read, per column name (a wrapping sum of a cheap function of every value) and the row count.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Acc {
    pub rows: u64,
    pub sums: BTreeMap<String, u64>,
}

impl Acc {
    pub fn merge(&mut self, o: &Acc) {
        self.rows += o.rows;
        for (k, v) in &o.sums {
            let e = self.sums.entry(k.clone()).or_insert(0);
            *e = e.wrapping_add(*v);
        }
    }
}

fn first_byte(s: &str) -> u64 {
    s.as_bytes().first().copied().unwrap_or(0) as u64
}

/// Touches every non-NULL value: numbers and dates by their value (decimals by their UNSCALED value, so Decimal64 and Decimal128 agree), strings by their first byte
/// (a dictionary column counts its keys and weighs the dictionary entries once per batch, which gives the same total as expanding it).
pub fn fold(b: &RecordBatch, acc: &mut Acc) {
    acc.rows += b.num_rows() as u64;
    for (i, f) in b.schema().fields().iter().enumerate() {
        let col = b.column(i);
        let s: u64 = match f.data_type() {
            DataType::Decimal64(_, _) => sum_prim(col.as_primitive::<Decimal64Type>().values(), col, |v| v as u64),
            DataType::Decimal128(_, _) => sum_prim(col.as_primitive::<Decimal128Type>().values(), col, |v| v as i64 as u64),
            DataType::Date32 => sum_prim(col.as_primitive::<Date32Type>().values(), col, |v| v as i64 as u64),
            DataType::Int32 => sum_prim(col.as_primitive::<Int32Type>().values(), col, |v| v as i64 as u64),
            DataType::Int64 => sum_prim(col.as_primitive::<Int64Type>().values(), col, |v| v as u64),
            DataType::Utf8 => {
                let a = col.as_string::<i32>();
                (0..a.len()).filter(|&j| a.is_valid(j)).fold(0u64, |s, j| s.wrapping_add(first_byte(a.value(j))))
            }
            DataType::Dictionary(_, _) => {
                let d = col.as_dictionary::<Int32Type>();
                let vals = d.values().as_string::<i32>();
                let mut counts = vec![0u64; vals.len()];
                for k in d.keys().iter().flatten() {
                    counts[k as usize] += 1;
                }
                #[cfg(feature = "bug-dict-skips-last-key")]
                let n = counts.len().saturating_sub(1);
                #[cfg(not(feature = "bug-dict-skips-last-key"))]
                let n = counts.len();
                (0..n).fold(0u64, |s, k| s.wrapping_add(counts[k].wrapping_mul(first_byte(vals.value(k)))))
            }
            other => panic!("the consumer does not know {other}"),
        };
        let e = acc.sums.entry(f.name().clone()).or_insert(0);
        *e = e.wrapping_add(s);
    }
}

fn sum_prim<T: Copy>(values: &[T], col: &dyn Array, f: impl Fn(T) -> u64) -> u64 {
    if col.null_count() == 0 {
        values.iter().fold(0u64, |s, &v| s.wrapping_add(f(v)))
    } else {
        values.iter().enumerate().filter(|(i, _)| col.is_valid(*i)).fold(0u64, |s, (_, &v)| s.wrapping_add(f(v)))
    }
}

// ------------------------------------------------------------------------------------------------------------------------------ morsels

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Morsel {
    pub rg: usize,
    /// None = the whole row group.
    pub rows: Option<Range<usize>>,
}

pub fn by_row_group(src: &Src) -> Vec<Morsel> {
    (0..src.row_group_rows.len()).map(|rg| Morsel { rg, rows: None }).collect()
}

/// Each row group cut into `parts` contiguous ranges whose sizes differ by at most one row; empty ranges are dropped.
pub fn by_parts(src: &Src, parts: usize) -> Vec<Morsel> {
    let mut v = Vec::new();
    for (rg, &n) in src.row_group_rows.iter().enumerate() {
        for p in 0..parts {
            let start = n * p / parts;
            #[cfg(feature = "bug-split-off-by-one")]
            let end = (n * (p + 1) / parts + 1).min(n);
            #[cfg(not(feature = "bug-split-off-by-one"))]
            let end = n * (p + 1) / parts;
            if end > start {
                v.push(Morsel { rg, rows: Some(start..end) });
            }
        }
    }
    v
}

/// `threads` workers claim morsels from one counter until none are left; their partial results are merged.
pub fn run(src: &Src, morsels: &[Morsel], threads: usize) -> Acc {
    let next = AtomicUsize::new(0);
    let parts: Vec<Acc> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut acc = Acc::default();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(m) = morsels.get(i) else { break };
                        for b in src.reader(m.rg, m.rows.clone()) {
                            fold(&b.expect("batch"), &mut acc);
                        }
                    }
                    acc
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().expect("worker")).collect()
    });
    let mut total = Acc::default();
    #[cfg(feature = "bug-drop-last-thread")]
    let parts = &parts[..parts.len().saturating_sub(1).max(1)];
    for p in parts.iter() {
        total.merge(p);
    }
    total
}

/// The whole file, one thread, one morsel per row group (the reference every other arm is compared with).
pub fn scan_all(src: &Src) -> Acc {
    run(src, &by_row_group(src), 1)
}

// ------------------------------------------------------------------------------------------------------------------------------ DuckDB, the reference

/// The SQL whose single result row equals `Acc` for these columns: count, then per column (alphabetical, like the BTreeMap) the same cheap function of every value.
pub fn duck_sql(path: &str, columns: &[&str]) -> String {
    let mut cols: Vec<&str> = columns.to_vec();
    cols.sort_unstable();
    let exprs: Vec<String> = cols
        .iter()
        .map(|c| match *c {
            "l_returnflag" | "l_linestatus" => format!("sum(ascii({c}))"),
            "l_quantity" | "l_extendedprice" | "l_discount" | "l_tax" => format!("sum(CAST({c} * 100 AS BIGINT))"),
            "l_shipdate" | "l_commitdate" | "l_receiptdate" => format!("sum(date_diff('day', DATE '1970-01-01', {c}))"),
            "l_orderkey" | "l_partkey" | "l_suppkey" | "l_linenumber" => format!("sum({c})"),
            other => panic!("no DuckDB expression for {other}"),
        })
        .collect();
    format!("SELECT count(*), {} FROM read_parquet('{}');", exprs.join(", "), path)
}

/// Runs one query with one thread; returns (result line, seconds as DuckDB's own timer reports them).
pub fn duck_run(sql: &str) -> Result<(String, f64), String> {
    let script = format!("SET threads=1;\n.timer on\n{sql}\n");
    let mut child = Command::new("duckdb")
        .args(["-bail", "-list", "-noheader", "-separator", "|"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start duckdb: {e}"))?;
    child.stdin.take().ok_or("no stdin")?.write_all(script.as_bytes()).map_err(|e| e.to_string())?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() {
        return Err(format!("duckdb failed: {} {}", String::from_utf8_lossy(&out.stderr).trim(), stdout.trim()));
    }
    parse_duck(&stdout)
}

pub fn parse_duck(stdout: &str) -> Result<(String, f64), String> {
    let (mut line, mut secs) = (None, None);
    for l in stdout.lines() {
        if let Some(rest) = l.strip_prefix("Run Time (s):") {
            let mut it = rest.split_whitespace();
            while let Some(w) = it.next() {
                if w == "real" {
                    secs = it.next().and_then(|x| x.parse::<f64>().ok());
                }
            }
        } else if !l.is_empty() {
            line = Some(l.to_string());
        }
    }
    Ok((line.ok_or("no result line")?, secs.ok_or("no 'Run Time' line")?))
}

/// The expected `count|sum|sum...` line for an `Acc` (columns alphabetical).
pub fn acc_line(a: &Acc) -> String {
    let mut s = a.rows.to_string();
    for v in a.sums.values() {
        s.push('|');
        s.push_str(&(*v as i64).to_string());
    }
    s
}
