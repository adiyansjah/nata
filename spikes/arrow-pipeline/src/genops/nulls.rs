//! Deterministic NULL injection, and an independent scalar reference.
//!
//! The reference shares no code with the engine: it reads the (masked) Arrow arrays row by row with
//! plain `Option` logic and exact `i128` arithmetic. Agreement with it is the correctness check for
//! the NULL-aware paths, where no DuckDB comparison is possible (the masked data is not in the file).
use std::collections::BTreeMap;
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, AsArray, Date32Array, Decimal64Array, RecordBatch};
use arrow::buffer::NullBuffer;
use arrow::datatypes::{DataType, Date32Type, Decimal64Type, Int32Type};

use crate::genops::engine::{Output, Q1Acc, Q1_CUTOFF, Q6Acc};
use crate::genops::source::Source;

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn is_null(rg: usize, col: usize, row: usize, pct: u32) -> bool {
    splitmix(((rg as u64) << 40) ^ ((col as u64) << 32) ^ row as u64) % 100 < pct as u64
}

const MASKED: [&str; 4] = ["l_quantity", "l_extendedprice", "l_discount", "l_shipdate"];

/// Make `pct` percent of the rows NULL in four columns. The choice depends only on (row group,
/// column, row within the group), never on which worker reads the data.
pub fn mask(rb: RecordBatch, rg: usize, offset: usize, pct: u32) -> RecordBatch {
    let schema = rb.schema();
    let n = rb.num_rows();
    let mut cols = rb.columns().to_vec();
    for (ci, field) in schema.fields().iter().enumerate() {
        if !MASKED.contains(&field.name().as_str()) {
            continue;
        }
        let nulls = NullBuffer::from((0..n).map(|i| !is_null(rg, ci, offset + i, pct)).collect::<Vec<bool>>());
        cols[ci] = match field.data_type() {
            DataType::Decimal64(p, s) => {
                let a = rb.column(ci).as_primitive::<Decimal64Type>();
                Arc::new(Decimal64Array::new(a.values().clone(), Some(nulls)).with_precision_and_scale(*p, *s).expect("precision")) as ArrayRef
            }
            DataType::Date32 => {
                let a = rb.column(ci).as_primitive::<Date32Type>();
                Arc::new(Date32Array::new(a.values().clone(), Some(nulls))) as ArrayRef
            }
            other => panic!("unexpected type {other:?}"),
        };
    }
    // The file declares these columns NOT NULL; the masked batch must say they may now contain NULLs.
    let fields: Vec<_> = schema
        .fields()
        .iter()
        .map(|f| if MASKED.contains(&f.name().as_str()) { Arc::new(f.as_ref().clone().with_nullable(true)) } else { f.clone() })
        .collect();
    RecordBatch::try_new(Arc::new(arrow::datatypes::Schema::new(fields)), cols).expect("masked batch")
}

fn for_each_masked_batch(src: &Source, pct: u32, mut f: impl FnMut(&RecordBatch)) {
    for rg in 0..src.row_groups {
        let mut offset = 0;
        for rb in src.reader(rg) {
            let rb = rb.expect("decode");
            let rb = if pct > 0 { mask(rb, rg, offset, pct) } else { rb };
            offset += rb.num_rows();
            f(&rb);
        }
    }
}

pub fn reference_q1(src: &Source, pct: u32) -> Output {
    let mut m: BTreeMap<(String, String), Q1Acc> = BTreeMap::new();
    for_each_masked_batch(src, pct, |rb| {
        let col = |n: &str| rb.column(rb.schema().index_of(n).expect("column")).clone();
        let (date, qty, price, disc) = (col("l_shipdate"), col("l_quantity"), col("l_extendedprice"), col("l_discount"));
        let (date, qty, price, disc) = (
            date.as_primitive::<Date32Type>().clone(),
            qty.as_primitive::<Decimal64Type>().clone(),
            price.as_primitive::<Decimal64Type>().clone(),
            disc.as_primitive::<Decimal64Type>().clone(),
        );
        let (rf, ls) = (col("l_returnflag"), col("l_linestatus"));
        let (rf, ls) = (rf.as_dictionary::<Int32Type>(), ls.as_dictionary::<Int32Type>());
        let name = |d: &arrow::array::DictionaryArray<Int32Type>, i: usize| {
            d.values().as_string::<i32>().value(d.keys().value(i) as usize).to_string()
        };
        for i in 0..rb.num_rows() {
            if date.is_null(i) || date.value(i) > Q1_CUTOFF {
                continue;
            }
            let a = m.entry((name(rf, i), name(ls, i))).or_default();
            a.count += 1;
            if qty.is_valid(i) {
                a.sums[0] += qty.value(i) as i128;
                a.nn[0] += 1;
            }
            if price.is_valid(i) {
                a.sums[1] += price.value(i) as i128;
                a.nn[1] += 1;
            }
            if price.is_valid(i) && disc.is_valid(i) {
                a.sums[2] += price.value(i) as i128 * (100 - disc.value(i) as i128);
                a.nn[2] += 1;
            }
        }
    });
    Output::Q1(m)
}

pub fn reference_q6(src: &Source, pct: u32) -> Output {
    let mut acc = Q6Acc::default();
    for_each_masked_batch(src, pct, |rb| {
        let col = |n: &str| rb.column(rb.schema().index_of(n).expect("column")).clone();
        let (date, qty, price, disc) = (col("l_shipdate"), col("l_quantity"), col("l_extendedprice"), col("l_discount"));
        let date = date.as_primitive::<Date32Type>().clone();
        let (qty, price, disc) = (
            qty.as_primitive::<Decimal64Type>().clone(),
            price.as_primitive::<Decimal64Type>().clone(),
            disc.as_primitive::<Decimal64Type>().clone(),
        );
        for i in 0..rb.num_rows() {
            if date.is_null(i) || disc.is_null(i) || qty.is_null(i) {
                continue;
            }
            let keep = (8766..=9130).contains(&date.value(i)) && (5..=7).contains(&disc.value(i)) && qty.value(i) < 2400;
            if keep && price.is_valid(i) {
                acc.sum += price.value(i) as i128 * disc.value(i) as i128;
                acc.nn += 1;
            }
        }
    });
    Output::Q6(acc)
}

/// Reads row group `rg` (masking `pct` percent of four columns when `pct > 0`) and calls `f` per batch.
pub fn parquet_visit(src: &Source, pct: u32) -> impl Fn(usize, &mut dyn FnMut(&RecordBatch)) + Sync + '_ {
    move |rg, f| {
        let mut offset = 0;
        for rb in src.reader(rg) {
            let rb = rb.expect("decode");
            let rb = if pct > 0 { mask(rb, rg, offset, pct) } else { rb };
            offset += rb.num_rows();
            f(&rb);
        }
    }
}

/// All batches of the file in memory (masked if `pct > 0`), so a timing can leave decoding out.
pub fn preload(src: &Source, pct: u32) -> Vec<RecordBatch> {
    let mut v = Vec::new();
    for_each_masked_batch(src, pct, |rb| v.push(rb.clone()));
    v
}

pub fn mem_visit(batches: &[RecordBatch]) -> impl Fn(usize, &mut dyn FnMut(&RecordBatch)) + Sync + '_ {
    move |i, f| f(&batches[i])
}
