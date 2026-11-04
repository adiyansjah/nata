//! General hash aggregate: GROUP BY any number of typed key columns (i64, i32/Date32, dictionary string,
//! plain string; NULL keys form their own group), COUNT(*) and exact i128 SUMs (with a non-NULL count each).
//!
//! One table per worker thread; at the end the tables are merged by hash partition, one partition per
//! task, so the merge is parallel and needs no locks. The result does not depend on the thread count.
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use hashbrown::HashTable;

use crate::engine::{Batch, Ref, Sink};
use crate::keys::*;
use crate::kernels::*;
use crate::out::*;

pub struct HashAgg {
    key_names: Vec<&'static str>,
    sums: Vec<Ref>,
    mask: u64,
    store: Option<KStore>,
    table: HashTable<u32>,
    hashes: Vec<u64>,
    /// Per group, `1 + K` slots: COUNT(*) then the K sums.
    state: Vec<i128>,
    /// Per group, K slots: NULL values skipped by each sum (so non-NULL count = count - nulls).
    nulls: Vec<u64>,
    groups: usize,
    hv: Vec<u64>,
    gids: Vec<u32>,
}

impl HashAgg {
    pub fn new(key_names: Vec<&'static str>, sums: Vec<Ref>) -> Self {
        HashAgg {
            key_names,
            sums,
            mask: u64::MAX,
            store: None,
            table: HashTable::new(),
            hashes: Vec::new(),
            state: Vec::new(),
            nulls: Vec::new(),
            groups: 0,
            hv: Vec::new(),
            gids: Vec::new(),
        }
    }

    /// Test hook: AND every hash with `mask` to force collisions.
    pub fn with_hash_mask(mut self, mask: u64) -> Self {
        self.mask = mask;
        self
    }

    pub fn groups(&self) -> usize {
        self.groups
    }

    fn empty_like(&self) -> HashAgg {
        HashAgg::new(self.key_names.clone(), self.sums.clone()).with_hash_mask(self.mask)
    }

    fn stride(&self) -> usize {
        1 + self.sums.len()
    }

    fn partitions(&self) -> Vec<Vec<u32>> {
        let mut p = vec![Vec::new(); PARTS];
        for (g, &h) in self.hashes.iter().enumerate() {
            p[part_of(h)].push(g as u32);
        }
        p
    }

    /// Add groups `gs` of `src` into this table.
    fn merge_partition(&mut self, src: &HashAgg, gs: &[u32]) -> R<()> {
        let Some(sstore) = &src.store else { return Ok(()) };
        let scols = sstore.as_cols();
        let store = self.store.get_or_insert_with(|| KStore::like(&scols));
        let (s, k) = (1 + self.sums.len(), self.sums.len());
        for &sg in gs {
            let sg = sg as usize;
            let h = src.hashes[sg];
            let g = match self.table.find(h, |&g| store.eq(g as usize, &scols, sg)) {
                Some(&g) => g as usize,
                None => {
                    let g = self.groups;
                    store.push(&scols, sg);
                    self.hashes.push(h);
                    self.groups += 1;
                    self.state.resize(self.groups * s, 0);
                    self.nulls.resize(self.groups * k, 0);
                    let hashes = &self.hashes;
                    self.table.insert_unique(h, g as u32, |&x| hashes[x as usize]);
                    g
                }
            };
            for j in 0..s {
                self.state[g * s + j] += src.state[sg * s + j];
            }
            for j in 0..k {
                self.nulls[g * k + j] += src.nulls[sg * k + j];
            }
        }
        if store.poisoned {
            return Err(EngineError("not supported yet: group keys larger than 2 GiB".into()));
        }
        Ok(())
    }

    /// Visit every group: key values, COUNT(*), and (sum, non-NULL count) per SUM.
    pub fn for_each_group(&self, f: &mut dyn FnMut(&[KRef], u64, &[(i128, u64)])) {
        let Some(store) = &self.store else { return };
        let (s, k) = (self.stride(), self.sums.len());
        let mut keys: Vec<KRef> = Vec::with_capacity(store.cols.len());
        let mut sums = vec![(0i128, 0u64); k];
        for g in 0..self.groups {
            keys.clear();
            for c in &store.cols {
                keys.push(if !c.nulls.is_empty() && c.nulls[g] != 0 {
                    KRef::Null
                } else {
                    match &c.v {
                        KV::I64(v) => KRef::I(v[g]),
                        KV::I32(v) => KRef::I(v[g] as i64),
                        KV::Str { offs, bytes } => KRef::S(&bytes[offs[g] as usize..offs[g + 1] as usize]),
                    }
                });
            }
            let count = self.state[g * s] as u64;
            for (j, sum) in sums.iter_mut().enumerate() {
                *sum = (self.state[g * s + 1 + j], count - self.nulls[g * k + j]);
            }
            f(&keys, count, &sums);
        }
    }
}

