//! Sources, operators and sinks used to exercise the protocol.

use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::api::*;
use crate::batch::Batch;
use crate::sched::lock;

/// Rows per output batch of an expanding operator.
pub const OUT_ROWS: usize = 1024;

// ---------------------------------------------------------------- generator source

struct GenInner {
    next: AtomicUsize,
    total: usize,
    produce: Box<dyn Fn(usize) -> Batch + Send + Sync>,
    produced: AtomicUsize,
}

/// A source of `total` morsels; each worker task takes the next unclaimed morsel.
#[derive(Clone)]
pub struct Generator(Arc<GenInner>);

impl Generator {
    pub fn new(total: usize, produce: impl Fn(usize) -> Batch + Send + Sync + 'static) -> Self {
        Generator(Arc::new(GenInner {
            next: AtomicUsize::new(0),
            total,
            produce: Box::new(produce),
            produced: AtomicUsize::new(0),
        }))
    }

    /// Morsels handed out so far (to see how much work early termination saved).
    pub fn produced(&self) -> usize {
        self.0.produced.load(SeqCst)
    }
}

struct GenLocal(Arc<GenInner>);

impl SourceFactory for Generator {
    fn make(&self) -> Box<dyn Source> {
        Box::new(GenLocal(Arc::clone(&self.0)))
    }
}

impl Source for GenLocal {
    fn next(&mut self, _w: &Waker) -> Result<SourceResult> {
        let m = self.0.next.fetch_add(1, SeqCst);
        if m >= self.0.total {
            return Ok(SourceResult::Finished);
        }
        self.0.produced.fetch_add(1, SeqCst);
        Ok(SourceResult::Batch((self.0.produce)(m)))
    }
}

// ---------------------------------------------------------------- simple operators

/// Keeps rows for which the predicate is true.
pub struct Filter(pub Arc<dyn Fn(&[i64]) -> bool + Send + Sync>);

impl OperatorFactory for Filter {
    fn make(&self) -> Box<dyn Operator> {
        Box::new(FilterOp(Arc::clone(&self.0)))
    }
}

struct FilterOp(Arc<dyn Fn(&[i64]) -> bool + Send + Sync>);

impl Operator for FilterOp {
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        *out = Batch::new(input.cols.len());
        for i in 0..input.len() {
            let row = input.row(i);
            if (self.0)(&row) {
                out.push_row(&row);
            }
        }
        Ok(OpResult::NeedMoreInput)
    }
}

/// Sleeps per batch, to make a stage slow.
pub struct Delay(pub Duration);

impl OperatorFactory for Delay {
    fn make(&self) -> Box<dyn Operator> {
        Box::new(DelayOp(self.0))
    }
}

struct DelayOp(Duration);

impl Operator for DelayOp {
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        std::thread::sleep(self.0);
        *out = input.clone();
        Ok(OpResult::NeedMoreInput)
    }
}

/// Passes batches through and fails on the `at`-th batch seen by any task (counting from 0).
pub struct FailAt {
    pub at: usize,
    pub seen: Arc<AtomicUsize>,
}

impl OperatorFactory for FailAt {
    fn make(&self) -> Box<dyn Operator> {
        Box::new(FailOp {
            at: self.at,
            seen: Arc::clone(&self.seen),
        })
    }
}

struct FailOp {
    at: usize,
    seen: Arc<AtomicUsize>,
}

impl Operator for FailOp {
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        if self.seen.fetch_add(1, SeqCst) == self.at {
            return err("decimal overflow in test operator");
        }
        *out = input.clone();
        Ok(OpResult::NeedMoreInput)
    }
}

/// `LIMIT n`, shared by all tasks of the pipeline, exact however the batches interleave.
#[derive(Clone)]
pub struct Limit(Arc<AtomicUsize>);

impl Limit {
    pub fn new(n: usize) -> Self {
        Limit(Arc::new(AtomicUsize::new(n)))
    }
}

impl OperatorFactory for Limit {
    fn make(&self) -> Box<dyn Operator> {
        Box::new(LimitOp(Arc::clone(&self.0)))
    }
}

