//! An INDEPENDENT checker of a Parquet file. It decodes the data, recomputes for every column chunk and every page the true minimum, maximum and
//! counts with its own comparators (by Arrow type: signed, unsigned, IEEE with NaN apart, decimal as signed big-endian, bytes as unsigned
//! lexicographic), and compares them with what the file claims: row-group statistics, the page index, sorting columns, field ids and bloom filters.
//!
//! SOUND is the requirement: a stored minimum must be <= every value and a stored maximum >= every value (truncation allowed), counts must be exact.
//! TIGHT is a quality note (a warning): the bound equals the truth when nothing forced it to be looser.
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::Path;

use arrow::array::*;
use arrow::compute::{cast, concat_batches};
use arrow::compute::SortOptions;
use arrow::datatypes::*;
use arrow::row::{RowConverter, SortField};
use parquet::arrow::arrow_reader::{ArrowReaderMetadata, ArrowReaderOptions, ParquetRecordBatchReaderBuilder};
use parquet::basic::BoundaryOrder;
use parquet::file::metadata::PageIndexPolicy;
use parquet::file::page_index::column_index::ColumnIndexMetaData;
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::schema::types::Type;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sev { Error, Warn }

#[derive(Clone, Debug)]
pub struct Violation { pub sev: Sev, pub rg: Option<usize>, pub col: String, pub kind: &'static str, pub detail: String }

#[derive(Default)]
pub struct Report { pub violations: Vec<Violation>, pub chunks: usize, pub pages: usize, pub values: usize }

impl Report {
    pub fn errors(&self) -> Vec<&Violation> { self.violations.iter().filter(|v| v.sev == Sev::Error).collect() }
    pub fn warns(&self) -> Vec<&Violation> { self.violations.iter().filter(|v| v.sev == Sev::Warn).collect() }
    pub fn error_kinds(&self) -> HashSet<&'static str> { self.errors().iter().map(|v| v.kind).collect() }
    fn add(&mut self, sev: Sev, rg: Option<usize>, col: &str, kind: &'static str, detail: String) {
        self.violations.push(Violation { sev, rg, col: col.to_string(), kind, detail });
    }
}

#[derive(Clone, Default)]
pub struct VerifyOpts {
    /// expected (path, field id) pairs; None = do not check ids
    pub field_ids: Option<Vec<(String, i32)>>,
    /// truncation lengths the writer was configured with (for tightness notes only)
    pub trunc: Option<usize>,
    pub index_trunc: Option<usize>,
    pub check_bloom: bool,
}

#[derive(Clone, Debug)]
enum Key { I(i128), F(f64), B(Vec<u8>), Bool(bool) }

fn kcmp(a: &Key, b: &Key) -> Ordering {
    match (a, b) {
        (Key::I(x), Key::I(y)) => x.cmp(y),
        (Key::F(x), Key::F(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),   // -0.0 == 0.0; NaN never reaches here
        (Key::B(x), Key::B(y)) => x.cmp(y),                                         // unsigned lexicographic
        (Key::Bool(x), Key::Bool(y)) => x.cmp(y),
        _ => panic!("comparing keys of different kinds"),
    }
}

fn is_bytes(t: &DataType) -> bool { matches!(t, DataType::Utf8 | DataType::LargeUtf8 | DataType::Binary | DataType::LargeBinary) }

fn dec_be(b: &[u8]) -> Option<i128> {
    if b.is_empty() || b.len() > 16 { return None; }
    let mut v: i128 = if b[0] & 0x80 != 0 { -1 } else { 0 };
    for &x in b { v = (v << 8) | x as i128; }
    Some(v)
}

fn decode(dt: &DataType, b: &[u8]) -> Result<Key, String> {
    let need = |n: usize| if b.len() == n { Ok(()) } else { Err(format!("{} bytes, expected {n}", b.len())) };
    Ok(match dt {
        DataType::Dictionary(_, v) => return decode(v, b),
        DataType::Int8 | DataType::Int16 | DataType::Int32 | DataType::Date32 | DataType::Time32(_) => { need(4)?; Key::I(i32::from_le_bytes(b.try_into().unwrap()) as i128) }
        DataType::UInt8 | DataType::UInt16 | DataType::UInt32 => { need(4)?; Key::I(u32::from_le_bytes(b.try_into().unwrap()) as i128) }
        DataType::Int64 | DataType::Timestamp(..) | DataType::Time64(_) | DataType::Date64 => { need(8)?; Key::I(i64::from_le_bytes(b.try_into().unwrap()) as i128) }
        DataType::UInt64 => { need(8)?; Key::I(u64::from_le_bytes(b.try_into().unwrap()) as i128) }
        DataType::Float32 => { need(4)?; Key::F(f32::from_le_bytes(b.try_into().unwrap()) as f64) }
        DataType::Float64 => { need(8)?; Key::F(f64::from_le_bytes(b.try_into().unwrap())) }
        DataType::Boolean => { need(1)?; Key::Bool(b[0] != 0) }
        t if is_bytes(t) => Key::B(b.to_vec()),
        DataType::Decimal128(p, _) if *p <= 9 => { need(4)?; Key::I(i32::from_le_bytes(b.try_into().unwrap()) as i128) }
        DataType::Decimal128(p, _) if *p <= 18 => { need(8)?; Key::I(i64::from_le_bytes(b.try_into().unwrap()) as i128) }
        DataType::Decimal128(..) => Key::I(dec_be(b).ok_or("bad decimal length")?),
        t => return Err(format!("no decoder for {t}")),
    })
}

struct Truth { min: Option<Key>, max: Option<Key>, nulls: usize, nans: usize, rows: usize, nonnull: usize }

macro_rules! scan {
    ($arr:expr, $A:ty, $t:ident, $f:expr) => {{
        let a = $arr.as_any().downcast_ref::<$A>().expect("array type");
        for i in 0..a.len() { if a.is_null(i) { $t.nulls += 1; } else { $f(&mut $t, a.value(i)); } }
    }};
}

fn upd(t: &mut Truth, k: Key) {
    t.nonnull += 1;
    if t.min.as_ref().is_none_or(|m| kcmp(&k, m) == Ordering::Less) { t.min = Some(k.clone()); }
    if t.max.as_ref().is_none_or(|m| kcmp(&k, m) == Ordering::Greater) { t.max = Some(k); }
}

fn truth(arr: &ArrayRef) -> Truth {
    let mut t = Truth { min: None, max: None, nulls: 0, nans: 0, rows: arr.len(), nonnull: 0 };
    let dt = arr.data_type().clone();
    if let DataType::Dictionary(_, v) = &dt {
        let flat = cast(arr, v).expect("dict");
        return truth(&flat);
    }
    match &dt {
        DataType::Int8 => scan!(arr, Int8Array, t, |t: &mut Truth, v: i8| upd(t, Key::I(v as i128))),
        DataType::Int16 => scan!(arr, Int16Array, t, |t: &mut Truth, v: i16| upd(t, Key::I(v as i128))),
        DataType::Int32 => scan!(arr, Int32Array, t, |t: &mut Truth, v: i32| upd(t, Key::I(v as i128))),
        DataType::Int64 => scan!(arr, Int64Array, t, |t: &mut Truth, v: i64| upd(t, Key::I(v as i128))),
        DataType::UInt8 => scan!(arr, UInt8Array, t, |t: &mut Truth, v: u8| upd(t, Key::I(v as i128))),
        DataType::UInt16 => scan!(arr, UInt16Array, t, |t: &mut Truth, v: u16| upd(t, Key::I(v as i128))),
        DataType::UInt32 => scan!(arr, UInt32Array, t, |t: &mut Truth, v: u32| upd(t, Key::I(v as i128))),
        DataType::UInt64 => scan!(arr, UInt64Array, t, |t: &mut Truth, v: u64| upd(t, Key::I(v as i128))),
        DataType::Float32 => scan!(arr, Float32Array, t, |t: &mut Truth, v: f32| if v.is_nan() { t.nans += 1; t.nonnull += 1 } else { upd(t, Key::F(v as f64)) }),
        DataType::Float64 => scan!(arr, Float64Array, t, |t: &mut Truth, v: f64| if v.is_nan() { t.nans += 1; t.nonnull += 1 } else { upd(t, Key::F(v)) }),
        DataType::Boolean => scan!(arr, BooleanArray, t, |t: &mut Truth, v: bool| upd(t, Key::Bool(v))),
        DataType::Utf8 => scan!(arr, StringArray, t, |t: &mut Truth, v: &str| upd(t, Key::B(v.as_bytes().to_vec()))),
        DataType::Binary => scan!(arr, BinaryArray, t, |t: &mut Truth, v: &[u8]| upd(t, Key::B(v.to_vec()))),
        DataType::Decimal128(..) => scan!(arr, Decimal128Array, t, |t: &mut Truth, v: i128| upd(t, Key::I(v))),
        DataType::Date32 => scan!(arr, Date32Array, t, |t: &mut Truth, v: i32| upd(t, Key::I(v as i128))),
        DataType::Time32(_) => { let a = cast(arr, &DataType::Int32).unwrap(); let tt = truth(&a); return Truth { rows: arr.len(), ..tt } }
        DataType::Time64(_) | DataType::Timestamp(..) => { let a = cast(arr, &DataType::Int64).unwrap(); let tt = truth(&a); return Truth { rows: arr.len(), ..tt } }
        other => panic!("no truth for {other}"),
    }
    t
}

fn is_zero(k: &Key) -> bool { matches!(k, Key::F(x) if *x == 0.0) }
fn neg_zero(b: &[u8], dt: &DataType) -> Option<bool> {
    match dt { DataType::Float32 if b.len() == 4 => Some(f32::from_le_bytes(b.try_into().unwrap()).to_bits() >> 31 == 1),
               DataType::Float64 if b.len() == 8 => Some(f64::from_le_bytes(b.try_into().unwrap()).to_bits() >> 63 == 1), _ => None }
}

/// Compare one (min, max, null count) claim with the truth. Used for row-group statistics AND for pages. `limit` = truncation length in force.
#[allow(clippy::too_many_arguments)]
fn check_claim(r: &mut Report, rg: Option<usize>, col: &str, scope: &str, dt: &DataType, t: &Truth, min: Option<&[u8]>, max: Option<&[u8]>, nulls: Option<i64>,
               limit: Option<usize>, min_exact: bool, max_exact: bool) {
    let sc = |k: &'static str| -> &'static str { k };
    let _ = sc;
    let dec = |b: Option<&[u8]>, what: &str, r: &mut Report| -> Option<Key> {
        let b = b?;
        match decode(dt, b) {
            Ok(Key::F(x)) if x.is_nan() => { r.add(Sev::Error, rg, col, "stat-nan", format!("{scope}: {what} is NaN")); None }
            Ok(k) => Some(k),
            Err(e) => { r.add(Sev::Error, rg, col, "stat-decode", format!("{scope}: {what}: {e}")); None }
        }
    };
    let (smin, smax) = (dec(min, "min", r), dec(max, "max", r));
    if is_bytes(dt) && matches!(dt, DataType::Utf8 | DataType::LargeUtf8) {
        for (w, b) in [("min", min), ("max", max)] {
            if let Some(b) = b { if std::str::from_utf8(b).is_err() { r.add(Sev::Error, rg, col, "stat-utf8", format!("{scope}: {w} is not valid UTF-8")); } }
        }
    }
    match (&t.min, &smin) {
        (Some(tm), Some(sm)) => {
            if kcmp(sm, tm) == Ordering::Greater { r.add(Sev::Error, rg, col, "min-not-lower-bound", format!("{scope}: stored min {sm:?} > true min {tm:?}")); }
            else if kcmp(sm, tm) == Ordering::Less {
                if min_exact { r.add(Sev::Error, rg, col, "stat-exact-mismatch", format!("{scope}: min marked exact but {sm:?} != {tm:?}")); }
                let truncated = is_bytes(dt) && limit.is_some_and(|l| matches!(tm, Key::B(b) if b.len() > l));
                if !truncated { r.add(Sev::Warn, rg, col, "not-tight", format!("{scope}: min {sm:?} below true min {tm:?} with no truncation")); }
            }
        }
        (Some(_), None) => if min.is_none() { r.add(Sev::Warn, rg, col, "no-stats", format!("{scope}: no min although there are values")) },
        (None, Some(sm)) => if t.nonnull > 0 && t.nans < t.nonnull { r.add(Sev::Error, rg, col, "min-not-lower-bound", format!("{scope}: min {sm:?} but no comparable values")) }
                            else { r.add(Sev::Error, rg, col, "stat-for-nothing", format!("{scope}: min {sm:?} although every value is NULL or NaN")) },
        (None, None) => {}
    }
    match (&t.max, &smax) {
        (Some(tm), Some(sm)) => {
            if kcmp(sm, tm) == Ordering::Less { r.add(Sev::Error, rg, col, "max-not-upper-bound", format!("{scope}: stored max {sm:?} < true max {tm:?}")); }
            else if kcmp(sm, tm) == Ordering::Greater {
                if max_exact { r.add(Sev::Error, rg, col, "stat-exact-mismatch", format!("{scope}: max marked exact but {sm:?} != {tm:?}")); }
                let truncated = is_bytes(dt) && limit.is_some_and(|l| matches!(tm, Key::B(b) if b.len() > l));
                if !truncated { r.add(Sev::Warn, rg, col, "not-tight", format!("{scope}: max {sm:?} above true max {tm:?} with no truncation")); }
            }
        }
        (Some(_), None) => if max.is_none() { r.add(Sev::Warn, rg, col, "no-stats", format!("{scope}: no max although there are values")) },
        (None, Some(sm)) => if !(t.nonnull > 0 && t.nans < t.nonnull) { r.add(Sev::Error, rg, col, "stat-for-nothing", format!("{scope}: max {sm:?} although every value is NULL or NaN")) },
        (None, None) => {}
    }
    if let (Some(l), true) = (limit, is_bytes(dt)) {
        for (w, b) in [("min", min), ("max", max)] { if b.is_some_and(|b| b.len() > l + 4) { r.add(Sev::Warn, rg, col, "stat-too-long", format!("{scope}: {w} longer than the truncation length {l}")); } }
    }
    // the floating-point zero rule: a stored min of +0.0 / max of -0.0 is a trap for readers that use a total order
    if let (Some(tm), Some(b)) = (&t.min, min) { if is_zero(tm) && neg_zero(b, dt) == Some(false) { r.add(Sev::Warn, rg, col, "zero-sign", format!("{scope}: min is +0.0 where the spec asks for -0.0")); } }
    if let (Some(tm), Some(b)) = (&t.max, max) { if is_zero(tm) && neg_zero(b, dt) == Some(true) { r.add(Sev::Warn, rg, col, "zero-sign", format!("{scope}: max is -0.0 where the spec asks for +0.0")); } }
    match nulls {
        Some(n) if n as usize != t.nulls => r.add(Sev::Error, rg, col, "null-count", format!("{scope}: null_count {n}, true {}", t.nulls)),
        None => r.add(Sev::Warn, rg, col, "no-stats", format!("{scope}: no null_count")),
        _ => {}
    }
}

fn ci_bytes(ci: &ColumnIndexMetaData, p: usize) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    use ColumnIndexMetaData as C;
    match ci {
        C::BOOLEAN(i) => (i.min_value(p).map(|v| vec![*v as u8]), i.max_value(p).map(|v| vec![*v as u8])),
        C::INT32(i) => (i.min_value(p).map(|v| v.to_le_bytes().to_vec()), i.max_value(p).map(|v| v.to_le_bytes().to_vec())),
        C::INT64(i) => (i.min_value(p).map(|v| v.to_le_bytes().to_vec()), i.max_value(p).map(|v| v.to_le_bytes().to_vec())),
        C::FLOAT(i) => (i.min_value(p).map(|v| v.to_le_bytes().to_vec()), i.max_value(p).map(|v| v.to_le_bytes().to_vec())),
        C::DOUBLE(i) => (i.min_value(p).map(|v| v.to_le_bytes().to_vec()), i.max_value(p).map(|v| v.to_le_bytes().to_vec())),
        C::BYTE_ARRAY(i) | C::FIXED_LEN_BYTE_ARRAY(i) => (i.min_value(p).map(|v| v.to_vec()), i.max_value(p).map(|v| v.to_vec())),
        C::INT96(_) => (None, None),
    }
}

fn walk_ids(t: &Type, prefix: &str, out: &mut Vec<(String, Option<i32>)>) {
    for c in t.get_fields() {
        let name = c.name();
        // the synthetic "list" repeated group of a LIST is not a field of the logical schema: skip it in the path
        let path = if prefix.is_empty() { name.to_string() } else { format!("{prefix}.{name}") };
        let info = c.get_basic_info();
        out.push((path.clone(), info.has_id().then(|| info.id())));
        if !c.is_primitive() { walk_ids(c, &path, out); }
    }
}

fn leaf_values(arr: &ArrayRef, path: &[String]) -> Option<ArrayRef> {
    // flat: the array itself; list<item>: its values; struct: the named child
    match (path.len(), arr.data_type()) {
        (1, _) => Some(arr.clone()),
        (_, DataType::Struct(_)) => { let s = arr.as_any().downcast_ref::<StructArray>()?; leaf_values(s.column_by_name(&path[1])?, &path[1..]) }
        (_, DataType::List(_)) => { let l = arr.as_any().downcast_ref::<ListArray>()?; Some(l.values().clone()) }
        _ => None,
    }
}

pub fn verify_file(path: &Path, o: &VerifyOpts) -> Report {
    let mut r = Report::default();
    let file = File::open(path).expect("open");
    let meta = ArrowReaderMetadata::load(&file, ArrowReaderOptions::new().with_page_index_policy(PageIndexPolicy::Required)).expect("metadata");
    let pm = meta.metadata();
    let fname = path.file_name().unwrap().to_string_lossy().to_string();
    // ---- field ids
    if let Some(expected) = &o.field_ids {
        let mut found = vec![];
        walk_ids(pm.file_metadata().schema_descr().root_schema(), "", &mut found);
        let by: HashMap<&str, Option<i32>> = found.iter().map(|(p, i)| (p.as_str(), *i)).collect();
        for (p, id) in expected {
            // a LIST's element is stored under "<name>.list.<element>": look for the id under any path that ends with the element name
            let got = by.get(p.as_str()).copied().or_else(|| found.iter().find(|(q, _)| q.starts_with(&format!("{}.", p.split('.').next().unwrap())) && q.ends_with(&format!(".{}", p.split('.').next_back().unwrap())) && p.contains('.')).map(|(_, i)| *i));
            match got { Some(Some(g)) if g == *id => {}, Some(Some(g)) => r.add(Sev::Error, None, p, "field-id-wrong", format!("{fname}: id {g}, expected {id}")),
                        _ => r.add(Sev::Error, None, p, "field-id-missing", format!("{fname}: no field id, expected {id}")) }
        }
        let mut seen = HashSet::new();
        for (p, i) in &found { if let Some(i) = i { if !seen.insert(*i) { r.add(Sev::Error, None, p, "field-id-dup", format!("{fname}: id {i} used twice")); } } }
    }
    let bloom_reader = if o.check_bloom { Some(SerializedFileReader::new(File::open(path).unwrap()).unwrap()) } else { None };
    let ncols = pm.file_metadata().schema_descr().num_columns();
    let arrow_schema = meta.schema().clone();
    for rg in 0..pm.num_row_groups() {
        let rgm = pm.row_group(rg);
        let reader = ParquetRecordBatchReaderBuilder::new_with_metadata(File::open(path).unwrap(), meta.clone()).with_row_groups(vec![rg]).with_batch_size(8192).build().unwrap();
        let batches: Vec<RecordBatch> = reader.map(|b| b.unwrap()).collect();
        let rb = concat_batches(&batches.first().map(|b| b.schema()).unwrap_or_else(|| arrow_schema.clone()), &batches).unwrap();
        if rb.num_rows() as i64 != rgm.num_rows() { r.add(Sev::Error, Some(rg), "", "rg-rows", format!("{fname}: row group says {} rows, decoded {}", rgm.num_rows(), rb.num_rows())); }
        r.values += rb.num_rows();
        for c in 0..ncols {
            let cc = rgm.column(c);
            let path_parts: Vec<String> = cc.column_path().parts().to_vec();
            let col = path_parts.join(".");
            let top = &path_parts[0];
            let Ok(ti) = rb.schema().index_of(top) else { continue };
            let Some(arr) = leaf_values(rb.column(ti), &path_parts) else { continue };
            let flat = path_parts.len() == 1;
            let dt = arr.data_type().clone();
            let t = truth(&arr);
            r.chunks += 1;
            if flat && cc.num_values() as usize != rb.num_rows() { r.add(Sev::Error, Some(rg), &col, "num-values", format!("{fname}: num_values {} for {} rows", cc.num_values(), rb.num_rows())); }
            // ---- row-group (chunk) statistics
            match cc.statistics() {
                None => if t.nonnull > 0 { r.add(Sev::Warn, Some(rg), &col, "no-stats", format!("{fname}: no statistics")) },
                Some(s) => {
                    let nc = if flat { s.null_count_opt().map(|v| v as i64) } else { Some(t.nulls as i64) };
                    check_claim(&mut r, Some(rg), &col, &format!("{fname} chunk"), &dt, &t, s.min_bytes_opt(), s.max_bytes_opt(), nc, o.trunc, s.min_is_exact(), s.max_is_exact());
                    if let Some(n) = s.nan_count_opt() { if n as usize != t.nans { r.add(Sev::Error, Some(rg), &col, "nan-count", format!("{fname}: nan_count {n}, true {}", t.nans)); } }
                }
            }
            // ---- page index (flat columns)
            if flat {
                let rpi = pm.page_index_for_row_group(rg);
                if let (Some(ci), Some(oi)) = (rpi.column_index(c), rpi.offset_index(c)) {
                    let locs = oi.page_locations();
                    if ci.num_pages() as usize != locs.len() { r.add(Sev::Error, Some(rg), &col, "page-count", format!("{fname}: column index has {} pages, offset index {}", ci.num_pages(), locs.len())); continue; }
                    let mut bounds: Vec<(Option<Key>, Option<Key>)> = vec![];
                    for p in 0..locs.len() {
                        let from = locs[p].first_row_index as usize;
                        let to = if p + 1 < locs.len() { locs[p + 1].first_row_index as usize } else { rb.num_rows() };
                        let pt = truth(&arr.slice(from, to - from));
                        r.pages += 1;
                        if ci.is_null_page(p) != (pt.nonnull == 0) { r.add(Sev::Error, Some(rg), &col, "page-null-flag", format!("{fname} page {p}: null_page={} but {} non-null values", ci.is_null_page(p), pt.nonnull)); }
                        let (mn, mx) = ci_bytes(ci, p);
                        let (mn, mx) = if ci.is_null_page(p) { (None, None) } else { (mn, mx) };
                        let nc = ci.null_count(p);
                        if nc.is_some_and(|n| n as usize != pt.nulls) { r.add(Sev::Error, Some(rg), &col, "page-null-count", format!("{fname} page {p}: null_count {:?}, true {}", nc, pt.nulls)); }
                        let before = r.violations.len();
                        check_claim(&mut r, Some(rg), &col, &format!("{fname} page {p}"), &dt, &pt, mn.as_deref(), mx.as_deref(), nc.or(Some(pt.nulls as i64)), o.index_trunc, false, false);
                        for v in &mut r.violations[before..] {
                            v.kind = match v.kind { "min-not-lower-bound" => "page-min", "max-not-upper-bound" => "page-max", k => k };
                        }
                        bounds.push((mn.as_deref().and_then(|b| decode(&dt, b).ok()), mx.as_deref().and_then(|b| decode(&dt, b).ok())));
                    }
                    // the claimed boundary order must be true of the page min/max
                    let seq = |f: &dyn Fn(&(Option<Key>, Option<Key>)) -> Option<Key>, asc: bool| -> bool {
                        let v: Vec<Key> = bounds.iter().filter_map(|b| f(b)).collect();
                        v.windows(2).all(|w| { let o = kcmp(&w[0], &w[1]); if asc { o != Ordering::Greater } else { o != Ordering::Less } })
                    };
                    match ci.get_boundary_order() {
                        Some(BoundaryOrder::ASCENDING) if !(seq(&|b| b.0.clone(), true) && seq(&|b| b.1.clone(), true)) => r.add(Sev::Error, Some(rg), &col, "page-boundary-order", format!("{fname}: ASCENDING claimed, pages are not")),
                        Some(BoundaryOrder::DESCENDING) if !(seq(&|b| b.0.clone(), false) && seq(&|b| b.1.clone(), false)) => r.add(Sev::Error, Some(rg), &col, "page-boundary-order", format!("{fname}: DESCENDING claimed, pages are not")),
                        _ => {}
                    }
                }
            }
            // ---- bloom filter: no false negatives
            if let (Some(br), true) = (&bloom_reader, flat) {
                if let Some(bf) = br.get_row_group(rg).unwrap().get_column_bloom_filter(c) {
                    let mut missing = 0usize;
                    macro_rules! chk { ($A:ty, $conv:expr) => {{ let a = arr.as_any().downcast_ref::<$A>().unwrap(); for i in 0..a.len() { if a.is_valid(i) && !bf.check(&$conv(a.value(i))) { missing += 1; } } }}; }
                    match &dt {
                        DataType::Int8 => chk!(Int8Array, |v: i8| v as i32), DataType::Int16 => chk!(Int16Array, |v: i16| v as i32), DataType::Int32 => chk!(Int32Array, |v: i32| v),
                        DataType::Int64 => chk!(Int64Array, |v: i64| v), DataType::UInt8 => chk!(UInt8Array, |v: u8| v as u32 as i32), DataType::UInt16 => chk!(UInt16Array, |v: u16| v as u32 as i32),
                        DataType::UInt32 => chk!(UInt32Array, |v: u32| v as i32), DataType::UInt64 => chk!(UInt64Array, |v: u64| v as i64),
                        DataType::Utf8 => chk!(StringArray, |v: &str| v.as_bytes().to_vec()), DataType::Binary => chk!(BinaryArray, |v: &[u8]| v.to_vec()),
                        DataType::Date32 => chk!(Date32Array, |v: i32| v), _ => {}
                    }
                    if missing > 0 { r.add(Sev::Error, Some(rg), &col, "bloom-false-negative", format!("{fname}: {missing} stored values are not found by the bloom filter")); }
                }
            }
        }
        // ---- sorting columns are true
        if let Some(sc) = rgm.sorting_columns() {
            if rb.num_rows() > 1 {
                let fields: Vec<SortField> = sc.iter().map(|k| SortField::new_with_options(rb.column(k.column_idx as usize).data_type().clone(), SortOptions { descending: k.descending, nulls_first: k.nulls_first })).collect();
                let conv = RowConverter::new(fields).unwrap();
                let cols: Vec<ArrayRef> = sc.iter().map(|k| rb.column(k.column_idx as usize).clone()).collect();
                let rows = conv.convert_columns(&cols).unwrap();
                if let Some(i) = (0..rb.num_rows() - 1).find(|&i| rows.row(i) > rows.row(i + 1)) {
                    r.add(Sev::Error, Some(rg), "", "unsorted", format!("{fname}: sorting_columns claimed but row {i} > row {}", i + 1));
                }
            }
        }
    }
    r
}

/// Across files written by a sorted writer: the first sort key of file k+1 must not go below that of file k (a globally sorted set of files).
pub fn files_in_order(paths: &[std::path::PathBuf], col: usize) -> bool {
    let mut last: Option<Vec<u8>> = None;
    for p in paths {
        let meta = ArrowReaderMetadata::load(&File::open(p).unwrap(), ArrowReaderOptions::new()).unwrap();
        let reader = ParquetRecordBatchReaderBuilder::new_with_metadata(File::open(p).unwrap(), meta).build().unwrap();
        for b in reader {
            let b = b.unwrap();
            let a = cast(b.column(col), &DataType::Int64).unwrap();
            let a = a.as_any().downcast_ref::<Int64Array>().unwrap();
            for i in 0..a.len() {
                if a.is_null(i) { continue; }
                let k = a.value(i).to_be_bytes().to_vec();
                if last.as_ref().is_some_and(|l| l > &k) { return false; }
                last = Some(k);
            }
        }
    }
    true
}
