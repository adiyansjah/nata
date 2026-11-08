//! Budgeted local footer decoding. Keep the wire image charged, but stage only
//! one row group's Thrift expansion at a time. Parsed capacities stay charged
//! until FileScan drops; this is not a separate allowance outside query B.
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;

use nata_sched::memory::{Grant, MemoryManager, Reservation};
use parquet::errors::{ParquetError, Result};
use parquet::file::metadata::{ParquetMetaData, ParquetMetaDataReader};

fn bad() -> ParquetError {
    ParquetError::General("invalid compact footer structure".into())
}

fn reserve(memory: &Arc<MemoryManager>, bytes: usize) -> Result<Grant> {
    match MemoryManager::request(memory, 0, bytes) {
        Ok(Reservation::Granted(g)) => Ok(g),
        _ => Err(ParquetError::General(format!(
            "resource error: aggregate footer metadata needs {bytes} additional bytes (budget {})",
            memory.snapshot().budget
        ))),
    }
}

// Allocation-free compact-protocol walker, used only to locate row-group
// boundaries. parquet-rs remains the semantic decoder. Lengths cannot drive an
// allocation or a loop beyond remaining input; recursion is capped before use.
struct Compact<'a> {
    data: &'a [u8],
    pos: usize,
}
impl Compact<'_> {
    fn byte(&mut self) -> Result<u8> {
        let b = *self.data.get(self.pos).ok_or_else(bad)?;
        self.pos += 1;
        Ok(b)
    }
    fn advance(&mut self, n: usize) -> Result<()> {
        self.pos = self
            .pos
            .checked_add(n)
            .filter(|p| *p <= self.data.len())
            .ok_or_else(bad)?;
        Ok(())
    }
    fn varint(&mut self) -> Result<u64> {
        let mut n = 0;
        for shift in (0..70).step_by(7) {
            let b = self.byte()?;
            if shift == 63 && b > 1 {
                return Err(bad());
            }
            n |= u64::from(b & 127) << shift;
            if b & 128 == 0 {
                return Ok(n);
            }
        }
        Err(bad())
    }
    fn field(&mut self, last: &mut i64) -> Result<u8> {
        let b = self.byte()?;
        if b == 0 {
            return Ok(0);
        }
        let delta = b >> 4;
        *last = if delta == 0 {
            let n = self.varint()?;
            ((n >> 1) as i64) ^ -((n & 1) as i64)
        } else {
            last.checked_add(i64::from(delta)).ok_or_else(bad)?
        };
        if i16::try_from(*last).is_err() {
            return Err(bad());
        }
        Ok(b & 15)
    }
    fn list(&mut self) -> Result<(usize, u8)> {
        let b = self.byte()?;
        let n = if b >> 4 == 15 {
            self.varint()?
        } else {
            u64::from(b >> 4)
        };
        let n = usize::try_from(n).map_err(|_| bad())?;
        if n > self.data.len() - self.pos {
            return Err(bad());
        }
        Ok((n, b & 15))
    }
    fn row_group(&mut self) -> Result<bool> {
        let mut last = 0;
        let mut ordinal = false;
        loop {
            let ty = self.field(&mut last)?;
            if ty == 0 {
                return Ok(ordinal);
            }
            ordinal |= last == 7;
            self.value(ty, 1, true)?;
        }
    }

    fn value(&mut self, ty: u8, depth: usize, field: bool) -> Result<()> {
        if depth >= 64 {
            return Err(bad());
        }
        match ty {
            1 | 2 if field => Ok(()), // struct booleans live in the field header
            1..=3 => self.advance(1),
            4..=6 => self.varint().map(|_| ()),
            7 => self.advance(8),
            8 => {
                let n = usize::try_from(self.varint()?).map_err(|_| bad())?;
                self.advance(n)
            }
            9 | 10 => {
                let (n, ty) = self.list()?;
                for _ in 0..n {
                    self.value(ty, depth + 1, false)?;
                }
                Ok(())
            }
            11 => {
                let n = usize::try_from(self.varint()?).map_err(|_| bad())?;
                if n > (self.data.len() - self.pos) / 2 {
                    return Err(bad());
                }
                if n != 0 {
                    let types = self.byte()?;
                    for _ in 0..n {
                        self.value(types >> 4, depth + 1, false)?;
                        self.value(types & 15, depth + 1, false)?;
                    }
                }
                Ok(())
            }
            12 => {
                let mut last = 0;
                loop {
                    let ty = self.field(&mut last)?;
                    if ty == 0 {
                        return Ok(());
                    }
                    self.value(ty, depth + 1, true)?;
                }
            }
            _ => Err(bad()),
        }
    }
}

