//! Sources: a Parquet scan (one row group per morsel) and an in-memory scan with fault injection.
//!
//! A morsel is claimed by a task from a shared counter inside `Source::next`; the decode of a row group happens
//! inside `next` (synchronous, as ADR 0001 says: blocking lives at the edge, and this edge never blocks).
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::time::Duration;

use arrow::array::RecordBatch;
use parquet::arrow::arrow_reader::ParquetRecordBatchReader;

use crate::batch::Batch;
use crate::genops::nulls::mask;
use crate::genops::source::Source as ParquetFile;
use crate::proto::api::*;

/// Counters of a scan, readable after (or during) the query. They are what the early-finish properties assert on.
#[derive(Default)]
pub struct ScanStats {
    /// Morsels a task started to read (claimed from the shared counter and within range).
    pub claimed: AtomicUsize,
    /// Batches handed to the pipeline.
    pub batches: AtomicUsize,
    /// `Source::finish()` calls (one per task, whatever the reason the task ended).
    pub finished: AtomicUsize,
    /// Times a source returned `Blocked`.
    pub blocked: AtomicUsize,
    /// `SourceFactory::make` calls (tasks created).
    pub made: AtomicUsize,
}

/// Fault injection for sources. All draws are deterministic per (seed, task); only the interleaving varies.
#[derive(Clone, Debug, Default)]
pub struct Chaos {
    pub seed: u64,
    /// Chance per `next()` call (per mille) of sleeping up to `max_delay_us` before answering (a slow stage).
    pub delay_per_mille: u32,
    pub max_delay_us: u64,
    /// Chance per `next()` call (per mille) of answering `Blocked`; a helper thread calls the waker after up to
    /// `max_block_us` (0 = immediately, so the wake can arrive before the task has parked).
    pub block_per_mille: u32,
    pub max_block_us: u64,
    /// Tasks with index >= this answer `Blocked` FOREVER and never call their waker: they can only end if the
    /// scheduler wakes them because the pipeline finished early or the query was cancelled.
    pub stuck_from_task: Option<usize>,
}

