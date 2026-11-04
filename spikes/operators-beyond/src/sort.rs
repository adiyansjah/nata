//! ORDER BY and top-N on the protocol, with spill.
//!
//! ORDER BY is a SINK (it must see all input before it can emit) followed by a SOURCE in a dependent pipeline that streams the merged
//! result. Each task of the sink sorts runs in memory and, when the shared memory budget is exhausted, writes them to spill files.
//! The source merges the runs (k-way, in passes when there are more runs than `fanin`, so at most `fanin + 1` files are open at once).
//! Sort keys are compared as Arrow row-format bytes, which gives ascending/descending and NULLS FIRST/LAST for every type.
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use arrow::array::{Array, ArrayRef, RecordBatch, UInt32Array};
use arrow::compute::{concat_batches, interleave, take_record_batch};
use arrow::datatypes::SchemaRef;
use arrow::row::{RowConverter, Rows, SortField};
use arrow::compute::SortOptions;

use crate::batch::Batch;
use crate::ops::SharedQueue;
use crate::proto::api::*;
use crate::proto::sched::lock;
use crate::spill::{Budget, RunReader, RunWriter, batch_bytes, ioerr, remove};

pub const OUT_ROWS: usize = 8192;

#[derive(Clone)]
pub struct SortKey {
    pub col: usize,
    pub desc: bool,
    pub nulls_first: bool,
}

#[derive(Clone)]
pub struct SortSpec {
    pub keys: Vec<SortKey>,
}

impl SortSpec {
    pub fn asc(cols: &[usize]) -> Self {
        SortSpec { keys: cols.iter().map(|&c| SortKey { col: c, desc: false, nulls_first: false }).collect() }
    }

    fn converter(&self, schema: &SchemaRef) -> Result<RowConverter> {
        let fields = self
            .keys
            .iter()
            .map(|k| {
                SortField::new_with_options(
                    schema.field(k.col).data_type().clone(),
                    SortOptions { descending: k.desc, nulls_first: k.nulls_first },
                )
            })
            .collect();
        RowConverter::new(fields).map_err(ioerr("row converter"))
    }

    fn rows(&self, conv: &RowConverter, rb: &RecordBatch) -> Result<Rows> {
        let cols: Vec<ArrayRef> = self.keys.iter().map(|k| rb.column(k.col).clone()).collect();
        conv.convert_columns(&cols).map_err(ioerr("row keys"))
    }
}

/// Sort `batches` (all of one schema) into one batch, stable with respect to their order.
pub fn sort_batches(spec: &SortSpec, schema: &SchemaRef, batches: &[RecordBatch]) -> Result<RecordBatch> {
    let all = concat_batches(schema, batches).map_err(ioerr("concat"))?;
    let conv = spec.converter(schema)?;
    let rows = spec.rows(&conv, &all)?;
    let mut idx: Vec<u32> = (0..all.num_rows() as u32).collect();
    idx.sort_by(|&a, &b| rows.row(a as usize).cmp(&rows.row(b as usize)));
    take_record_batch(&all, &UInt32Array::from(idx)).map_err(ioerr("take"))
}

/// Like `sort_batches` but the result is cut into chunks of at most `OUT_ROWS` rows that each OWN their buffers (a slice of one big batch
/// reports the size of the whole buffer, which would wreck the memory accounting).
pub fn sort_chunks(spec: &SortSpec, schema: &SchemaRef, batches: &[RecordBatch]) -> Result<Vec<RecordBatch>> {
    let all = concat_batches(schema, batches).map_err(ioerr("concat"))?;
    let conv = spec.converter(schema)?;
    let rows = spec.rows(&conv, &all)?;
    let mut idx: Vec<u32> = (0..all.num_rows() as u32).collect();
    idx.sort_by(|&a, &b| rows.row(a as usize).cmp(&rows.row(b as usize)));
    idx.chunks(OUT_ROWS).map(|c| take_record_batch(&all, &UInt32Array::from(c.to_vec())).map_err(ioerr("take"))).collect()
}

enum Run {
    Mem { batches: Vec<RecordBatch>, bytes: usize },
    File(PathBuf),
}

pub struct SortShared {
    spec: SortSpec,
    budget: Arc<Budget>,
    schema: Mutex<Option<SchemaRef>>,
    runs: Mutex<Vec<Run>>,
    fanin: usize,
    out: Arc<SortOut>,
}

