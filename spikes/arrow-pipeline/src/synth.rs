//! Deterministic synthetic data and an independent scalar reference for the LIMIT / early-finish properties.
//!
//! Columns: `id` Int64 (unique over the whole data set: it identifies a row), `a` Decimal64(15,2) nullable,
//! `b` Date32 nullable, `c` Decimal64(15,2). The reference reads plain `Row`s with `Option` logic and shares no
//! code with the kernels.
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, AsArray, Date32Array, Decimal64Array, Int64Array, RecordBatch};
use arrow::buffer::NullBuffer;
use arrow::datatypes::{DataType, Date32Type, Decimal64Type, Field, Int64Type, Schema};

use crate::batch::{Batch, Ref};
use crate::ops::{FilterDate, FilterDec, ProjectOpFactory, StepSpec};
use crate::proto::api::OperatorFactory;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Row {
    pub id: i64,
    pub a: Option<i64>,
    pub b: Option<i32>,
    pub c: i64,
}

pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut x = self.0;
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^ (x >> 31)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + self.below((hi - lo) as u64 + 1) as i64
    }
}

#[derive(Clone, Debug)]
pub struct SynthCfg {
    pub seed: u64,
    pub morsels: usize,
    pub max_batches_per_morsel: usize,
    /// Rows per batch are drawn from `0..=max_rows` (so empty batches and empty morsels occur).
    pub max_rows: usize,
    pub null_pct: u64,
    /// Percent of rows whose `c` is huge, so `a * c` overflows `i64`.
    pub huge_c_pct: u64,
    /// If set: every morsel is ONE batch of `rows` rows of which exactly `m` have `a = 1` (the rest `a = 0`), no
    /// NULLs. Used for the provable bound on morsels read.
    pub stripe: Option<(usize, usize)>,
}

impl SynthCfg {
    pub fn random(seed: u64) -> Self {
        SynthCfg { seed, morsels: 0, max_batches_per_morsel: 3, max_rows: 60, null_pct: 0, huge_c_pct: 0, stripe: None }
    }
}

pub fn schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("a", DataType::Decimal64(15, 2), true),
        Field::new("b", DataType::Date32, true),
        Field::new("c", DataType::Decimal64(15, 2), false),
    ]))
}

fn batch_of(rows: &[Row]) -> RecordBatch {
    let nulls = |f: &dyn Fn(&Row) -> bool| {
        let v: Vec<bool> = rows.iter().map(f).collect();
        if v.iter().all(|x| *x) { None } else { Some(NullBuffer::from(v)) }
    };
    let id: ArrayRef = Arc::new(Int64Array::from(rows.iter().map(|r| r.id).collect::<Vec<_>>()));
    let a: ArrayRef = Arc::new(
        Decimal64Array::new(rows.iter().map(|r| r.a.unwrap_or(0)).collect::<Vec<_>>().into(), nulls(&|r| r.a.is_some()))
            .with_precision_and_scale(15, 2)
            .expect("precision"),
    );
    let b: ArrayRef = Arc::new(Date32Array::new(rows.iter().map(|r| r.b.unwrap_or(0)).collect::<Vec<_>>().into(), nulls(&|r| r.b.is_some())));
    let c: ArrayRef = Arc::new(
        Decimal64Array::new(rows.iter().map(|r| r.c).collect::<Vec<_>>().into(), None).with_precision_and_scale(15, 2).expect("precision"),
    );
    RecordBatch::try_new(schema(), vec![id, a, b, c]).expect("batch")
}

