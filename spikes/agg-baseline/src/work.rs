//! Same hashbrown::HashTable<group-id> + owned key/state arena pattern as
//! hash-ops/agg.rs, extended only in this throwaway harness. No custom probing.
use crate::spill::Key;
use crossbeam_deque::{Steal, Stealer, Worker};
use hashbrown::HashTable;
use std::{
    collections::{HashMap, VecDeque},
    hash::{BuildHasherDefault, Hash, Hasher},
    sync::{Arc, Barrier, Mutex},
    time::Instant,
};
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Agg {
    count: u64,
    nonnull: u64,
    sum: i128,
    min: Option<i128>,
    max: Option<i128>,
}
impl Agg {
    fn update(&mut self, v: Option<i128>) {
        self.count += 1;
        if let Some(v) = v {
            self.nonnull += 1;
            self.sum += v;
            self.min = Some(self.min.map_or(v, |old| old.min(v)));
            self.max = Some(self.max.map_or(v, |old| old.max(v)));
        }
    }
}
#[derive(Default)]
pub struct IdentityHasher(u64);
impl Hasher for IdentityHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, _: &[u8]) {
        panic!("hashed keys use write_u64");
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = v;
    }
}
#[derive(Clone, PartialEq, Eq)]
struct HashedKey {
    key: Key,
    hash: u64,
}
impl Hash for HashedKey {
    fn hash<H: Hasher>(&self, h: &mut H) {
        h.write_u64(self.hash);
    }
}
#[derive(Clone)]
pub struct Row {
    key: HashedKey,
    value: Option<i128>,
}
pub fn rows(n: usize, collision: bool) -> Vec<Row> {
    (0..n)
        .map(|i| {
            let group = (i * 97 + i / 128) % 128;
            let key = Key {
                text: (group % 11 != 0).then(|| format!("group{}λ", group % 19)),
                decimal: (group % 7 != 0).then_some(10i128.pow(36) + group as i128),
            };
            // Stable small reference hash over VALUES, including null markers.
            let mut hash = 0xcbf29ce484222325u64;
            for b in format!("{key:?}").bytes() {
                hash = (hash ^ b as u64).wrapping_mul(0x100000001b3);
            }
            Row {
                key: HashedKey {
                    key,
                    hash: if collision { 0 } else { hash },
                },
                value: (i % 5 != 0).then_some(i as i128 % 1001 - 500),
            }
        })
        .collect()
}
#[derive(Default)]
pub struct ReusedTable {
    table: HashTable<usize>,
    keys: Vec<HashedKey>,
    states: Vec<Agg>,
}
impl ReusedTable {
    pub fn aggregate(&mut self, rows: &[Row]) -> Vec<(Key, Agg)> {
        self.table.clear();
        self.keys.clear();
        self.states.clear();
        for r in rows {
            let g = match self.table.find(r.key.hash, |&g| self.keys[g] == r.key) {
                Some(&g) => g,
                None => {
                    let g = self.keys.len();
                    self.keys.push(r.key.clone());
                    self.states.push(Agg::default());
                    self.table
                        .insert_unique(r.key.hash, g, |&id| self.keys[id].hash);
                    g
                }
            };
            self.states[g].update(r.value);
        }
        let mut out: Vec<_> = self
            .keys
            .iter()
            .zip(&self.states)
            .map(|(k, s)| (k.key.clone(), s.clone()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
    pub fn capacity(&self) -> usize {
        self.table.capacity()
    }
}
pub fn std_control(rows: &[Row]) -> Vec<(Key, Agg)> {
    let mut table: HashMap<HashedKey, Agg, BuildHasherDefault<IdentityHasher>> = HashMap::default();
    for r in rows {
        table.entry(r.key.clone()).or_default().update(r.value);
    }
    let mut out: Vec<_> = table.into_iter().map(|(k, s)| (k.key, s)).collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}
#[derive(Clone, Copy, Debug)]
pub enum Queue {
    Fifo,
    Deque,
}
#[derive(Default, Debug)]
pub struct Counters {
    pub claims: usize,
    pub local: usize,
    pub steals: usize,
    pub retries: usize,
    pub probes: usize,
    pub pins: usize,
}
impl Counters {
    fn add(&mut self, c: &Self) {
        self.claims += c.claims;
        self.local += c.local;
        self.steals += c.steals;
        self.retries += c.retries;
        self.probes += c.probes;
        self.pins += c.pins;
    }
}
/// Both queue arms implement the same unordered `claim()` seam. No result order
/// requirement; we restore task ids solely to compare exact outputs afterwards.
enum Source<'a> {
    Fifo(&'a Mutex<VecDeque<usize>>),
    Deque(Worker<usize>, &'a [Stealer<usize>]),
}
impl Source<'_> {
    fn claim(&self, c: &mut Counters) -> Option<usize> {
        match self {
            Self::Fifo(q) => q.lock().unwrap().pop_front(),
            Self::Deque(local, others) => {
                if let Some(id) = local.pop() {
                    c.local += 1;
                    return Some(id);
                }
                for s in *others {
                    loop {
                        c.probes += 1;
                        match s.steal() {
                            Steal::Success(id) => {
                                c.steals += 1;
                                return Some(id);
                            }
                            Steal::Empty => break,
                            Steal::Retry => c.retries += 1,
                        }
                    }
                }
                None
            }
        }
    }
}
pub fn schedule(
    queue: Queue,
    affinity: bool,
    threads: usize,
    tasks: usize,
    input: &[Row],
) -> (f64, Counters) {
    let ids = core_affinity::get_core_ids().expect("allowed CPU ids");
    assert!(ids.len() >= threads);
    let fifo = Mutex::new((0..tasks).collect());
    let locals: Vec<_> = (0..threads).map(|_| Worker::new_lifo()).collect();
    let stealers: Vec<_> = locals.iter().map(Worker::stealer).collect();
    for id in 0..tasks {
        locals[id % threads].push(id);
    }
    let barrier = Arc::new(Barrier::new(threads + 1));
    let oracle = std_control(input);
    let mut outputs = Vec::new();
    let mut total = Counters::default();
    let duration = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for (worker, local) in locals.into_iter().enumerate() {
            let source = match queue {
                Queue::Fifo => Source::Fifo(&fifo),
                Queue::Deque => Source::Deque(local, &stealers),
            };
            let ready = barrier.clone();
            let ids = &ids;
            handles.push(scope.spawn(move || {
                let mut c = Counters::default();
                if affinity {
                    assert!(
                        core_affinity::set_for_current(ids[worker]),
                        "affinity failed"
                    );
                    c.pins = 1;
                }
                let mut table = ReusedTable::default();
                let mut out = Vec::new();
                ready.wait();
                while let Some(id) = source.claim(&mut c) {
                    c.claims += 1;
                    out.push((id, table.aggregate(input)));
                }
                (c, out)
            }));
        }
        let start = Instant::now();
        barrier.wait();
        for h in handles {
            let (c, out) = h.join().unwrap();
            total.add(&c);
            outputs.extend(out);
        }
        start.elapsed().as_secs_f64() * 1000.
    });
    outputs.sort_by_key(|o| o.0);
    assert_eq!(outputs.len(), tasks);
    assert_eq!(total.claims, tasks);
    if matches!(queue, Queue::Deque) {
        assert_eq!(total.local + total.steals, tasks);
    }
    assert_eq!(total.pins, if affinity { threads } else { 0 });
    for (expect, (id, got)) in outputs.into_iter().enumerate() {
        assert_eq!(id, expect);
        assert_eq!(got, oracle);
    }
    (duration, total)
}
