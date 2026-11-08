//! Opt-in, query-owned attribution. Wall service sums are never CPU or critical
//! path estimates. Disabled handles do not allocate, read clocks or lock.
use crate::sync::{Arc, AtomicBool, AtomicU64, Mutex, Ordering};
use std::time::Instant;

#[cfg(not(nata_loom))]
static NEXT_QUERY: AtomicU64 = AtomicU64::new(1);
#[cfg(nata_loom)]
loom::lazy_static! { static ref NEXT_QUERY: AtomicU64 = AtomicU64::new(1); }
#[cfg(not(nata_loom))]
static HAS_STATS: AtomicBool = AtomicBool::new(false);
#[cfg(nata_loom)]
loom::lazy_static! { static ref HAS_STATS: AtomicBool = AtomicBool::new(false); }
const STAGES: [&str; 13] = [
    "scan_decode",
    "filter",
    "expression",
    "group_id",
    "state_update",
    "merge",
    "finalize",
    "output_sink",
    "spill_write",
    "spill_read",
    "teardown",
    "admission_wait",
    "bind_metadata",
];

#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Stage {
    ScanDecode,
    Filter,
    Expression,
    GroupId,
    StateUpdate,
    Merge,
    Finalize,
    OutputSink,
    SpillWrite,
    SpillRead,
    Teardown,
    AdmissionWait,
    BindMetadata,
}

