//! Inner hash join.
//!
//! Build side: a pipeline breaker (`JoinBuild` sink). Each worker keeps its filtered rows (keys, payload and
//! hashes) in worker-local chunks, with no shared state. After the barrier the chunks are concatenated and
//! the hash table is built in PARTS independent partitions (by hash bits), in parallel, lock-free; this
//! avoids both a serial merge of per-thread tables and a shared table with locks or atomics.
//! A partition maps each distinct key to the head of a chain of the build rows with that key, so duplicate
//! build keys fan out. Probe side: `Probe` is a streaming operator; it reads the shared table (immutable),
//! and emits output batches of at most `cap` rows (probe columns gathered by the matching probe row,
//! build payload gathered by the matching build row).
//! NULL keys never match: build rows with a NULL key are dropped, probe rows with a NULL key find nothing.
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use arrow::array::{Array, ArrayRef, RecordBatch, RecordBatchOptions, UInt32Array};
use arrow::compute::{concat, take};
use arrow::datatypes::{Field, Schema};
use hashbrown::HashTable;

use crate::engine::{Batch, Op, Sink};
use crate::kernels::*;
use crate::keys::*;
use crate::source::BATCH_ROWS;

const END: u32 = u32::MAX;

fn arrow_err(e: arrow::error::ArrowError) -> EngineError {
    EngineError(format!("arrow: {e}"))
}

struct BuildChunk {
    keys: Vec<ArrayRef>,
    payload: Vec<ArrayRef>,
    hashes: Vec<u64>,
}

/// The build-side sink of one worker.
pub struct JoinBuild {
    key_names: Vec<&'static str>,
    payload_names: Vec<&'static str>,
    mask: u64,
    chunks: Vec<BuildChunk>,
    hv: Vec<u64>,
}

impl JoinBuild {
    pub fn new(key_names: Vec<&'static str>, payload_names: Vec<&'static str>) -> Self {
        JoinBuild { key_names, payload_names, mask: u64::MAX, chunks: Vec::new(), hv: Vec::new() }
    }

    pub fn with_hash_mask(mut self, mask: u64) -> Self {
        self.mask = mask;
        self
    }
}

impl Sink for JoinBuild {
    fn consume(&mut self, b: &Batch) -> R<()> {
        if b.live() == 0 {
            return Ok(());
        }
        let kcols: Vec<KCol> = self.key_names.iter().map(|k| b.key(k)).collect::<R<_>>()?;
        // Keep the live rows whose keys are all non-NULL.
        let keep = filter_with(b.rows, b.sel.as_deref(), |i| !kcols.iter().any(|c| c.is_null(i)));
        if keep.is_empty() {
            return Ok(());
        }
        let identity = b.sel.is_none() && keep.len() == b.rows;
        let idx = UInt32Array::from(keep);
        let gather = |name: &str| -> R<ArrayRef> {
            let a = &b.cols[b.idx(name)];
            if identity { Ok(a.clone()) } else { take(a.as_ref(), &idx, None).map_err(arrow_err) }
        };
        let keys: Vec<ArrayRef> = self.key_names.iter().map(|k| gather(k)).collect::<R<_>>()?;
        let payload: Vec<ArrayRef> = self.payload_names.iter().map(|k| gather(k)).collect::<R<_>>()?;
        let kc: Vec<KCol> = keys.iter().map(|a| KCol::from_array(a.as_ref())).collect::<R<_>>()?;
        hash_cols(&kc, idx.len(), None, self.mask, &mut self.hv);
        let hashes = std::mem::take(&mut self.hv);
        self.chunks.push(BuildChunk { keys, payload, hashes });
        Ok(())
    }
}

struct Part {
    /// Global build row ids of this partition, in build order; the table and `next` index into this list.
    rows: Vec<u32>,
    /// One entry per distinct key: the local index of the head of its chain.
    table: HashTable<u32>,
    next: Vec<u32>,
    /// Key comparisons made while building this partition (the engine's own measure of hash collisions).
    compares: u64,
}

/// Counters of the built table, for the hash-quality tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildStats {
    pub rows: usize,
    /// Rows in the fullest of the PARTS partitions (a hash that ignores key columns makes this skewed).
    pub max_partition_rows: usize,
    /// Key-equality comparisons made while inserting all rows (about `rows` for a good hash; `rows^2 / 2` when every row collides).
    pub key_compares: u64,
}