struct LimitOp(Arc<AtomicUsize>);

impl Operator for LimitOp {
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        loop {
            let remaining = self.0.load(SeqCst);
            if remaining == 0 {
                return Ok(OpResult::Finished);
            }
            let take = remaining.min(input.len());
            if self
                .0
                .compare_exchange(remaining, remaining - take, SeqCst, SeqCst)
                .is_ok()
            {
                *out = Batch::new(input.cols.len());
                for i in 0..take {
                    out.push_row(&input.row(i));
                }
                return Ok(if take == remaining {
                    OpResult::Finished
                } else {
                    OpResult::NeedMoreInput
                });
            }
        }
    }
}

// ---------------------------------------------------------------- collecting sink

/// Collects everything it is given. Clone it to read the result after the query ends.
#[derive(Clone, Default)]
pub struct Collect(Arc<Mutex<Vec<Batch>>>);

impl Collect {
    pub fn new() -> Self {
        Self::default()
    }

    /// All rows, sorted, so tests do not depend on task interleaving.
    pub fn rows(&self) -> Vec<Vec<i64>> {
        let mut rows: Vec<Vec<i64>> = lock(&self.0).iter().flat_map(Batch::rows).collect();
        rows.sort();
        rows
    }
}

struct CollectLocal {
    batches: Vec<Batch>,
    global: Arc<Mutex<Vec<Batch>>>,
}

impl GlobalSink for Collect {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        Box::new(CollectLocal {
            batches: Vec::new(),
            global: Arc::clone(&self.0),
        })
    }

    fn finalize(&self) -> Result<()> {
        Ok(())
    }
}

impl LocalSink for CollectLocal {
    fn push(&mut self, input: &mut Batch, _w: &Waker) -> Result<SinkResult> {
        self.batches.push(std::mem::take(input));
        Ok(SinkResult::NeedMoreInput)
    }

    fn combine(self: Box<Self>) -> Result<()> {
        lock(&self.global).extend(self.batches);
        Ok(())
    }
}

// ---------------------------------------------------------------- exchange

struct ChanState {
    queue: VecDeque<Batch>,
    producers: usize,
    consumers_closed: bool,
    consumer_wakers: Vec<Waker>,
    producer_wakers: Vec<Waker>,
}

/// A bounded queue of batches between two pipelines that run at the same time.
pub struct Channel {
    cap: usize,
    state: Mutex<ChanState>,
    pub pushed: AtomicUsize,
}

fn wake_all(wakers: Vec<Waker>) {
    wakers.iter().for_each(Waker::wake);
}

impl Channel {
    fn push(&self, batch: &mut Batch, waker: &Waker) -> SinkResult {
        let mut s = lock(&self.state);
        if s.consumers_closed {
            return SinkResult::Finished;
        }
        if s.queue.len() >= self.cap {
            s.producer_wakers.push(waker.clone());
            return SinkResult::Blocked;
        }
        s.queue.push_back(std::mem::take(batch));
        self.pushed.fetch_add(1, SeqCst);
        let to_wake = std::mem::take(&mut s.consumer_wakers);
        drop(s);
        wake_all(to_wake);
        SinkResult::NeedMoreInput
    }

    fn pop(&self, waker: &Waker) -> SourceResult {
        let mut s = lock(&self.state);
        if let Some(b) = s.queue.pop_front() {
            let to_wake = std::mem::take(&mut s.producer_wakers);
            drop(s);
            wake_all(to_wake);
            return SourceResult::Batch(b);
        }
        if s.producers == 0 {
            return SourceResult::Finished;
        }
        s.consumer_wakers.push(waker.clone());
        SourceResult::Blocked
    }

    fn producer_done(&self) {
        let mut s = lock(&self.state);
        s.producers -= 1;
        let to_wake = if s.producers == 0 {
            std::mem::take(&mut s.consumer_wakers)
        } else {
            Vec::new()
        };
        drop(s);
        wake_all(to_wake);
    }