#[derive(Debug)]
pub struct QueryStats {
    id: u64,
    cpu_start: Option<(u64, u64)>,
    started: Instant,
    next_pipeline: AtomicU64,
    dropped: AtomicU64,
    trace: bool,
    event_totals: [AtomicU64; 6],
    io_totals: [[AtomicU64; 3]; 3],
    events: Mutex<Vec<Event>>,
    counters: [AtomicU64; 8],
    levels: [[AtomicU64; 8]; 9],
    service_ns: [AtomicU64; 13],
    calls: [AtomicU64; 13],
    admission: Mutex<Option<(usize, usize, usize, usize)>>,
}
impl QueryStats {
    pub const fn available() -> bool {
        !cfg!(any(nata_stats_disabled, nata_loom))
    }
    pub fn new() -> Arc<Self> {
        Self::with_trace(std::env::var_os("NATA_QUERY_TRACE").is_some_and(|v| v == "1"))
    }
    pub fn with_trace(trace: bool) -> Arc<Self> {
        HAS_STATS.store(true, Ordering::Relaxed);
        Arc::new(Self {
            id: NEXT_QUERY.fetch_add(1, Ordering::Relaxed),
            cpu_start: process_cpu_ticks(),
            started: Instant::now(),
            next_pipeline: AtomicU64::new(1),
            dropped: AtomicU64::new(0),
            trace,
            event_totals: std::array::from_fn(|_| AtomicU64::new(0)),
            io_totals: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))),
            events: Mutex::new(Vec::with_capacity(if trace { 32768 } else { 0 })),
            counters: std::array::from_fn(|_| AtomicU64::new(0)),
            levels: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))),
            service_ns: std::array::from_fn(|_| AtomicU64::new(0)),
            calls: std::array::from_fn(|_| AtomicU64::new(0)),
            admission: Mutex::new(None),
        })
    }
    pub fn pipeline(&self) -> u64 {
        self.next_pipeline.fetch_add(1, Ordering::Relaxed)
    }
    pub fn event(&self, kind: &'static str, pipeline: u64, task: u64, wave: u64, value: u64) {
        let counter = match kind {
            "wave_start" => Some(0),
            "step_start" => Some(1),
            "yield" => Some(2),
            "blocked" => Some(3),
            "done" => Some(4),
            _ => None,
        };
        if let Some(i) = counter {
            self.event_totals[i].fetch_add(1, Ordering::Relaxed);
        }
        if matches!(kind, "yield" | "blocked" | "done") && value != u64::MAX {
            self.event_totals[5].fetch_add(value, Ordering::Relaxed);
        }
        if !self.trace {
            return;
        }
        let ns = nanos(self.started.elapsed());
        let mut events = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if events.len() == 32768 {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        events.push(Event {
            kind,
            ns,
            pipeline,
            task,
            wave,
            value,
        });
    }
    pub fn io(&self, kind: usize, queue_ns: u64, busy_ns: u64) {
        if let Some(totals) = self.io_totals.get(kind) {
            for (slot, value) in totals.iter().zip([1, queue_ns, busy_ns]) {
                slot.fetch_add(value, Ordering::Relaxed);
            }
        }
    }
    pub fn level(&self, depth: u8, field: usize, value: u64) {
        if let Some(slot) = self.levels.get(depth as usize).and_then(|v| v.get(field)) {
            slot.fetch_add(value, Ordering::Relaxed);
        }
    }
    pub fn count(&self, counter: Counter, value: u64) {
        self.counters[counter as usize].fetch_add(value, Ordering::Relaxed);
    }
    pub fn admission(&self, tasks: usize, free: usize, decode: usize, task: usize) {
        *self
            .admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((tasks, free, decode, task));
    }
    pub fn add(&self, stage: Stage, ns: u64) {
        self.service_ns[stage as usize].fetch_add(ns, Ordering::Relaxed);
        self.calls[stage as usize].fetch_add(1, Ordering::Relaxed);
    }
    /// A partial record is deliberately not a T6 certificate. Missing sources
    /// are explicit, rather than zero counters masquerading as measurements.
    pub fn json(&self) -> String {
        let cpu = self
            .cpu_start
            .zip(process_cpu_ticks())
            .map_or("null".into(), |(a, b)| {
                format!(
                    "{{\"user_ticks\":{},\"system_ticks\":{},\"scope\":\"process_delta\"}}",
                    b.0.saturating_sub(a.0),
                    b.1.saturating_sub(a.1)
                )
            });
        let event_totals = self
            .event_totals
            .iter()
            .map(|n| n.load(Ordering::Relaxed).to_string())
            .collect::<Vec<_>>()
            .join(",");
        let io_totals = self
            .io_totals
            .iter()
            .map(|v| {
                format!(
                    "[{}]",
                    v.iter()
                        .map(|n| n.load(Ordering::Relaxed).to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let trace = self.trace;
        let levels = self
            .levels
            .iter()
            .map(|fields| {
                format!(
                    "[{}]",
                    fields
                        .iter()
                        .map(|n| n.load(Ordering::Relaxed).to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let stages = STAGES
            .iter()
            .enumerate()
            .map(|(i, name)| {
                format!(
                    "\"{name}\":{{\"service_wall_ns\":{},\"calls\":{}}}",
                    self.service_ns[i].load(Ordering::Relaxed),
                    self.calls[i].load(Ordering::Relaxed)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let events = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|e| {
                format!(
                    "[\"{}\",{},{},{},{},{}]",
                    e.kind, e.ns, e.pipeline, e.task, e.wave, e.value
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let counters = self
            .counters
            .iter()
            .map(|c| c.load(Ordering::Relaxed).to_string())
            .collect::<Vec<_>>()
            .join(",");
        let admission = self.admission.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
            .map_or("null".into(), |(n, free, decode, task)| format!(
                "{{\"tasks\":{n},\"free_bytes\":{free},\"decode_bytes\":{decode},\"task_bytes\":{task}}}"
            ));
        format!(
            "{{\"version\":1,\"query_id\":{},\"execution_wall_ns\":{},\"process_cpu\":{cpu},\"complete\":false,\"dropped_events\":{},\"trace_enabled\":{trace},\"events\":[{events}],\"event_fields\":[\"kind\",\"ns\",\"pipeline\",\"task\",\"wave_or_level\",\"value\"],\"scheduler_fields\":[\"waves\",\"steps\",\"yields\",\"blocked\",\"done\",\"worker_cpu_ns\"],\"scheduler\":[{event_totals}],\"io_kinds\":[\"write\",\"read\",\"repartition\"],\"io_fields\":[\"jobs\",\"queue_wall_ns\",\"busy_wall_ns\"],\"io\":[{io_totals}],\"spill_levels\":[{levels}],\"spill_level_fields\":[\"write_bytes\",\"read_bytes\",\"write_frames\",\"read_frames\",\"opens\",\"tickets\",\"write_service_ns\",\"read_service_ns\"],\"counter_values\":[{counters}],\"counter_names\":[\"copied_bytes\",\"materialized_bytes\",\"col_materializations\",\"allocation_capacity_bytes\",\"allocation_sites\",\"grants\",\"grant_bytes\",\"releases\"],\"stages\":{{{stages}}},\"admission\":{admission},\"missing\":[\"complete_allocation_inventory\",\"all_worker_cpu_scopes\",\"ready_wake_edges\",\"native_sink_finish\"]}}",
            self.id,
            nanos(self.started.elapsed()),
            self.dropped.load(Ordering::Relaxed)
        )
    }
}
fn nanos(d: std::time::Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}
pub struct Timer<'a>(Option<(&'a QueryStats, Stage, Instant)>);
thread_local! { static CHILD_NS: std::cell::RefCell<Vec<u64>> = const { std::cell::RefCell::new(Vec::new()) }; }
impl<'a> Timer<'a> {
    const fn enabled() -> bool {
        !cfg!(any(nata_stats_disabled, nata_loom))
    }
    #[inline]
    pub fn new(stats: &'a Option<Arc<QueryStats>>, stage: Stage) -> Self {
        Self(if Self::enabled() {
            stats.as_ref().map(|s| {
                CHILD_NS.with(|v| v.borrow_mut().push(0));
                (s.as_ref(), stage, Instant::now())
            })
        } else {
            None
        })
    }
}
impl Drop for Timer<'_> {
    fn drop(&mut self) {
        if let Some((stats, stage, started)) = self.0 {
            let elapsed = nanos(started.elapsed());
            let exclusive = CHILD_NS.with(|v| {
                let mut stack = v.borrow_mut();
                let children = stack.pop().unwrap_or(0);
                if let Some(parent) = stack.last_mut() {
                    *parent = parent.saturating_add(elapsed);
                }
                elapsed.saturating_sub(children)
            });
            stats.add(stage, exclusive);
        }
    }
}

#[cfg(all(test, not(nata_loom)))]
mod tests {
    use super::*;
    #[test]
    fn stage_aggregation_and_identity() {
        let a = QueryStats::new();
        let b = QueryStats::new();
        assert_ne!(a.id, b.id);
        a.add(Stage::GroupId, 13);
        a.add(Stage::GroupId, 17);
        a.add(Stage::StateUpdate, 19);
        assert_eq!(
            a.service_ns[Stage::GroupId as usize].load(Ordering::Relaxed),
            30
        );
        assert_eq!(a.calls[Stage::GroupId as usize].load(Ordering::Relaxed), 2);
        assert_eq!(
            a.service_ns[Stage::StateUpdate as usize].load(Ordering::Relaxed),
            19
        );
        assert_eq!(b.calls[Stage::GroupId as usize].load(Ordering::Relaxed), 0);
        let json = a.json();
        assert!(json.contains("\"group_id\":{\"service_wall_ns\":30,\"calls\":2}"));
        assert!(json.contains("\"complete\":false"));
        assert!(json.contains("\"admission\":null"));
        a.admission(3, 100, 20, 30);
        assert!(
            a.json()
                .contains("\"tasks\":3,\"free_bytes\":100,\"decode_bytes\":20,\"task_bytes\":30")
        );
    }
    #[test]
    fn disabled_timer_has_no_clock_or_owner() {
        assert!(Timer::new(&None, Stage::ScanDecode).0.is_none());
    }
}

#[derive(Debug)]
struct Event {
    kind: &'static str,
    ns: u64,
    pipeline: u64,
    task: u64,
    wave: u64,
    value: u64,
}
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Counter {
    CopiedBytes,
    MaterializedBytes,
    ColMaterializations,
    AllocationCapacityBytes,
    AllocationSites,
    Grants,
    GrantBytes,
    Releases,
}
thread_local! { static CURRENT: std::cell::RefCell<Option<Arc<QueryStats>>> = const { std::cell::RefCell::new(None) }; }
/// Install the owner on a worker only at task boundaries. Restores nested runs.
pub struct Scope {
    previous: Option<Option<Arc<QueryStats>>>,
}
impl Scope {
    pub fn new(stats: &Option<Arc<QueryStats>>) -> Self {
        Self {
            previous: if QueryStats::available() {
                stats
                    .as_ref()
                    .map(|s| CURRENT.with(|v| v.replace(Some(Arc::clone(s)))))
            } else {
                None
            },
        }
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            CURRENT.with(|v| {
                v.replace(previous);
            });
        }
    }
}
#[inline]
pub fn current(f: impl FnOnce(&QueryStats)) {
    if QueryStats::available() && HAS_STATS.load(Ordering::Relaxed) {
        CURRENT.with(|s| {
            if let Some(s) = s.borrow().as_ref() {
                f(s);
            }
        });
    }
}
/// Linux per-thread scheduler runtime, in ns; unavailable clocks remain None.
pub fn thread_cpu_ns() -> Option<u64> {
    std::fs::read_to_string("/proc/thread-self/schedstat")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Current owner, used at a coarse boundary, never once per row.
pub fn owner() -> Option<Arc<QueryStats>> {
    if QueryStats::available() && HAS_STATS.load(Ordering::Relaxed) {
        CURRENT.with(|v| v.borrow().clone())
    } else {
        None
    }
}

/// Per-level codec/disk service wall timer, distinct from queue intervals.
pub struct LevelTimer(Option<(Arc<QueryStats>, u8, usize, Instant)>);
impl LevelTimer {
    pub fn new(depth: u8, write: bool) -> Self {
        Self(owner().map(|s| (s, depth, if write { 6 } else { 7 }, Instant::now())))
    }
}
impl Drop for LevelTimer {
    fn drop(&mut self) {
        if let Some((s, depth, field, start)) = &self.0 {
            s.level(*depth, *field, nanos(start.elapsed()));
        }
    }
}

#[cfg(all(test, not(nata_loom)))]
mod attribution_checks {
    use super::*;
    #[test]
    fn levels_and_trace_bounds_are_independent() {
        let s = QueryStats::with_trace(true);
        s.level(2, 0, 17);
        s.level(2, 0, 19);
        s.level(2, 2, 2);
        assert_eq!(s.levels[2][0].load(Ordering::Relaxed), 36);
        assert_eq!(s.levels[2][2].load(Ordering::Relaxed), 2);
        assert_eq!(s.levels[1][0].load(Ordering::Relaxed), 0);
        let id = s.pipeline();
        assert_ne!(s.pipeline(), id);
        for _ in 0..32770 {
            s.event("dispatch", id, 3, 5, 7);
        }
        assert_eq!(s.events.lock().expect("events").len(), 32768);
        assert_eq!(s.dropped.load(Ordering::Relaxed), 2);
        s.count(Counter::CopiedBytes, 42);
        assert_eq!(
            s.counters[Counter::CopiedBytes as usize].load(Ordering::Relaxed),
            42
        );
        assert!(s.json().contains("\"dropped_events\":2"));
    }
    #[test]
    fn nested_owners_restore_and_disabled_owner_does_not_install() {
        let a = Some(QueryStats::new());
        let b = Some(QueryStats::new());
        let _a = Scope::new(&a);
        if !QueryStats::available() {
            assert!(owner().is_none());
            return;
        }
        let outer = owner().expect("a").id;
        {
            let _b = Scope::new(&b);
            assert_ne!(owner().expect("b").id, outer);
        }
        {
            let _disabled = Scope::new(&None);
            assert_eq!(owner().expect("a").id, outer);
        }
        assert_eq!(owner().expect("restored").id, outer);
    }
}

/// Linux process CPU ticks; no guessed conversion to seconds. The external
/// runner also records /usr/bin/time's user/system seconds for this same run.
fn process_cpu_ticks() -> Option<(u64, u64)> {
    let text = std::fs::read_to_string("/proc/self/stat").ok()?;
    let tail = text.get(text.rfind(')')? + 2..)?;
    let mut fields = tail.split_whitespace().skip(11);
    Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
}

/// Publish one complete JSON document atomically; failures never reach query
/// output. Caller owns the path and must check the returned error.
pub fn write_json(path: &std::path::Path, record: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut temporary = path.as_os_str().to_os_string();
    temporary.push(format!(".{}.tmp", std::process::id()));
    let temporary = std::path::PathBuf::from(temporary);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(record.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;
        drop(file);
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod file_tests {
    #[test]
    fn query_stats_file_publishes_replaces_and_reports_failure() {
        let root = std::env::temp_dir().join(format!("nata-stats-writer-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("stats file test");
        let path = root.join("record.json");
        super::write_json(&path, "{\"first\":true}").expect("stats file test");
        assert_eq!(
            std::fs::read_to_string(&path).expect("stats file test"),
            "{\"first\":true}\n"
        );
        super::write_json(&path, "{\"second\":true}").expect("stats file test");
        assert_eq!(
            std::fs::read_to_string(&path).expect("stats file test"),
            "{\"second\":true}\n"
        );
        assert_eq!(
            std::fs::read_dir(&root).expect("stats file test").count(),
            1
        );
        assert!(super::write_json(&root.join("missing/record.json"), "{}").is_err());
        assert!(super::write_json(&root, "{}").is_err());
        assert!(
            !root
                .with_extension(format!("{}.tmp", std::process::id()))
                .exists()
        );
        std::fs::remove_dir_all(root).expect("stats file test");
    }
}