struct Groups {
    header: usize,
    start: usize,
    end: usize,
    count: usize,
    ordinals: usize,
}
fn groups(data: &[u8]) -> Result<Groups> {
    let mut c = Compact { data, pos: 0 };
    let mut last = 0;
    let mut found = None;
    loop {
        let ty = c.field(&mut last)?;
        if ty == 0 {
            break;
        }
        if last == 4 {
            if ty != 9 || found.is_some() {
                return Err(bad());
            }
            let header = c.pos;
            let (count, element) = c.list()?;
            if element != 12 {
                return Err(bad());
            }
            let start = c.pos;
            let mut ordinals = 0;
            for _ in 0..count {
                ordinals += usize::from(c.row_group()?);
            }
            found = Some(Groups {
                header,
                start,
                end: c.pos,
                count,
                ordinals,
            });
        } else {
            c.value(ty, 0, true)?;
        }
    }
    if c.pos != data.len() {
        return Err(bad());
    }
    found.ok_or_else(bad)
}

fn retained_bytes(meta: &ParquetMetaData) -> Result<usize> {
    // A second copy of metadata's measured capacity bounds the Arrow schema,
    // schema checks, projection/filter views, and pruning strings in read_file.
    meta.memory_size()
        .checked_mul(2)
        .and_then(|n| n.checked_add(4096))
        .ok_or_else(bad)
}

/// Same conservative 128x parser staging bound as the original reader, applied
/// to one row group, including its file-level schema/KVs, plus scratch wire.
fn staging_bytes(wire: usize) -> Result<usize> {
    wire.checked_mul(129)
        .and_then(|n| n.checked_add(4096))
        .ok_or_else(bad)
}

