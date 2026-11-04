//! The event trace of a simulated run. Its FNV-1a hash is what "the same seed gives the same run" is checked
//! on: every message sent and delivered, every task poll, every protocol decision goes into it, with the
//! virtual time. Integers are hashed as little-endian bytes, so the hash does not depend on the machine.

use std::collections::BTreeMap;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

pub fn fnv(bytes: &[u8]) -> u64 {
    let mut h = FNV_OFFSET;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

pub struct Trace {
    hash: u64,
    pub events: u64,
    /// Number of events per kind (polls excluded): the coverage report of a sweep.
    pub stats: BTreeMap<&'static str, u64>,
    /// Kept only when asked (replay with `--log`).
    pub log: Option<Vec<String>>,
}

impl Trace {
    pub fn new(keep_log: bool) -> Self {
        Trace {
            hash: FNV_OFFSET,
            events: 0,
            stats: BTreeMap::new(),
            log: if keep_log { Some(Vec::new()) } else { None },
        }
    }

    fn put(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.hash ^= b as u64;
            self.hash = self.hash.wrapping_mul(FNV_PRIME);
        }
    }

    pub fn ev(&mut self, now: u64, node: u32, kind: &'static str, a: u64, b: u64) {
        self.put(&now.to_le_bytes());
        self.put(&node.to_le_bytes());
        self.put(kind.as_bytes());
        self.put(&[0xff]);
        self.put(&a.to_le_bytes());
        self.put(&b.to_le_bytes());
        self.events += 1;
        *self.stats.entry(kind).or_insert(0) += 1;
        if let Some(log) = &mut self.log {
            log.push(format!("{:>10}us n{node} {kind} {a} {b}", now / 1_000));
        }
    }

    /// A task poll: hashed (so the scheduler's order is part of the trace) but neither counted nor logged.
    pub fn poll(&mut self, now: u64, task: u64, node: u32, runq: usize) {
        self.put(&now.to_le_bytes());
        self.put(&[0xfe]);
        self.put(&task.to_le_bytes());
        self.put(&node.to_le_bytes());
        self.put(&(runq as u64).to_le_bytes());
        self.events += 1;
    }

    pub fn hash(&self) -> u64 {
        self.hash
    }
}