impl Sink for HashAgg {
    fn consume(&mut self, b: &Batch) -> R<()> {
        let n = b.live();
        if n == 0 {
            return Ok(());
        }
        let (rows, sel) = (b.rows, b.sel.as_deref());
        let cols: Vec<KCol> = self.key_names.iter().map(|k| b.key(k)).collect::<R<_>>()?;
        let store = self.store.get_or_insert_with(|| KStore::like(&cols));
        hash_cols(&cols, rows, sel, self.mask, &mut self.hv);
        self.gids.clear();
        for k in 0..n {
            let row = sel.map_or(k, |s| s[k] as usize);
            let h = self.hv[k];
            let g = match self.table.find(h, |&g| store.eq(g as usize, &cols, row)) {
                Some(&g) => g,
                None => {
                    if self.groups >= u32::MAX as usize {
                        return Err(EngineError("not supported yet: more than 2^32 groups".into()));
                    }
                    let g = self.groups as u32;
                    store.push(&cols, row);
                    self.hashes.push(h);
                    self.groups += 1;
                    let hashes = &self.hashes;
                    self.table.insert_unique(h, g, |&x| hashes[x as usize]);
                    g
                }
            };
            self.gids.push(g);
        }
        if store.poisoned {
            return Err(EngineError("not supported yet: group keys larger than 2 GiB".into()));
        }
        let (s, kk) = (1 + self.sums.len(), self.sums.len());
        self.state.resize(self.groups * s, 0);
        self.nulls.resize(self.groups * kk, 0);
        let gids = &self.gids;
        let state = &mut self.state;
        let mut k = 0;
        each(rows, sel, |_| {
            state[gids[k] as usize * s] += 1;
            k += 1;
        });
        for (j, r) in self.sums.iter().enumerate() {
            let (v, valid) = b.dec(r);
            let mut k = 0;
            if valid.is_all() {
                each(rows, sel, |i| {
                    state[gids[k] as usize * s + 1 + j] += v[i] as i128;
                    k += 1;
                });
            } else {
                let nulls = &mut self.nulls;
                each(rows, sel, |i| {
                    let ok = valid.ok(i);
                    let g = gids[k] as usize;
                    state[g * s + 1 + j] += (v[i] as i128) * (ok as i128);
                    nulls[g * kk + j] += !ok as u64;
                    k += 1;
                });
            }
        }
        Ok(())
    }
}

/// Merge the per-thread tables into disjoint hash partitions (parallel, lock-free). One table is returned as is.
pub fn merge_all(locals: Vec<HashAgg>, threads: usize) -> R<Vec<HashAgg>> {
    let locals: Vec<HashAgg> = locals.into_iter().filter(|l| l.groups > 0).collect();
    if locals.len() <= 1 {
        return Ok(locals);
    }
    let lists: Vec<Vec<Vec<u32>>> = std::thread::scope(|s| {
        let hs: Vec<_> = locals.iter().map(|l| s.spawn(|| l.partitions())).collect();
        hs.into_iter().map(|h| h.join().expect("partition thread")).collect()
    });
    let next = AtomicUsize::new(0);
    let parts: Vec<R<Vec<HashAgg>>> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..threads.max(1))
            .map(|_| {
                s.spawn(|| -> R<Vec<HashAgg>> {
                    let mut mine = Vec::new();
                    loop {
                        let p = next.fetch_add(1, Relaxed);
                        if p >= PARTS {
                            break;
                        }
                        let mut dst = locals[0].empty_like();
                        for (t, l) in locals.iter().enumerate() {
                            dst.merge_partition(l, &lists[t][p])?;
                        }
                        if dst.groups > 0 {
                            mine.push(dst);
                        }
                    }
                    Ok(mine)
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().expect("merge thread")).collect()
    });
    let mut out = Vec::new();
    for p in parts {
        out.extend(p?);
    }
    Ok(out)
}

impl Groups for Vec<HashAgg> {
    fn for_each_group(&self, f: &mut dyn FnMut(&[KRef], u64, &[(i128, u64)])) {
        for p in self {
            p.for_each_group(f);
        }
    }
}

pub fn to_output(parts: &[HashAgg], scales: Vec<u32>, show_count: bool) -> Output {
    let mut o = Output::new(scales, show_count);
    for p in parts {
        p.for_each_group(&mut |k, c, s| o.add(k, c, s));
    }
    o
}
