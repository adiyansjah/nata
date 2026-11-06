//! A fault-injecting, leak-counting file opener (the `Tracked` pattern of spikes/sim-harness): it counts handles while they are alive, can fail an
//! open, make reads fail after some bytes, and delay opens, per file and row group.
#![allow(dead_code, reason = "each test file uses a different part")]

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use nata_parquet::{OpenFile, OpenFor, Opener};

/// Counted while alive. A handle that is never dropped leaves the counter above zero.
pub struct Tracked(Arc<AtomicI64>);

impl Tracked {
    pub fn new(c: &Arc<AtomicI64>) -> Tracked {
        c.fetch_add(1, Ordering::SeqCst);
        Tracked(Arc::clone(c))
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FaultKind {
    /// `open` returns an I/O error.
    FailOpen,
    /// The handle opens, and reads fail once this many bytes were requested through it.
    FailReadsAfter(u64),
    /// `open` sleeps this many milliseconds first (widens race windows).
    DelayMs(u64),
}

/// A fault at file number `file` (position in the list given to the plan), at its footer (`rg == None`) or at row group `rg`.
#[derive(Clone, Debug)]
pub struct Fault {
    pub file: usize,
    pub rg: Option<usize>,
    pub kind: FaultKind,
}

#[derive(Debug)]
pub struct FaultOpener {
    paths: Vec<PathBuf>,
    faults: Vec<Fault>,
    /// Handles alive.
    pub live: Arc<AtomicI64>,
    /// Opens attempted.
    pub opens: AtomicU64,
    /// Opens that were made to fail.
    pub failed: AtomicU64,
}

impl FaultOpener {
    pub fn new(paths: &[PathBuf], faults: Vec<Fault>) -> Arc<FaultOpener> {
        Arc::new(FaultOpener {
            paths: paths.to_vec(),
            faults,
            live: Arc::new(AtomicI64::new(0)),
            opens: AtomicU64::new(0),
            failed: AtomicU64::new(0),
        })
    }

    pub fn live_handles(&self) -> i64 {
        self.live.load(Ordering::SeqCst)
    }
}

impl Opener for FaultOpener {
    fn open(&self, path: &Path, why: OpenFor) -> io::Result<OpenFile> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        let file_no = self.paths.iter().position(|p| p == path);
        let rg = match why {
            OpenFor::Footer(_) => None,
            OpenFor::RowGroup(r) => Some(r),
        };
        let mut read_fault = None;
        for f in &self.faults {
            if Some(f.file) == file_no && f.rg == rg {
                match f.kind {
                    FaultKind::DelayMs(ms) => std::thread::sleep(Duration::from_millis(ms)),
                    FaultKind::FailOpen => {
                        self.failed.fetch_add(1, Ordering::SeqCst);
                        return Err(io::Error::other("injected open failure"));
                    }
                    FaultKind::FailReadsAfter(n) => read_fault = Some(n),
                }
            }
        }
        let file = File::open(path)?;
        let mut open = OpenFile::new(file).with_guard(Box::new(Tracked::new(&self.live)));
        if let Some(n) = read_fault {
            open = open.failing_reads_after(n);
        }
        Ok(open)
    }
}
