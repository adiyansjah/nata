//! Hash joins on the protocol: inner, left, right, full, semi and anti, one `Int64`-castable equality key plus an optional residual
//! `probe_col <> build_col` (what TPC-H Q21's EXISTS / NOT EXISTS need).
//!
//! NULL semantics: a NULL key never matches. Inner drops such rows; Left and Full keep them with NULLs on the other side; Semi drops them;
//! Anti KEEPS them (this is NOT EXISTS; `NOT IN` has different NULL rules and is not implemented). A residual that is NULL counts as not matching.
//!
//! Two shapes: (1) in memory: `JoinBuild` is a sink, `Probe` an operator, `UnmatchedBuild` a source of a later pipeline (Right and Full
//! emit the build rows nobody matched only after every probe task is done, so that needs a dependent pipeline); (2) partitioned
//! (grace): both sides go through a `PartitionSink` (spilling), then a `PartitionJoin` source joins one partition pair at a time.
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex, OnceLock};

use arrow::array::{Array, ArrayRef, AsArray, Int64Array, RecordBatch, UInt32Array, new_null_array};
use arrow::compute::{cast, concat_batches, take};
use arrow::datatypes::{DataType, Field, Int64Type, Schema, SchemaRef};

use crate::batch::Batch;
use crate::ops::SharedQueue;
use crate::proto::api::*;
use crate::proto::sched::lock;
use crate::sort::OUT_ROWS;
use crate::spill::{Budget, RunReader, RunWriter, batch_bytes, ioerr, remove};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JoinKind {
    Inner,
    Left,
    Right,
    Full,
    Semi,
    Anti,
}

#[derive(Clone, Copy)]
pub struct JoinSpec {
    pub kind: JoinKind,
    pub probe_key: usize,
    pub build_key: usize,
    /// `probe.col[.0] <> build.col[.1]` must also hold.
    pub residual_ne: Option<(usize, usize)>,
}

pub fn out_schema(kind: JoinKind, probe: &SchemaRef, build: &SchemaRef) -> SchemaRef {
    let nullable = |f: &Field, n: bool| Field::new(f.name(), f.data_type().clone(), f.is_nullable() || n);
    let p_null = matches!(kind, JoinKind::Right | JoinKind::Full);
    let b_null = matches!(kind, JoinKind::Left | JoinKind::Full);
    let mut fields: Vec<Field> = probe.fields().iter().map(|f| nullable(f, p_null)).collect();
    if !matches!(kind, JoinKind::Semi | JoinKind::Anti) {
        fields.extend(build.fields().iter().map(|f| nullable(f, b_null)));
    }
    Arc::new(Schema::new(fields))
}

fn key_of(a: &ArrayRef) -> Result<Int64Array> {
    let c = cast(a, &DataType::Int64).map_err(ioerr("join key cast"))?;
    Ok(c.as_primitive::<Int64Type>().clone())
}

const NONE: u32 = u32::MAX;

pub struct JoinTable {
    spec: JoinSpec,
    build: RecordBatch,
    head: HashMap<i64, u32>,
    next: Vec<u32>,
    resid_build: Option<Int64Array>,
    matched: Vec<AtomicBool>,
    out: SchemaRef,
}

impl JoinTable {
    pub fn bytes_estimate(build_rows: usize) -> usize {
        build_rows * (8 + 4 + 4 + 1) * 2
    }

    pub fn build(batches: &[RecordBatch], build_schema: &SchemaRef, probe_schema: &SchemaRef, spec: JoinSpec) -> Result<Self> {
        let build = if batches.is_empty() {
            RecordBatch::new_empty(build_schema.clone())
        } else {
            concat_batches(build_schema, batches).map_err(ioerr("concat build"))?
        };
        let n = build.num_rows();
        if n >= NONE as usize {
            return err("join build side has 2^32 rows or more: not supported yet");
        }
        let keys = key_of(build.column(spec.build_key))?;
        let mut head: HashMap<i64, u32> = HashMap::with_capacity(n);
        let mut next = vec![NONE; n];
        for i in (0..n).rev() {
            if keys.is_valid(i) {
                next[i] = head.insert(keys.value(i), i as u32).unwrap_or(NONE);
            }
        }
        let resid_build = spec.residual_ne.map(|(_, b)| key_of(build.column(b))).transpose()?;
        let track = matches!(spec.kind, JoinKind::Right | JoinKind::Full);
        let matched = if track { (0..n).map(|_| AtomicBool::new(false)).collect() } else { Vec::new() };
        Ok(JoinTable { spec, build, head, next, resid_build, matched, out: out_schema(spec.kind, probe_schema, build_schema) })
    }

