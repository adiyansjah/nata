//! Disk jobs own the tables/buffers they consume. Only the bounded I/O worker
//! executes these methods; CPU continuations poll their tickets and requeue.
use super::{Atom, CANCEL_ROWS, State, Table, replay};
use crate::spill::{Registry, SpillBatch, SpillFile, Workspace, schema};
use arrow_array::{Array, ArrayRef, BinaryArray, RecordBatch, UInt64Array};
use nata_sql::bind::Aggregate;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub(super) enum Completion {
    Written,
    Read {
        frame: Option<SpillBatch>,
        handle: crate::spill::SpillHandle,
    },
}
pub(super) struct Storage {
    pub registry: Arc<Registry>,
    pub runtime: Arc<nata_sched::memory::Grant>,
    pub cancel: Option<nata_parquet::ScanCancel>,
    pub hash: fn(&[Atom], u8) -> u64,
    pub initial_fanout: usize,
    pub resident_estimate: usize,
    pub partial_rows: Arc<AtomicUsize>,
    pub largest_partial: Arc<AtomicUsize>,
    pub rehashes: Arc<AtomicUsize>,
    pub workspace: Mutex<Workspace>,
}
impl Storage {
    pub fn write(
        &self,
        tables: Vec<Table>,
        files: &[Arc<SpillFile>],
        a: &Aggregate,
        depth: u8,
    ) -> io::Result<()> {
        self.largest_partial.fetch_max(
            tables.iter().map(|t| t.groups.len()).sum(),
            Ordering::SeqCst,
        );
        let mut workspace = self
            .workspace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (resident_partition, mut table) in tables.into_iter().enumerate() {
            self.partial_rows
                .fetch_add(table.groups.len(), Ordering::SeqCst);
            self.rehashes
                .fetch_add(table.groups.rehashes, Ordering::SeqCst);
            if table.groups.is_empty() {
                continue;
            }
            // Keep the existing 16 resident tables/chunk charges. Increasing
            // their count would waste one minimum allocation chunk per bucket.
            // In-place routing needs no uncharged fan-out-sized record buffers.
            let fanout = if depth == 0 { self.initial_fanout } else { 16 };
            let bucket = |key: &[Atom]| (self.hash)(key, depth) & (fanout as u64 - 1);
            if fanout != 16 {
                table
                    .groups
                    .entries
                    .sort_unstable_by_key(|(key, _)| bucket(key));
            }
            let mut partition = usize::try_from(
                table
                    .groups
                    .entries
                    .first()
                    .map_or(resident_partition as u64, |(key, _)| bucket(key)),
            )
            .map_err(|_| invalid())?;
            let mut handle = files[partition].open()?;
            let mut keys: Vec<Vec<u8>> = Vec::new();
            let mut states: Vec<Vec<u8>> = Vec::new();
            let mut hashes = Vec::new();
            let mut bytes = 0;
            for (row, (key, state)) in table.groups.into_iter().enumerate() {
                if row.is_multiple_of(CANCEL_ROWS)
                    && self
                        .cancel
                        .as_ref()
                        .is_some_and(nata_parquet::ScanCancel::is_cancelled)
                {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "aggregate spill cancelled",
                    ));
                }
                let size = replay::size(&key, &state)?;
                let target = usize::try_from(bucket(&key)).map_err(|_| invalid())?;
                if !keys.is_empty()
                    && (target != partition || bytes + size > 24 << 10 || keys.len() == 128)
                {
                    flush(
                        &mut workspace,
                        &mut handle,
                        &keys,
                        &states,
                        &hashes,
                        partition,
                        depth,
                    )?;
                    keys.clear();
                    states.clear();
                    hashes.clear();
                    bytes = 0;
                }
                if target != partition {
                    drop(handle);
                    partition = target;
                    handle = files[partition].open()?;
                }
                let (k, s) = replay::encode(&key, &state, a)?;
                hashes.push((self.hash)(&key, depth));
                keys.push(k);
                states.push(s);
                bytes += size;
            }
            if !keys.is_empty() {
                flush(
                    &mut workspace,
                    &mut handle,
                    &keys,
                    &states,
                    &hashes,
                    partition,
                    depth,
                )?;
            }
            // The source allocations were freed above; their grants release here.
            drop(table.grants);
        }
        Ok(())
    }
}
fn flush(
    w: &mut Workspace,
    h: &mut crate::spill::SpillHandle,
    keys: &[Vec<u8>],
    states: &[Vec<u8>],
    hashes: &[u64],
    partition: usize,
    depth: u8,
) -> io::Result<()> {
    let arrays: Vec<ArrayRef> = vec![
        Arc::new(UInt64Array::from_iter_values(hashes.iter().copied())),
        Arc::new(BinaryArray::from_iter_values(keys.iter())),
        Arc::new(BinaryArray::from_iter_values(states.iter())),
    ];
    let batch = RecordBatch::try_new(Arc::new(schema()), arrays)
        .map_err(|_| io::Error::other("spill encode schema"))?;
    w.write(
        h,
        &batch,
        u32::try_from(partition).map_err(|_| io::Error::other("spill partition"))?,
        depth,
    )
}
pub(super) fn record(
    frame: &SpillBatch,
    row: usize,
    a: &Aggregate,
    partition: usize,
    depth: u8,
    hash: fn(&[Atom], u8) -> u64,
    initial_fanout: usize,
) -> io::Result<(Vec<Atom>, Vec<State>)> {
    let b = frame.batch();
    let hashes = b
        .column(0)
        .as_any()
        .downcast_ref::<UInt64Array>()
        .ok_or_else(invalid)?;
    let keys = b
        .column(1)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .ok_or_else(invalid)?;
    let states = b
        .column(2)
        .as_any()
        .downcast_ref::<BinaryArray>()
        .ok_or_else(invalid)?;
    if row >= b.num_rows() || keys.is_null(row) || states.is_null(row) || hashes.is_null(row) {
        return Err(invalid());
    }
    let (key, state) = replay::decode(keys.value(row), states.value(row), a)?;
    let hash = hash(&key, depth);
    if hashes.value(row) != hash || !member(hash, partition, depth, initial_fanout) {
        return Err(invalid());
    }
    Ok((key, state))
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "typed spill partition mismatch")
}
impl Storage {
    pub fn repartition(
        &self,
        parent: Arc<SpillFile>,
        children: &[Arc<SpillFile>],
        a: &Aggregate,
        partition: usize,
        depth: u8,
        mut output: Workspace,
    ) -> io::Result<()> {
        let mut w = self
            .workspace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut input = parent.open()?;
        let mut keys: Vec<Vec<Vec<u8>>> = (0..16).map(|_| Vec::new()).collect();
        let mut states: Vec<Vec<Vec<u8>>> = (0..16).map(|_| Vec::new()).collect();
        let mut hashes: Vec<Vec<u64>> = (0..16).map(|_| Vec::new()).collect();
        let mut bytes = 0;
        while let Some(frame) = w.read(
            &mut input,
            u32::try_from(partition).map_err(|_| invalid())?,
            depth,
        )? {
            if self
                .cancel
                .as_ref()
                .is_some_and(nata_parquet::ScanCancel::is_cancelled)
            {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "aggregate repartition cancelled",
                ));
            }
            for row in 0..frame.batch().num_rows() {
                let (key, state) = record(
                    &frame,
                    row,
                    a,
                    partition,
                    depth,
                    self.hash,
                    self.initial_fanout,
                )?;
                let size = replay::size(&key, &state)?;
                if bytes + size > 24 << 10 {
                    for p in 0..16 {
                        if !keys[p].is_empty() {
                            flush(
                                &mut output,
                                &mut children[p].open()?,
                                &keys[p],
                                &states[p],
                                &hashes[p],
                                p,
                                depth + 1,
                            )?;
                            keys[p].clear();
                            states[p].clear();
                            hashes[p].clear();
                        }
                    }
                    bytes = 0;
                }
                let h = (self.hash)(&key, depth + 1);
                let p = usize::try_from(h & 15).map_err(|_| invalid())?;
                if keys[p].len() == 128 {
                    flush(
                        &mut output,
                        &mut children[p].open()?,
                        &keys[p],
                        &states[p],
                        &hashes[p],
                        p,
                        depth + 1,
                    )?;
                    bytes = bytes.saturating_sub(
                        keys[p]
                            .iter()
                            .zip(&states[p])
                            .map(|(k, s)| k.len() + s.len())
                            .sum::<usize>(),
                    );
                    keys[p].clear();
                    states[p].clear();
                    hashes[p].clear();
                }
                let (k, s) = replay::encode(&key, &state, a)?;
                keys[p].push(k);
                states[p].push(s);
                hashes[p].push(h);
                bytes += size;
            }
        }
        for p in 0..16 {
            if !keys[p].is_empty() {
                flush(
                    &mut output,
                    &mut children[p].open()?,
                    &keys[p],
                    &states[p],
                    &hashes[p],
                    p,
                    depth + 1,
                )?;
            }
        }
        Ok(())
    }
}

