//! Sinks: the exact grouped / global sums of Q1 and Q6 (per-task local state, `combine` merges into the global
//! state, `finalize` publishes the result), a collecting sink and a `LIMIT` sink.
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};

use arrow::array::{Array, AsArray};
use arrow::datatypes::Int32Type;

use crate::batch::{Batch, Ref};
use crate::genops::engine::{Output, Q1Acc, Q6Acc};
use crate::genops::kernels::{EngineError, group_sums_dense, group_sums_masked, sum};
use crate::ops::{q, truncate};
use crate::proto::api::*;
use crate::proto::sched::lock;

const G: usize = 32; // distinct group-key values supported per run (as in generic-ops)

// ---------------------------------------------------------------- Q1 / Q6 aggregates

#[derive(Clone)]
pub enum Agg {
    /// GROUP BY two dictionary columns; SUMs over three decimal references; COUNT(*).
    Q1 { vals: [Ref; 3] },
    /// One global SUM over a decimal reference.
    Q6 { val: Ref },
}

/// The global (shared) half of the aggregate sink.
pub struct AggSink {
    agg: Agg,
    merged: Mutex<Option<Output>>,
    result: Mutex<Option<Output>>,
    /// How many tasks combined (to assert every task's partial state arrived).
    pub combined: AtomicUsize,
    pub finalized: AtomicUsize,
}

impl AggSink {
    pub fn new(agg: Agg) -> Arc<Self> {
        Arc::new(AggSink { agg, merged: Mutex::new(None), result: Mutex::new(None), combined: AtomicUsize::new(0), finalized: AtomicUsize::new(0) })
    }

    /// The finalized result (after the query ended successfully).
    pub fn result(&self) -> Option<Output> {
        lock(&self.result).clone()
    }
}

impl GlobalSink for Arc<AggSink> {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        match &self.agg {
            Agg::Q1 { vals } => Box::new(Q1Local::new(vals.clone(), Arc::clone(self))),
            Agg::Q6 { val } => Box::new(Q6Local { r: val.clone(), acc: Q6Acc::default(), global: Arc::clone(self) }),
        }
    }

    fn finalize(&self) -> Result<()> {
        let merged = lock(&self.merged).take();
        let out = merged.unwrap_or_else(|| match &self.agg {
            Agg::Q1 { .. } => Output::Q1(BTreeMap::new()),
            Agg::Q6 { .. } => Output::Q6(Q6Acc::default()),
        });
        *lock(&self.result) = Some(out);
        self.finalized.fetch_add(1, SeqCst);
        Ok(())
    }
}

fn merge_into(global: &AggSink, local: Output) {
    let mut m = lock(&global.merged);
    match m.as_mut() {
        None => *m = Some(local),
        Some(t) => t.merge(local),
    }
    global.combined.fetch_add(1, SeqCst);
}

pub struct Q6Local {
    r: Ref,
    acc: Q6Acc,
    global: Arc<AggSink>,
}

impl LocalSink for Q6Local {
    fn push(&mut self, input: &mut Batch, _waker: &Waker) -> Result<SinkResult> {
        let (s, n) = sum(input.dec(&self.r), input.rows(), input.sel());
        self.acc.sum += s;
        self.acc.nn += n;
        Ok(SinkResult::NeedMoreInput)
    }

    fn combine(self: Box<Self>) -> Result<()> {
        merge_into(&self.global, Output::Q6(self.acc));
        Ok(())
    }
}

/// `nn` (non-NULL count per SUM) is `count - nulls`, so the all-valid path never touches it.
pub struct Q1Local {
    interned: HashMap<Vec<u8>, u32>,
    names: Vec<String>,
    acc: Vec<[i128; 3]>,
    count: Vec<u64>,
    nulls: [Vec<u64>; 3],
    vals: [Ref; 3],
    global: Arc<AggSink>,
}

impl Q1Local {
    fn new(vals: [Ref; 3], global: Arc<AggSink>) -> Self {
        Q1Local {
            interned: HashMap::new(),
            names: Vec::new(),
            acc: vec![[0; 3]; G * G],
            count: vec![0; G * G],
            nulls: std::array::from_fn(|_| vec![0; G * G]),
            vals,
            global,
        }
    }

    fn intern(&mut self, bytes: &[u8]) -> u32 {
        if let Some(&id) = self.interned.get(bytes) {
            return id;
        }
        let id = self.names.len() as u32;
        assert!((id as usize) < G, "too many distinct group-key values: not supported yet");
        self.names.push(String::from_utf8_lossy(bytes).into_owned());
        self.interned.insert(bytes.to_vec(), id);
        id
    }

    fn output(&self) -> Output {
        let mut m = BTreeMap::new();
        for g in 0..G * G {
            if self.count[g] > 0 {
                let key = (self.names[g / G].clone(), self.names[g % G].clone());
                m.insert(
                    key,
                    Q1Acc {
                        sums: self.acc[g],
                        nn: [self.count[g] - self.nulls[0][g], self.count[g] - self.nulls[1][g], self.count[g] - self.nulls[2][g]],
                        count: self.count[g],
                    },
                );
            }
        }
        Output::Q1(m)
    }
}

