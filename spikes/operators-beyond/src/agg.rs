//! Hash aggregation as a sink (per-task tables merged at `combine`, result published by `finalize`). Group keys are compared as Arrow
//! row-format bytes (NULLs group together). Sums are exact (`i128`, overflow is an error). NOT spillable: its memory is the number of
//! groups; the plan for spilling aggregates is the partitioned one used by the join.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use arrow::array::{Array, ArrayRef, AsArray, Decimal128Array, Int64Array, RecordBatch};
use arrow::compute::cast;
use arrow::datatypes::{DataType, Decimal128Type, Field, Schema, SchemaRef};
use arrow::row::{RowConverter, SortField};

use crate::batch::Batch;
use crate::ops::SharedQueue;
use crate::proto::api::*;
use crate::proto::sched::lock;
use crate::spill::ioerr;

#[derive(Clone, Copy)]
pub enum AggFn {
    CountStar,
    /// COUNT(col): non-NULL values.
    Count(usize),
    /// SUM(col) of a decimal or integer column; NULL if every value is NULL.
    Sum(usize),
}

pub struct HashAgg {
    group: Vec<usize>,
    aggs: Vec<AggFn>,
    in_schema: SchemaRef,
    out_schema: SchemaRef,
    sum_scale: Vec<i8>,
    state: Mutex<Table>,
    pub out: Arc<SharedQueue>,
}

#[derive(Default)]
struct Table {
    index: HashMap<Vec<u8>, usize>,
    keys: Vec<Vec<u8>>,
    sums: Vec<Vec<i128>>,  // per aggregate, per group
    valid: Vec<Vec<bool>>, // per aggregate, per group (a SUM is NULL until a value arrives)
}

impl Table {
    fn group_of(&mut self, key: &[u8], naggs: usize) -> usize {
        if let Some(&g) = self.index.get(key) {
            return g;
        }
        let g = self.keys.len();
        self.keys.push(key.to_vec());
        self.index.insert(key.to_vec(), g);
        for k in 0..naggs {
            if self.sums.len() <= k {
                self.sums.push(vec![]);
                self.valid.push(vec![]);
            }
            self.sums[k].push(0);
            self.valid[k].push(false);
        }
        g
    }
}

impl HashAgg {
    pub fn new(in_schema: &SchemaRef, group: &[usize], aggs: &[AggFn], names: &[&str]) -> Arc<Self> {
        let mut fields: Vec<Field> = group.iter().map(|&c| in_schema.field(c).clone()).collect();
        let mut scale = vec![];
        for (a, n) in aggs.iter().zip(names) {
            match a {
                AggFn::CountStar | AggFn::Count(_) => {
                    fields.push(Field::new(*n, DataType::Int64, false));
                    scale.push(0);
                }
                AggFn::Sum(c) => {
                    let s = match in_schema.field(*c).data_type() {
                        DataType::Decimal128(_, s) | DataType::Decimal256(_, s) => *s,
                        DataType::Decimal64(_, s) => *s,
                        _ => 0,
                    };
                    fields.push(Field::new(*n, DataType::Decimal128(38, s), true));
                    scale.push(s);
                }
            }
        }
        Arc::new(HashAgg {
            group: group.to_vec(),
            aggs: aggs.to_vec(),
            in_schema: in_schema.clone(),
            out_schema: Arc::new(Schema::new(fields)),
            sum_scale: scale,
            state: Mutex::new(Table::default()),
            out: SharedQueue::new(),
        })
    }

    pub fn out_schema(&self) -> SchemaRef {
        self.out_schema.clone()
    }

    fn converter(&self) -> Result<RowConverter> {
        RowConverter::new(self.group.iter().map(|&c| SortField::new(self.in_schema.field(c).data_type().clone())).collect()).map_err(ioerr("agg keys"))
    }
}

struct AggLocal {
    sh: Arc<HashAgg>,
    conv: RowConverter,
    t: Table,
}