    pub fn out_schema(&self) -> SchemaRef {
        self.out.clone()
    }

    fn chunked(&self, rb: RecordBatch) -> Vec<RecordBatch> {
        (0..rb.num_rows()).step_by(OUT_ROWS).map(|o| rb.slice(o, OUT_ROWS.min(rb.num_rows() - o))).collect()
    }

    /// Join one probe batch. The result is cut into batches of at most `OUT_ROWS` rows.
    pub fn probe(&self, rb: &RecordBatch) -> Result<Vec<RecordBatch>> {
        let s = &self.spec;
        let pk = key_of(rb.column(s.probe_key))?;
        let resid_probe = s.residual_ne.map(|(p, _)| key_of(rb.column(p))).transpose()?;
        let (mut pi, mut bi): (Vec<u32>, Vec<Option<u32>>) = (Vec::new(), Vec::new());
        let track = !self.matched.is_empty();
        for i in 0..rb.num_rows() {
            let mut any = false;
            if pk.is_valid(i) {
                let mut j = self.head.get(&pk.value(i)).copied().unwrap_or(NONE);
                while j != NONE {
                    let ok = match (&resid_probe, &self.resid_build) {
                        (Some(rp), Some(rbk)) => rp.is_valid(i) && rbk.is_valid(j as usize) && rp.value(i) != rbk.value(j as usize),
                        _ => true,
                    };
                    if ok {
                        any = true;
                        match s.kind {
                            JoinKind::Semi => {
                                pi.push(i as u32);
                                break;
                            }
                            JoinKind::Anti => break,
                            _ => {
                                pi.push(i as u32);
                                bi.push(Some(j));
                                if track {
                                    self.matched[j as usize].store(true, SeqCst);
                                }
                            }
                        }
                    }
                    j = self.next[j as usize];
                }
            }
            if !any {
                match s.kind {
                    JoinKind::Left | JoinKind::Full => {
                        pi.push(i as u32);
                        bi.push(None);
                    }
                    JoinKind::Anti => pi.push(i as u32),
                    _ => {}
                }
            }
        }
        if pi.is_empty() {
            return Ok(vec![]);
        }
        let pidx = UInt32Array::from(pi);
        let mut cols: Vec<ArrayRef> = rb.columns().iter().map(|c| take(c.as_ref(), &pidx, None).map_err(ioerr("take probe"))).collect::<Result<_>>()?;
        if !matches!(s.kind, JoinKind::Semi | JoinKind::Anti) {
            let bidx = UInt32Array::from(bi);
            for c in self.build.columns() {
                cols.push(take(c.as_ref(), &bidx, None).map_err(ioerr("take build"))?);
            }
        }
        let out = RecordBatch::try_new(self.out.clone(), cols).map_err(ioerr("join output"))?;
        Ok(self.chunked(out))
    }

    /// Right and Full: the build rows no probe row matched, with NULLs for the probe columns. Call after the last `probe`.
    pub fn unmatched_build(&self) -> Result<Vec<RecordBatch>> {
        if self.matched.is_empty() {
            return Ok(vec![]);
        }
        let idx: Vec<u32> = (0..self.build.num_rows() as u32).filter(|&j| !self.matched[j as usize].load(SeqCst)).collect();
        if idx.is_empty() {
            return Ok(vec![]);
        }
        let n = idx.len();
        let bidx = UInt32Array::from(idx);
        let np = self.out.fields().len() - self.build.num_columns();
        let mut cols: Vec<ArrayRef> = (0..np).map(|k| new_null_array(self.out.field(k).data_type(), n)).collect();
        for c in self.build.columns() {
            cols.push(take(c.as_ref(), &bidx, None).map_err(ioerr("take unmatched"))?);
        }
        let out = RecordBatch::try_new(self.out.clone(), cols).map_err(ioerr("unmatched output"))?;
        Ok(self.chunked(out))
    }
}

// ------------------------------------------------------------------------------------------------ in-memory pipeline pieces