/// Fan-out comes only from the query, never from the spill envelope.
fn member(hash: u64, partition: usize, depth: u8, initial_fanout: usize) -> bool {
    if !(16..=256).contains(&initial_fanout) || !initial_fanout.is_power_of_two() || depth > 8 {
        return false;
    }
    let fanout = if depth == 0 { initial_fanout } else { 16 };
    partition < fanout && hash & (fanout as u64 - 1) == partition as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn membership_uses_query_fanout_and_recursive_sixteen() {
        for f in [16, 32, 64, 128, 256] {
            for hash in [0, 15, 16, 31, 255, 256, u64::MAX] {
                for depth in 0..=8 {
                    let mask = if depth == 0 { f - 1 } else { 15 };
                    let p = usize::try_from(hash & mask as u64).expect("bounded");
                    assert!(member(hash, p, depth, f));
                    assert!(!member(hash, p ^ 1, depth, f));
                    assert!(!member(hash, p + mask + 1, depth, f));
                }
            }
        }
        for f in [0, 1, 15, 17, 255, 257, usize::MAX] {
            assert!(!member(0, 0, 0, f));
        }
        assert!(!member(0, 0, 9, 256));
    }
    #[test]
    fn hostile_valid_ipc_cannot_change_query_partition_membership() {
        use nata_sched::memory::MemoryManager;
        use nata_sql::{Pos, bind::AggregateExpr, types::SqlType};
        use std::io::{Seek, SeekFrom};
        let base = std::env::temp_dir().join(format!("nata-membership-{}", std::process::id()));
        std::fs::create_dir(&base).expect("base");
        let m = MemoryManager::new(4 << 20, 4096, 1).expect("memory");
        {
            let registry = Registry::new(&base, 4 << 20, 32, Arc::clone(&m)).expect("registry");
            let file = registry.create().expect("file");
            let mut h = file.open().expect("handle");
            let mut w = Workspace::new(&m).expect("workspace");
            let a = Aggregate {
                keys: vec![],
                aggregates: vec![AggregateExpr {
                    name: "count".into(),
                    arg: None,
                    ty: SqlType::BigInt,
                    pos: Pos::NONE,
                }],
                having: None,
            };
            let (k, s) = replay::encode(&[], &[State::Count(1)], &a).expect("encode");
            // Correct CRC, schema, key/state tags and declared partition: the
            // stored hash is still untrusted and must be recomputed from keys.
            for stored in [31, 30] {
                h.seek(SeekFrom::End(0)).expect("end");
                flush(
                    &mut w,
                    &mut h,
                    std::slice::from_ref(&k),
                    std::slice::from_ref(&s),
                    &[stored],
                    31,
                    0,
                )
                .expect("write");
            }
            h.rewind().expect("rewind");
            let frame = w.read(&mut h, 31, 0).expect("IPC").expect("frame");
            assert!(record(&frame, 0, &a, 31, 0, |_, _| 31, 256).is_ok());
            assert!(record(&frame, 0, &a, 31, 0, |_, _| 31, 16).is_err());
            assert!(record(&frame, 0, &a, 15, 0, |_, _| 31, 256).is_err());
            assert!(record(&frame, 1, &a, 31, 0, |_, _| 31, 256).is_err());
            drop(frame);
            let frame = w.read(&mut h, 31, 0).expect("IPC").expect("frame");
            assert!(record(&frame, 0, &a, 31, 0, |_, _| 31, 256).is_err());
        }
        assert_eq!(m.snapshot().held, 0);
        assert_eq!(std::fs::read_dir(&base).expect("base").count(), 0);
        std::fs::remove_dir(base).expect("cleanup");
    }
}
