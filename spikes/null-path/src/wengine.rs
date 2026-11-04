//! The generic pipeline again, with validity and dense selections as bitmaps (see `wk.rs`).
//! Same plans, same results as `engine.rs`; the options select how much of the word-wise design is on,
//! so each step can be timed against the one before it.
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};

use arrow::array::{Array, ArrayRef, AsArray, RecordBatch};
use arrow::datatypes::{Date32Type, Decimal64Type, Int32Type, SchemaRef};

use crate::bits::*;
use crate::engine::{Output, Q1_CUTOFF, Q1Acc, Q6Acc, Ref, Visit};
use crate::wk::*;

/// Which parts of the design are on. `Opts::STEP0 .. STEP5` are the cumulative steps of the report.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Opts {
    /// Filters keep a bitmap selection while at least `rows / dense_div` rows survive (0 = always an index vector).
    pub dense_div: usize,
    /// A dense selection is handled 64 rows at a time by Project (else it is turned into an index vector first).
    pub dense_project: bool,
    /// A dense selection is handled 64 rows at a time by the sinks.
    pub dense_sink: bool,
    /// A filter on a column drops its NULL rows, so the column is known valid for every later operator.
    pub null_reject: bool,
    /// Project checks overflow with one OR of the flags per word, and looks at validity and selection only if a flag was set.
    pub defer_overflow: bool,
    /// The sink's accumulations use `wrapping_add` (they cannot overflow) instead of the checked `+`.
    pub wrapping_sink: bool,
    /// Grouped sums add every selected row and subtract the (few) NULL rows through the set bits of the NULL words,
    /// instead of masking every row by its validity bit and counting NULLs per row.
    pub subtract_nulls: bool,
}

impl Opts {
    /// Bitmaps for validity, predicate bitmask ANDed with validity per word, index vector for the selection.
    pub const STEP1: Opts = Opts { dense_div: 0, dense_project: false, dense_sink: false, null_reject: false, defer_overflow: false, wrapping_sink: false, subtract_nulls: false };
    pub const STEP2: Opts = Opts { dense_div: 4, dense_project: true, dense_sink: false, null_reject: false, defer_overflow: false, wrapping_sink: false, subtract_nulls: false };
    /// The first version of step 3 (per-row masks in the sink), kept to measure what step 3 replaced.
    pub const STEP3A: Opts = Opts { dense_div: 4, dense_project: true, dense_sink: true, null_reject: false, defer_overflow: false, wrapping_sink: false, subtract_nulls: false };
    pub const STEP3: Opts = Opts { subtract_nulls: true, ..Opts::STEP3A };
    pub const STEP4: Opts = Opts { null_reject: true, ..Opts::STEP3 };
    pub const STEP5: Opts = Opts { defer_overflow: true, ..Opts::STEP4 };
    pub const STEP6: Opts = Opts { wrapping_sink: true, ..Opts::STEP5 };
}

pub struct Batch {
    pub schema: SchemaRef,
    pub rows: usize,
    pub sel: Sel,
    pub cols: Vec<ArrayRef>,
    /// Validity words of the input columns that have NULLs.
    valid: Vec<Option<Vec<u64>>>,
    /// Columns a null-rejecting filter has made valid for every selected row.
    known_valid: Vec<bool>,
    pub tmp: Vec<Out>,
    opts: Opts,
}

impl Batch {
    pub fn new(rb: &RecordBatch, opts: Opts) -> Self {
        let rows = rb.num_rows();
        let valid: Vec<_> = rb.columns().iter().map(|c| validity_words(c.nulls(), rows)).collect();
        Batch {
            schema: rb.schema(),
            rows,
            sel: Sel::All,
            cols: rb.columns().to_vec(),
            known_valid: vec![false; valid.len()],
            valid,
            tmp: Vec::new(),
            opts,
        }
    }

    fn idx(&self, name: &str) -> usize {
        self.schema.index_of(name).expect("column present in the batch")
    }

    fn validity(&self, k: usize) -> Option<&[u64]> {
        if self.known_valid[k] { None } else { self.valid[k].as_deref() }
    }