    fn close_consumers(&self) {
        let mut s = lock(&self.state);
        s.consumers_closed = true;
        let to_wake = std::mem::take(&mut s.producer_wakers);
        drop(s);
        wake_all(to_wake);
    }
}

/// Make a channel plus its sink (producer side) and source (consumer side). `producers` and
/// `consumers` must equal the `tasks` of the two pipelines.
pub fn exchange(
    cap: usize,
    producers: usize,
    consumers: usize,
) -> (Arc<dyn GlobalSink>, Arc<dyn SourceFactory>, Arc<Channel>) {
    let chan = Arc::new(Channel {
        cap,
        state: Mutex::new(ChanState {
            queue: VecDeque::new(),
            producers,
            consumers_closed: false,
            consumer_wakers: Vec::new(),
            producer_wakers: Vec::new(),
        }),
        pushed: AtomicUsize::new(0),
    });
    let sink = Arc::new(ExchangeSink(Arc::clone(&chan)));
    let source = Arc::new(ExchangeSource {
        chan: Arc::clone(&chan),
        consumers: Arc::new(AtomicUsize::new(consumers)),
    });
    (sink, source, chan)
}

struct ExchangeSink(Arc<Channel>);

struct ExchangeSinkLocal(Arc<Channel>);

impl GlobalSink for ExchangeSink {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        Box::new(ExchangeSinkLocal(Arc::clone(&self.0)))
    }

    fn finalize(&self) -> Result<()> {
        Ok(())
    }
}

impl LocalSink for ExchangeSinkLocal {
    fn push(&mut self, input: &mut Batch, waker: &Waker) -> Result<SinkResult> {
        Ok(self.0.push(input, waker))
    }

    fn combine(self: Box<Self>) -> Result<()> {
        self.0.producer_done();
        Ok(())
    }
}

struct ExchangeSource {
    chan: Arc<Channel>,
    consumers: Arc<AtomicUsize>,
}

struct ExchangeSourceLocal {
    chan: Arc<Channel>,
    consumers: Arc<AtomicUsize>,
}

impl SourceFactory for ExchangeSource {
    fn make(&self) -> Box<dyn Source> {
        Box::new(ExchangeSourceLocal {
            chan: Arc::clone(&self.chan),
            consumers: Arc::clone(&self.consumers),
        })
    }
}

impl Source for ExchangeSourceLocal {
    fn next(&mut self, waker: &Waker) -> Result<SourceResult> {
        Ok(self.chan.pop(waker))
    }

    fn finish(&mut self) {
        if self.consumers.fetch_sub(1, SeqCst) == 1 {
            self.chan.close_consumers();
        }
    }
}

// ---------------------------------------------------------------- hash join

/// The build side's table, filled once by the build pipeline's finalize.
#[derive(Default)]
pub struct JoinSlot(OnceLock<HashMap<i64, Vec<i64>>>);

#[derive(Clone)]
pub struct JoinBuild {
    rows: Arc<Mutex<Vec<(i64, i64)>>>,
    slot: Arc<JoinSlot>,
}

impl JoinBuild {
    pub fn new() -> Self {
        JoinBuild {
            rows: Arc::default(),
            slot: Arc::default(),
        }
    }

    pub fn probe(&self) -> Arc<dyn OperatorFactory> {
        Arc::new(Probe(Arc::clone(&self.slot)))
    }
}

impl Default for JoinBuild {
    fn default() -> Self {
        Self::new()
    }
}

struct JoinBuildLocal {
    rows: Vec<(i64, i64)>,
    global: Arc<Mutex<Vec<(i64, i64)>>>,
}

impl GlobalSink for JoinBuild {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        Box::new(JoinBuildLocal {
            rows: Vec::new(),
            global: Arc::clone(&self.rows),
        })
    }

    fn finalize(&self) -> Result<()> {
        let mut table: HashMap<i64, Vec<i64>> = HashMap::new();
        for (k, v) in lock(&self.rows).drain(..) {
            table.entry(k).or_default().push(v);
        }
        self.slot
            .0
            .set(table)
            .map_err(|_| QueryError("join table built twice".into()))
    }
}

