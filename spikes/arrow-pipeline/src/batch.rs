//! The protocol's `Batch`, now carrying an Arrow `RecordBatch` and an optional selection vector (ADR 0003 decision 1).
//!
//! The protocol hands an operator `&Batch` (input) and `&mut Batch` (output). So an operator cannot narrow the
//! selection in place (the generic-ops `Op::exec(&mut Batch)` could): it builds a NEW `Batch` that shares the
//! columns by `Arc` (no data copy) and carries its own selection and computed columns. Everything a stage may need
//! to share is behind an `Arc`, so building the output batch costs a few reference-count updates and one small
//! allocation for a new selection vector.
use std::sync::{Arc, OnceLock};

use arrow::array::{Array, ArrayRef, AsArray, RecordBatch};
use arrow::buffer::NullBuffer;
use arrow::datatypes::{Date32Type, Decimal64Type, SchemaRef};

pub use crate::genops::engine::Ref;
use crate::genops::kernels::{Col, Out, Valid};

/// The immutable part of a batch, shared between the batches of consecutive stages.
pub struct Data {
    schema: SchemaRef,
    rows: usize,
    cols: Vec<ArrayRef>,
    /// NULL bitmaps of the columns that have any NULL. Dense selections read them as one byte per row, expanded
    /// lazily ONCE per batch (shared by every stage, hence the `OnceLock`); sparse selections test the bits.
    nulls: Vec<Option<NullBuffer>>,
    bytes: Vec<OnceLock<Vec<u8>>>,
}

#[derive(Clone, Default)]
pub struct Batch {
    data: Option<Arc<Data>>,
    /// Row indexes still alive (`None` = all of them). Meaningful with `data`.
    sel: Option<Arc<Vec<u32>>>,
    /// Columns computed by `Project`, full length, meaningful only at the selected rows.
    tmp: Vec<Arc<Out>>,
}

impl Batch {
    pub fn new(rb: &RecordBatch) -> Self {
        let nulls: Vec<Option<NullBuffer>> = rb.columns().iter().map(|c| c.nulls().filter(|n| n.null_count() > 0).cloned()).collect();
        let bytes = (0..nulls.len()).map(|_| OnceLock::new()).collect();
        let data = Data { schema: rb.schema(), rows: rb.num_rows(), cols: rb.columns().to_vec(), nulls, bytes };
        Batch { data: Some(Arc::new(data)), sel: None, tmp: Vec::new() }
    }

    fn d(&self) -> &Data {
        self.data.as_deref().expect("a batch with data")
    }

    /// Physical rows (the length of every column).
    pub fn rows(&self) -> usize {
        self.data.as_ref().map_or(0, |d| d.rows)
    }

    /// Rows still alive after the selection. The protocol's `is_empty` is about THESE: a batch whose selection
    /// removed every row is not forwarded.
    pub fn len(&self) -> usize {
        match &self.sel {
            Some(s) => s.len(),
            None => self.rows(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn sel(&self) -> Option<&[u32]> {
        self.sel.as_deref().map(Vec::as_slice)
    }

    /// Same columns and computed columns, a different selection. No column data is copied.
    pub fn with_sel(&self, sel: Vec<u32>) -> Batch {
        Batch { data: self.data.clone(), sel: Some(Arc::new(sel)), tmp: self.tmp.clone() }
    }

    /// Same columns and selection, one more computed column.
    pub fn with_tmp(&self, extra: Vec<Out>) -> Batch {
        let mut tmp = self.tmp.clone();
        tmp.extend(extra.into_iter().map(Arc::new));
        Batch { data: self.data.clone(), sel: self.sel.clone(), tmp }
    }

    pub fn idx(&self, name: &str) -> usize {
        self.d().schema.index_of(name).expect("column present in the batch")
    }

    pub fn column(&self, name: &str) -> &ArrayRef {
        &self.d().cols[self.idx(name)]
    }

    pub fn tmp_col(&self, k: usize) -> &Out {
        &self.tmp[k]
    }

    pub fn tmp_count(&self) -> usize {
        self.tmp.len()
    }

    fn valid(&self, k: usize) -> Valid<'_> {
        let d = self.d();
        match &d.nulls[k] {
            None => Valid::All,
            Some(n) if self.sel.as_ref().is_none_or(|s| s.len() * 4 >= d.rows) => {
                Valid::Bytes(d.bytes[k].get_or_init(|| n.iter().map(|v| v as u8).collect()))
            }
            Some(n) => Valid::Bits(n),
        }
    }

    pub fn dec(&self, r: &Ref) -> Col<'_, i64> {
        match r {
            Ref::Col(n) => {
                let k = self.idx(n);
                let a = self.d().cols[k].as_primitive::<Decimal64Type>();
                (&a.values()[..], self.valid(k))
            }
            Ref::Tmp(k) => self.tmp[*k].col(),
        }
    }

    pub fn date(&self, name: &str) -> Col<'_, i32> {
        let k = self.idx(name);
        let a = self.d().cols[k].as_primitive::<Date32Type>();
        (&a.values()[..], self.valid(k))
    }

    /// Apply the selection: a new batch with only the live rows, columns gathered (this IS a copy). Required before
    /// a batch enters an exchange (ADR 0001 / 0003 decision 1). Computed columns are gathered the same way.
    pub fn compact(&self) -> Result<Batch, arrow::error::ArrowError> {
        let Some(sel) = self.sel() else { return Ok(self.clone()) };
        let d = self.d();
        let idx = arrow::array::UInt32Array::from(sel.to_vec());
        let cols = d.cols.iter().map(|c| arrow::compute::take(c.as_ref(), &idx, None)).collect::<Result<Vec<_>, _>>()?;
        let rb = RecordBatch::try_new(Arc::clone(&d.schema), cols)?;
        let mut out = Batch::new(&rb);
        out.tmp = self
            .tmp
            .iter()
            .map(|t| {
                let v = sel.iter().map(|&i| t.v[i as usize]).collect();
                let valid = t.valid.as_ref().map(|m| sel.iter().map(|&i| m[i as usize]).collect());
                Arc::new(Out { v, valid })
            })
            .collect();
        Ok(out)
    }

    /// The live rows' indexes, in order.
    pub fn live(&self) -> Vec<usize> {
        match self.sel() {
            Some(s) => s.iter().map(|&i| i as usize).collect(),
            None => (0..self.rows()).collect(),
        }
    }
}