fn next_rand(s: &mut u64) -> u64 {
    *s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut x = *s;
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

struct ChaosLocal {
    cfg: Chaos,
    task: usize,
    rng: u64,
}

impl ChaosLocal {
    fn new(cfg: Chaos, task: usize) -> Self {
        let rng = cfg.seed ^ ((task as u64 + 1) << 32);
        ChaosLocal { cfg, task, rng }
    }

    /// `Some(Blocked)` if this call must block (the waker has been arranged), after any injected delay.
    fn before_next(&mut self, waker: &Waker, stats: &ScanStats) -> Option<SourceResult> {
        if self.cfg.stuck_from_task.is_some_and(|s| self.task >= s) {
            stats.blocked.fetch_add(1, SeqCst);
            return Some(SourceResult::Blocked);
        }
        if self.cfg.delay_per_mille > 0 && next_rand(&mut self.rng) % 1000 < u64::from(self.cfg.delay_per_mille) {
            let us = next_rand(&mut self.rng) % (self.cfg.max_delay_us + 1);
            if us == 0 {
                std::thread::yield_now();
            } else {
                std::thread::sleep(Duration::from_micros(us));
            }
        }
        if self.cfg.block_per_mille > 0 && next_rand(&mut self.rng) % 1000 < u64::from(self.cfg.block_per_mille) {
            let us = next_rand(&mut self.rng) % (self.cfg.max_block_us + 1);
            let w = waker.clone();
            stats.blocked.fetch_add(1, SeqCst);
            std::thread::spawn(move || {
                if us > 0 {
                    std::thread::sleep(Duration::from_micros(us));
                }
                w.wake();
            });
            return Some(SourceResult::Blocked);
        }
        None
    }
}

// ---------------------------------------------------------------- Parquet

/// One row group per morsel. `null_pct > 0` makes that percent of four columns NULL, as the generic-ops
/// `nullcheck` does (same deterministic mask), so the scalar reference applies.
pub struct ParquetScan {
    file: Arc<ParquetFile>,
    next: AtomicUsize,
    null_pct: u32,
    pub stats: Arc<ScanStats>,
    chaos: Chaos,
}

impl ParquetScan {
    pub fn new(path: &str, columns: &[&str], null_pct: u32) -> Arc<Self> {
        Self::with_chaos(path, columns, null_pct, Chaos::default())
    }

    pub fn with_chaos(path: &str, columns: &[&str], null_pct: u32, chaos: Chaos) -> Arc<Self> {
        Self::from_file(Arc::new(ParquetFile::open(path, columns)), null_pct, chaos)
    }

    /// A fresh scan (counters at zero) over an already opened file, so a benchmark does not reload the metadata.
    pub fn from_file(file: Arc<ParquetFile>, null_pct: u32, chaos: Chaos) -> Arc<Self> {
        Arc::new(ParquetScan { file, next: AtomicUsize::new(0), null_pct, stats: Arc::default(), chaos })
    }

    pub fn morsels(&self) -> usize {
        self.file.row_groups
    }

    pub fn file(&self) -> &ParquetFile {
        &self.file
    }
}

struct ParquetLocal {
    scan: Arc<ParquetScan>,
    cur: Option<(usize, ParquetRecordBatchReader, usize)>,
    chaos: ChaosLocal,
}

impl SourceFactory for Arc<ParquetScan> {
    fn make(&self) -> Box<dyn Source> {
        let task = self.stats.made.fetch_add(1, SeqCst);
        Box::new(ParquetLocal { scan: Arc::clone(self), cur: None, chaos: ChaosLocal::new(self.chaos.clone(), task) })
    }
}

impl Source for ParquetLocal {
    fn next(&mut self, waker: &Waker) -> Result<SourceResult> {
        if let Some(b) = self.chaos.before_next(waker, &self.scan.stats) {
            return Ok(b);
        }
        loop {
            if self.cur.is_none() {
                let m = self.scan.next.fetch_add(1, SeqCst);
                if m >= self.scan.file.row_groups {
                    return Ok(SourceResult::Finished);
                }
                self.scan.stats.claimed.fetch_add(1, SeqCst);
                self.cur = Some((m, self.scan.file.reader(m), 0));
            }
            let (rg, reader, offset) = self.cur.as_mut().expect("reader");
            match reader.next() {
                None => self.cur = None,
                Some(rb) => {
                    let rb = rb.map_err(|e| QueryError(format!("parquet decode: {e}")))?;
                    let rb = if self.scan.null_pct > 0 { mask(rb, *rg, *offset, self.scan.null_pct) } else { rb };
                    *offset += rb.num_rows();
                    self.scan.stats.batches.fetch_add(1, SeqCst);
                    return Ok(SourceResult::Batch(Batch::new(&rb)));
                }
            }
        }
    }

    fn finish(&mut self) {
        self.scan.stats.finished.fetch_add(1, SeqCst);
    }
}

// ---------------------------------------------------------------- in memory

/// Morsels held in memory (`morsels[m]` is a list of batches), claimed one at a time like row groups.
pub struct MemScan {
    morsels: Arc<Vec<Vec<RecordBatch>>>,
    next: AtomicUsize,
    pub stats: Arc<ScanStats>,
    chaos: Chaos,
}

impl MemScan {
    pub fn new(morsels: Arc<Vec<Vec<RecordBatch>>>, chaos: Chaos) -> Arc<Self> {
        Arc::new(MemScan { morsels, next: AtomicUsize::new(0), stats: Arc::default(), chaos })
    }

    pub fn morsel_count(&self) -> usize {
        self.morsels.len()
    }
}

struct MemLocal {
    scan: Arc<MemScan>,
    cur: Option<(usize, usize)>,
    chaos: ChaosLocal,
}

impl SourceFactory for Arc<MemScan> {
    fn make(&self) -> Box<dyn Source> {
        let task = self.stats.made.fetch_add(1, SeqCst);
        Box::new(MemLocal { scan: Arc::clone(self), cur: None, chaos: ChaosLocal::new(self.chaos.clone(), task) })
    }
}

impl Source for MemLocal {
    fn next(&mut self, waker: &Waker) -> Result<SourceResult> {
        if let Some(b) = self.chaos.before_next(waker, &self.scan.stats) {
            return Ok(b);
        }
        loop {
            if self.cur.is_none() {
                let m = self.scan.next.fetch_add(1, SeqCst);
                if m >= self.scan.morsels.len() {
                    return Ok(SourceResult::Finished);
                }
                self.scan.stats.claimed.fetch_add(1, SeqCst);
                self.cur = Some((m, 0));
            }
            let (m, i) = self.cur.as_mut().expect("morsel");
            match self.scan.morsels[*m].get(*i) {
                None => self.cur = None,
                Some(rb) => {
                    *i += 1;
                    self.scan.stats.batches.fetch_add(1, SeqCst);
                    return Ok(SourceResult::Batch(Batch::new(rb)));
                }
            }
        }
    }

    fn finish(&mut self) {
        self.scan.stats.finished.fetch_add(1, SeqCst);
    }
}