    pub fn dec(&self, r: &Ref) -> WCol<'_, i64> {
        match r {
            Ref::Col(n) => {
                let k = self.idx(n);
                (&self.cols[k].as_primitive::<Decimal64Type>().values()[..], self.validity(k))
            }
            Ref::Tmp(k) => self.tmp[*k].col(),
        }
    }

    /// The selection as the form `dense` allows: a bitmap only if the consumer works on words.
    fn sel_for(&self, dense: bool) -> std::borrow::Cow<'_, Sel> {
        match &self.sel {
            Sel::Mask(m) if !dense => std::borrow::Cow::Owned(Sel::Idx(mask_to_idx(m))),
            s => std::borrow::Cow::Borrowed(s),
        }
    }
}

pub trait WOp: Send {
    fn exec(&mut self, b: &mut Batch) -> R<()>;
}

pub enum FilterCol {
    Date(&'static str, i32, i32),
    Dec(&'static str, i64, i64),
}

pub struct Filter(pub FilterCol);

impl WOp for Filter {
    fn exec(&mut self, b: &mut Batch) -> R<()> {
        assert!(b.tmp.is_empty(), "a filter after Project is not supported by this spike");
        let fo = FilterOpts { dense_div: b.opts.dense_div };
        let (name, new) = match self.0 {
            FilterCol::Date(name, lo, hi) => {
                let k = b.idx(name);
                let c = b.cols[k].as_primitive::<Date32Type>().values();
                (name, filter_range(c, b.validity(k), lo, hi, b.rows, &b.sel, fo))
            }
            FilterCol::Dec(name, lo, hi) => {
                let k = b.idx(name);
                let c = b.cols[k].as_primitive::<Decimal64Type>().values();
                (name, filter_range(c, b.validity(k), lo, hi, b.rows, &b.sel, fo))
            }
        };
        b.sel = new;
        if b.opts.null_reject {
            let k = b.idx(name);
            b.known_valid[k] = true;
        }
        Ok(())
    }
}

pub enum Step {
    SubConstLeft { c: i64, a: Ref },
    Mul { a: Ref, b: Ref },
}

pub struct Project(pub Vec<Step>);

impl WOp for Project {
    fn exec(&mut self, b: &mut Batch) -> R<()> {
        for step in &self.0 {
            let sel = b.sel_for(b.opts.dense_project);
            let out = match step {
                Step::SubConstLeft { c, a } => sub_const_left(*c, b.dec(a), b.rows, &sel, b.opts.defer_overflow)?,
                Step::Mul { a, b: rhs } => mul(b.dec(a), b.dec(rhs), b.rows, &sel, b.opts.defer_overflow)?,
            };
            b.tmp.push(out);
        }
        Ok(())
    }
}

pub trait WSink: Send {
    fn consume(&mut self, b: &Batch) -> R<()>;
    fn finish(self: Box<Self>) -> Output;
}

const G: usize = 32;

pub struct Q1Sink {
    interned: HashMap<Vec<u8>, u32>,
    names: Vec<String>,
    acc: Vec<[i128; 3]>,
    count: Vec<u64>,
    nulls: [Vec<u64>; 3],
    vals: [Ref; 3],
    gids: Vec<u16>,
}

impl Q1Sink {
    pub fn new(vals: [Ref; 3]) -> Self {
        Q1Sink {
            interned: HashMap::new(),
            names: Vec::new(),
            acc: vec![[0; 3]; NG],
            count: vec![0; NG],
            nulls: std::array::from_fn(|_| vec![0; NG]),
            vals,
            gids: Vec::new(),
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

impl WSink for Q1Sink {
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
        // group ids first, in a loop of their own (G * G = NG, so they fit a u16 below NG); only the selected rows when sparse
        const _: () = assert!(G * G == NG);
        let sel = b.sel_for(b.opts.dense_sink);
        let gid = |i: usize| (t0[keys0[i] as usize] * G as u32 + t1[keys1[i] as usize]) as u16;
        match &*sel {
            Sel::Idx(idx) => {
                self.gids.resize(b.rows, 0);
                for &i in idx {
                    self.gids[i as usize] = gid(i as usize);
                }
            }
            _ => {
                self.gids.clear();
                self.gids.extend(keys0[..b.rows].iter().zip(&keys1[..b.rows]).map(|(&a, &c)| (t0[a as usize] * G as u32 + t1[c as usize]) as u16));
            }
        }
        let cols = [b.dec(&self.vals[0]), b.dec(&self.vals[1]), b.dec(&self.vals[2])];
        let (g, c, a, n) = (&self.gids, b.rows, &mut self.acc, &mut self.count);
        match (b.opts.wrapping_sink, b.opts.subtract_nulls) {
            (false, false) => group_sums::<3, false, false>(g, cols, c, &sel, a, n, &mut self.nulls),
            (false, true) => group_sums::<3, false, true>(g, cols, c, &sel, a, n, &mut self.nulls),
            (true, false) => group_sums::<3, true, false>(g, cols, c, &sel, a, n, &mut self.nulls),
            (true, true) => group_sums::<3, true, true>(g, cols, c, &sel, a, n, &mut self.nulls),
        }
        Ok(())
    }

    fn finish(self: Box<Self>) -> Output {
        let mut m = BTreeMap::new();
        for g in 0..NG {
            if self.count[g] > 0 {
                let key = (self.names[g / G].clone(), self.names[g % G].clone());
                m.insert(key, Q1Acc { sums: self.acc[g], nn: [self.count[g] - self.nulls[0][g], self.count[g] - self.nulls[1][g], self.count[g] - self.nulls[2][g]], count: self.count[g] });
            }
        }
        Output::Q1(m)
    }
}

pub struct Q6Sink {
    pub r: Ref,
    pub acc: Q6Acc,
}

impl WSink for Q6Sink {
    fn consume(&mut self, b: &Batch) -> R<()> {
        let sel = b.sel_for(b.opts.dense_sink);
        let (s, n) = sum(b.dec(&self.r), b.rows, &sel);
        self.acc.sum += s;
        self.acc.nn += n;
        Ok(())
    }

    fn finish(self: Box<Self>) -> Output {
        Output::Q6(self.acc)
    }
}

pub struct WPlan {
    pub name: &'static str,
    pub columns: &'static [&'static str],
    pub make: fn() -> (Vec<Box<dyn WOp>>, Box<dyn WSink>),
}

pub fn q1() -> WPlan {
    WPlan {
        name: "q1",
        columns: &["l_returnflag", "l_linestatus", "l_quantity", "l_extendedprice", "l_discount", "l_shipdate"],
        make: || {
            (
                vec![
                    Box::new(Filter(FilterCol::Date("l_shipdate", i32::MIN, Q1_CUTOFF))),
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

pub fn q6() -> WPlan {
    WPlan {
        name: "q6",
        columns: &["l_quantity", "l_extendedprice", "l_discount", "l_shipdate"],
        make: || {
            (
                vec![
                    Box::new(Filter(FilterCol::Date("l_shipdate", 8766, 9130))),
                    Box::new(Filter(FilterCol::Dec("l_discount", 5, 7))),
                    Box::new(Filter(FilterCol::Dec("l_quantity", i64::MIN, 2399))),
                    Box::new(Project(vec![Step::Mul { a: Ref::Col("l_extendedprice"), b: Ref::Col("l_discount") }])),
                ],
                Box::new(Q6Sink { r: Ref::Tmp(0), acc: Q6Acc::default() }),
            )
        },
    }
}

/// Run a plan with morsel-driven workers.
pub fn run(morsels: usize, visit: Visit, plan: &WPlan, threads: usize, opts: Opts) -> R<Output> {
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
                            let mut batch = Batch::new(rb, opts);
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

/// One thread over batches held in memory, timing each operator and the sink separately (nanoseconds, summed over
/// the batches). The stage timers add about 20 ns per batch per stage, nothing next to 8,192 rows.
pub fn run_staged(batches: &[RecordBatch], plan: &WPlan, opts: Opts) -> Vec<(String, f64)> {
    let (mut ops, mut sink) = (plan.make)();
    let mut t = vec![0.0f64; ops.len() + 2];
    for rb in batches {
        let t0 = std::time::Instant::now();
        let mut batch = Batch::new(rb, opts);
        let mut last = std::time::Instant::now();
        t[0] += last.duration_since(t0).as_secs_f64();
        for (k, op) in ops.iter_mut().enumerate() {
            op.exec(&mut batch).expect("op");
            let now = std::time::Instant::now();
            t[1 + k] += now.duration_since(last).as_secs_f64();
            last = now;
        }
        sink.consume(&batch).expect("sink");
        t[ops.len() + 1] += last.elapsed().as_secs_f64();
    }
    let mut names: Vec<String> = vec!["batch setup (validity words)".into()];
    names.extend((0..ops.len()).map(|k| format!("op {k}")));
    names.push("sink".into());
    names.into_iter().zip(t.into_iter().map(|x| x * 1e3)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wk::tests_support::Rng;
    use arrow::array::{DictionaryArray, Date32Array, Decimal64Array, Int32Array, StringArray};
    use std::sync::Arc;

    struct Rows {
        flag: Vec<usize>,
        status: Vec<usize>,
        qty: Vec<Option<i64>>,
        price: Vec<Option<i64>>,
        disc: Vec<Option<i64>>,
        date: Vec<Option<i32>>,
        /// What the value buffer holds under a NULL slot (Arrow leaves it undefined; real data keeps the original
        /// value). Non-zero, sometimes huge, so that a kernel that lets a NULL slot into a sum, a count or an
        /// overflow check is caught. The reference never looks at these.
        under: Vec<[i64; 4]>,
    }

    fn gen_rows(rng: &mut Rng, n: usize, null_pct: u64, big_pct: u64) -> Rows {
        let nullable = |r: &mut Rng, x: i64| if r.pct(null_pct) { None } else { Some(x) };
        let mut v = |r: &mut Rng, lo: i64, hi: i64| if r.pct(big_pct) { (r.next() >> 1) as i64 * if r.pct(50) { 1 } else { -1 } } else { lo + (r.next() % (hi - lo + 1) as u64) as i64 };
        let (mut qty, mut price, mut disc, mut date) = (vec![], vec![], vec![], vec![]);
        for _ in 0..n {
            let x = v(rng, 0, 5000);
            qty.push(nullable(rng, x));
            let x = v(rng, 0, 10_000_000);
            price.push(nullable(rng, x));
            let x = v(rng, 0, 10);
            disc.push(nullable(rng, x));
            let d = 8000 + (rng.next() % 3000) as i32;
            date.push(if rng.pct(null_pct) { None } else { Some(d) });
        }
        let under = (0..n).map(|_| std::array::from_fn(|_| if rng.pct(30) { (rng.next() >> 1) as i64 * if rng.pct(50) { 1 } else { -1 } } else { 1 + (rng.next() % 9999) as i64 })).collect();
        Rows { flag: (0..n).map(|_| (rng.next() % 3) as usize).collect(), status: (0..n).map(|_| (rng.next() % 2) as usize).collect(), qty, price, disc, date, under }
    }

    fn batch(r: &Rows) -> RecordBatch {
        use arrow::datatypes::{DataType, Field, Schema};
        let dict = |keys: &[usize], vals: &[&str]| -> ArrayRef {
            Arc::new(DictionaryArray::<Int32Type>::try_new(Int32Array::from(keys.iter().map(|&k| k as i32).collect::<Vec<_>>()), Arc::new(StringArray::from(vals.to_vec()))).expect("dict"))
        };
        use arrow::buffer::{NullBuffer, ScalarBuffer};
        let n = r.date.len();
        let nulls = |valid: Vec<bool>| if valid.iter().all(|&b| b) { None } else { Some(NullBuffer::from(valid)) };
        let dec = |v: &[Option<i64>], col: usize| -> ArrayRef {
            let values: Vec<i64> = (0..n).map(|i| v[i].unwrap_or(r.under[i][col])).collect();
            Arc::new(Decimal64Array::new(ScalarBuffer::from(values), nulls(v.iter().map(Option::is_some).collect())).with_precision_and_scale(15, 2).expect("precision"))
        };
        let date: ArrayRef = {
            let values: Vec<i32> = (0..n).map(|i| r.date[i].unwrap_or(r.under[i][3] as i32)).collect();
            Arc::new(Date32Array::new(ScalarBuffer::from(values), nulls(r.date.iter().map(Option::is_some).collect())))
        };
        let d = DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8));
        let schema = Schema::new(vec![
            Field::new("l_returnflag", d.clone(), false),
            Field::new("l_linestatus", d, false),
            Field::new("l_quantity", DataType::Decimal64(15, 2), true),
            Field::new("l_extendedprice", DataType::Decimal64(15, 2), true),
            Field::new("l_discount", DataType::Decimal64(15, 2), true),
            Field::new("l_shipdate", DataType::Date32, true),
        ]);
        RecordBatch::try_new(
            Arc::new(schema),
            vec![dict(&r.flag, &["A", "N", "R"]), dict(&r.status, &["F", "O"]), dec(&r.qty, 0), dec(&r.price, 1), dec(&r.disc, 2), date],
        )
        .expect("batch")
    }

    /// Row by row, Option semantics, i128; `None` = an error must be raised.
    fn ref_q6(rows: &[&Rows]) -> Option<Output> {
        let mut acc = Q6Acc::default();
        for r in rows {
            for i in 0..r.date.len() {
                let (Some(d), Some(dc), Some(q)) = (r.date[i], r.disc[i], r.qty[i]) else { continue };
                if (8766..=9130).contains(&d) && (5..=7).contains(&dc) && q < 2400 {
                    if let Some(p) = r.price[i] {
                        acc.sum += p.checked_mul(dc)? as i128;
                        acc.nn += 1;
                    }
                }
            }
        }
        Some(Output::Q6(acc))
    }

    fn ref_q1(rows: &[&Rows]) -> Option<Output> {
        let mut m: BTreeMap<(String, String), Q1Acc> = BTreeMap::new();
        for r in rows {
            for i in 0..r.date.len() {
                let Some(d) = r.date[i] else { continue };
                if d > Q1_CUTOFF {
                    continue;
                }
                let key = (["A", "N", "R"][r.flag[i]].to_string(), ["F", "O"][r.status[i]].to_string());
                let a = m.entry(key).or_default();
                a.count += 1;
                if let Some(q) = r.qty[i] {
                    a.sums[0] += q as i128;
                    a.nn[0] += 1;
                }
                if let Some(p) = r.price[i] {
                    a.sums[1] += p as i128;
                    a.nn[1] += 1;
                }
                // 100 - discount is checked whenever the discount is valid, whether or not the price is
                let t = match r.disc[i] {
                    Some(dc) => Some(100i64.checked_sub(dc)?),
                    None => None,
                };
                if let (Some(p), Some(t)) = (r.price[i], t) {
                    a.sums[2] += p.checked_mul(t)? as i128;
                    a.nn[2] += 1;
                }
            }
        }
        Some(Output::Q1(m))
    }

    #[test]
    fn the_options_decide_the_representation_and_what_is_known_valid() {
        let mut rng = Rng(0x5EED_5EED_1234_5678);
        let r = gen_rows(&mut rng, 200, 30, 0);
        let rb = batch(&r);
        // a bitmap selection is turned into indexes for a consumer that does not work on words, and only then
        let mut b = Batch::new(&rb, Opts::STEP3);
        b.sel = Sel::Mask(vec![0b1011, 0, 0, 0]);
        assert!(matches!(&*b.sel_for(true), Sel::Mask(_)));
        assert!(matches!(&*b.sel_for(false), Sel::Idx(i) if i == &[0, 1, 3]));
        b.sel = Sel::Idx(vec![2]);
        assert!(matches!(&*b.sel_for(true), Sel::Idx(_)) && matches!(&*b.sel_for(false), Sel::Idx(_)));
        // a filter marks its column valid only with the option on, and only that column
        for (opts, want) in [(Opts::STEP3, false), (Opts::STEP4, true)] {
            let mut b = Batch::new(&rb, opts);
            Filter(FilterCol::Dec("l_discount", 0, 10)).exec(&mut b).expect("filter");
            let k = b.idx("l_discount");
            assert_eq!(b.known_valid[k], want, "{opts:?}");
            assert_eq!(b.known_valid.iter().filter(|&&v| v).count(), want as usize, "only the filtered column: {opts:?}");
            // and the NULL rows of the filtered column are really gone from the selection
            let kept = b.sel.to_vec(b.rows);
            assert!(kept.iter().all(|&i| r.disc[i as usize].is_some_and(|d| (0..=10).contains(&d))));
        }
    }

    /// The shrunk case of the 2026-11-04 incident (a mutant that computed the first column's non-NULL count from the
    /// second column's NULL counter): one group, four selected rows plus one filtered out, the first column has two NULLs and the second one, so
    /// the two counters differ. Garbage (non-zero) values sit under the NULL slots, so every step has to leave them
    /// out of the sums and the counts.
    #[test]
    fn non_null_counts_are_per_column_not_shared_between_columns() {
        use arrow::buffer::{NullBuffer, ScalarBuffer};
        let dec = |v: [i64; 5], valid: [bool; 5]| -> ArrayRef {
            Arc::new(Decimal64Array::new(ScalarBuffer::from(v.to_vec()), Some(NullBuffer::from(valid.to_vec()))).with_precision_and_scale(15, 2).expect("p"))
        };
        let d = arrow::datatypes::DataType::Dictionary(Box::new(arrow::datatypes::DataType::Int32), Box::new(arrow::datatypes::DataType::Utf8));
        let key = |s: &str| -> ArrayRef { Arc::new(DictionaryArray::<Int32Type>::try_new(Int32Array::from(vec![0; 5]), Arc::new(StringArray::from(vec![s]))).expect("dict")) };
        let f = |n: &str, t: arrow::datatypes::DataType| arrow::datatypes::Field::new(n, t, true);
        let dec_t = arrow::datatypes::DataType::Decimal64(15, 2);
        let schema = arrow::datatypes::Schema::new(vec![
            f("l_returnflag", d.clone()),
            f("l_linestatus", d),
            f("l_quantity", dec_t.clone()),
            f("l_extendedprice", dec_t.clone()),
            f("l_discount", dec_t),
            f("l_shipdate", arrow::datatypes::DataType::Date32),
        ]);
        // quantity NULL in rows 0 and 3 (garbage 777, 999 underneath), price NULL in row 1 (garbage 888); row 4 has a
        // NULL ship date, so it is filtered out (this makes the selection a proper subset: an index vector in step 1,
        // a bitmap in the later steps) and its large values must stay out of every sum and count
        
        let rb = RecordBatch::try_new(
            Arc::new(schema),
            vec![
                key("A"),
                key("F"),
                dec([777, 20, 30, 999, 5000], [false, true, true, false, true]),
                dec([100, 888, 300, 400, 6000], [true, false, true, true, true]),
                dec([5, 5, 5, 5, 5], [true; 5]),
                Arc::new(Date32Array::from(vec![Some(8000), Some(8000), Some(8000), Some(8000), None])),
            ],
        )
        .expect("batch");
        let want = Q1Acc { sums: [50, 800, 800 * 95], nn: [2, 3, 3], count: 4 };
        for opts in [Opts::STEP1, Opts::STEP2, Opts::STEP3A, Opts::STEP3, Opts::STEP4, Opts::STEP5, Opts::STEP6] {
            let visit = |_m: usize, f: &mut dyn FnMut(&RecordBatch)| f(&rb);
            let Output::Q1(m) = run(1, &visit, &q1(), 1, opts).expect("run") else { panic!("Q1") };
            assert_eq!(m.get(&("A".to_string(), "F".to_string())), Some(&want), "{opts:?}");
        }
    }

    #[test]
    fn the_word_wise_engine_matches_a_scalar_reference_for_every_step_density_and_overflow() {
        let mut rng = Rng(0xC0FF_EE12_3456_789B);
        let steps = [Opts::STEP1, Opts::STEP2, Opts::STEP3A, Opts::STEP3, Opts::STEP4, Opts::STEP5, Opts::STEP6];
        let mut errors = 0;
        const DENSITIES: [u64; 11] = [0, 1, 3, 5, 10, 25, 50, 75, 90, 95, 100];
        for round in 0..132 {
            let null_pct = DENSITIES[round % 11];
            let big_pct = [0, 0, 0, 3][(round / 11) % 4];
            // several batches per run, of awkward sizes, so that morsels, partial words and thread merging are exercised
            let rows: Vec<Rows> = [1usize, 63, 64, 65, 200, 1000].iter().map(|&n| gen_rows(&mut rng, n, null_pct, big_pct)).collect();
            let batches: Vec<RecordBatch> = rows.iter().map(batch).collect();
            let refs: Vec<&Rows> = rows.iter().collect();
            let visit = |m: usize, f: &mut dyn FnMut(&RecordBatch)| f(&batches[m]);
            for (name, plan, want) in [("q1", q1(), ref_q1(&refs)), ("q6", q6(), ref_q6(&refs))] {
                for opts in steps {
                    for threads in [1usize, 3] {
                        let got = run(batches.len(), &visit, &plan, threads, opts);
                        match &want {
                            None => {
                                errors += 1;
                                assert!(got.is_err(), "round {round} {name} {opts:?}: the reference overflows, the engine did not");
                            }
                            Some(w) => assert_eq!(got.as_ref().ok(), Some(w), "round {round} {name} nulls {null_pct}% {opts:?} threads {threads}: {got:?}"),
                        }
                    }
                }
            }
        }
        assert!(errors > 0, "the test must include cases where an error is expected");
    }
}