pub struct JoinShared {
    spec: JoinSpec,
    build_schema: SchemaRef,
    probe_schema: SchemaRef,
    budget: Arc<Budget>,
    parts: Mutex<Vec<RecordBatch>>,
    table: OnceLock<Arc<JoinTable>>,
    pub unmatched: Arc<SharedQueue>,
    unmatched_done: AtomicBool,
}

impl JoinShared {
    pub fn new(spec: JoinSpec, build_schema: &SchemaRef, probe_schema: &SchemaRef, budget: &Arc<Budget>) -> Arc<Self> {
        Arc::new(JoinShared {
            spec,
            build_schema: build_schema.clone(),
            probe_schema: probe_schema.clone(),
            budget: Arc::clone(budget),
            parts: Mutex::new(vec![]),
            table: OnceLock::new(),
            unmatched: SharedQueue::new(),
            unmatched_done: AtomicBool::new(false),
        })
    }
    pub fn out_schema(&self) -> SchemaRef {
        out_schema(self.spec.kind, &self.probe_schema, &self.build_schema)
    }
}

/// The build side: a sink. Fails (loudly) if the build side does not fit the memory budget; use the partitioned join then.
pub struct JoinBuild(pub Arc<JoinShared>);

struct BuildLocal(Arc<JoinShared>, Vec<RecordBatch>, usize);

impl GlobalSink for JoinBuild {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        Box::new(BuildLocal(Arc::clone(&self.0), vec![], 0))
    }
    fn finalize(&self) -> Result<()> {
        let sh = &self.0;
        let parts = std::mem::take(&mut *lock(&sh.parts));
        let rows: usize = parts.iter().map(|b| b.num_rows()).sum();
        let est = JoinTable::bytes_estimate(rows);
        if !sh.budget.try_reserve(est) {
            return err(format!("join build side needs about {est} bytes for its hash table, over the memory budget: use the partitioned join"));
        }
        let t = JoinTable::build(&parts, &sh.build_schema, &sh.probe_schema, sh.spec)?;
        let _ = sh.table.set(Arc::new(t));
        Ok(())
    }
}

impl LocalSink for BuildLocal {
    fn push(&mut self, input: &mut Batch, _w: &Waker) -> Result<SinkResult> {
        if input.is_empty() {
            return Ok(SinkResult::NeedMoreInput);
        }
        let b = input.rb().clone();
        let n = batch_bytes(&b);
        if !self.0.budget.try_reserve(n) {
            return err("join build side exceeds the memory budget: use the partitioned join");
        }
        self.2 += n;
        self.1.push(b);
        Ok(SinkResult::NeedMoreInput)
    }
    fn combine(self: Box<Self>) -> Result<()> {
        lock(&self.0.parts).extend(self.1);
        Ok(())
    }
}

/// The probe side: an operator of the probe pipeline (which must depend on the build pipeline).
pub struct Probe(pub Arc<JoinShared>);

struct ProbeOp {
    sh: Arc<JoinShared>,
    pending: VecDeque<RecordBatch>,
}

impl OperatorFactory for Probe {
    fn make(&self) -> Box<dyn Operator> {
        Box::new(ProbeOp { sh: Arc::clone(&self.0), pending: VecDeque::new() })
    }
}

impl Operator for ProbeOp {
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        // A call that follows `HaveMoreOutput` carries the SAME input: just hand out the next piece.
        if self.pending.is_empty() {
            let Some(t) = self.sh.table.get() else { return err("probe ran before the build side finished") };
            self.pending = t.probe(input.rb())?.into();
        }
        if let Some(b) = self.pending.pop_front() {
            *out = Batch::new(b);
        }
        Ok(if self.pending.is_empty() { OpResult::NeedMoreInput } else { OpResult::HaveMoreOutput })
    }
}

/// Right and Full joins: a source for a pipeline that depends on the probe pipeline. It emits the build rows nobody matched.
pub struct UnmatchedBuild(pub Arc<JoinShared>);

struct UnmatchedSource(Arc<JoinShared>);

impl SourceFactory for UnmatchedBuild {
    fn make(&self) -> Box<dyn Source> {
        Box::new(UnmatchedSource(Arc::clone(&self.0)))
    }
}

impl Source for UnmatchedSource {
    fn next(&mut self, w: &Waker) -> Result<SourceResult> {
        let sh = &self.0;
        // The first task to get here fills the shared queue (an error is returned, not swallowed); every task then drains it.
        if !sh.unmatched_done.swap(true, SeqCst) {
            let Some(t) = sh.table.get() else { return err("unmatched build rows requested before the build side finished") };
            sh.unmatched.fill(t.unmatched_build()?);
        }
        sh.unmatched.make().next(w)
    }
}