pub struct JoinTable {
    n: usize,
    mask: u64,
    keys: Vec<ArrayRef>,
    payload: Vec<ArrayRef>,
    payload_names: Vec<&'static str>,
    parts: Vec<Part>,
}

impl JoinTable {
    pub fn rows(&self) -> usize {
        self.n
    }

    pub fn stats(&self) -> BuildStats {
        BuildStats {
            rows: self.n,
            max_partition_rows: self.parts.iter().map(|p| p.rows.len()).max().unwrap_or(0),
            key_compares: self.parts.iter().map(|p| p.compares).sum(),
        }
    }

    fn key_cols(&self) -> R<Vec<KCol<'_>>> {
        self.keys.iter().map(|a| KCol::from_array(a.as_ref())).collect()
    }
}

fn concat_col(chunks: &[BuildChunk], pick: impl Fn(&BuildChunk) -> &ArrayRef) -> R<ArrayRef> {
    let refs: Vec<&dyn Array> = chunks.iter().map(|c| pick(c).as_ref()).collect();
    concat(&refs).map_err(arrow_err)
}

/// The barrier: concatenate the workers' chunks and build the partitioned hash table.
pub fn build_table(sinks: Vec<JoinBuild>, threads: usize) -> R<JoinTable> {
    let (payload_names, mask) = match sinks.first() {
        Some(s) => (s.payload_names.clone(), s.mask),
        None => (Vec::new(), u64::MAX),
    };
    let chunks: Vec<BuildChunk> = sinks.into_iter().flat_map(|s| s.chunks).collect();
    let n: usize = chunks.iter().map(|c| c.hashes.len()).sum();
    if n == 0 {
        return Ok(JoinTable { n: 0, mask, keys: Vec::new(), payload: Vec::new(), payload_names, parts: Vec::new() });
    }
    if n >= u32::MAX as usize {
        return Err(EngineError("not supported yet: more than 2^32 - 1 build rows".into()));
    }
    let nk = chunks[0].keys.len();
    let keys: Vec<ArrayRef> = (0..nk).map(|j| concat_col(&chunks, |c| &c.keys[j])).collect::<R<_>>()?;
    let payload: Vec<ArrayRef> = (0..payload_names.len()).map(|j| concat_col(&chunks, |c| &c.payload[j])).collect::<R<_>>()?;
    let mut hashes = Vec::with_capacity(n);
    for c in &chunks {
        hashes.extend_from_slice(&c.hashes);
    }
    drop(chunks);

    // Scatter the row ids by partition: each worker handles a contiguous range of rows.
    let t = threads.clamp(1, n);
    let step = n.div_ceil(t);
    let ranges: Vec<(usize, usize)> = (0..t).map(|i| (i * step, ((i + 1) * step).min(n))).filter(|(a, b)| a < b).collect();
    let scattered: Vec<Vec<Vec<u32>>> = std::thread::scope(|s| {
        let hs: Vec<_> = ranges
            .iter()
            .map(|&(a, b)| {
                let hashes = &hashes;
                s.spawn(move || {
                    let mut p: Vec<Vec<u32>> = vec![Vec::new(); PARTS];
                    for r in a..b {
                        p[part_of(hashes[r])].push(r as u32);
                    }
                    p
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().expect("scatter thread")).collect()
    });

    let kc = keys.iter().map(|a| KCol::from_array(a.as_ref())).collect::<R<Vec<_>>>()?;
    let next_part = AtomicUsize::new(0);
    let built: Vec<Vec<(usize, Part)>> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..t)
            .map(|_| {
                s.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        let p = next_part.fetch_add(1, Relaxed);
                        if p >= PARTS {
                            break;
                        }
                        let rows: Vec<u32> = scattered.iter().flat_map(|sc| sc[p].iter().copied()).collect();
                        let mut table: HashTable<u32> = HashTable::with_capacity(rows.len());
                        let mut next = vec![END; rows.len()];
                        let mut compares = 0u64;
                        for l in 0..rows.len() {
                            let g = rows[l] as usize;
                            let h = hashes[g];
                            let found = table.find_mut(h, |&hd| {
                                compares += 1;
                                kc.iter().all(|c| c.eq_at(rows[hd as usize] as usize, c, g))
                            });
                            match found {
                                Some(hd) => {
                                    next[l] = *hd;
                                    *hd = l as u32;
                                }
                                None => {
                                    table.insert_unique(h, l as u32, |&x| hashes[rows[x as usize] as usize]);
                                }
                            }
                        }
                        mine.push((p, Part { rows, table, next, compares }));
                    }
                    mine
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().expect("build thread")).collect()
    });
    let mut parts: Vec<Option<Part>> = (0..PARTS).map(|_| None).collect();
    for (p, part) in built.into_iter().flatten() {
        parts[p] = Some(part);
    }
    Ok(JoinTable { n, mask, keys, payload, payload_names, parts: parts.into_iter().map(|p| p.expect("every partition is built")).collect() })
}

