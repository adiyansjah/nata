//! Adversarial tables. Every column has its own special values; every field (and nested field) carries a Parquet field id.
use std::collections::HashMap;
use std::sync::Arc;

use arrow::array::*;
use arrow::buffer::{BooleanBuffer, NullBuffer, OffsetBuffer};
use arrow::datatypes::*;
use parquet::arrow::PARQUET_FIELD_ID_META_KEY;

use crate::Rng;

pub struct Profile {
    pub null_pct: u64,
    pub special_pct: u64,
    /// longest string/binary value
    pub max_len: usize,
}

impl Default for Profile {
    fn default() -> Self {
        Profile { null_pct: 10, special_pct: 15, max_len: 60 }
    }
}

fn fid(name: &str, t: DataType, nullable: bool, id: i32) -> Field {
    Field::new(name, t, nullable).with_metadata(HashMap::from([(PARQUET_FIELD_ID_META_KEY.to_string(), id.to_string())]))
}

pub fn schema() -> SchemaRef {
    let mut id = 0;
    let mut f = |name: &str, t: DataType| {
        id += 1;
        fid(name, t, true, id)
    };
    let mut fields = vec![
        fid("id", DataType::Int64, false, 100),
        f("i8", DataType::Int8), f("i16", DataType::Int16), f("i32", DataType::Int32), f("i64", DataType::Int64),
        f("u8", DataType::UInt8), f("u16", DataType::UInt16), f("u32", DataType::UInt32), f("u64", DataType::UInt64),
        f("f32", DataType::Float32), f("f64", DataType::Float64), f("f64_allnan", DataType::Float64), f("f64_zeros", DataType::Float64),
        f("b", DataType::Boolean),
        f("s", DataType::Utf8), f("s_long", DataType::Utf8), f("bin", DataType::Binary), f("s_ff", DataType::Binary),
        f("d9", DataType::Decimal128(9, 2)), f("d18", DataType::Decimal128(18, 4)), f("d38", DataType::Decimal128(38, 10)),
        f("date", DataType::Date32), f("ts_ms", DataType::Timestamp(TimeUnit::Millisecond, None)),
        f("ts_us_tz", DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into()))), f("ts_ns", DataType::Timestamp(TimeUnit::Nanosecond, None)),
        f("t32", DataType::Time32(TimeUnit::Millisecond)), f("t64", DataType::Time64(TimeUnit::Microsecond)),
        f("c_allnull", DataType::Int32), f("c_const", DataType::Int64),
        f("dict", DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))),
    ];
    // nested columns LAST, so leaf indexes of the flat columns equal their top-level indexes
    let item = Arc::new(fid("item", DataType::Int32, true, 201));
    fields.push(fid("list", DataType::List(item), true, 200));
    let st = Fields::from(vec![fid("a", DataType::Int64, true, 301), fid("b", DataType::Utf8, true, 302)]);
    fields.push(fid("st", DataType::Struct(st), false, 300));
    Arc::new(Schema::new(fields))
}

pub fn expected_field_ids() -> Vec<(String, i32)> {
    let s = schema();
    let mut v = vec![];
    for f in s.fields() {
        v.push((f.name().clone(), f.metadata()[PARQUET_FIELD_ID_META_KEY].parse().unwrap()));
        match f.data_type() {
            DataType::Struct(cs) => for c in cs { v.push((format!("{}.{}", f.name(), c.name()), c.metadata()[PARQUET_FIELD_ID_META_KEY].parse().unwrap())) },
            DataType::List(c) => v.push((format!("{}.{}", f.name(), c.name()), c.metadata()[PARQUET_FIELD_ID_META_KEY].parse().unwrap())),
            _ => {}
        }
    }
    v
}

const CHUNKS: &[&str] = &["a", "b", "z", "A", "é", "ß", "日本語", "€", "😀", "𝔘", "", " ", "%", "\u{0}", "~", "ǅ"];

fn text(rng: &mut Rng, max: usize, common_prefix: &str) -> String {
    let mut s = String::from(common_prefix);
    let target = rng.below(max as u64 + 1) as usize;
    while s.len() < target {
        s.push_str(CHUNKS[rng.below(CHUNKS.len() as u64) as usize]);
    }
    s
}

fn bytes(rng: &mut Rng, max: usize, ff: bool) -> Vec<u8> {
    let n = rng.below(max as u64 + 1) as usize;
    if ff && rng.chance(40) {
        return vec![0xFF; n.max(1)];          // all 0xFF: its truncated upper bound cannot be incremented
    }
    (0..n).map(|_| if rng.chance(30) { 0xFF } else { rng.below(256) as u8 }).collect()
}

fn nulls(rng: &mut Rng, n: usize, pct: u64) -> Option<NullBuffer> {
    if pct == 0 { return None; }
    Some(NullBuffer::from((0..n).map(|_| !rng.chance(pct)).collect::<Vec<bool>>()))
}