// ------------------------------------------------------------------------------------------------ partitioned (grace) join

pub fn partition_of(key: i64, nparts: usize) -> usize {
    (((key as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 33) % nparts as u64) as usize
}

/// The files of one side of a partitioned join, one per partition (absent = no rows). Deleted when dropped.
pub struct PartFiles {
    nparts: usize,
    writers: Vec<Mutex<Option<RunWriter>>>,
    paths: Mutex<Vec<Option<PathBuf>>>,
    schema: OnceLock<SchemaRef>,
}

impl PartFiles {
    fn new(nparts: usize) -> Arc<Self> {
        Arc::new(PartFiles {
            nparts,
            writers: (0..nparts).map(|_| Mutex::new(None)).collect(),
            paths: Mutex::new(vec![None; nparts]),
            schema: OnceLock::new(),
        })
    }
}

impl Drop for PartFiles {
    fn drop(&mut self) {
        for w in &self.writers {
            if let Some(w) = lock(w).take() {
                let p = w.path.clone();
                drop(w);
                remove(&p);
            }
        }
        for p in lock(&self.paths).iter().flatten() {
            remove(p);
        }
    }
}

/// Splits its input by the hash of the key column into `nparts` spill files. NULL keys go to partition 0 (they never match, but
/// Left, Right, Full and Anti must still see them).
pub struct PartitionSink {
    key: usize,
    budget: Arc<Budget>,
    tag: &'static str,
    pub files: Arc<PartFiles>,
}

impl PartitionSink {
    pub fn new(key: usize, nparts: usize, budget: &Arc<Budget>, tag: &'static str) -> Arc<Self> {
        assert!(nparts >= 1);
        Arc::new(PartitionSink { key, budget: Arc::clone(budget), tag, files: PartFiles::new(nparts) })
    }
}

struct PartLocal {
    sh: Arc<PartitionSink>,
    bufs: Vec<Vec<RecordBatch>>,
    rows: Vec<usize>,
}

const FLUSH_ROWS: usize = 4096;

impl PartLocal {
    fn flush(&mut self, p: usize) -> Result<()> {
        if self.bufs[p].is_empty() {
            return Ok(());
        }
        let schema = self.bufs[p][0].schema();
        let b = concat_batches(&schema, &self.bufs[p]).map_err(ioerr("concat partition"))?;
        self.bufs[p].clear();
        self.rows[p] = 0;
        let f = &self.sh.files;
        let mut g = lock(&f.writers[p]);
        if g.is_none() {
            *g = Some(RunWriter::create(&self.sh.budget, self.sh.tag, &schema)?);
        }
        g.as_mut().unwrap().write(&b)
    }
}

impl GlobalSink for Arc<PartitionSink> {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        let n = self.files.nparts;
        Box::new(PartLocal { sh: Arc::clone(self), bufs: vec![vec![]; n], rows: vec![0; n] })
    }
    fn finalize(&self) -> Result<()> {
        let f = &self.files;
        for p in 0..f.nparts {
            if let Some(w) = lock(&f.writers[p]).take() {
                lock(&f.paths)[p] = Some(w.finish()?);
            }
        }
        Ok(())
    }
}

impl LocalSink for PartLocal {
    fn push(&mut self, input: &mut Batch, _w: &Waker) -> Result<SinkResult> {
        if input.is_empty() {
            return Ok(SinkResult::NeedMoreInput);
        }
        let b = input.rb();
        let _ = self.sh.files.schema.set(b.schema());
        let keys = key_of(b.column(self.sh.key))?;
        let n = self.sh.files.nparts;
        let mut idx: Vec<Vec<u32>> = vec![vec![]; n];
        for i in 0..b.num_rows() {
            let p = if keys.is_valid(i) { partition_of(keys.value(i), n) } else { 0 };
            idx[p].push(i as u32);
        }
        for (p, ix) in idx.into_iter().enumerate() {
            if ix.is_empty() {
                continue;
            }
            let ia = UInt32Array::from(ix);
            let cols = b.columns().iter().map(|c| take(c.as_ref(), &ia, None).map_err(ioerr("take partition"))).collect::<Result<Vec<_>>>()?;
            let piece = RecordBatch::try_new(b.schema(), cols).map_err(ioerr("partition piece"))?;
            self.rows[p] += piece.num_rows();
            self.bufs[p].push(piece);
            if self.rows[p] >= FLUSH_ROWS {
                self.flush(p)?;
            }
        }
        Ok(SinkResult::NeedMoreInput)
    }
    fn combine(mut self: Box<Self>) -> Result<()> {
        for p in 0..self.bufs.len() {
            self.flush(p)?;
        }
        Ok(())
    }
}