/// Probe operator: streams probe batches against a built table.
pub struct Probe {
    pub table: Arc<JoinTable>,
    pub key_names: Vec<&'static str>,
    /// Probe columns to carry into the output (followed by the build payload columns).
    pub probe_out: Vec<&'static str>,
    /// Maximum rows per output batch.
    pub cap: usize,
    hv: Vec<u64>,
}

impl Probe {
    pub fn new(table: Arc<JoinTable>, key_names: Vec<&'static str>, probe_out: Vec<&'static str>) -> Self {
        Probe { table, key_names, probe_out, cap: BATCH_ROWS, hv: Vec::new() }
    }

    pub fn with_cap(mut self, cap: usize) -> Self {
        assert!(cap > 0);
        self.cap = cap;
        self
    }

    fn emit(&self, b: &Batch, pr: &[u32], br: &[u32]) -> R<Batch> {
        let (pidx, bidx) = (UInt32Array::from(pr.to_vec()), UInt32Array::from(br.to_vec()));
        let mut fields = Vec::new();
        let mut arrays: Vec<ArrayRef> = Vec::new();
        for name in &self.probe_out {
            let a = take(b.cols[b.idx(name)].as_ref(), &pidx, None).map_err(arrow_err)?;
            fields.push(Field::new(*name, a.data_type().clone(), true));
            arrays.push(a);
        }
        for (name, col) in self.table.payload_names.iter().zip(&self.table.payload) {
            let a = take(col.as_ref(), &bidx, None).map_err(arrow_err)?;
            fields.push(Field::new(*name, a.data_type().clone(), true));
            arrays.push(a);
        }
        let opts = RecordBatchOptions::new().with_row_count(Some(pr.len()));
        let rb = RecordBatch::try_new_with_options(Arc::new(Schema::new(fields)), arrays, &opts).map_err(arrow_err)?;
        Ok(Batch::new(&rb))
    }
}

impl Op for Probe {
    fn exec(&mut self, b: Batch, out: &mut dyn FnMut(Batch) -> R<()>) -> R<()> {
        let table = self.table.clone();
        let n = b.live();
        if n == 0 || table.n == 0 {
            return Ok(());
        }
        let (rows, sel) = (b.rows, b.sel.as_deref());
        let pcols: Vec<KCol> = self.key_names.iter().map(|k| b.key(k)).collect::<R<_>>()?;
        let bcols = table.key_cols()?;
        if pcols.len() != bcols.len() || pcols.iter().zip(&bcols).any(|(p, q)| !p.same_kind(q)) {
            return Err(EngineError("not supported yet: join keys of different types".into()));
        }
        let mut hv = std::mem::take(&mut self.hv);
        hash_cols(&pcols, rows, sel, table.mask, &mut hv);
        let has_null = pcols.iter().any(|c| c.nulls.any());
        let (mut pr, mut br): (Vec<u32>, Vec<u32>) = (Vec::new(), Vec::new());
        for k in 0..n {
            let row = sel.map_or(k, |s| s[k] as usize);
            if has_null && pcols.iter().any(|c| c.is_null(row)) {
                continue;
            }
            let h = hv[k];
            let part = &table.parts[part_of(h)];
            let found = part.table.find(h, |&l| {
                let brow = part.rows[l as usize] as usize;
                bcols.iter().zip(&pcols).all(|(bc, pc)| bc.eq_at(brow, pc, row))
            });
            let Some(&head) = found else { continue };
            let mut l = head;
            while l != END {
                pr.push(row as u32);
                br.push(part.rows[l as usize]);
                if pr.len() >= self.cap {
                    out(self.emit(&b, &pr, &br)?)?;
                    pr.clear();
                    br.clear();
                }
                l = part.next[l as usize];
            }
        }
        if !pr.is_empty() {
            out(self.emit(&b, &pr, &br)?)?;
        }
        self.hv = hv;
        Ok(())
    }
}