/// The data as morsels of batches, and the same rows flat (in id order) for the reference.
pub fn generate(cfg: &SynthCfg) -> (Arc<Vec<Vec<RecordBatch>>>, Vec<Row>) {
    let mut rng = Rng(cfg.seed ^ 0xABCD_EF01_2345_6789);
    let (mut morsels, mut all, mut id) = (Vec::new(), Vec::new(), 0i64);
    for _ in 0..cfg.morsels {
        let mut batches = Vec::new();
        let nb = if cfg.stripe.is_some() { 1 } else { rng.below(cfg.max_batches_per_morsel as u64 + 1) as usize };
        for _ in 0..nb {
            let n = match cfg.stripe {
                Some((rows, _)) => rows,
                None => rng.below(cfg.max_rows as u64 + 1) as usize,
            };
            let mut rows = Vec::with_capacity(n);
            for j in 0..n {
                let row = match cfg.stripe {
                    Some((_, m)) => Row { id, a: Some((j < m) as i64), b: Some(1), c: 2 },
                    None => {
                        let null_a = rng.below(100) < cfg.null_pct;
                        let null_b = rng.below(100) < cfg.null_pct;
                        let c = if rng.below(100) < cfg.huge_c_pct { i64::MAX / 2 + rng.range(0, 1000) } else { rng.range(-20, 20) };
                        Row { id, a: (!null_a).then(|| rng.range(-50, 149)), b: (!null_b).then(|| rng.range(0, 99) as i32), c }
                    }
                };
                id += 1;
                rows.push(row);
            }
            batches.push(batch_of(&rows));
            all.extend(rows);
        }
        morsels.push(batches);
    }
    (Arc::new(morsels), all)
}

/// A filter on the synthetic schema: a closed range on one column; a NULL fails it.
#[derive(Clone, Debug)]
pub enum F {
    A(i64, i64),
    B(i32, i32),
    C(i64, i64),
}

impl F {
    pub fn random(rng: &mut Rng) -> F {
        match rng.below(3) {
            0 => {
                let lo = rng.range(-50, 149);
                F::A(lo, lo + rng.range(0, 120))
            }
            1 => {
                let lo = rng.range(0, 99) as i32;
                F::B(lo, lo + rng.range(0, 80) as i32)
            }
            _ => {
                let lo = rng.range(-20, 20);
                F::C(lo, lo + rng.range(0, 30))
            }
        }
    }

    pub fn holds(&self, r: &Row) -> bool {
        match *self {
            F::A(lo, hi) => r.a.is_some_and(|a| lo <= a && a <= hi),
            F::B(lo, hi) => r.b.is_some_and(|b| lo <= b && b <= hi),
            F::C(lo, hi) => lo <= r.c && r.c <= hi,
        }
    }

    pub fn op(&self) -> Arc<dyn OperatorFactory> {
        match *self {
            F::A(lo, hi) => Arc::new(FilterDec { col: "a", lo, hi }),
            F::B(lo, hi) => Arc::new(FilterDate { col: "b", lo, hi }),
            F::C(lo, hi) => Arc::new(FilterDec { col: "c", lo, hi }),
        }
    }
}

pub fn filtered(rows: &[Row], fs: &[F]) -> Vec<Row> {
    rows.iter().filter(|r| fs.iter().all(|f| f.holds(r))).cloned().collect()
}

/// `p = a * c`, computed over the selected rows (Tmp(0)).
pub fn project_op() -> Arc<dyn OperatorFactory> {
    Arc::new(ProjectOpFactory(vec![StepSpec::Mul { a: Ref::Col("a"), b: Ref::Col("c") }]))
}

/// What the reference says `p` is for a row: `Err` = i64 overflow, `Ok(None)` = NULL.
pub fn ref_p(r: &Row) -> Result<Option<i64>, ()> {
    match r.a {
        None => Ok(None),
        Some(a) => a.checked_mul(r.c).map(Some).ok_or(()),
    }
}

/// An output row read back from the collected batches (`p` only if the pipeline had a project).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OutRow {
    pub row: Row,
    pub p: Option<Option<i64>>,
}

pub fn read_rows(batches: &[Batch]) -> Vec<OutRow> {
    let mut out = Vec::new();
    for b in batches {
        let id = b.column("id").as_primitive::<Int64Type>();
        let a = b.column("a").as_primitive::<Decimal64Type>();
        let bb = b.column("b").as_primitive::<Date32Type>();
        let c = b.column("c").as_primitive::<Decimal64Type>();
        for i in b.live() {
            let p = (b.tmp_count() > 0).then(|| {
                let t = b.tmp_col(0);
                t.valid.as_ref().is_none_or(|v| v[i] != 0).then(|| t.v[i])
            });
            out.push(OutRow {
                row: Row { id: id.value(i), a: a.is_valid(i).then(|| a.value(i)), b: bb.is_valid(i).then(|| bb.value(i)), c: c.value(i) },
                p,
            });
        }
    }
    out
}
