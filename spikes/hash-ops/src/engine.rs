//! The generic pipeline: Arrow batches with a selection vector, separate operators, checked kernels.
//! Operators receive a batch by value and push zero, one or several batches to the next stage, so a join
//! can fan out; filters and projections modify the batch in place.
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};

use arrow::array::{Array, ArrayRef, AsArray, RecordBatch};
use arrow::buffer::NullBuffer;
use arrow::datatypes::{DataType, Date32Type, Decimal64Type, Int64Type, SchemaRef};

use crate::keys::{KCol, KData};
use crate::kernels::*;

#[derive(Clone)]
pub enum Ref {
    Col(&'static str),
    Tmp(usize),
}

/// One batch flowing through the operators. `sel` lists the rows still alive (None = all of them);
/// `tmp` holds columns computed by `Project`, full length, meaningful only at the selected rows.
pub struct Batch {
    pub schema: SchemaRef,
    pub rows: usize,
    pub sel: Option<Vec<u32>>,
    pub cols: Vec<ArrayRef>,
    nulls: Vec<Option<NullBuffer>>,
    bytes: Vec<OnceLock<Vec<u8>>>,
    pub tmp: Vec<Out>,
}

impl Batch {
    pub fn new(rb: &RecordBatch) -> Self {
        let nulls: Vec<Option<NullBuffer>> = rb.columns().iter().map(|c| c.nulls().filter(|n| n.null_count() > 0).cloned()).collect();
        let bytes = (0..nulls.len()).map(|_| OnceLock::new()).collect();
        Batch { schema: rb.schema(), rows: rb.num_rows(), sel: None, cols: rb.columns().to_vec(), nulls, bytes, tmp: Vec::new() }
    }

    pub fn idx(&self, name: &str) -> usize {
        self.schema.index_of(name).unwrap_or_else(|_| panic!("column {name} is not in the batch (plan bug)"))
    }

    /// Number of live rows.
    pub fn live(&self) -> usize {
        self.sel.as_ref().map_or(self.rows, Vec::len)
    }

    fn valid(&self, k: usize) -> Valid<'_> {
        match &self.nulls[k] {
            None => Valid::All,
            Some(n) if self.sel.as_ref().is_none_or(|s| s.len() * 4 >= self.rows) => {
                Valid::Bytes(self.bytes[k].get_or_init(|| n.iter().map(|v| v as u8).collect()))
            }
            Some(n) => Valid::Bits(n),
        }
    }

    /// An i64-valued column: Decimal64, Int64, or a computed column.
    pub fn dec(&self, r: &Ref) -> Col<'_, i64> {
        match r {
            Ref::Col(n) => {
                let k = self.idx(n);
                match self.cols[k].data_type() {
                    DataType::Decimal64(..) => (&self.cols[k].as_primitive::<Decimal64Type>().values()[..], self.valid(k)),
                    DataType::Int64 => (&self.cols[k].as_primitive::<Int64Type>().values()[..], self.valid(k)),
                    other => panic!("column {n} of type {other:?} is not an i64 value column (plan bug)"),
                }
            }
            Ref::Tmp(k) => self.tmp[*k].col(),
        }
    }

    pub fn date(&self, name: &str) -> Col<'_, i32> {
        let k = self.idx(name);
        let a = self.cols[k].as_primitive::<Date32Type>();
        (&a.values()[..], self.valid(k))
    }

    pub fn key(&self, name: &str) -> R<KCol<'_>> {
        KCol::from_array(self.cols[self.idx(name)].as_ref())
    }
}

pub trait Op: Send {
    fn exec(&mut self, b: Batch, out: &mut dyn FnMut(Batch) -> R<()>) -> R<()>;
}

/// An operator that changes the batch in place and passes it on.
pub trait Stage: Send {
    fn apply(&mut self, b: &mut Batch) -> R<()>;
}

impl<T: Stage> Op for T {
    fn exec(&mut self, mut b: Batch, out: &mut dyn FnMut(Batch) -> R<()>) -> R<()> {
        self.apply(&mut b)?;
        if b.live() == 0 {
            return Ok(());
        }
        out(b)
    }
}

/// Narrows the selection to rows whose date column is in `[lo, hi]` (NULL rows drop out).
pub struct FilterDate {
    pub col: &'static str,
    pub lo: i32,
    pub hi: i32,
}

impl Stage for FilterDate {
    fn apply(&mut self, b: &mut Batch) -> R<()> {
        let new = {
            let (c, v) = b.date(self.col);
            filter_range(c, v, self.lo, self.hi, b.rows, b.sel.as_deref())
        };
        b.sel = Some(new);
        Ok(())
    }
}

/// `a < b` on two date columns (a NULL on either side drops the row).
pub struct FilterDateLt {
    pub a: &'static str,
    pub b: &'static str,
}

impl Stage for FilterDateLt {
    fn apply(&mut self, b: &mut Batch) -> R<()> {
        let new = {
            let ((x, vx), (y, vy)) = (b.date(self.a), b.date(self.b));
            filter_with(b.rows, b.sel.as_deref(), |i| (x[i] < y[i]) & vx.ok(i) & vy.ok(i))
        };
        b.sel = Some(new);
        Ok(())
    }
}

pub struct FilterDec {
    pub col: &'static str,
    pub lo: i64,
    pub hi: i64,
}

