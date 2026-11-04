//! Pieces shared by the build-or-buy trials (`trials/madsim-trial`, `trials/turmoil-trial`): the state that
//! lives outside a simulated node (the object store, the oracle gauges, the trace hash), behind `Send + Sync`
//! so that it can be captured by whatever node closure the candidate framework wants.
//! None of this is used by the harness itself (`sim.rs` has its own, single-threaded versions).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering::SeqCst};
use std::sync::{Arc, Mutex};

use crate::env::{NodeId, StoreError};
use crate::trace::fnv;

#[derive(Default)]
pub struct Shared {
    objects: Mutex<BTreeMap<String, Vec<u8>>>,
    gauges: Mutex<BTreeMap<(NodeId, &'static str), i64>>,
    violations: Mutex<Vec<String>>,
    pub buffers: Arc<AtomicI64>,
    trace: Mutex<u64>,
    pub events: AtomicU64,
    pub store_latency_ns: (u64, u64),
    pub store_fail_ppm: u32,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    pub fn new(store_latency_ns: (u64, u64), store_fail_ppm: u32) -> Arc<Shared> {
        Arc::new(Shared {
            trace: Mutex::new(0xcbf2_9ce4_8422_2325),
            store_latency_ns,
            store_fail_ppm,
            ..Shared::default()
        })
    }

    pub fn put_object(&self, key: &str, v: Vec<u8>) {
        lock(&self.objects).insert(key.to_string(), v);
    }

    pub fn object(&self, key: &str) -> Option<Vec<u8>> {
        lock(&self.objects).get(key).cloned()
    }

    pub fn get_now(&self, key: &str) -> Result<Vec<u8>, StoreError> {
        self.object(key).ok_or(StoreError::NotFound)
    }

    pub fn gauge_set(&self, node: NodeId, name: &'static str, v: i64) {
        lock(&self.gauges).insert((node, name), v);
    }

    pub fn gauge(&self, node: NodeId, name: &'static str) -> i64 {
        lock(&self.gauges).get(&(node, name)).copied().unwrap_or(0)
    }

    pub fn clear_gauges(&self, node: NodeId) {
        lock(&self.gauges).retain(|(n, _), _| *n != node);
    }

    pub fn violation(&self, msg: String) {
        lock(&self.violations).push(msg);
    }

    pub fn violations(&self) -> Vec<String> {
        lock(&self.violations).clone()
    }

    /// Fold an event into the trace hash (same byte layout as `trace::Trace::ev`, so the numbers are comparable
    /// in kind, though not equal: the executors differ).
    pub fn event(&self, now: u64, node: NodeId, kind: &'static str, a: u64, b: u64) {
        let mut h = lock(&self.trace);
        let mut x = *h;
        let mut put = |bytes: &[u8]| {
            for &c in bytes {
                x ^= c as u64;
                x = x.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        put(&now.to_le_bytes());
        put(&node.to_le_bytes());
        put(&fnv(kind.as_bytes()).to_le_bytes());
        put(&a.to_le_bytes());
        put(&b.to_le_bytes());
        *h = x;
        self.events.fetch_add(1, SeqCst);
    }

    pub fn trace_hash(&self) -> u64 {
        *lock(&self.trace)
    }
}
