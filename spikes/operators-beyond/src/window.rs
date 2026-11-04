//! Window functions on the protocol.
//!
//! FINDING: an operator cannot emit anything after its input ends (the protocol has no "input finished" call for operators), and a
//! window function must hold the last partition until it knows it is over. So the window is NOT an operator here: it is a SOURCE that
//! wraps the sorted stream of a `SortSink` (sorted by partition keys, then order keys) and computes the functions while it pulls from the
//! merge, holding back only the last, possibly unfinished partition. The cost: the window is fused with its sort, runs as one task, and
//! cannot take an arbitrary upstream.
//!
//! Supported: row_number, rank, dense_rank, sum(col) OVER (PARTITION BY ...) (the whole partition, no frame), lag(col, 1).
//! A partition is held in memory whole (a single partition larger than the budget is NOT handled).
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, AsArray, Decimal128Array, Int64Array, RecordBatch, UInt32Array};
use arrow::compute::{cast, concat_batches, take};
use arrow::datatypes::{DataType, Decimal128Type, Field, Schema, SchemaRef};
use arrow::row::{RowConverter, SortField};

use crate::batch::Batch;
use crate::proto::api::*;
use crate::sort::{SortOut, SortSpec};
use crate::spill::ioerr;

#[derive(Clone, Copy)]
pub enum WinFn {
    RowNumber,
    Rank,
    DenseRank,
    /// sum(col) over the whole partition
    SumPartition(usize),
    /// the previous row's value of col in the partition (NULL for the first row)
    Lag(usize),
}

pub struct Window {
    pub partition: Vec<usize>,
    /// the ORDER BY columns of the window (peers = equal on all of them), a prefix of the sort that feeds it after `partition`
    pub order: Vec<usize>,
    pub fns: Vec<(WinFn, &'static str)>,
}

impl Window {
    pub fn out_schema(&self, input: &SchemaRef) -> SchemaRef {
        let mut f: Vec<Field> = input.fields().iter().map(|f| f.as_ref().clone()).collect();
        for (w, n) in &self.fns {
            f.push(match w {
                WinFn::RowNumber | WinFn::Rank | WinFn::DenseRank => Field::new(*n, DataType::Int64, false),
                WinFn::SumPartition(c) => Field::new(*n, DataType::Decimal128(38, scale(input.field(*c).data_type())), true),
                WinFn::Lag(c) => Field::new(*n, input.field(*c).data_type().clone(), true),
            });
        }
        Arc::new(Schema::new(f))
    }

    /// The sort a window needs: partition columns ascending, then the given order keys.
    pub fn sort_spec(&self, order_keys: &SortSpec) -> SortSpec {
        let mut keys = SortSpec::asc(&self.partition).keys;
        for k in &order_keys.keys {
            keys.push(k.clone());
        }
        SortSpec { keys }
    }