/// The result side: filled by `finalize`, drained by `source()`.
pub struct SortOut {
    merger: Mutex<Option<Merger>>,
}

pub struct SortSink(pub Arc<SortShared>);

impl SortSink {
    pub fn new(spec: SortSpec, budget: &Arc<Budget>, fanin: usize) -> Self {
        assert!(fanin >= 2);
        SortSink(Arc::new(SortShared {
            spec,
            budget: Arc::clone(budget),
            schema: Mutex::new(None),
            runs: Mutex::new(Vec::new()),
            fanin,
            out: Arc::new(SortOut { merger: Mutex::new(None) }),
        }))
    }

    /// The merged result, as a source for a pipeline that depends on the pipeline of this sink.
    pub fn source(&self) -> Arc<SortOut> {
        Arc::clone(&self.0.out)
    }
}

struct SortLocal {
    sh: Arc<SortShared>,
    buf: Vec<RecordBatch>,
    bytes: usize,
    schema: Option<SchemaRef>,
}

impl SortLocal {
    fn spill_batches(&self, sorted: &[RecordBatch]) -> Result<PathBuf> {
        let mut w = RunWriter::create(&self.sh.budget, "sort", self.schema.as_ref().expect("schema"))?;
        for c in sorted {
            w.write(c)?;
        }
        w.finish()
    }

    fn spill_buffer(&mut self) -> Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let sorted = sort_chunks(&self.sh.spec, self.schema.as_ref().expect("schema"), &self.buf)?;
        let p = self.spill_batches(&sorted)?;
        lock(&self.sh.runs).push(Run::File(p));
        self.sh.budget.release(self.bytes);
        self.buf.clear();
        self.bytes = 0;
        Ok(())
    }
}

/// A task that ends without `combine` (cancel, error) gives its reservation back.
impl Drop for SortLocal {
    fn drop(&mut self) {
        if self.bytes > 0 {
            self.sh.budget.release(self.bytes);
        }
    }
}

impl GlobalSink for SortSink {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        Box::new(SortLocal { sh: Arc::clone(&self.0), buf: Vec::new(), bytes: 0, schema: None })
    }

    fn finalize(&self) -> Result<()> {
        let sh = &self.0;
        let runs = std::mem::take(&mut *lock(&sh.runs));
        let schema = lock(&sh.schema).clone();
        let Some(schema) = schema else { return Ok(()) }; // no input at all: empty result
        let m = Merger::new(runs, &sh.spec, &schema, &sh.budget, sh.fanin)?;
        *lock(&sh.out.merger) = Some(m);
        Ok(())
    }
}

impl LocalSink for SortLocal {
    fn push(&mut self, input: &mut Batch, _w: &Waker) -> Result<SinkResult> {
        if input.is_empty() {
            return Ok(SinkResult::NeedMoreInput);
        }
        let b = input.rb().clone();
        if self.schema.is_none() {
            self.schema = Some(b.schema());
            lock(&self.sh.schema).get_or_insert(b.schema());
        }
        let n = batch_bytes(&b);
        if !self.sh.budget.try_reserve(n) {
            self.spill_buffer()?;
            if !self.sh.budget.try_reserve(n) {
                // one batch alone does not fit: sort it and write it straight out
                let sorted = sort_chunks(&self.sh.spec, self.schema.as_ref().unwrap(), &[b])?;
                let p = self.spill_batches(&sorted)?;
                lock(&self.sh.runs).push(Run::File(p));
                return Ok(SinkResult::NeedMoreInput);
            }
        }
        self.bytes += n;
        self.buf.push(b);
        Ok(SinkResult::NeedMoreInput)
    }