impl LocalSink for Q1Local {
    fn push(&mut self, b: &mut Batch, _waker: &Waker) -> Result<SinkResult> {
        let (d0, d1) = (b.column("l_returnflag").as_dictionary::<Int32Type>(), b.column("l_linestatus").as_dictionary::<Int32Type>());
        if d0.keys().null_count() + d1.keys().null_count() > 0 {
            return q(Err(EngineError("NULL group key: not supported yet".into())));
        }
        let (v0, v1) = (d0.values().as_string::<i32>(), d1.values().as_string::<i32>());
        let t0: Vec<u32> = (0..v0.len()).map(|j| self.intern(v0.value(j).as_bytes())).collect();
        let t1: Vec<u32> = (0..v1.len()).map(|j| self.intern(v1.value(j).as_bytes())).collect();
        let (keys0, keys1) = (d0.keys().values(), d1.keys().values());
        let (rows, sel) = (b.rows(), b.sel());
        let gid = |i: usize| (t0[keys0[i] as usize] * G as u32 + t1[keys1[i] as usize]) as usize;
        let cols = [b.dec(&self.vals[0]), b.dec(&self.vals[1]), b.dec(&self.vals[2])];
        if cols.iter().all(|(_, v)| v.is_all()) {
            group_sums_dense(gid, [cols[0].0, cols[1].0, cols[2].0], rows, sel, &mut self.acc, &mut self.count);
        } else {
            group_sums_masked(gid, cols, rows, sel, &mut self.acc, &mut self.count, &mut self.nulls);
        }
        Ok(SinkResult::NeedMoreInput)
    }

    fn combine(self: Box<Self>) -> Result<()> {
        merge_into(&self.global, self.output());
        Ok(())
    }
}

// ---------------------------------------------------------------- collecting sinks

/// Collects the batches it is given (columns shared, selection kept: no copy). With `limit`, it is the `LIMIT`
/// sink: it claims rows from a shared counter, keeps exactly the claimed rows, and answers `Finished` when the
/// counter reaches zero. This is the second route of early finish (`SinkResult::Finished`).
pub struct CollectSink {
    limit: Option<Arc<AtomicUsize>>,
    out: Mutex<Vec<Batch>>,
    /// Fault injection: answer `Blocked` (input untouched) now and then; a helper thread wakes the task.
    block: Option<SinkChaos>,
}

#[derive(Clone, Debug)]
pub struct SinkChaos {
    pub seed: u64,
    pub block_per_mille: u32,
    pub max_block_us: u64,
}

impl CollectSink {
    pub fn new() -> Arc<Self> {
        Arc::new(CollectSink { limit: None, out: Mutex::new(Vec::new()), block: None })
    }

    pub fn with_limit(n: usize) -> Arc<Self> {
        Arc::new(CollectSink { limit: Some(Arc::new(AtomicUsize::new(n))), out: Mutex::new(Vec::new()), block: None })
    }

    pub fn blocking(mut self: Arc<Self>, chaos: SinkChaos) -> Arc<Self> {
        Arc::get_mut(&mut self).expect("not shared yet").block = Some(chaos);
        self
    }

    pub fn batches(&self) -> Vec<Batch> {
        lock(&self.out).clone()
    }
}

struct CollectLocal {
    limit: Option<Arc<AtomicUsize>>,
    mine: Vec<Batch>,
    global: Arc<CollectSink>,
    rng: u64,
}

impl GlobalSink for Arc<CollectSink> {
    fn make(&self, task: usize) -> Box<dyn LocalSink> {
        let rng = self.block.as_ref().map_or(0, |b| b.seed) ^ ((task as u64 + 1) << 40);
        Box::new(CollectLocal { limit: self.limit.clone(), mine: Vec::new(), global: Arc::clone(self), rng })
    }

    fn finalize(&self) -> Result<()> {
        Ok(())
    }
}

impl LocalSink for CollectLocal {
    fn push(&mut self, input: &mut Batch, waker: &Waker) -> Result<SinkResult> {
        if let Some(c) = self.global.block.clone() {
            self.rng = self.rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut x = self.rng;
            x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            x ^= x >> 31;
            if x % 1000 < u64::from(c.block_per_mille) {
                let us = (x >> 20) % (c.max_block_us + 1);
                let w = waker.clone();
                std::thread::spawn(move || {
                    if us > 0 {
                        std::thread::sleep(std::time::Duration::from_micros(us));
                    }
                    w.wake();
                });
                return Ok(SinkResult::Blocked);
            }
        }
        let Some(limit) = &self.limit else {
            if !input.is_empty() {
                self.mine.push(std::mem::take(input));
            }
            return Ok(SinkResult::NeedMoreInput);
        };
        loop {
            let remaining = limit.load(SeqCst);
            if remaining == 0 {
                return Ok(SinkResult::Finished);
            }
            let take = remaining.min(input.len());
            if limit.compare_exchange(remaining, remaining - take, SeqCst, SeqCst).is_ok() {
                if take > 0 {
                    self.mine.push(truncate(input, take));
                }
                return Ok(if take == remaining { SinkResult::Finished } else { SinkResult::NeedMoreInput });
            }
        }
    }

    fn combine(self: Box<Self>) -> Result<()> {
        lock(&self.global.out).extend(self.mine);
        Ok(())
    }
}