macro_rules! prim {
    ($rng:expr, $n:expr, $p:expr, $A:ty, $special:expr, $gen:expr) => {{
        let vals: Vec<_> = (0..$n).map(|_| if $rng.chance($p.special_pct) { $special[$rng.below($special.len() as u64) as usize] } else { $gen }).collect();
        let nb = nulls($rng, $n, $p.null_pct);
        Arc::new(<$A>::new(vals.into(), nb)) as ArrayRef
    }};
}

/// `rows` rows starting at row number `start` (the `id` column is `start..start+rows`, ascending).
pub fn batch(rng: &mut Rng, start: i64, rows: usize, p: &Profile) -> RecordBatch {
    let n = rows;
    let mut cols: Vec<ArrayRef> = vec![];
    cols.push(Arc::new(Int64Array::from_iter_values(start..start + n as i64)));
    cols.push(prim!(rng, n, p, Int8Array, [i8::MIN, i8::MAX, 0, -1], rng.next() as i8));
    cols.push(prim!(rng, n, p, Int16Array, [i16::MIN, i16::MAX, 0, -1], rng.next() as i16));
    cols.push(prim!(rng, n, p, Int32Array, [i32::MIN, i32::MAX, 0, -1], rng.next() as i32));
    cols.push(prim!(rng, n, p, Int64Array, [i64::MIN, i64::MAX, 0, -1], rng.next() as i64));
    cols.push(prim!(rng, n, p, UInt8Array, [0u8, 255, 128, 127], rng.next() as u8));
    cols.push(prim!(rng, n, p, UInt16Array, [0u16, 65535, 32768, 32767], rng.next() as u16));
    cols.push(prim!(rng, n, p, UInt32Array, [0u32, u32::MAX, 1 << 31, (1 << 31) - 1], rng.next() as u32));
    cols.push(prim!(rng, n, p, UInt64Array, [0u64, u64::MAX, 1 << 63, (1 << 63) - 1], rng.next()));
    let fsp32 = [f32::NAN, -0.0, 0.0, f32::INFINITY, f32::NEG_INFINITY, f32::MIN_POSITIVE / 2.0, f32::MAX, f32::MIN];
    let fsp64 = [f64::NAN, -0.0, 0.0, f64::INFINITY, f64::NEG_INFINITY, f64::MIN_POSITIVE / 2.0, f64::MAX, f64::MIN];
    cols.push(prim!(rng, n, p, Float32Array, fsp32, (rng.below(2000) as f32 - 1000.0) / 7.0));
    cols.push(prim!(rng, n, p, Float64Array, fsp64, (rng.below(2000) as f64 - 1000.0) / 7.0));
    cols.push(Arc::new(Float64Array::new(vec![f64::NAN; n].into(), nulls(rng, n, 30))));
    cols.push(Arc::new(Float64Array::new((0..n).map(|_| if rng.chance(50) { -0.0 } else { 0.0 }).collect::<Vec<_>>().into(), nulls(rng, n, 5))));
    cols.push(Arc::new(BooleanArray::new(BooleanBuffer::from((0..n).map(|_| rng.chance(50)).collect::<Vec<_>>()), nulls(rng, n, p.null_pct))));
    let nb = nulls(rng, n, p.null_pct);
    cols.push(Arc::new(StringArray::from_iter((0..n).map(|i| nb.as_ref().is_none_or(|b| b.is_valid(i)).then(|| text(rng, 8, ""))))));
    let nb = nulls(rng, n, p.null_pct);
    cols.push(Arc::new(StringArray::from_iter((0..n).map(|i| nb.as_ref().is_none_or(|b| b.is_valid(i)).then(|| text(rng, p.max_len, "common-prefix-of-sixteen+"))))));
    let nb = nulls(rng, n, p.null_pct);
    cols.push(Arc::new(BinaryArray::from_iter((0..n).map(|i| nb.as_ref().is_none_or(|b| b.is_valid(i)).then(|| bytes(rng, p.max_len, false))))));
    let nb = nulls(rng, n, p.null_pct);
    cols.push(Arc::new(BinaryArray::from_iter((0..n).map(|i| nb.as_ref().is_none_or(|b| b.is_valid(i)).then(|| bytes(rng, p.max_len, true))))));
    cols.push(Arc::new(Decimal128Array::new((0..n).map(|_| if rng.chance(p.special_pct) { [999_999_99, -999_999_99, 0, -1][rng.below(4) as usize] } else { rng.below(2_000_000) as i128 - 1_000_000 }).collect::<Vec<_>>().into(), nulls(rng, n, p.null_pct)).with_precision_and_scale(9, 2).unwrap()));
    cols.push(Arc::new(Decimal128Array::new((0..n).map(|_| if rng.chance(p.special_pct) { [10i128.pow(18) - 1, -(10i128.pow(18) - 1), 0, -1][rng.below(4) as usize] } else { rng.next() as i64 as i128 }).collect::<Vec<_>>().into(), nulls(rng, n, p.null_pct)).with_precision_and_scale(18, 4).unwrap()));
    cols.push(Arc::new(Decimal128Array::new((0..n).map(|_| if rng.chance(p.special_pct) { [10i128.pow(38) - 1, -(10i128.pow(38) - 1), 0, -1][rng.below(4) as usize] } else { ((rng.next() as i64 as i128) << 40) ^ rng.next() as i128 }).collect::<Vec<_>>().into(), nulls(rng, n, p.null_pct)).with_precision_and_scale(38, 10).unwrap()));
    cols.push(prim!(rng, n, p, Date32Array, [0, -1, i32::MAX / 2, -25567], rng.below(40000) as i32 - 5000));
    cols.push(prim!(rng, n, p, TimestampMillisecondArray, [0i64, -1, 253_402_300_799_999, -62_135_596_800_000], rng.below(4_000_000_000_000) as i64 - 1_000_000_000_000));
    cols.push(Arc::new(prim!(rng, n, p, TimestampMicrosecondArray, [0i64, -1, 253_402_300_799_999_999, -62_135_596_800_000_000], rng.next() as i64 / 100_000).as_any().downcast_ref::<TimestampMicrosecondArray>().unwrap().clone().with_timezone("UTC")));
    cols.push(prim!(rng, n, p, TimestampNanosecondArray, [0, -1, i64::MAX, i64::MIN], rng.next() as i64));
    cols.push(prim!(rng, n, p, Time32MillisecondArray, [0, 86_399_999, 1], rng.below(86_400_000) as i32));
    cols.push(prim!(rng, n, p, Time64MicrosecondArray, [0i64, 86_399_999_999, 1], rng.below(86_400_000_000) as i64));
    cols.push(Arc::new(Int32Array::new_null(n)));
    cols.push(Arc::new(Int64Array::new(vec![42; n].into(), nulls(rng, n, 5))));
    let nb = nulls(rng, n, p.null_pct);
    let keys = Int32Array::new((0..n).map(|_| rng.below(5) as i32).collect::<Vec<_>>().into(), nb);
    cols.push(Arc::new(DictionaryArray::<Int32Type>::try_new(keys, Arc::new(StringArray::from(vec!["dd", "aa", "é", "", "zz"]))).unwrap()));
    // list<int32>
    let lens: Vec<usize> = (0..n).map(|_| rng.below(4) as usize).collect();
    let total: usize = lens.iter().sum();
    let items = Int32Array::new((0..total).map(|_| rng.next() as i32 % 1000).collect::<Vec<_>>().into(), nulls(rng, total, 10));
    let list_nulls = nulls(rng, n, p.null_pct).map(|b| {
        // a null list must have zero length, so rebuild the offsets from `lens` with null entries emptied
        b
    });
    let lens: Vec<usize> = lens.iter().enumerate().map(|(i, &l)| if list_nulls.as_ref().is_none_or(|b| b.is_valid(i)) { l } else { 0 }).collect();
    let total2: usize = lens.iter().sum();
    let items = Int32Array::new(items.values()[..total2].to_vec().into(), nulls(rng, total2, 10));
    let item_field = match schema().field_with_name("list").unwrap().data_type() { DataType::List(f) => f.clone(), _ => unreachable!() };
    cols.push(Arc::new(ListArray::new(item_field, OffsetBuffer::from_lengths(lens), Arc::new(items), list_nulls)));
    // struct<a: i64, b: utf8>, never null itself
    let sa = Int64Array::new((0..n).map(|_| rng.next() as i64).collect::<Vec<_>>().into(), nulls(rng, n, p.null_pct));
    let nb = nulls(rng, n, p.null_pct);
    let sb = StringArray::from_iter((0..n).map(|i| nb.as_ref().is_none_or(|b| b.is_valid(i)).then(|| text(rng, 20, ""))));
    let st_fields = match schema().field_with_name("st").unwrap().data_type() { DataType::Struct(f) => f.clone(), _ => unreachable!() };
    cols.push(Arc::new(StructArray::new(st_fields, vec![Arc::new(sa), Arc::new(sb)], None)));
    RecordBatch::try_new(schema(), cols).expect("batch")
}

pub fn table(seed: u64, rows: usize, chunk: usize, p: &Profile) -> Vec<RecordBatch> {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut out = vec![];
    let mut at = 0;
    while at < rows {
        let n = chunk.min(rows - at);
        out.push(batch(&mut rng, at as i64, n, p));
        at += n;
    }
    out
}