    fn combine(mut self: Box<Self>) -> Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let cs = sort_chunks(&self.sh.spec, self.schema.as_ref().expect("schema"), &self.buf)?;
        let nb: usize = cs.iter().map(batch_bytes).sum();
        self.sh.budget.release(self.bytes);
        self.bytes = 0;
        self.buf.clear();
        if self.sh.budget.try_reserve(nb) {
            lock(&self.sh.runs).push(Run::Mem { batches: cs, bytes: nb });
        } else {
            let p = self.spill_batches(&cs)?;
            lock(&self.sh.runs).push(Run::File(p));
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------------ merging

enum RunSrc {
    Mem(VecDeque<RecordBatch>),
    File(RunReader, PathBuf),
}

struct Head {
    src: RunSrc,
    cur: Option<(RecordBatch, Rows, usize)>,
    block_id: Option<usize>,
    mem_bytes: usize,
}

pub struct Merger {
    spec: SortSpec,
    schema: SchemaRef,
    conv: RowConverter,
    heads: Vec<Head>,
    budget: Arc<Budget>,
}

impl Merger {
    fn new(mut runs: Vec<Run>, spec: &SortSpec, schema: &SchemaRef, budget: &Arc<Budget>, fanin: usize) -> Result<Self> {
        // Preliminary passes: merge `fanin` runs into one file until at most `fanin` remain.
        while runs.len() > fanin {
            let group: Vec<Run> = runs.drain(..fanin).collect();
            let mut m = Merger::open(group, spec, schema, budget)?;
            let mut w = RunWriter::create(budget, "merge", schema)?;
            while let Some(b) = m.next_batch(OUT_ROWS)? {
                w.write(&b)?;
            }
            drop(m);
            runs.push(Run::File(w.finish()?));
        }
        Merger::open(runs, spec, schema, budget)
    }

    fn open(runs: Vec<Run>, spec: &SortSpec, schema: &SchemaRef, budget: &Arc<Budget>) -> Result<Self> {
        let conv = spec.converter(schema)?;
        let mut heads = Vec::new();
        for r in runs {
            let (src, mem_bytes) = match r {
                Run::Mem { batches, bytes } => (RunSrc::Mem(batches.into()), bytes),
                Run::File(p) => (RunSrc::File(RunReader::open(budget, &p)?, p), 0),
            };
            heads.push(Head { src, cur: None, block_id: None, mem_bytes });
        }
        let mut m = Merger { spec: spec.clone(), schema: schema.clone(), conv, heads, budget: Arc::clone(budget) };
        for i in 0..m.heads.len() {
            m.refill(i)?;
        }
        Ok(m)
    }

    fn refill(&mut self, i: usize) -> Result<()> {
        loop {
            let next = match &mut self.heads[i].src {
                RunSrc::Mem(q) => q.pop_front(),
                RunSrc::File(r, _) => r.next_batch()?,
            };
            match next {
                None => {
                    self.heads[i].cur = None;
                    return Ok(());
                }
                Some(b) if b.num_rows() == 0 => continue,
                Some(b) => {
                    let rows = self.spec.rows(&self.conv, &b)?;
                    self.heads[i].cur = Some((b, rows, 0));
                    return Ok(());
                }
            }
        }
    }

    /// The next up-to-`max` rows of the merged order, or None at the end.
    pub fn next_batch(&mut self, max: usize) -> Result<Option<RecordBatch>> {
        let mut blocks: Vec<RecordBatch> = Vec::new();
        let mut picks: Vec<(usize, usize)> = Vec::new();
        for h in &mut self.heads {
            h.block_id = None;
        }
        while picks.len() < max {
            let mut best: Option<usize> = None;
            for i in 0..self.heads.len() {
                let Some((_, rows, pos)) = &self.heads[i].cur else { continue };
                best = match best {
                    None => Some(i),
                    Some(j) => {
                        let (_, rj, pj) = self.heads[j].cur.as_ref().unwrap();
                        if rows.row(*pos) < rj.row(*pj) { Some(i) } else { Some(j) }
                    }
                };
            }
            let Some(i) = best else { break };
            let h = &mut self.heads[i];
            let (b, _, pos) = h.cur.as_mut().unwrap();
            let id = *h.block_id.get_or_insert_with(|| {
                blocks.push(b.clone());
                blocks.len() - 1
            });
            picks.push((id, *pos));
            *pos += 1;
            if *pos == b.num_rows() {
                h.block_id = None;
                self.refill(i)?;
            }
        }
        if picks.is_empty() {
            return Ok(None);
        }
        let cols = (0..self.schema.fields().len())
            .map(|c| {
                let arrays: Vec<&dyn Array> = blocks.iter().map(|b| b.column(c).as_ref()).collect();
                interleave(&arrays, &picks).map_err(ioerr("interleave"))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Some(RecordBatch::try_new(self.schema.clone(), cols).map_err(ioerr("merge batch"))?))
    }
}

impl Drop for Merger {
    fn drop(&mut self) {
        for h in self.heads.drain(..) {
            if h.mem_bytes > 0 {
                self.budget.release(h.mem_bytes);
            }
            if let RunSrc::File(r, p) = h.src {
                drop(r);
                remove(&p);
            }
        }
    }
}

impl SortOut {
    /// The next merged batch, or None at the end (then the runs are freed and their files deleted).
    pub fn next_merged(&self) -> Result<Option<RecordBatch>> {
        let mut g = lock(&self.merger);
        let Some(m) = g.as_mut() else { return Ok(None) };
        let b = m.next_batch(OUT_ROWS)?;
        if b.is_none() {
            *g = None;
        }
        Ok(b)
    }
    /// Free the runs and delete the spill files now (a task that ended early: LIMIT, cancel, error).
    pub fn close(&self) {
        *lock(&self.merger) = None;
    }
}

struct SortedSource(Arc<SortOut>);

impl Source for SortedSource {
    fn next(&mut self, _w: &Waker) -> Result<SourceResult> {
        Ok(match self.0.next_merged()? {
            Some(b) => SourceResult::Batch(Batch::new(b)),
            None => SourceResult::Finished,
        })
    }
    /// Called when the task ends for any reason (also LIMIT or cancel): the spill files must not survive it.
    fn finish(&mut self) {
        self.0.close();
    }
}

/// Run the sorted source with ONE task: the merge is a single ordered stream.
impl SourceFactory for Arc<SortOut> {
    fn make(&self) -> Box<dyn Source> {
        Box::new(SortedSource(Arc::clone(self)))
    }
}

// ------------------------------------------------------------------------------------------------ top-N

/// ORDER BY ... LIMIT n without spill: each task keeps at most about `4n` candidate rows, the final result is `n` rows.
pub struct TopNSink {
    n: usize,
    spec: SortSpec,
    cands: Mutex<Vec<RecordBatch>>,
    schema: Mutex<Option<SchemaRef>>,
    pub out: Arc<SharedQueue>,
}

impl TopNSink {
    pub fn new(n: usize, spec: SortSpec) -> Arc<Self> {
        Arc::new(TopNSink { n, spec, cands: Mutex::new(Vec::new()), schema: Mutex::new(None), out: SharedQueue::new() })
    }
}

fn reduce(spec: &SortSpec, schema: &SchemaRef, bs: &[RecordBatch], n: usize) -> Result<Vec<RecordBatch>> {
    if bs.is_empty() {
        return Ok(vec![]);
    }
    let s = sort_batches(spec, schema, bs)?;
    Ok(vec![s.slice(0, n.min(s.num_rows()))])
}

struct TopNLocal {
    sh: Arc<TopNSink>,
    buf: Vec<RecordBatch>,
    rows: usize,
    schema: Option<SchemaRef>,
}

impl GlobalSink for Arc<TopNSink> {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        Box::new(TopNLocal { sh: Arc::clone(self), buf: vec![], rows: 0, schema: None })
    }
    fn finalize(&self) -> Result<()> {
        let schema = lock(&self.schema).clone();
        if let Some(schema) = schema {
            let all = std::mem::take(&mut *lock(&self.cands));
            self.out.fill(reduce(&self.spec, &schema, &all, self.n)?);
        }
        Ok(())
    }
}

impl LocalSink for TopNLocal {
    fn push(&mut self, input: &mut Batch, _w: &Waker) -> Result<SinkResult> {
        if input.is_empty() {
            return Ok(SinkResult::NeedMoreInput);
        }
        let b = input.rb().clone();
        if self.schema.is_none() {
            self.schema = Some(b.schema());
            lock(&self.sh.schema).get_or_insert(b.schema());
        }
        self.rows += b.num_rows();
        self.buf.push(b);
        if self.rows >= (4 * self.sh.n).max(OUT_ROWS) {
            self.buf = reduce(&self.sh.spec, self.schema.as_ref().unwrap(), &self.buf, self.sh.n)?;
            self.rows = self.buf.iter().map(|b| b.num_rows()).sum();
        }
        Ok(SinkResult::NeedMoreInput)
    }
    fn combine(self: Box<Self>) -> Result<()> {
        if let Some(schema) = &self.schema {
            let r = reduce(&self.sh.spec, schema, &self.buf, self.sh.n)?;
            lock(&self.sh.cands).extend(r);
        }
        Ok(())
    }
}

impl Drop for SortShared {
    fn drop(&mut self) {
        for r in lock(&self.runs).drain(..) {
            match r {
                Run::File(p) => remove(&p),
                Run::Mem { bytes, .. } => self.budget.release(bytes),
            }
        }
    }
}