pub(crate) fn load(
    file: &mut File,
    memory: &Arc<MemoryManager>,
) -> Result<(ParquetMetaData, Vec<Grant>)> {
    let size = file.metadata()?.len();
    let mut tail = [0; 8];
    file.seek(SeekFrom::End(-8))?;
    file.read_exact(&mut tail)?;
    let len = u32::from_le_bytes(tail[..4].try_into().map_err(|_| bad())?) as usize;
    if len == 0 || len as u64 > size.saturating_sub(12) {
        return Err(bad());
    }
    let wire_grant = reserve(memory, len)?;
    let mut wire = vec![0; len];
    file.seek(SeekFrom::End(-8 - len as i64))?;
    file.read_exact(&mut wire)?;
    let g = groups(&wire)?;
    // Mixed ordinal presence must preserve the dependency's exact behavior:
    // it leaves missing ordinals unset. Its public builder cannot unset one,
    // so keep the bounded whole-footer fallback for that unusual shape.
    if g.ordinals != 0 && g.ordinals != g.count {
        let mut hold = reserve(memory, staging_bytes(wire.len())?)?;
        let meta = ParquetMetaDataReader::decode_metadata(&wire)?;
        let retained = retained_bytes(&meta)?;
        if retained > hold.bytes() {
            return Err(bad());
        }
        hold.shrink_to(retained).map_err(|_| bad())?;
        return Ok((meta, vec![hold]));
    }
    // Charge the assembled metadata and grant vectors before allocation.
    // read_file's pruning vector can round up to <2x length; allow two task
    // descriptor vectors with the same growth bound during scheduling.
    let slots = g.count.max(1);
    let per_group = std::mem::size_of::<parquet::file::metadata::RowGroupMetaData>()
        + std::mem::size_of::<Grant>()
        + 2 * std::mem::size_of::<crate::reader::RowGroupInfo>()
        + 4 * std::mem::size_of::<crate::reader::RowGroupTask>();
    let vectors = slots
        .checked_mul(per_group)
        .and_then(|n| n.checked_add(std::mem::size_of::<Grant>() + 4096))
        .ok_or_else(bad)?;
    let mut grants = Vec::new();
    let vector_grant = reserve(memory, vectors)?;
    grants.reserve_exact(slots + 1);
    grants.push(vector_grant);
    let mut row_groups = Vec::with_capacity(g.count);
    let mut file_metadata = None;
    let mut c = Compact {
        data: &wire,
        pos: g.start,
    };
    for index in 0..slots {
        let start = c.pos;
        if g.count != 0 {
            c.value(12, 0, false)?;
        }
        let piece_len = g
            .header
            .checked_add(1)
            .and_then(|n| n.checked_add(c.pos - start))
            .and_then(|n| n.checked_add(wire.len() - g.end))
            .ok_or_else(bad)?;
        let stage = staging_bytes(piece_len)?;
        let mut hold = reserve(memory, stage)?;
        let mut piece = Vec::with_capacity(piece_len);
        piece.extend_from_slice(&wire[..g.header]);
        piece.push(if g.count == 0 { 0x0c } else { 0x1c });
        piece.extend_from_slice(&wire[start..c.pos]);
        piece.extend_from_slice(&wire[g.end..]);
        let meta = ParquetMetaDataReader::decode_metadata(&piece)?;
        // memory_size counts Vec/String capacities and Arc contents (including
        // repeated schema pointers conservatively), as in the remote cache.
        let retained = retained_bytes(&meta)?;
        if retained > stage {
            return Err(bad());
        }
        drop(piece);
        hold.shrink_to(retained).map_err(|_| bad())?;
        let mut builder = meta.into_builder();
        for rg in builder.take_row_groups() {
            let rg = if g.ordinals == 0 {
                rg.into_builder()
                    .set_ordinal(i32::try_from(index).map_err(|_| bad())?)
                    .build()?
            } else {
                rg
            };
            row_groups.push(rg);
        }
        if file_metadata.is_none() {
            file_metadata = Some(builder.build());
        }
        grants.push(hold);
    }
    drop(wire);
    drop(wire_grant);
    let meta = file_metadata
        .ok_or_else(bad)?
        .into_builder()
        .set_row_groups(row_groups)
        .build();
    Ok((meta, grants))
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{ArrayRef, Int64Array, RecordBatch};
    use parquet::arrow::ArrowWriter;
    use parquet::file::properties::WriterProperties;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    fn fixture(rows: i64, columns: usize, group_rows: usize) -> Fixture {
        let path = std::env::temp_dir().join(format!(
            "nata-footer-{}-{}.parquet",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let columns = (0..columns).map(|i| {
            (
                format!("column_{i}"),
                Arc::new(Int64Array::from_iter_values(0..rows)) as ArrayRef,
            )
        });
        let batch = RecordBatch::try_from_iter(columns).expect("batch");
        let props = WriterProperties::builder()
            .set_max_row_group_row_count(Some(group_rows))
            .build();
        let mut writer = ArrowWriter::try_new(
            File::create(&path).expect("file"),
            batch.schema(),
            Some(props),
        )
        .expect("writer");
        writer.write(&batch).expect("write");
        writer.close().expect("close");
        Fixture(path)
    }
    fn compare(path: &std::path::Path, cap: usize) -> (usize, usize, usize) {
        let mut file = File::open(path).expect("file");
        let expected =
            parquet::arrow::arrow_reader::ArrowReaderMetadata::load(&file, Default::default())
                .expect("reference");
        let memory = MemoryManager::new(cap, 4096, 1).expect("manager");
        let (meta, grants) = load(&mut file, &memory).expect("bounded decode");
        assert_eq!(&meta, expected.metadata().as_ref());
        let retained = grants.iter().map(Grant::bytes).sum::<usize>();
        assert_eq!(retained, memory.snapshot().held);
        assert!(memory.snapshot().peak <= cap);
        let peak = memory.snapshot().peak;
        let measured = meta.memory_size();
        assert!(retained >= measured);
        drop(meta);
        drop(grants);
        assert_eq!(memory.snapshot().held, 0);
        assert_eq!(memory.snapshot().live_grants, 0);
        (measured, retained, peak)
    }
    #[test]
    fn many_row_groups_fit_without_uncharged_metadata() {
        let f = fixture(1024, 16, 2);
        let data = std::fs::read(&f.0).expect("read");
        let len = u32::from_le_bytes(
            data[data.len() - 8..data.len() - 4]
                .try_into()
                .expect("tail"),
        ) as usize;
        assert!(
            len * 128 + 4096 > 64 << 20,
            "fixture must refuse with the original charge: {len}"
        );
        let (measured, retained, peak) = compare(&f.0, 64 << 20);
        eprintln!(
            "many row groups: wire={len} original={} parsed={measured} retained={retained} peak={peak}",
            len * 128 + 4096
        );
    }
    #[test]
    fn needed_metadata_still_refuses_and_releases_every_grant() {
        let f = fixture(16, 16, 2);
        let cap = 32 << 10;
        let reference = parquet::arrow::arrow_reader::ArrowReaderMetadata::load(
            &File::open(&f.0).expect("file"),
            Default::default(),
        )
        .expect("reference");
        assert!(
            reference.metadata().memory_size() > cap,
            "metadata itself cannot fit"
        );
        let memory = MemoryManager::new(cap, 4096, 1).expect("manager");
        let err =
            load(&mut File::open(&f.0).expect("file"), &memory).expect_err("minimum cannot fit");
        assert!(
            err.to_string()
                .contains("resource error: aggregate footer metadata needs")
        );
        assert_eq!(memory.snapshot().held, 0);
        assert_eq!(memory.snapshot().live_grants, 0);
    }
    #[test]
    fn charging_counts_capacity_and_reserves_before_work() {
        let f = fixture(8, 2, 2);
        let reference = parquet::arrow::arrow_reader::ArrowReaderMetadata::load(
            &File::open(&f.0).expect("file"),
            Default::default(),
        )
        .expect("reference");
        assert_eq!(
            retained_bytes(reference.metadata()).expect("retained"),
            reference.metadata().memory_size() * 2 + 4096
        );
        assert_eq!(staging_bytes(8192).expect("staging"), 8192 * 129 + 4096);
        assert!(staging_bytes(usize::MAX).is_err());
        let memory = MemoryManager::new(64 << 10, 4096, 1).expect("manager");
        let grant = reserve(&memory, 8192).expect("grant");
        assert_eq!(grant.bytes(), 8192);
        assert_eq!(memory.snapshot().held, 8192);
        assert!(reserve(&memory, 64 << 10).is_err());
        assert_eq!(memory.snapshot().held, 8192);
        drop(grant);
        assert_eq!(memory.snapshot().held, 0);
    }
    #[test]
    fn empty_and_single_group_match_reference() {
        for rows in [0, 1, 7] {
            let f = fixture(rows, 3, 8192);
            compare(&f.0, 8 << 20);
        }
    }
    #[test]
    fn omitted_and_mixed_ordinals_match_dependency() {
        for omit_all in [false, true] {
            let f = fixture(8, 2, 2);
            let data = std::fs::read(&f.0).expect("read");
            let len = u32::from_le_bytes(
                data[data.len() - 8..data.len() - 4]
                    .try_into()
                    .expect("tail"),
            ) as usize;
            let start = data.len() - 8 - len;
            let wire = &data[start..data.len() - 8];
            let g = groups(wire).expect("groups");
            let mut rewritten = wire[..g.start].to_vec();
            let mut c = Compact {
                data: wire,
                pos: g.start,
            };
            for index in 0..g.count {
                let mut last = 0;
                loop {
                    let field_start = c.pos;
                    let ty = c.field(&mut last).expect("field");
                    if ty == 0 {
                        rewritten.push(0);
                        break;
                    }
                    c.value(ty, 0, true).expect("value");
                    if last != 7 || (!omit_all && index != 0) {
                        rewritten.extend_from_slice(&wire[field_start..c.pos]);
                    }
                }
            }
            rewritten.extend_from_slice(&wire[g.end..]);
            let mut changed = data[..start].to_vec();
            changed.extend_from_slice(&rewritten);
            changed.extend_from_slice(&(rewritten.len() as u32).to_le_bytes());
            changed.extend_from_slice(b"PAR1");
            std::fs::write(&f.0, changed).expect("rewrite");
            compare(&f.0, 8 << 20);
        }
    }
    #[test]
    fn compact_lengths_truncations_and_depth_are_bounded() {
        assert!(groups(&[0x49, 0xfc, 0xff, 0xff, 0xff, 0xff, 0x0f]).is_err());
        assert!(groups(&[0x4c]).is_err());
        let mut deep = vec![0x1c; 65];
        deep.resize(131, 0);
        assert!(groups(&deep).is_err());
        let f = fixture(8, 2, 2);
        let data = std::fs::read(&f.0).expect("read");
        let len = u32::from_le_bytes(
            data[data.len() - 8..data.len() - 4]
                .try_into()
                .expect("tail"),
        ) as usize;
        let wire = &data[data.len() - 8 - len..data.len() - 8];
        assert_eq!(groups(wire).expect("valid").count, 4);
        for end in 0..wire.len() {
            assert!(groups(&wire[..end]).is_err());
        }
    }
    #[test]
    fn damaged_budgeted_footers_release_their_charges() {
        let f = fixture(8, 2, 2);
        let data = std::fs::read(&f.0).expect("read");
        let len = u32::from_le_bytes(
            data[data.len() - 8..data.len() - 4]
                .try_into()
                .expect("tail"),
        ) as usize;
        let start = data.len() - 8 - len;
        for offset in (start..data.len()).step_by(7) {
            let mut damaged = data.clone();
            damaged[offset] ^= 0xff;
            std::fs::write(&f.0, &damaged).expect("damage");
            let memory = MemoryManager::new(8 << 20, 4096, 1).expect("manager");
            // Invalid semantics may be accepted by Parquet, but no panic,
            // unbounded decode or lost reservation is permissible.
            let result = load(&mut File::open(&f.0).expect("file"), &memory);
            drop(result);
            assert_eq!(memory.snapshot().held, 0, "offset {offset}");
            assert_eq!(memory.snapshot().live_grants, 0, "offset {offset}");
        }
    }
    #[test]
    #[ignore = "local SF1/SF10 capacity evidence, paths supplied by NATA_FOOTER_FIXTURES"]
    fn local_footer_capacity_evidence() {
        for path in std::env::var("NATA_FOOTER_FIXTURES")
            .expect("paths")
            .split(':')
        {
            let mut file = File::open(path).expect("file");
            let mut tail = [0; 8];
            file.seek(SeekFrom::End(-8)).expect("seek");
            file.read_exact(&mut tail).expect("tail");
            let wire = u32::from_le_bytes(tail[..4].try_into().expect("length"));
            let (parsed, retained, peak) = compare(std::path::Path::new(path), 64 << 20);
            eprintln!(
                "{path}: wire={wire} original={} parsed={parsed} retained={retained} footer_peak={peak}",
                u64::from(wire) * 128 + 4096
            );
        }
    }
}