impl GlobalSink for Arc<HashAgg> {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        Box::new(AggLocal { sh: Arc::clone(self), conv: self.converter().expect("converter"), t: Table::default() })
    }

    fn finalize(&self) -> Result<()> {
        let t = std::mem::take(&mut *lock(&self.state));
        let conv = self.converter()?;
        let parser = conv.parser();
        let n = t.keys.len();
        let mut cols: Vec<ArrayRef> = if self.group.is_empty() {
            vec![]
        } else {
            let rows = t.keys.iter().map(|k| parser.parse(k));
            conv.convert_rows(rows).map_err(ioerr("agg keys back"))?
        };
        for (k, a) in self.aggs.iter().enumerate() {
            match a {
                AggFn::CountStar | AggFn::Count(_) => {
                    cols.push(Arc::new(Int64Array::from((0..n).map(|g| t.sums[k][g] as i64).collect::<Vec<_>>())));
                }
                AggFn::Sum(_) => {
                    let v: Decimal128Array = (0..n).map(|g| t.valid[k][g].then_some(t.sums[k][g])).collect();
                    cols.push(Arc::new(v.with_precision_and_scale(38, self.sum_scale[k]).map_err(ioerr("sum type"))?));
                }
            }
        }
        // a global aggregate (no GROUP BY) over no rows still returns one row
        if n == 0 && self.group.is_empty() {
            return err("global aggregate over an empty input is not implemented here");
        }
        if n > 0 {
            self.out.fill([RecordBatch::try_new(self.out_schema.clone(), cols).map_err(ioerr("agg output"))?]);
        }
        Ok(())
    }
}

fn add(a: i128, b: i128) -> Result<i128> {
    a.checked_add(b).ok_or_else(|| QueryError("SUM overflows 128 bits".into()))
}

impl LocalSink for AggLocal {
    fn push(&mut self, input: &mut Batch, _w: &Waker) -> Result<SinkResult> {
        if input.is_empty() {
            return Ok(SinkResult::NeedMoreInput);
        }
        let b = input.rb();
        let keycols: Vec<ArrayRef> = self.sh.group.iter().map(|&c| b.column(c).clone()).collect();
        let rows = self.conv.convert_columns(&keycols).map_err(ioerr("agg keys"))?;
        let naggs = self.sh.aggs.len();
        let gids: Vec<usize> = (0..b.num_rows()).map(|i| self.t.group_of(rows.row(i).as_ref(), naggs)).collect();
        for (k, a) in self.sh.aggs.iter().enumerate() {
            match a {
                AggFn::CountStar => {
                    for &g in &gids {
                        self.t.sums[k][g] += 1;
                    }
                }
                AggFn::Count(c) => {
                    let col = b.column(*c);
                    for (i, &g) in gids.iter().enumerate() {
                        self.t.sums[k][g] += col.is_valid(i) as i128;
                    }
                }
                AggFn::Sum(c) => {
                    let col = cast(b.column(*c), &DataType::Decimal128(38, self.sh.sum_scale[k])).map_err(ioerr("sum cast"))?;
                    let col = col.as_primitive::<Decimal128Type>();
                    for (i, &g) in gids.iter().enumerate() {
                        if col.is_valid(i) {
                            self.t.sums[k][g] = add(self.t.sums[k][g], col.value(i))?;
                            self.t.valid[k][g] = true;
                        }
                    }
                }
            }
        }
        Ok(SinkResult::NeedMoreInput)
    }

    fn combine(self: Box<Self>) -> Result<()> {
        let mut g = lock(&self.sh.state);
        let naggs = self.sh.aggs.len();
        for (src, key) in self.t.keys.iter().enumerate() {
            let dst = g.group_of(key, naggs);
            for k in 0..naggs {
                g.sums[k][dst] = add(g.sums[k][dst], self.t.sums[k][src])?;
                g.valid[k][dst] |= self.t.valid[k][src];
            }
        }
        Ok(())
    }
}
