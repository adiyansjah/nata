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
        for (partition, table) in tables.into_iter().enumerate() {
            self.partial_rows
                .fetch_add(table.groups.len(), Ordering::SeqCst);
            self.rehashes
                .fetch_add(table.groups.rehashes, Ordering::SeqCst);
            if table.groups.is_empty() {
                continue;
            }
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
                if !keys.is_empty() && (bytes + size > 24 << 10 || keys.len() == 128) {
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
    if hashes.value(row) != hash || usize::try_from(hash & 15).map_err(|_| invalid())? != partition
    {
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
                let (key, state) = record(&frame, row, a, partition, depth, self.hash)?;
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