impl LocalSink for JoinBuildLocal {
    fn push(&mut self, input: &mut Batch, _w: &Waker) -> Result<SinkResult> {
        self.rows.extend(input.rows().map(|r| (r[0], r[1])));
        Ok(SinkResult::NeedMoreInput)
    }

    fn combine(self: Box<Self>) -> Result<()> {
        lock(&self.global).extend(self.rows);
        Ok(())
    }
}

struct Probe(Arc<JoinSlot>);

impl OperatorFactory for Probe {
    fn make(&self) -> Box<dyn Operator> {
        Box::new(ProbeOp {
            slot: Arc::clone(&self.0),
            row: 0,
            matched: 0,
        })
    }
}

/// Inner join on column 0; output `[key, probe value, build value]`. One probe row can match
/// many build rows, so one input batch can need many calls: `HaveMoreOutput`.
struct ProbeOp {
    slot: Arc<JoinSlot>,
    row: usize,
    matched: usize,
}

impl Operator for ProbeOp {
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        let Some(table) = self.slot.0.get() else {
            return err("probe ran before the build side finished");
        };
        *out = Batch::new(3);
        while self.row < input.len() {
            let (key, val) = (input.cols[0][self.row], input.cols[1][self.row]);
            if let Some(matches) = table.get(&key) {
                while self.matched < matches.len() {
                    if out.len() >= OUT_ROWS {
                        return Ok(OpResult::HaveMoreOutput);
                    }
                    out.push_row(&[key, val, matches[self.matched]]);
                    self.matched += 1;
                }
            }
            self.row += 1;
            self.matched = 0;
        }
        self.row = 0;
        Ok(OpResult::NeedMoreInput)
    }
}

// ---------------------------------------------------------------- aggregate with spill

/// A directory removed when dropped, so a cancelled or failed query leaves nothing behind.
pub struct SpillDir(PathBuf);

