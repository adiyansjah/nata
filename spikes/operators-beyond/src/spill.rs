//! Memory budget and spill files. One `Budget` per query: a byte limit shared by every blocking operator, a directory for spill
//! files, and counters (spilled bytes, files created, the most files open at once) that the tests and the report read.
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

use arrow::array::RecordBatch;
use arrow::datatypes::SchemaRef;
use arrow::ipc::reader::StreamReader;
use arrow::ipc::writer::StreamWriter;

use crate::proto::api::{QueryError, Result, err};

pub fn ioerr<E: std::fmt::Display>(what: &str) -> impl Fn(E) -> QueryError + '_ {
    move |e| QueryError(format!("{what}: {e}"))
}

pub struct Budget {
    limit: usize,
    used: AtomicUsize,
    dir: PathBuf,
    seq: AtomicUsize,
    pub peak: AtomicUsize,
    pub spilled_bytes: AtomicUsize,
    pub files_created: AtomicUsize,
    open: AtomicUsize,
    pub max_open: AtomicUsize,
}

impl Budget {
    /// `limit` in bytes (`usize::MAX` = unlimited); `dir` must exist and be empty or private to this query.
    pub fn new(limit: usize, dir: &Path) -> Arc<Self> {
        std::fs::create_dir_all(dir).expect("spill dir");
        Arc::new(Budget {
            limit,
            used: AtomicUsize::new(0),
            dir: dir.to_path_buf(),
            seq: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            spilled_bytes: AtomicUsize::new(0),
            files_created: AtomicUsize::new(0),
            open: AtomicUsize::new(0),
            max_open: AtomicUsize::new(0),
        })
    }

    pub fn limit(&self) -> usize {
        self.limit
    }
    pub fn used(&self) -> usize {
        self.used.load(SeqCst)
    }

    /// Reserve `n` bytes; false if that would exceed the limit (nothing is reserved then).
    pub fn try_reserve(&self, n: usize) -> bool {
        let mut cur = self.used.load(SeqCst);
        loop {
            if cur.saturating_add(n) > self.limit {
                return false;
            }
            match self.used.compare_exchange(cur, cur + n, SeqCst, SeqCst) {
                Ok(_) => {
                    self.peak.fetch_max(cur + n, SeqCst);
                    return true;
                }
                Err(c) => cur = c,
            }
        }
    }

    pub fn release(&self, n: usize) {
        let prev = self.used.fetch_sub(n, SeqCst);
        assert!(prev >= n, "released more memory than was reserved");
    }

    pub fn new_path(&self, tag: &str) -> PathBuf {
        let k = self.seq.fetch_add(1, SeqCst);
        self.files_created.fetch_add(1, SeqCst);
        self.dir.join(format!("{tag}-{k}.arrows"))
    }

    fn opened(&self) {
        let n = self.open.fetch_add(1, SeqCst) + 1;
        self.max_open.fetch_max(n, SeqCst);
    }
    fn closed(&self) {
        self.open.fetch_sub(1, SeqCst);
    }

    /// Files left in the spill directory. After a finished (or cancelled) query this must be 0: no orphans.
    pub fn leftover_files(&self) -> usize {
        std::fs::read_dir(&self.dir).map_or(0, |d| d.count())
    }
}

pub fn batch_bytes(b: &RecordBatch) -> usize {
    b.get_array_memory_size()
}

/// Appends batches to one spill file.
pub struct RunWriter {
    w: Option<StreamWriter<BufWriter<File>>>,
    pub path: PathBuf,
    pub rows: usize,
    budget: Arc<Budget>,
}

impl RunWriter {
    pub fn create(budget: &Arc<Budget>, tag: &str, schema: &SchemaRef) -> Result<Self> {
        let path = budget.new_path(tag);
        let f = File::create(&path).map_err(ioerr("create spill file"))?;
        budget.opened();
        let w = StreamWriter::try_new(BufWriter::new(f), schema).map_err(ioerr("spill writer"))?;
        Ok(RunWriter { w: Some(w), path, rows: 0, budget: Arc::clone(budget) })
    }

    pub fn write(&mut self, b: &RecordBatch) -> Result<()> {
        if b.num_rows() == 0 {
            return Ok(());
        }
        self.budget.spilled_bytes.fetch_add(batch_bytes(b), SeqCst);
        self.rows += b.num_rows();
        self.w.as_mut().expect("open").write(b).map_err(ioerr("spill write"))
    }

    /// Finish the stream and close the file. Must be called; dropping without `finish` leaves a truncated file.
    pub fn finish(mut self) -> Result<PathBuf> {
        let mut w = self.w.take().expect("open");
        w.finish().map_err(ioerr("spill finish"))?;
        w.into_inner().map_err(ioerr("spill flush"))?;
        self.budget.closed();
        Ok(self.path.clone())
    }
}

impl Drop for RunWriter {
    fn drop(&mut self) {
        if self.w.take().is_some() {
            self.budget.closed();
        }
    }
}

/// Reads the batches of one spill file back, in order.
pub struct RunReader {
    r: StreamReader<BufReader<File>>,
    budget: Arc<Budget>,
}

impl RunReader {
    pub fn open(budget: &Arc<Budget>, path: &Path) -> Result<Self> {
        let f = File::open(path).map_err(ioerr("open spill file"))?;
        budget.opened();
        let r = StreamReader::try_new(BufReader::new(f), None).map_err(ioerr("spill reader"))?;
        Ok(RunReader { r, budget: Arc::clone(budget) })
    }
    pub fn schema(&self) -> SchemaRef {
        self.r.schema()
    }
    pub fn next_batch(&mut self) -> Result<Option<RecordBatch>> {
        match self.r.next() {
            None => Ok(None),
            Some(Ok(b)) => Ok(Some(b)),
            Some(Err(e)) => err(format!("spill read: {e}")),
        }
    }
}

impl Drop for RunReader {
    fn drop(&mut self) {
        self.budget.closed();
    }
}

pub fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
}