/// Joins partition pair `p` of the two sides, for every `p`, in a pipeline that depends on both partition pipelines. Each task claims
/// whole partitions. A build partition that does not fit the budget is an error (recursive repartitioning is not implemented).
pub struct PartitionJoin {
    pub spec: JoinSpec,
    pub build: Arc<PartFiles>,
    pub probe: Arc<PartFiles>,
    pub build_schema: SchemaRef,
    pub probe_schema: SchemaRef,
    pub budget: Arc<Budget>,
    next: AtomicUsize,
}

impl PartitionJoin {
    pub fn new(spec: JoinSpec, build: &Arc<PartitionSink>, probe: &Arc<PartitionSink>, build_schema: &SchemaRef, probe_schema: &SchemaRef, budget: &Arc<Budget>) -> Arc<Self> {
        assert_eq!(build.files.nparts, probe.files.nparts);
        Arc::new(PartitionJoin {
            spec,
            build: Arc::clone(&build.files),
            probe: Arc::clone(&probe.files),
            build_schema: build_schema.clone(),
            probe_schema: probe_schema.clone(),
            budget: Arc::clone(budget),
            next: AtomicUsize::new(0),
        })
    }
    pub fn out_schema(&self) -> SchemaRef {
        out_schema(self.spec.kind, &self.probe_schema, &self.build_schema)
    }
}

struct PartJoinSource {
    sh: Arc<PartitionJoin>,
    queue: VecDeque<RecordBatch>,
}

impl SourceFactory for Arc<PartitionJoin> {
    fn make(&self) -> Box<dyn Source> {
        Box::new(PartJoinSource { sh: Arc::clone(self), queue: VecDeque::new() })
    }
}

impl PartJoinSource {
    fn join_partition(&mut self, p: usize) -> Result<()> {
        let sh = Arc::clone(&self.sh);
        let read_all = |files: &Arc<PartFiles>| -> Result<(Vec<RecordBatch>, Option<PathBuf>)> {
            let path = lock(&files.paths)[p].clone();
            let mut out = vec![];
            if let Some(path) = &path {
                let mut r = RunReader::open(&sh.budget, path)?;
                while let Some(b) = r.next_batch()? {
                    out.push(b);
                }
            }
            Ok((out, path))
        };
        let (build_batches, build_path) = read_all(&sh.build)?;
        let bytes: usize = build_batches.iter().map(batch_bytes).sum::<usize>() + JoinTable::bytes_estimate(build_batches.iter().map(|b| b.num_rows()).sum());
        if !sh.budget.try_reserve(bytes) {
            return err(format!("join partition {p} needs about {bytes} bytes, over the memory budget: more partitions or recursive partitioning needed"));
        }
        let res = (|| -> Result<()> {
            let t = JoinTable::build(&build_batches, &sh.build_schema, &sh.probe_schema, sh.spec)?;
            drop(build_batches);
            let probe_path = lock(&sh.probe.paths)[p].clone();
            if let Some(path) = &probe_path {
                let mut r = RunReader::open(&sh.budget, path)?;
                while let Some(b) = r.next_batch()? {
                    self.queue.extend(t.probe(&b)?);
                }
            }
            self.queue.extend(t.unmatched_build()?);
            Ok(())
        })();
        sh.budget.release(bytes);
        for path in [build_path, lock(&sh.probe.paths)[p].clone()].into_iter().flatten() {
            remove(&path);
        }
        res
    }
}

impl Source for PartJoinSource {
    fn next(&mut self, _w: &Waker) -> Result<SourceResult> {
        loop {
            if let Some(b) = self.queue.pop_front() {
                return Ok(SourceResult::Batch(Batch::new(b)));
            }
            let p = self.sh.next.fetch_add(1, SeqCst);
            if p >= self.sh.build.nparts {
                return Ok(SourceResult::Finished);
            }
            self.join_partition(p)?;
        }
    }
}
