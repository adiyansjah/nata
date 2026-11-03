//! The generic pipeline: Arrow batches with a selection vector, separate operators, checked kernels.
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};

use arrow::array::{Array, ArrayRef, AsArray, RecordBatch};
use arrow::buffer::NullBuffer;
use arrow::datatypes::{Date32Type, Decimal64Type, Int32Type, SchemaRef};

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
    /// NULL bitmaps of the input columns that have any. Dense selections read them as one byte per row,
    /// expanded lazily once per batch; sparse selections test the bits directly (expanding every row to
    /// read a few would cost more than it saves).
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

    fn idx(&self, name: &str) -> usize {
        self.schema.index_of(name).expect("column present in the batch")
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

    pub fn dec(&self, r: &Ref) -> Col<'_, i64> {
        match r {
            Ref::Col(n) => {
                let k = self.idx(n);
                let a = self.cols[k].as_primitive::<Decimal64Type>();
                (&a.values()[..], self.valid(k))
            }
            Ref::Tmp(k) => self.tmp[*k].col(),
        }
    }

    pub fn date(&self, name: &str) -> Col<'_, i32> {
        let k = self.idx(name);
        let a = self.cols[k].as_primitive::<Date32Type>();
        (&a.values()[..], self.valid(k))
    }
}

pub trait Op: Send {
    fn exec(&mut self, b: &mut Batch) -> R<()>;
}

/// Narrows the selection to rows whose date column is in `[lo, hi]` (NULL rows drop out).
pub struct FilterDate {
    pub col: &'static str,
    pub lo: i32,
    pub hi: i32,
}