    fn compute(&self, rb: &RecordBatch, out: &SchemaRef) -> Result<RecordBatch> {
        let n = rb.num_rows();
        let mk = |cols: &[usize]| -> Result<Option<arrow::row::Rows>> {
            if cols.is_empty() {
                return Ok(None);
            }
            let conv = RowConverter::new(cols.iter().map(|&c| SortField::new(rb.column(c).data_type().clone())).collect()).map_err(ioerr("window keys"))?;
            let arrs: Vec<ArrayRef> = cols.iter().map(|&c| rb.column(c).clone()).collect();
            Ok(Some(conv.convert_columns(&arrs).map_err(ioerr("window keys"))?))
        };
        let (prows, orows) = (mk(&self.partition)?, mk(&self.order)?);
        let same_part = |i: usize, j: usize| prows.as_ref().is_none_or(|r| r.row(i) == r.row(j));
        let same_peer = |i: usize, j: usize| orows.as_ref().is_some_and(|r| r.row(i) == r.row(j));
        let (mut rn, mut rk, mut dr) = (vec![0i64; n], vec![0i64; n], vec![0i64; n]);
        let mut group_start = vec![0usize; n];
        let mut lag_idx: Vec<Option<u32>> = vec![None; n];
        for i in 0..n {
            if i == 0 || !same_part(i, i - 1) {
                (rn[i], rk[i], dr[i], group_start[i]) = (1, 1, 1, i);
            } else {
                group_start[i] = group_start[i - 1];
                rn[i] = rn[i - 1] + 1;
                lag_idx[i] = Some(i as u32 - 1);
                if same_peer(i, i - 1) {
                    (rk[i], dr[i]) = (rk[i - 1], dr[i - 1]);
                } else {
                    (rk[i], dr[i]) = (rn[i], dr[i - 1] + 1);
                }
            }
        }
        let mut cols: Vec<ArrayRef> = rb.columns().to_vec();
        for (w, _) in &self.fns {
            cols.push(match w {
                WinFn::RowNumber => Arc::new(Int64Array::from(rn.clone())),
                WinFn::Rank => Arc::new(Int64Array::from(rk.clone())),
                WinFn::DenseRank => Arc::new(Int64Array::from(dr.clone())),
                WinFn::Lag(c) => take(rb.column(*c).as_ref(), &UInt32Array::from(lag_idx.clone()), None).map_err(ioerr("lag"))?,
                WinFn::SumPartition(c) => {
                    let s = scale(rb.column(*c).data_type());
                    let v = cast(rb.column(*c), &DataType::Decimal128(38, s)).map_err(ioerr("sum cast"))?;
                    let v = v.as_primitive::<Decimal128Type>();
                    let mut tot: Vec<Option<i128>> = vec![None; n];
                    let mut i = 0;
                    while i < n {
                        let mut j = i;
                        let mut acc: Option<i128> = None;
                        while j < n && group_start[j] == i {
                            if v.is_valid(j) {
                                acc = Some(acc.unwrap_or(0).checked_add(v.value(j)).ok_or_else(|| QueryError("window SUM overflows".into()))?);
                            }
                            j += 1;
                        }
                        tot[i..j].iter_mut().for_each(|t| *t = acc);
                        i = j;
                    }
                    let a: Decimal128Array = tot.into_iter().collect();
                    Arc::new(a.with_precision_and_scale(38, s).map_err(ioerr("sum type"))?)
                }
            });
        }
        RecordBatch::try_new(out.clone(), cols).map_err(ioerr("window output"))
    }
}

fn scale(t: &DataType) -> i8 {
    match t {
        DataType::Decimal128(_, s) | DataType::Decimal256(_, s) | DataType::Decimal64(_, s) => *s,
        _ => 0,
    }
}

/// A source: the sorted stream of `sorted` with the window functions applied. Run its pipeline with ONE task.
pub struct WindowSource {
    pub win: Arc<Window>,
    pub sorted: Arc<SortOut>,
    pub in_schema: SchemaRef,
}

struct WinSrc {
    sh: Arc<WindowSource>,
    out: SchemaRef,
    held: Vec<RecordBatch>,
    done: bool,
}

impl SourceFactory for Arc<WindowSource> {
    fn make(&self) -> Box<dyn Source> {
        Box::new(WinSrc { out: self.win.out_schema(&self.in_schema), sh: Arc::clone(self), held: vec![], done: false })
    }
}

impl WinSrc {
    /// Rows of `rb` from the start of the last partition on (the part that may continue in the next batch).
    fn last_partition_start(&self, rb: &RecordBatch) -> Result<usize> {
        let n = rb.num_rows();
        if self.sh.win.partition.is_empty() {
            return Ok(0);
        }
        let conv = RowConverter::new(self.sh.win.partition.iter().map(|&c| SortField::new(rb.column(c).data_type().clone())).collect()).map_err(ioerr("window keys"))?;
        let arrs: Vec<ArrayRef> = self.sh.win.partition.iter().map(|&c| rb.column(c).clone()).collect();
        let rows = conv.convert_columns(&arrs).map_err(ioerr("window keys"))?;
        let mut s = n - 1;
        while s > 0 && rows.row(s - 1) == rows.row(n - 1) {
            s -= 1;
        }
        Ok(s)
    }
}

impl Source for WinSrc {
    fn next(&mut self, _w: &Waker) -> Result<SourceResult> {
        loop {
            if self.done {
                return Ok(SourceResult::Finished);
            }
            match self.sh.sorted.next_merged()? {
                Some(b) => {
                    // hold = unfinished partition from before + this batch; emit everything before its last partition
                    self.held.push(b);
                    let all = concat_batches(&self.held[0].schema(), &self.held).map_err(ioerr("window concat"))?;
                    let s = self.last_partition_start(&all)?;
                    self.held = vec![all.slice(s, all.num_rows() - s)];
                    if s > 0 {
                        let done = all.slice(0, s);
                        return Ok(SourceResult::Batch(Batch::new(self.sh.win.compute(&done, &self.out)?)));
                    }
                }
                None => {
                    self.done = true;
                    if let Some(h) = self.held.pop() {
                        if h.num_rows() > 0 {
                            return Ok(SourceResult::Batch(Batch::new(self.sh.win.compute(&h, &self.out)?)));
                        }
                    }
                }
            }
        }
    }
    fn finish(&mut self) {
        self.sh.sorted.close();
    }
}