impl Stage for FilterDec {
    fn apply(&mut self, b: &mut Batch) -> R<()> {
        let new = {
            let (c, v) = b.dec(&Ref::Col(self.col));
            filter_range(c, v, self.lo, self.hi, b.rows, b.sel.as_deref())
        };
        b.sel = Some(new);
        Ok(())
    }
}

/// `col IN (set)` on a string column (dictionary or plain); NULL drops the row.
pub struct FilterStrIn {
    pub col: &'static str,
    pub set: &'static [&'static str],
}

fn in_set(set: &[&str], v: &[u8]) -> bool {
    set.iter().any(|s| s.as_bytes() == v)
}

impl Stage for FilterStrIn {
    fn apply(&mut self, b: &mut Batch) -> R<()> {
        let new = {
            let kc = b.key(self.col)?;
            match kc.d {
                KData::Dict { offs, bytes, keys } => {
                    let ok: Vec<bool> = (0..offs.len() - 1).map(|j| in_set(self.set, &bytes[offs[j] as usize..offs[j + 1] as usize])).collect();
                    // `&&`: the key in a NULL slot is arbitrary and may be out of range of the dictionary.
                    filter_with(b.rows, b.sel.as_deref(), |i| !kc.is_null(i) && ok[keys[i] as usize])
                }
                KData::Str { .. } => filter_with(b.rows, b.sel.as_deref(), |i| !kc.is_null(i) && in_set(self.set, kc.bytes(i))),
                _ => return Err(EngineError("not supported yet: string filter on a non-string column".into())),
            }
        };
        b.sel = Some(new);
        Ok(())
    }
}

pub enum Step {
    SubConstLeft { c: i64, a: Ref },
    Mul { a: Ref, b: Ref },
    /// `CASE WHEN col [NOT] IN (set) THEN 1 ELSE 0 END` (a NULL condition takes the ELSE branch; never NULL).
    FlagIn { col: &'static str, set: &'static [&'static str], negate: bool },
}

/// Computes expression columns over the selected rows, one primitive kernel per step.
pub struct Project(pub Vec<Step>);

impl Stage for Project {
    fn apply(&mut self, b: &mut Batch) -> R<()> {
        for step in &self.0 {
            let out = match step {
                Step::SubConstLeft { c, a } => sub_const_left(*c, b.dec(a), b.rows, b.sel.as_deref())?,
                Step::Mul { a, b: rhs } => mul(b.dec(a), b.dec(rhs), b.rows, b.sel.as_deref())?,
                Step::FlagIn { col, set, negate } => {
                    let kc = b.key(col)?;
                    let mut v = vec![0i64; b.rows];
                    match kc.d {
                        KData::Dict { offs, bytes, keys } => {
                            let ok: Vec<bool> = (0..offs.len() - 1).map(|j| in_set(set, &bytes[offs[j] as usize..offs[j + 1] as usize])).collect();
                            each(b.rows, b.sel.as_deref(), |i| v[i] = (!kc.is_null(i) && (ok[keys[i] as usize] != *negate)) as i64);
                        }
                        KData::Str { .. } => each(b.rows, b.sel.as_deref(), |i| v[i] = (!kc.is_null(i) & (in_set(set, kc.bytes(i)) != *negate)) as i64),
                        _ => return Err(EngineError("not supported yet: string flag on a non-string column".into())),
                    }
                    Out { v, valid: None }
                }
            };
            b.tmp.push(out);
        }
        Ok(())
    }
}

pub trait Sink: Send {
    fn consume(&mut self, b: &Batch) -> R<()>;
}

/// Calls the closure for every batch of morsel `m` (a row group when reading Parquet, one batch in memory).
pub type Visit<'a> = &'a (dyn Fn(usize, &mut dyn FnMut(&RecordBatch)) + Sync);

fn drive<S: Sink>(ops: &mut [Box<dyn Op>], sink: &mut S, b: Batch) -> R<()> {
    match ops.split_first_mut() {
        None => sink.consume(&b),
        Some((op, rest)) => op.exec(b, &mut |nb| drive(rest, sink, nb)),
    }
}

/// Run one pipeline with morsel-driven workers; returns one sink per worker (in worker order).
pub fn run_pipeline<S: Sink>(
    morsels: usize,
    visit: Visit,
    threads: usize,
    mk_ops: &(dyn Fn() -> Vec<Box<dyn Op>> + Sync),
    mk_sink: &(dyn Fn() -> S + Sync),
) -> R<Vec<S>> {
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| -> R<S> {
                    let (mut ops, mut sink) = (mk_ops(), mk_sink());
                    let mut err: Option<EngineError> = None;
                    loop {
                        if failed.load(Relaxed) {
                            break;
                        }
                        let m = next.fetch_add(1, Relaxed);
                        if m >= morsels {
                            break;
                        }
                        visit(m, &mut |rb| {
                            if err.is_some() {
                                return;
                            }
                            if let Err(e) = drive(&mut ops, &mut sink, Batch::new(rb)) {
                                err = Some(e);
                            }
                        });
                        if let Some(e) = err.take() {
                            failed.store(true, Relaxed);
                            return Err(e);
                        }
                    }
                    Ok(sink)
                })
            })
            .collect();
        let mut sinks = Vec::new();
        let mut first_err = None;
        for h in handles {
            match h.join() {
                Ok(Ok(sink)) => sinks.push(sink),
                Ok(Err(e)) => first_err = first_err.or(Some(e)),
                Err(_) => first_err = first_err.or(Some(EngineError("worker panicked".into()))),
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(sinks),
        }
    })
}
