//! The seam between the Parquet scan and an object store (card 0006, ADRs 0011 and 0012).
//!
//! The scan never talks to the network: it asks a [`RemoteQuery`] for the footer of an object ([`RemoteQuery::open`]: identity lookup, then the cached
//! parsed footer) and for the byte ranges of one row group ([`RemoteQuery::fetch`]). The implementation (`nata-objstore`) owns the I/O runtime, the
//! request policy, the footer cache and the memory grants; a `fetch` returns bytes that are already in memory and charged, so the decoder on the CPU
//! worker only reads from memory. Both calls wait for the answer on the calling worker thread and poll `cancel` while they wait.
//!
//! Locations are `s3://bucket/key` and `gs://bucket/key` strings carried in the scan's `PathBuf`s; nothing here resolves them.

use std::any::Any;
use std::fmt::Debug;
use std::io::Read;
use std::ops::Range;
use std::sync::Arc;

use bytes::Bytes;
use parquet::errors::ParquetError;
use parquet::file::metadata::ParquetMetaData;

/// The URI schemes the remote readers understand.
pub const REMOTE_SCHEMES: [&str; 2] = ["s3://", "gs://"];

/// Whether `location` names an object in a store (as opposed to a local file).
pub fn is_remote_location(location: &str) -> bool {
    REMOTE_SCHEMES.iter().any(|s| location.starts_with(s))
}

/// Why a remote call failed. Messages never carry an endpoint, a credential, a signed URL or a response body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteErrorKind {
    /// Missing object, denied access, transport failure, timeout, changed object: the object cannot be read (a data error, exit code 1).
    Io,
    /// The bytes are not what the footer promises (a bad length or range, a damaged footer).
    Corrupt,
    /// The caller's cancel answered true, or the query was cancelled.
    Cancelled,
    /// A resource bound (a grant, the footer cap) stopped the read.
    Resource,
    NotSupported,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteError {
    pub kind: RemoteErrorKind,
    pub message: String,
}

impl RemoteError {
    pub fn new(kind: RemoteErrorKind, message: impl Into<String>) -> RemoteError {
        RemoteError {
            kind,
            message: message.into(),
        }
    }
}

/// An opened object: the pinned length and the validated, parsed footer. `pinned` is the implementation's own handle (identity, location, the cache
/// pin that keeps the entry charged while the scan holds the footer).
#[derive(Clone)]
pub struct RemoteFile {
    pub metadata: Arc<ParquetMetaData>,
    pub length: u64,
    pub pinned: Arc<dyn Any + Send + Sync>,
}

impl Debug for RemoteFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteFile")
            .field("length", &self.length)
            .finish_non_exhaustive()
    }
}

/// Fetched byte ranges of one object, with whatever keeps them charged against the memory grant until the last reader drops them.
pub struct RemoteBytes {
    /// Sorted by start, non-overlapping.
    pub pieces: Vec<(u64, Bytes)>,
    pub hold: Box<dyn Any + Send + Sync>,
}

/// Process-wide: knows the stores, the cache and the runtime.
pub trait RemoteIo: Send + Sync + Debug {
    /// Start a query: its own attempt cap, memory grant and cancellation. Dropping the query cancels whatever it still has in flight.
    fn begin(&self) -> Result<Arc<dyn RemoteQuery>, RemoteError>;
    /// The objects a location or glob names, sorted by name (`s3://bucket/dir/*.parquet`). A location without a glob character is returned as it is
    /// (its existence is the reader's business). At most a bounded number of objects.
    fn expand(&self, pattern: &str, cancel: &dyn Fn() -> bool) -> Result<Vec<String>, RemoteError>;
    /// The parsed footer of one object (identity lookup plus the cached footer): what the binder reads the schema from.
    fn footer(
        &self,
        location: &str,
        cancel: &dyn Fn() -> bool,
    ) -> Result<Arc<ParquetMetaData>, RemoteError>;
}

pub trait RemoteQuery: Send + Sync + Debug {
    /// Pin the identity of the object and return its parsed footer (from the cache when the identity is unchanged).
    fn open(&self, location: &str, cancel: &dyn Fn() -> bool) -> Result<RemoteFile, RemoteError>;
    /// The bytes of `ranges` (each inside the pinned length), all conditional on the pinned identity.
    fn fetch(
        &self,
        file: &RemoteFile,
        ranges: &[Range<u64>],
        cancel: &dyn Fn() -> bool,
    ) -> Result<RemoteBytes, RemoteError>;
    /// Stop everything this query has in flight; later calls fail with `Cancelled`.
    fn cancel(&self);
}

