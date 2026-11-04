//! Random table generation for the property tests, and NULL injection for the TPC-H data.
use std::sync::Arc;

use arrow::array::*;
use arrow::buffer::{Buffer, NullBuffer, OffsetBuffer};
use arrow::datatypes::{DataType, Field, Int32Type, Schema};

use crate::reference::{Cell, Row, V};

pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
    pub fn pct(&mut self, p: u64) -> bool {
        self.below(100) < p
    }
    pub fn pick<T: Copy>(&mut self, v: &[T]) -> T {
        v[self.below(v.len() as u64) as usize]
    }
}

/// A generator for a test: `base` is the fixed seed of the test; the environment variable `HASH_OPS_SEED`
/// (default 0, which gives exactly `base`) selects another stream, so a loop of runs explores new inputs.
pub fn seeded(base: u64) -> Rng {
    let extra: u64 = std::env::var("HASH_OPS_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let s = base ^ extra.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut r = Rng(if s == 0 { 1 } else { s });
    for _ in 0..4 {
        r.next(); // decorrelate nearby seeds
    }
    if extra == 0 { Rng(base) } else { r }
}

/// `n` cases, times `HASH_OPS_SCALE` (default 1).
pub fn scaled(n: usize) -> usize {
    n * std::env::var("HASH_OPS_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(1usize)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    I64,
    I32,
    Date,
    Dec,
    /// Dictionary string, Int32 keys; the dictionary of each batch is different and has unused entries.
    Dict,
    Str,
}

pub const KEY_TYS: [Ty; 6] = [Ty::I64, Ty::I32, Ty::Date, Ty::Dec, Ty::Dict, Ty::Str];

/// How the cells of a column are drawn.
#[derive(Debug, Clone, Copy)]
pub struct Dom {
    pub distinct: u64,
    /// Percent of rows that take the single hot value (0 = uniform).
    pub hot_pct: u64,
    pub null_pct: u64,
}

fn string_of(n: u64) -> String {
    match n % 7 {
        0 => format!("k{n}"),
        1 => String::new(), // the empty string is a value, not a NULL
        2 => format!("é{n}"),
        3 => format!("same-prefix-{}", n / 7),
        4 => format!("same-prefix-{}x", n / 7),
        5 => "k".repeat((n % 40) as usize + 1),
        _ => format!("{n}"),
    }
}

pub fn gen_cell(rng: &mut Rng, ty: Ty, d: Dom) -> Cell {
    if rng.pct(d.null_pct) {
        return None;
    }
    let n = if rng.pct(d.hot_pct) { 0 } else { rng.below(d.distinct.max(1)) };
    Some(match ty {
        Ty::I64 => V::I(if n == 1 { i64::MIN } else if n == 2 { i64::MAX } else { n as i64 - 3 }),
        Ty::I32 | Ty::Date => V::I(if n == 1 { i32::MIN as i64 } else if n == 2 { i32::MAX as i64 } else { n as i64 - 3 }),
        Ty::Dec => V::I(n as i64 - 3),
        Ty::Dict | Ty::Str => V::S(string_of(n)),
    })
}

pub fn gen_rows(rng: &mut Rng, tys: &[Ty], doms: &[Dom], n: usize) -> Vec<Row> {
    (0..n).map(|_| tys.iter().zip(doms).map(|(&t, &d)| gen_cell(rng, t, d)).collect()).collect()
}

/// Deterministic junk for the slots under a NULL: Arrow leaves them unspecified, so an engine must never
/// read them. Values depend only on the position and a per-kind salt (no draw from the test's Rng).
fn junk(i: usize, salt: u64) -> u64 {
    splitmix((i as u64) ^ (salt << 48) ^ 0xDEAD_BEEF)
}

fn build_array(ty: Ty, cells: &[Cell], rng: &mut Rng) -> ArrayRef {
    // Integer columns: the real value in valid slots, junk (non-zero, often huge) under a NULL.
    let ints = |salt: u64| -> (Vec<i64>, NullBuffer) {
        let vals = cells.iter().enumerate().map(|(i, c)| match c { Some(V::I(v)) => *v, _ => junk(i, salt) as i64 | 1 }).collect();
        (vals, NullBuffer::from(cells.iter().map(|c| c.is_some()).collect::<Vec<bool>>()))
    };
    match ty {
        Ty::I64 => {
            let (v, n) = ints(1);
            Arc::new(Int64Array::new(v.into(), Some(n)))
        }
        Ty::I32 => {
            let (v, n) = ints(2);
            Arc::new(Int32Array::new(v.into_iter().map(|x| x as i32).collect::<Vec<_>>().into(), Some(n)))
        }
        Ty::Date => {
            let (v, n) = ints(3);
            Arc::new(Date32Array::new(v.into_iter().map(|x| x as i32).collect::<Vec<_>>().into(), Some(n)))
        }
        Ty::Dec => {
            let (v, n) = ints(4);
            Arc::new(Decimal64Array::new(v.into(), Some(n)).with_precision_and_scale(15, 2).expect("precision"))
        }
        Ty::Str => {
            // Junk bytes sit under a NULL slot (the offsets cover them), so a read of a NULL slot is visible.
            let (mut offs, mut bytes, mut valid) = (vec![0i32], Vec::new(), Vec::new());
            for (i, c) in cells.iter().enumerate() {
                match c {
                    Some(V::S(s)) => {
                        bytes.extend_from_slice(s.as_bytes());
                        valid.push(true);
                    }
                    _ => {
                        bytes.extend_from_slice(format!("junk-{}", junk(i, 5) % 97).as_bytes());
                        valid.push(false);
                    }
                }
                offs.push(bytes.len() as i32);
            }
            Arc::new(StringArray::new(OffsetBuffer::new(offs.into()), Buffer::from_vec(bytes), Some(NullBuffer::from(valid))))
        }
        Ty::Dict => {
            // A dictionary with the used values in random order plus unused extras. NULL keys are valid-bitmap
            // NULLs whose key slot holds junk: out of range, negative, i32::MIN/MAX, or in range.
            let mut vals: Vec<String> = cells.iter().filter_map(|c| match c { Some(V::S(s)) => Some(s.clone()), _ => None }).collect();
            vals.sort();
            vals.dedup();
            for e in 0..rng.below(4) {
                vals.push(format!("unused-{e}"));
            }
            for i in (1..vals.len()).rev() {
                vals.swap(i, rng.below(i as u64 + 1) as usize);
            }
            let nd = vals.len() as u64;
            let keys: Vec<i32> = cells
                .iter()
                .enumerate()
                .map(|(i, c)| match c {
                    Some(V::S(s)) => vals.iter().position(|v| v == s).expect("value in dictionary") as i32,
                    _ => match junk(i, 6) % 6 {
                        0 => 99,
                        1 => -1,
                        2 => i32::MAX,
                        3 => i32::MIN,
                        4 => (junk(i, 7) % nd.max(1)) as i32,
                        _ => 0,
                    },
                })
                .collect();
            let valid = NullBuffer::from(cells.iter().map(|c| c.is_some()).collect::<Vec<bool>>());
            let values = StringArray::from(vals.iter().map(String::as_str).collect::<Vec<_>>());
            Arc::new(DictionaryArray::<Int32Type>::try_new(Int32Array::new(keys.into(), Some(valid)), Arc::new(values)).expect("dictionary"))
        }
    }
}

/// Split rows into batches of random sizes (including empty ones), and sometimes slice each batch out of a
/// longer array so the arrays have a non-zero offset.
pub fn to_batches(rng: &mut Rng, rows: &[Row], tys: &[Ty], names: &[&str], max_batch: usize) -> Vec<RecordBatch> {
    let schema_for = |arrs: &[ArrayRef]| {
        Arc::new(Schema::new(names.iter().zip(arrs).map(|(n, a)| Field::new(*n, a.data_type().clone(), true)).collect::<Vec<_>>()))
    };
    let mut out = Vec::new();
    let mut at = 0;
    while at < rows.len() || out.is_empty() {
        let len = if rng.pct(10) { 0 } else { (1 + rng.below(max_batch as u64) as usize).min(rows.len() - at) };
        let chunk = &rows[at..at + len];
        at += len;
        let (front, back) = if rng.pct(50) { (rng.below(5) as usize, rng.below(5) as usize) } else { (0, 0) };
        let doms: Vec<Dom> = tys.iter().map(|_| Dom { distinct: 5, hot_pct: 0, null_pct: 20 }).collect();
        let padded: Vec<Row> = gen_rows(rng, tys, &doms, front)
            .into_iter()
            .chain(chunk.iter().cloned())
            .chain(gen_rows(rng, tys, &doms, back))
            .collect();
        let arrs: Vec<ArrayRef> = (0..tys.len())
            .map(|c| {
                let cells: Vec<Cell> = padded.iter().map(|r| r[c].clone()).collect();
                build_array(tys[c], &cells, rng).slice(front, len)
            })
            .collect();
        let opts = RecordBatchOptions::new().with_row_count(Some(len));
        out.push(RecordBatch::try_new_with_options(schema_for(&arrs), arrs, &opts).expect("batch"));
        if rows.is_empty() && out.len() >= 1 {
            break;
        }
    }
    out
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Make `pct` percent of the rows NULL in the named columns. The choice depends only on (morsel, column,
/// row within the morsel), never on which worker reads the data.
pub fn mask_columns(rb: &RecordBatch, names: &[&str], morsel: usize, offset: usize, pct: u32) -> RecordBatch {
    let schema = rb.schema();
    let n = rb.num_rows();
    let mut cols = rb.columns().to_vec();
    let mut fields: Vec<Field> = schema.fields().iter().map(|f| f.as_ref().clone()).collect();
    for (ci, f) in schema.fields().iter().enumerate() {
        if !names.contains(&f.name().as_str()) {
            continue;
        }
        let nulls = NullBuffer::from(
            (0..n).map(|i| splitmix(((morsel as u64) << 40) ^ ((ci as u64) << 32) ^ (offset + i) as u64) % 100 >= pct as u64).collect::<Vec<bool>>(),
        );
        let data = rb.column(ci).to_data().into_builder().nulls(Some(nulls)).build().expect("masked array");
        cols[ci] = make_array(data);
        fields[ci] = f.as_ref().clone().with_nullable(true);
    }
    RecordBatch::try_new(Arc::new(Schema::new(fields)), cols).expect("masked batch")
}

pub fn is_dict(dt: &DataType) -> bool {
    matches!(dt, DataType::Dictionary(..))
}