impl Op for FilterDate {
    fn exec(&mut self, b: &mut Batch) -> R<()> {
        let new = {
            let (c, v) = b.date(self.col);
            filter_range(c, v, self.lo, self.hi, b.rows, b.sel.as_deref())
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

impl Op for FilterDec {
    fn exec(&mut self, b: &mut Batch) -> R<()> {
        let new = {
            let (c, v) = b.dec(&Ref::Col(self.col));
            filter_range(c, v, self.lo, self.hi, b.rows, b.sel.as_deref())
        };
        b.sel = Some(new);
        Ok(())
    }
}

pub enum Step {
    SubConstLeft { c: i64, a: Ref },
    Mul { a: Ref, b: Ref },
}

/// Computes expression columns over the selected rows, one primitive kernel per step.
pub struct Project(pub Vec<Step>);

impl Op for Project {
    fn exec(&mut self, b: &mut Batch) -> R<()> {
        for step in &self.0 {
            let out = match step {
                Step::SubConstLeft { c, a } => sub_const_left(*c, b.dec(a), b.rows, b.sel.as_deref())?,
                Step::Mul { a, b: rhs } => mul(b.dec(a), b.dec(rhs), b.rows, b.sel.as_deref())?,
            };
            b.tmp.push(out);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Q1Acc {
    pub sums: [i128; 3],
    pub nn: [u64; 3],
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Q6Acc {
    pub sum: i128,
    pub nn: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    Q1(BTreeMap<(String, String), Q1Acc>),
    Q6(Q6Acc),
}

fn dec_text(v: i128, scale: u32) -> String {
    let p = 10i128.pow(scale);
    format!("{}{}.{:0w$}", if v < 0 { "-" } else { "" }, v.abs() / p, v.abs() % p, w = scale as usize)
}

fn sum_text(sum: i128, nn: u64, scale: u32) -> String {
    if nn == 0 { "NULL".into() } else { dec_text(sum, scale) }
}

impl Output {
    pub fn merge(&mut self, other: Output) {
        match (self, other) {
            (Output::Q1(a), Output::Q1(b)) => {
                for (k, v) in b {
                    let e = a.entry(k).or_default();
                    for j in 0..3 {
                        e.sums[j] += v.sums[j];
                        e.nn[j] += v.nn[j];
                    }
                    e.count += v.count;
                }
            }
            (Output::Q6(a), Output::Q6(b)) => {
                a.sum += b.sum;
                a.nn += b.nn;
            }
            _ => panic!("merging outputs of different queries"),
        }
    }

    /// The result as DuckDB prints it with `-csv` (no header).
    pub fn csv(&self) -> String {
        match self {
            Output::Q1(m) => m
                .iter()
                .map(|((rf, ls), a)| {
                    format!(
                        "{rf},{ls},{},{},{},{}",
                        sum_text(a.sums[0], a.nn[0], 2),
                        sum_text(a.sums[1], a.nn[1], 2),
                        sum_text(a.sums[2], a.nn[2], 4),
                        a.count
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Output::Q6(a) => sum_text(a.sum, a.nn, 4),
        }
    }
}

pub trait Sink: Send {
    fn consume(&mut self, b: &Batch) -> R<()>;
    fn finish(self: Box<Self>) -> Output;
}

const G: usize = 32; // distinct group-key values supported per run

/// GROUP BY two dictionary columns; SUMs over three decimal references; COUNT(*).
/// `nn` (non-NULL count per SUM) is `count - nulls`, so the all-valid path never touches it.
pub struct Q1Sink {
    interned: HashMap<Vec<u8>, u32>,
    names: Vec<String>,
    acc: Vec<[i128; 3]>,
    count: Vec<u64>,
    nulls: [Vec<u64>; 3],
    vals: [Ref; 3],
}

impl Q1Sink {
    pub fn new(vals: [Ref; 3]) -> Self {
        Q1Sink {
            interned: HashMap::new(),
            names: Vec::new(),
            acc: vec![[0; 3]; G * G],
            count: vec![0; G * G],
            nulls: std::array::from_fn(|_| vec![0; G * G]),
            vals,
        }
    }

    fn intern(&mut self, bytes: &[u8]) -> u32 {
        if let Some(&id) = self.interned.get(bytes) {
            return id;
        }
        let id = self.names.len() as u32;
        assert!((id as usize) < G, "too many distinct group-key values: not supported yet");
        self.names.push(String::from_utf8_lossy(bytes).into_owned());
        self.interned.insert(bytes.to_vec(), id);
        id
    }
}

impl Sink for Q1Sink {
    fn consume(&mut self, b: &Batch) -> R<()> {
        let (k0, k1) = (b.idx("l_returnflag"), b.idx("l_linestatus"));
        let (d0, d1) = (b.cols[k0].as_dictionary::<Int32Type>(), b.cols[k1].as_dictionary::<Int32Type>());
        if d0.keys().null_count() + d1.keys().null_count() > 0 {
            return Err(EngineError("NULL group key: not supported yet".into()));
        }
        let (v0, v1) = (d0.values().as_string::<i32>(), d1.values().as_string::<i32>());
        let t0: Vec<u32> = (0..v0.len()).map(|j| self.intern(v0.value(j).as_bytes())).collect();
        let t1: Vec<u32> = (0..v1.len()).map(|j| self.intern(v1.value(j).as_bytes())).collect();
        let (keys0, keys1) = (d0.keys().values(), d1.keys().values());
        let (rows, sel) = (b.rows, b.sel.as_deref());
        let gid = |i: usize| (t0[keys0[i] as usize] * G as u32 + t1[keys1[i] as usize]) as usize;
        let cols = [b.dec(&self.vals[0]), b.dec(&self.vals[1]), b.dec(&self.vals[2])];
        if cols.iter().all(|(_, v)| v.is_all()) {
            // The common case: one pass, group ids, count and all three sums together.
            group_sums_dense(gid, [cols[0].0, cols[1].0, cols[2].0], rows, sel, &mut self.acc, &mut self.count);
        } else {
            group_sums_masked(gid, cols, rows, sel, &mut self.acc, &mut self.count, &mut self.nulls);
        }
        Ok(())
    }

    fn finish(self: Box<Self>) -> Output {
        let mut m = BTreeMap::new();
        for g in 0..G * G {
            if self.count[g] > 0 {
                let key = (self.names[g / G].clone(), self.names[g % G].clone());
                m.insert(
                    key,
                    Q1Acc {
                        sums: self.acc[g],
                        nn: [self.count[g] - self.nulls[0][g], self.count[g] - self.nulls[1][g], self.count[g] - self.nulls[2][g]],
                        count: self.count[g],
                    },
                );
            }
        }
        Output::Q1(m)
    }
}

pub struct Q6Sink {
    r: Ref,
    acc: Q6Acc,
}

impl Sink for Q6Sink {
    fn consume(&mut self, b: &Batch) -> R<()> {
        let (s, n) = sum(b.dec(&self.r), b.rows, b.sel.as_deref());
        self.acc.sum += s;
        self.acc.nn += n;
        Ok(())
    }

    fn finish(self: Box<Self>) -> Output {
        Output::Q6(self.acc)
    }
}

pub struct Plan {
    pub name: &'static str,
    pub columns: &'static [&'static str],
    pub make: fn() -> (Vec<Box<dyn Op>>, Box<dyn Sink>),
}

pub const Q1_CUTOFF: i32 = 10471; // 1998-09-02

/// SELECT l_returnflag, l_linestatus, sum(l_quantity), sum(l_extendedprice),
///        sum(l_extendedprice * (1 - l_discount)), count(*)
/// FROM lineitem WHERE l_shipdate <= DATE '1998-09-02' GROUP BY 1, 2
pub fn q1() -> Plan {
    Plan {
        name: "q1",
        columns: &["l_returnflag", "l_linestatus", "l_quantity", "l_extendedprice", "l_discount", "l_shipdate"],
        make: || {
            (
                vec![
                    Box::new(FilterDate { col: "l_shipdate", lo: i32::MIN, hi: Q1_CUTOFF }),
                    Box::new(Project(vec![
                        Step::SubConstLeft { c: 100, a: Ref::Col("l_discount") },
                        Step::Mul { a: Ref::Col("l_extendedprice"), b: Ref::Tmp(0) },
                    ])),
                ],
                Box::new(Q1Sink::new([Ref::Col("l_quantity"), Ref::Col("l_extendedprice"), Ref::Tmp(1)])),
            )
        },
    }
}

/// SELECT sum(l_extendedprice * l_discount) FROM lineitem
/// WHERE l_shipdate >= DATE '1994-01-01' AND l_shipdate < DATE '1995-01-01'
///   AND l_discount BETWEEN 0.05 AND 0.07 AND l_quantity < 24
pub fn q6() -> Plan {
    Plan {
        name: "q6",
        columns: &["l_quantity", "l_extendedprice", "l_discount", "l_shipdate"],
        make: || {
            (
                vec![
                    Box::new(FilterDate { col: "l_shipdate", lo: 8766, hi: 9130 }),
                    Box::new(FilterDec { col: "l_discount", lo: 5, hi: 7 }),
                    Box::new(FilterDec { col: "l_quantity", lo: i64::MIN, hi: 2399 }),
                    Box::new(Project(vec![Step::Mul { a: Ref::Col("l_extendedprice"), b: Ref::Col("l_discount") }])),
                ],
                Box::new(Q6Sink { r: Ref::Tmp(0), acc: Q6Acc::default() }),
            )
        },
    }
}

/// Calls the closure for every batch of morsel `m` (a row group when reading Parquet, one batch in memory).
pub type Visit<'a> = &'a (dyn Fn(usize, &mut dyn FnMut(&RecordBatch)) + Sync);

/// Run a plan with morsel-driven workers.
pub fn run(morsels: usize, visit: Visit, plan: &Plan, threads: usize) -> R<Output> {
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| -> R<Output> {
                    let (mut ops, mut sink) = (plan.make)();
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
                            let mut batch = Batch::new(rb);
                            for op in ops.iter_mut() {
                                if let Err(e) = op.exec(&mut batch) {
                                    err = Some(e);
                                    return;
                                }
                            }
                            if let Err(e) = sink.consume(&batch) {
                                err = Some(e);
                            }
                        });
                        if let Some(e) = err.take() {
                            failed.store(true, Relaxed);
                            return Err(e);
                        }
                    }
                    Ok(sink.finish())
                })
            })
            .collect();
        let mut total: Option<Output> = None;
        for h in handles {
            let out = h.join().expect("worker panicked")?;
            match &mut total {
                None => total = Some(out),
                Some(t) => t.merge(out),
            }
        }
        Ok(total.expect("at least one worker"))
    })
}