/// The in-memory file a task decodes from: the fetched pieces of one row group (and nothing else of the object).
pub(crate) struct Blob {
    length: u64,
    pieces: Vec<(u64, Bytes)>,
    _hold: Box<dyn Any + Send + Sync>,
}

impl Blob {
    pub(crate) fn new(length: u64, fetched: RemoteBytes) -> Blob {
        Blob {
            length,
            pieces: fetched.pieces,
            _hold: fetched.hold,
        }
    }

    pub(crate) fn len(&self) -> u64 {
        self.length
    }

    fn piece_at(&self, pos: u64) -> Option<&(u64, Bytes)> {
        let i = self.pieces.partition_point(|(s, _)| *s <= pos);
        let p = self.pieces.get(i.checked_sub(1)?)?;
        let end = p.0.checked_add(u64::try_from(p.1.len()).ok()?)?;
        (pos < end).then_some(p)
    }

    pub(crate) fn get_bytes(&self, start: u64, length: usize) -> Result<Bytes, ParquetError> {
        let end = start
            .checked_add(length as u64)
            .filter(|e| *e <= self.length)
            .ok_or_else(|| ParquetError::General("a read beyond the end of the object".into()))?;
        if length == 0 {
            return Ok(Bytes::new());
        }
        let missing = || ParquetError::General("a read outside the fetched ranges".into());
        let (ps, pb) = self.piece_at(start).ok_or_else(missing)?;
        let off = usize::try_from(start - ps).map_err(|_| missing())?;
        if off + length <= pb.len() {
            return Ok(pb.slice(off..off + length));
        }
        // spans adjacent pieces: assemble (bounded by `length`, which the caller took from the footer and the grant covers)
        let mut out = Vec::with_capacity(length);
        let mut at = start;
        while at < end {
            let (s, b) = self.piece_at(at).ok_or_else(missing)?;
            let from = usize::try_from(at - s).map_err(|_| missing())?;
            let take = (b.len() - from).min(usize::try_from(end - at).map_err(|_| missing())?);
            out.extend_from_slice(&b[from..from + take]);
            at += take as u64;
        }
        Ok(Bytes::from(out))
    }

    pub(crate) fn reader(self: &Arc<Self>, start: u64) -> BlobRead {
        BlobRead {
            blob: Arc::clone(self),
            pos: start,
        }
    }
}

/// Reads the fetched pieces from `pos` on; end of file where the fetched bytes end (the `CountingRead` around it turns a second read there into an error).
pub(crate) struct BlobRead {
    blob: Arc<Blob>,
    pos: u64,
}

impl Read for BlobRead {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let Some((s, b)) = self.blob.piece_at(self.pos) else {
            return Ok(0);
        };
        let from = usize::try_from(self.pos - s).unwrap_or(usize::MAX);
        let src = b.get(from..).unwrap_or(&[]);
        let n = src.len().min(buf.len());
        buf[..n].copy_from_slice(&src[..n]);
        self.pos += n as u64;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(pieces: &[(u64, &[u8])], length: u64) -> Arc<Blob> {
        Arc::new(Blob::new(
            length,
            RemoteBytes {
                pieces: pieces
                    .iter()
                    .map(|(s, b)| (*s, Bytes::copy_from_slice(b)))
                    .collect(),
                hold: Box::new(()),
            },
        ))
    }

    #[test]
    fn schemes_are_exactly_s3_and_gs() {
        assert!(is_remote_location("s3://b/k"));
        assert!(is_remote_location("gs://b/k"));
        assert!(!is_remote_location("/data/s3://b"));
        assert!(!is_remote_location("https://b/k"));
        assert!(!is_remote_location("S3://b/k"));
    }

    #[test]
    fn reads_inside_and_across_adjacent_pieces() {
        let b = blob(&[(10, b"abcde"), (15, b"fghij"), (30, b"xyz")], 100);
        assert_eq!(&b.get_bytes(11, 3).expect("inside")[..], b"bcd");
        assert_eq!(&b.get_bytes(13, 4).expect("across")[..], b"defg");
        assert!(b.get_bytes(20, 1).is_err(), "a gap was not fetched");
        assert!(b.get_bytes(98, 5).is_err(), "beyond the object");
        assert!(
            b.get_bytes(u64::MAX - 1, 8).is_err(),
            "overflow is an error"
        );
        let mut r = b.reader(12);
        let mut out = Vec::new();
        r.read_to_end(&mut out).expect("read");
        assert_eq!(
            out, b"cdefghij",
            "a reader runs on through adjacent pieces and stops at the gap"
        );
        assert_eq!(b.reader(20).read(&mut [0u8; 4]).expect("eof"), 0);
    }
}
