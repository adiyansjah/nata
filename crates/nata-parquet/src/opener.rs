//! Where file handles come from. Every handle the scan holds (a footer read, a row-group read) is made by an [`Opener`], so that tests can count them,
//! make `open` fail, or make reads fail after some bytes (the injected I/O errors of the card's fault-tolerance row). The default opens real files.

use std::fmt::Debug;
use std::fs::File;
use std::io;
use std::path::Path;

/// Why a handle is wanted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenFor {
    /// To read the footer of file number `usize` of the plan.
    Footer(usize),
    /// To read row group `usize`.
    RowGroup(usize),
}

pub trait Opener: Send + Sync + Debug {
    fn open(&self, path: &Path, why: OpenFor) -> io::Result<OpenFile>;
}

/// A handle plus what a test may attach to it.
pub struct OpenFile {
    pub(crate) file: File,
    /// Reads fail with an I/O error once this many bytes were requested through the handle.
    pub(crate) fail_reads_after: Option<u64>,
    /// Dropped with the handle (a counter for the `Tracked` leak check).
    pub(crate) _guard: Option<Box<dyn Send + Sync>>,
}

impl OpenFile {
    pub fn new(file: File) -> OpenFile {
        OpenFile {
            file,
            fail_reads_after: None,
            _guard: None,
        }
    }

    pub fn failing_reads_after(mut self, bytes: u64) -> OpenFile {
        self.fail_reads_after = Some(bytes);
        self
    }

    pub fn with_guard(mut self, guard: Box<dyn Send + Sync>) -> OpenFile {
        self._guard = Some(guard);
        self
    }
}

/// Real files.
#[derive(Debug, Default, Clone, Copy)]
pub struct FsOpener;

impl Opener for FsOpener {
    fn open(&self, path: &Path, _why: OpenFor) -> io::Result<OpenFile> {
        File::open(path).map(OpenFile::new)
    }
}