impl SpillDir {
    pub fn create(tag: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "spike-spill-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, SeqCst)
        ));
        fs::create_dir_all(&path).expect("create spill dir");
        SpillDir(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for SpillDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Counts the spill files that are open at the same time (and how often one is opened), per
/// aggregate. Added by the fairness spike after a clean VM (file-descriptor limit 1024) showed that
/// time slicing multiplies the number of open spill files.
#[derive(Default)]
pub struct FdCount {
    cur: AtomicUsize,
    peak: AtomicUsize,
    opens: AtomicU64,
}

/// Held for as long as one spill file is open.
pub struct FdGuard(Arc<FdCount>);

impl FdCount {
    fn enter(self: &Arc<Self>) -> FdGuard {
        let now = self.cur.fetch_add(1, SeqCst) + 1;
        self.peak.fetch_max(now, SeqCst);
        self.opens.fetch_add(1, SeqCst);
        FdGuard(Arc::clone(self))
    }
}

impl Drop for FdGuard {
    fn drop(&mut self) {
        self.0.cur.fetch_sub(1, SeqCst);
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AggConfig {
    /// Most groups one task keeps in memory before it spills.
    pub budget_groups: usize,
    /// Spill partitions; the finalize step holds one partition's groups at a time.
    pub partitions: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AggStats {
    pub spilled_entries: u64,
    pub spill_files: usize,
    pub peak_local_groups: usize,
    pub peak_partition_groups: usize,
    /// Most spill files open at the same moment during the query.
    pub peak_open_spill_files: usize,
    /// How many times a spill file was opened in total.
    pub spill_file_opens: u64,
}

struct AggInner {
    cfg: AggConfig,
    dir: SpillDir,
    any_spilled: AtomicBool,
    mem_partials: Mutex<Vec<HashMap<i64, i64>>>,
    files: Mutex<Vec<Vec<PathBuf>>>,
    result: Mutex<Vec<Batch>>,
    spilled_entries: AtomicU64,
    peak_local: AtomicUsize,
    peak_partition: AtomicUsize,
    fds: Arc<FdCount>,
}

/// `SELECT key, SUM(value) ... GROUP BY key` over columns 0 and 1, with a per-task memory budget.
#[derive(Clone)]
pub struct Aggregate(Arc<AggInner>);

fn partition_of(key: i64, partitions: usize) -> usize {
    ((key as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 33) as usize % partitions
}

impl Aggregate {
    pub fn new(cfg: AggConfig) -> Self {
        Aggregate(Arc::new(AggInner {
            cfg,
            dir: SpillDir::create("agg"),
            any_spilled: AtomicBool::new(false),
            mem_partials: Mutex::new(Vec::new()),
            files: Mutex::new(vec![Vec::new(); cfg.partitions]),
            result: Mutex::new(Vec::new()),
            spilled_entries: AtomicU64::new(0),
            peak_local: AtomicUsize::new(0),
            peak_partition: AtomicUsize::new(0),
            fds: Arc::new(FdCount::default()),
        }))
    }

    pub fn spill_dir(&self) -> PathBuf {
        self.0.dir.path().to_path_buf()
    }

    pub fn stats(&self) -> AggStats {
        AggStats {
            spilled_entries: self.0.spilled_entries.load(SeqCst),
            spill_files: lock(&self.0.files).iter().map(Vec::len).sum(),
            peak_local_groups: self.0.peak_local.load(SeqCst),
            peak_partition_groups: self.0.peak_partition.load(SeqCst),
            peak_open_spill_files: self.0.fds.peak.load(SeqCst),
            spill_file_opens: self.0.fds.opens.load(SeqCst),
        }
    }

    /// The finalized result as the source of a following pipeline.
    pub fn result_source(&self) -> Arc<dyn SourceFactory> {
        Arc::new(AggResult {
            g: Arc::clone(&self.0),
            next: Arc::new(AtomicUsize::new(0)),
        })
    }
}

struct AggLocal {
    g: Arc<AggInner>,
    task: usize,
    map: HashMap<i64, i64>,
    spilled: bool,
}

fn overflow() -> QueryError {
    QueryError("overflow in SUM: the result does not fit in 64 bits".into())
}

impl AggLocal {
    /// Write the in-memory groups to the spill files. Every partition's file is opened, appended to
    /// and closed within this call, so a task holds no file between calls: the number of open spill
    /// files is at most one per running worker, whatever the number of tasks, partitions or
    /// time-slice yields. (Keeping one writer per partition open until `combine` made it
    /// `tasks x partitions` once slicing interleaved the tasks: a clean VM with a descriptor limit of
    /// 1024 failed with "Too many open files".)
    fn spill(&mut self) -> Result<()> {
        let io = |e: std::io::Error| QueryError(format!("spill failed: {e}"));
        let parts = self.g.cfg.partitions;
        let n = self.map.len() as u64;
        let mut rows: Vec<(usize, i64, i64)> = self
            .map
            .drain()
            .map(|(k, v)| (partition_of(k, parts), k, v))
            .collect();
        rows.sort_unstable_by_key(|r| r.0);
        let mut buf = Vec::new();
        for run in rows.chunk_by(|a, b| a.0 == b.0) {
            let p = run[0].0;
            buf.clear();
            for &(_, k, v) in run {
                buf.extend_from_slice(&k.to_le_bytes());
                buf.extend_from_slice(&v.to_le_bytes());
            }
            let path = self.g.dir.path().join(format!("p{p}_t{}.run", self.task));
            {
                // Counted before the open and released after the close (it never undercounts).
                let _open = self.g.fds.enter();
                let mut f = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .map_err(io)?;
                f.write_all(&buf).map_err(io)?;
            } // closed here
            let mut files = lock(&self.g.files);
            if !files[p].contains(&path) {
                files[p].push(path);
            }
        }
        self.g.spilled_entries.fetch_add(n, SeqCst);
        self.g.any_spilled.store(true, SeqCst);
        self.spilled = true;
        Ok(())
    }
}

impl GlobalSink for Aggregate {
    fn make(&self, task: usize) -> Box<dyn LocalSink> {
        Box::new(AggLocal {
            g: Arc::clone(&self.0),
            task,
            map: HashMap::new(),
            spilled: false,
        })
    }

    /// Merge the in-memory groups and the spilled runs, one partition at a time.
    fn finalize(&self) -> Result<()> {
        let g = &self.0;
        let io = |e: std::io::Error| QueryError(format!("reading spill failed: {e}"));
        let partials = std::mem::take(&mut *lock(&g.mem_partials));
        let mut result = Vec::new();
        let emit = |acc: HashMap<i64, i64>, result: &mut Vec<Batch>| {
            let mut batch = Batch::new(2);
            for (k, v) in acc {
                batch.push_row(&[k, v]);
                if batch.len() >= OUT_ROWS {
                    result.push(std::mem::replace(&mut batch, Batch::new(2)));
                }
            }
            if !batch.is_empty() {
                result.push(batch);
            }
        };
        if !g.any_spilled.load(SeqCst) {
            let mut acc: HashMap<i64, i64> = HashMap::new();
            for m in partials {
                for (k, v) in m {
                    let e = acc.entry(k).or_insert(0);
                    *e = e.checked_add(v).ok_or_else(overflow)?;
                }
            }
            g.peak_partition.fetch_max(acc.len(), SeqCst);
            emit(acc, &mut result);
        } else {
            let files = lock(&g.files).clone();
            for (p, paths) in files.iter().enumerate() {
                let mut acc: HashMap<i64, i64> = HashMap::new();
                for path in paths {
                    let mut r = BufReader::new(File::open(path).map_err(io)?);
                    let mut rec = [0u8; 16];
                    while r.read_exact(&mut rec).is_ok() {
                        let k = i64::from_le_bytes(rec[..8].try_into().expect("8 bytes"));
                        let v = i64::from_le_bytes(rec[8..].try_into().expect("8 bytes"));
                        let e = acc.entry(k).or_insert(0);
                        *e = e.checked_add(v).ok_or_else(overflow)?;
                    }
                    let _ = fs::remove_file(path);
                }
                for m in &partials {
                    for (&k, &v) in m {
                        if partition_of(k, g.cfg.partitions) == p {
                            let e = acc.entry(k).or_insert(0);
                            *e = e.checked_add(v).ok_or_else(overflow)?;
                        }
                    }
                }
                g.peak_partition.fetch_max(acc.len(), SeqCst);
                emit(acc, &mut result);
            }
        }
        *lock(&g.result) = result;
        Ok(())
    }
}

impl LocalSink for AggLocal {
    fn push(&mut self, input: &mut Batch, _w: &Waker) -> Result<SinkResult> {
        for i in 0..input.len() {
            let (k, v) = (input.cols[0][i], input.cols[1][i]);
            if !self.map.contains_key(&k) && self.map.len() >= self.g.cfg.budget_groups {
                self.spill()?;
            }
            let e = self.map.entry(k).or_insert(0);
            *e = e.checked_add(v).ok_or_else(overflow)?;
            self.g.peak_local.fetch_max(self.map.len(), SeqCst);
        }
        Ok(SinkResult::NeedMoreInput)
    }

    fn combine(mut self: Box<Self>) -> Result<()> {
        if self.spilled {
            self.spill()?;
        } else {
            let map = std::mem::take(&mut self.map);
            lock(&self.g.mem_partials).push(map);
        }
        Ok(())
    }
}

struct AggResult {
    g: Arc<AggInner>,
    next: Arc<AtomicUsize>,
}

struct AggResultLocal {
    g: Arc<AggInner>,
    next: Arc<AtomicUsize>,
}

impl SourceFactory for AggResult {
    fn make(&self) -> Box<dyn Source> {
        Box::new(AggResultLocal {
            g: Arc::clone(&self.g),
            next: Arc::clone(&self.next),
        })
    }
}

impl Source for AggResultLocal {
    fn next(&mut self, _w: &Waker) -> Result<SourceResult> {
        let i = self.next.fetch_add(1, SeqCst);
        Ok(match lock(&self.g.result).get(i) {
            Some(b) => SourceResult::Batch(b.clone()),
            None => SourceResult::Finished,
        })
    }
}
