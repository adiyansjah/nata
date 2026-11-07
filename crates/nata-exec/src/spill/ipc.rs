//! Fixed trusted schema, verified before invoking Arrow's stream decoder.
//! Adapted from agg-baseline's preflight, extended to binary keys and exact
//! state bytes. No dictionaries, nested types, compression or schema discovery.
use super::{SpillHandle, invalid};
use arrow_array::RecordBatch;
use arrow_ipc::{
    MessageHeader, reader::StreamReader, root_as_message_with_opts, writer::StreamWriter,
};
use arrow_schema::{DataType, Field, Schema};
use nata_sched::memory::{Grant, MemoryManager, Reservation};
use std::io::{self, Cursor, Read, Seek};
use std::sync::Arc;

pub const MAX_ROWS: usize = 256;
pub const MAX_PAYLOAD: usize = 64 << 10;
const MAX_META: usize = 16 << 10;
const HEADER: usize = 32;
const WORKSPACE: usize = 8 * MAX_PAYLOAD;
const MAX_DEPTH: u8 = 8;

pub fn schema() -> Schema {
    Schema::new(vec![
        Field::new("hash", DataType::UInt64, false),
        Field::new("keys", DataType::Binary, false),
        Field::new("states", DataType::Binary, false),
    ])
}
/// The conservative workspace includes simultaneous input, Arrow body copies,
/// arrays, encoded frame, trusted metadata and decoded row buffers. Allocate
/// only after admission; one workspace belongs to one disk job at a time.
pub struct Workspace {
    trusted_schema: Vec<u8>,
    _grant: Arc<Grant>,
}
/// Owns the decoded buffers and their workspace reservation together. Keeping a
/// frame alive prevents reuse of that workspace for another read. Operators
/// borrow the batch to replay into a separately charged table; they must not
/// retain raw Arrow clones outside this owner.
pub struct SpillBatch {
    batch: RecordBatch,
    _grant: Arc<Grant>,
}
impl std::fmt::Debug for SpillBatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpillBatch")
            .field("rows", &self.batch.num_rows())
            .finish_non_exhaustive()
    }
}
impl SpillBatch {
    pub fn batch(&self) -> &RecordBatch {
        &self.batch
    }
}
impl Workspace {
    pub fn new(memory: &Arc<MemoryManager>) -> io::Result<Self> {
        let grant = match MemoryManager::request(memory, 0, WORKSPACE) {
            Ok(Reservation::Granted(g)) => g,
            _ => return Err(io::Error::other("spill workspace memory limit")),
        };
        let mut w = StreamWriter::try_new(Vec::new(), &schema()).map_err(|_| invalid())?;
        w.finish().map_err(|_| invalid())?;
        let bytes = w.into_inner().map_err(|_| invalid())?;
        let n = usize::try_from(u32_at(&bytes[4..8])?).map_err(|_| invalid())?;
        Ok(Self {
            trusted_schema: bytes[8..8 + n].to_vec(),
            _grant: Arc::new(grant),
        })
    }
    /// `batch` is already owned by a charged table/encode buffer. Reject its
    /// logical size before Arrow can construct a serialized copy.
    pub fn write(
        &mut self,
        file: &mut SpillHandle,
        batch: &RecordBatch,
        partition: u32,
        depth: u8,
    ) -> io::Result<()> {
        self.write_inner(file, batch, partition, depth)
            .map_err(|e| named(file, e))
    }
    fn write_inner(
        &mut self,
        file: &mut SpillHandle,
        batch: &RecordBatch,
        partition: u32,
        depth: u8,
    ) -> io::Result<()> {
        if batch.schema().as_ref() != &schema()
            || batch.num_rows() > MAX_ROWS
            || depth > MAX_DEPTH
            || batch.get_array_memory_size() > MAX_PAYLOAD / 2
        {
            return Err(invalid());
        }
        let mut w = StreamWriter::try_new(Vec::new(), &schema()).map_err(|_| invalid())?;
        w.write(batch).map_err(|_| invalid())?;
        w.finish().map_err(|_| invalid())?;
        let payload = w.into_inner().map_err(|_| invalid())?;
        if payload.len() > MAX_PAYLOAD {
            return Err(invalid());
        }
        // Check writer output through exactly the same trust boundary as disk.
        self.preflight(&payload, batch.num_rows())?;
        let mut frame = vec![0; HEADER];
        frame[..8].copy_from_slice(b"NATAAG02");
        frame[8..12].copy_from_slice(&partition.to_le_bytes());
        frame[12] = depth;
        frame[16..20].copy_from_slice(
            &u32::try_from(batch.num_rows())
                .map_err(|_| invalid())?
                .to_le_bytes(),
        );
        frame[20..28].copy_from_slice(
            &u64::try_from(payload.len())
                .map_err(|_| invalid())?
                .to_le_bytes(),
        );
        frame.extend_from_slice(&payload);
        seal(&mut frame);
        file.append(&frame)
    }
    /// A file may contain multiple independently bounded frames. EOF is valid
    /// only between frames; every other short read is a named bounded error.
    pub fn read(
        &mut self,
        file: &mut SpillHandle,
        partition: u32,
        depth: u8,
    ) -> io::Result<Option<SpillBatch>> {
        if Arc::strong_count(&self._grant) != 1 {
            return Err(named(
                file,
                io::Error::other("spill workspace still borrowed"),
            ));
        }
        self.read_inner(file, partition, depth)
            .map(|b| {
                b.map(|batch| SpillBatch {
                    batch,
                    _grant: Arc::clone(&self._grant),
                })
            })
            .map_err(|e| named(file, e))
    }
    fn read_inner(
        &mut self,
        file: &mut SpillHandle,
        partition: u32,
        depth: u8,
    ) -> io::Result<Option<RecordBatch>> {
        let remaining = file
            .size()?
            .checked_sub(file.stream_position()?)
            .ok_or_else(invalid)?;
        if remaining == 0 {
            return Ok(None);
        }
        if remaining < HEADER as u64 {
            return Err(invalid());
        }
        let mut h = [0; HEADER];
        file.read_exact(&mut h)?;
        let rows = usize::try_from(u32_at(&h[16..20])?).map_err(|_| invalid())?;
        let n = u64::from_le_bytes(h[20..28].try_into().map_err(|_| invalid())?);
        if &h[..8] != b"NATAAG02"
            || u32_at(&h[8..12])? != partition
            || h[12] != depth
            || depth > MAX_DEPTH
            || h[13..16] != [0; 3]
            || rows > MAX_ROWS
            || n > MAX_PAYLOAD as u64
            || n.checked_add(HEADER as u64).is_none_or(|v| v > remaining)
        {
            return Err(invalid());
        }
        let mut payload = vec![0; usize::try_from(n).map_err(|_| invalid())?];
        file.read_exact(&mut payload)?;
        let mut crc = crc32fast::Hasher::new();
        crc.update(&h[..28]);
        crc.update(&payload);
        if crc.finalize() != u32_at(&h[28..32])? {
            return Err(invalid());
        }
        self.preflight(&payload, rows)?;
        let mut reader =
            StreamReader::try_new(Cursor::new(&payload), None).map_err(|_| invalid())?;
        let b = reader.next().ok_or_else(invalid)?.map_err(|_| invalid())?;
        if b.schema().as_ref() != &schema() || b.num_rows() != rows || reader.next().is_some() {
            return Err(invalid());
        }
        Ok(Some(b))
    }
    fn preflight(&self, payload: &[u8], rows: usize) -> io::Result<()> {
        let mut p = payload;
        let mut messages = 0;
        loop {
            if take(&mut p, 4)? != [255; 4] {
                return Err(invalid());
            }
            let n = usize::try_from(u32_at(take(&mut p, 4)?)?).map_err(|_| invalid())?;
            if n == 0 {
                if !p.is_empty() || messages != 2 {
                    return Err(invalid());
                }
                return Ok(());
            }
            if n > MAX_META || messages >= 2 {
                return Err(invalid());
            }
            let metadata = take(&mut p, n)?;
            // Byte equality forbids even parsing an attacker-chosen schema.
            if messages == 0 && metadata != self.trusted_schema {
                return Err(invalid());
            }
            let options = flatbuffers::VerifierOptions {
                max_depth: 16,
                max_tables: 64,
                max_apparent_size: MAX_META,
                ..Default::default()
            };
            let m = root_as_message_with_opts(&options, metadata).map_err(|_| invalid())?;
            let body_len = usize::try_from(m.bodyLength()).map_err(|_| invalid())?;
            if body_len > MAX_PAYLOAD {
                return Err(invalid());
            }
            let body = take(&mut p, body_len)?;
            match (messages, m.header_type()) {
                (0, MessageHeader::Schema) if body.is_empty() => (),
                (1, MessageHeader::RecordBatch) => {
                    check_batch(m.header_as_record_batch().ok_or_else(invalid)?, rows, body)?
                }
                _ => return Err(invalid()),
            }
            messages += 1;
        }
    }
}
fn named(file: &SpillHandle, e: io::Error) -> io::Error {
    // Never propagate Arrow's message, which could contain a key/cell value.
    io::Error::new(
        e.kind(),
        format!(
            "spill file {}: {}",
            file.path().display(),
            match e.kind() {
                io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof =>
                    "bounded spill rejection",
                io::ErrorKind::StorageFull if e.raw_os_error() == Some(28) =>
                    "spill write failed: ENOSPC",
                io::ErrorKind::StorageFull => "spill disk quota exceeded",
                io::ErrorKind::Interrupted => "spill cancelled",
                _ => "spill I/O or resource error",
            }
        ),
    )
}
fn u32_at(b: &[u8]) -> io::Result<u32> {
    Ok(u32::from_le_bytes(b.try_into().map_err(|_| invalid())?))
}
fn take<'a>(p: &mut &'a [u8], n: usize) -> io::Result<&'a [u8]> {
    if n > p.len() {
        return Err(invalid());
    }
    let (a, b) = p.split_at(n);
    *p = b;
    Ok(a)
}
fn buffer<'a>(body: &'a [u8], b: &arrow_ipc::Buffer) -> io::Result<&'a [u8]> {
    let start = usize::try_from(b.offset()).map_err(|_| invalid())?;
    let end = start
        .checked_add(usize::try_from(b.length()).map_err(|_| invalid())?)
        .ok_or_else(invalid)?;
    body.get(start..end).ok_or_else(invalid)
}
fn check_batch(b: arrow_ipc::RecordBatch<'_>, rows: usize, body: &[u8]) -> io::Result<()> {
    if usize::try_from(b.length()).map_err(|_| invalid())? != rows
        || b.compression().is_some()
        || b.variadicBufferCounts().is_some()
    {
        return Err(invalid());
    }
    let nodes = b.nodes().ok_or_else(invalid)?;
    let buffers = b.buffers().ok_or_else(invalid)?;
    if nodes.len() != 3 || buffers.len() != 8 {
        return Err(invalid());
    }
    for node in nodes {
        if node.length() != b.length() || node.null_count() != 0 {
            return Err(invalid());
        }
    }
    let mut end = 0;
    for b in buffers {
        let start = usize::try_from(b.offset()).map_err(|_| invalid())?;
        let size = usize::try_from(b.length()).map_err(|_| invalid())?;
        end = if start < end {
            return Err(invalid());
        } else {
            start.checked_add(size).ok_or_else(invalid)?
        };
        if end > body.len() {
            return Err(invalid());
        }
    }
    if buffer(body, buffers.get(1))?.len() != rows * 8 {
        return Err(invalid());
    }
    for i in [2, 5] {
        let offsets = buffer(body, buffers.get(i + 1))?;
        let values = buffer(body, buffers.get(i + 2))?;
        if offsets.len() != (rows + 1) * 4 {
            return Err(invalid());
        }
        let mut previous = 0;
        for n in 0..=rows {
            let value = i32::from_le_bytes(
                offsets[n * 4..n * 4 + 4]
                    .try_into()
                    .map_err(|_| invalid())?,
            );
            let value = usize::try_from(value).map_err(|_| invalid())?;
            if value < previous || value > values.len() || (n == 0 && value != 0) {
                return Err(invalid());
            }
            previous = value;
        }
        if previous != values.len() {
            return Err(invalid());
        }
    }
    // Explicitly validate the binary arrays too; there are no dictionary indices
    // or UTF-8 buffers in this schema. Typed state replay validates text later.
    Ok(())
}
pub(super) fn seal(frame: &mut [u8]) {
    let mut crc = crc32fast::Hasher::new();
    crc.update(&frame[..28]);
    crc.update(&frame[HEADER..]);
    frame[28..32].copy_from_slice(&crc.finalize().to_le_bytes());
}
