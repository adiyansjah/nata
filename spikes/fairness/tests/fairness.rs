//! The fairness guarantee and its tests.
//!
//! Guarantee (cooperative, at morsel boundaries). Let S be the longest time a task keeps a worker
//! once it starts a slice: `max_morsels x (longest morsel)`, or `max_time + (longest morsel)` for a
//! time budget. With W workers and K other tasks that are always runnable (the worst case), a task
//! that becomes runnable starts running within ceil(K / W) x S, and a task that needs `m` morsels
//! of CPU `c` in total completes within
//!     n x ceil(K / W) x S + c,    n = floor(m / max_morsels) + 1 slices
//! (the "+ 1": the source's end-of-input is found by a call that is itself preceded by a slice
//! check). The tests below measure this with the heavy tasks of `tests/starvation.rs`, plus a
//! tolerance for OS scheduling noise on a shared machine, and check the round-robin order and the
//! number of yields exactly.
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use spike_fairness::load::*;
use spike_fairness::*;

/// Allowed excess over the bound: `(slack - 1) x bound + 10 ms`. The bound assumes each pool thread
/// has a core to itself; the default slack of 1.25 is for a quiet machine. On an oversubscribed
/// machine the OS time-shares the pool threads and latencies stretch (about 2x at load 14 on 8 cores
/// was seen): set `NATA_FAIRNESS_SLACK=3` there. Starvation is unbounded, so any finite slack
/// still separates fair from unfair.
fn tolerance(bound: Duration) -> Duration {
    let slack: f64 = std::env::var("NATA_FAIRNESS_SLACK")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.25);
    bound.mul_f64(slack - 1.0) + Duration::from_millis(10)
}

fn ceil_div(a: usize, b: usize) -> u32 {
    a.div_ceil(b) as u32
}

/// Run the starvation scenario with slice `slice` and check the completion bound.
fn bounded(workers: usize, heavy: usize, slice: SliceConfig, heavy_morsel_ms: u64, small_morsels: u64) {
    let hm = Duration::from_millis(heavy_morsel_ms);
    let sm = Duration::from_millis(1);
    let s = Scenario {
        workers,
        heavy,
        heavy_morsel: hm,
        heavy_total: Duration::from_secs(60),
        small_morsels,
        small_morsel: sm,
        timeout: Duration::from_secs(3),
    };
    let slice_len = match slice.max_time {
        Some(t) if slice.max_morsels == usize::MAX => t + hm,
        _ => hm * slice.max_morsels as u32,
    };
    let n = (small_morsels as usize / slice.max_morsels.min(usize::MAX / 2)) as u32 + 1;
    let bound = slice_len * (n * ceil_div(heavy, workers)) + sm * small_morsels as u32;
    let o = run_starvation(&s, |q, w| q.with_slice(slice).start(w).unwrap());
    let l = o.latency.unwrap_or_else(|| panic!("STARVED: {s:?} {slice:?}"));
    eprintln!(
        "W={workers} K={heavy} slice={slice:?} heavy_morsel={hm:?} small={small_morsels}x1ms: latency {l:?}, bound {bound:?} (+{:?}), ratio {:.2}",
        tolerance(bound),
        l.as_secs_f64() / bound.as_secs_f64()
    );
    assert!(l <= bound + tolerance(bound), "latency {l:?} exceeds bound {bound:?} + {:?}", tolerance(bound));
}

#[test]
fn bound_w1_k2_morsel_budget() {
    bounded(1, 2, SliceConfig::morsels(2), 2, 3);
}

#[test]
fn bound_w1_k4_morsel_budget() {
    bounded(1, 4, SliceConfig::morsels(2), 2, 3);
}

#[test]
fn bound_w2_k3_morsel_budget() {
    bounded(2, 3, SliceConfig::morsels(2), 2, 3);
}

#[test]
fn bound_w2_k8_morsel_budget() {
    bounded(2, 8, SliceConfig::morsels(2), 2, 3);
}

#[test]
fn bound_w2_k8_single_slice_small_task() {
    bounded(2, 8, SliceConfig::morsels(4), 1, 3);
}

#[test]
fn bound_w2_k5_time_budget() {
    bounded(
        2,
        5,
        SliceConfig {
            max_morsels: usize::MAX,
            max_time: Some(Duration::from_millis(3)),
        },
        1,
        3,
    );
}

#[test]
fn bound_w1_k3_default_config() {
    bounded(1, 3, SliceConfig::default(), 1, 3);
}

/// Control: the same scenario with slicing switched off starves the small task (the old scheduler).
#[test]
fn control_unlimited_slice_starves() {
    let s = Scenario {
        workers: 2,
        heavy: 3,
        heavy_morsel: Duration::from_millis(2),
        heavy_total: Duration::from_secs(60),
        small_morsels: 3,
        small_morsel: Duration::from_millis(1),
        timeout: Duration::from_secs(1),
    };
    let o = run_starvation(&s, |q, w| q.with_slice(SliceConfig::unlimited()).start(w).unwrap());
    assert_eq!(o.latency, None, "without slicing the small task must not complete: {o:?}");
}

// ---------------------------------------------------------------- exact order and yield count

struct Logged {
    log: Arc<Mutex<Vec<usize>>>,
    morsels: u64,
    created: Arc<AtomicUsize>,
    tasks: usize,
}

struct LoggedSource {
    id: usize,
    left: u64,
    first: bool,
    log: Arc<Mutex<Vec<usize>>>,
    created: Arc<AtomicUsize>,
    tasks: usize,
}

impl SourceFactory for Logged {
    fn make(&self) -> Box<dyn Source> {
        let id = self.created.fetch_add(1, SeqCst);
        Box::new(LoggedSource {
            id,
            left: self.morsels,
            first: true,
            log: Arc::clone(&self.log),
            created: Arc::clone(&self.created),
            tasks: self.tasks,
        })
    }
}

impl Source for LoggedSource {
    fn next(&mut self, _w: &Waker) -> Result<SourceResult> {
        if self.first {
            // Make the scenario deterministic: every task exists and is queued before any runs on.
            self.first = false;
            while self.created.load(SeqCst) < self.tasks {
                std::thread::yield_now();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if self.left == 0 {
            return Ok(SourceResult::Finished);
        }
        self.left -= 1;
        spin(Duration::from_micros(200));
        self.log.lock().unwrap().push(self.id);
        Ok(SourceResult::Batch(Batch::from_cols(vec![vec![1]])))
    }
}

fn logged_run(tasks: usize, morsels: u64, slice: SliceConfig) -> (Vec<usize>, usize) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let p = Pipeline {
        name: "logged".into(),
        source: Arc::new(Logged {
            log: Arc::clone(&log),
            morsels,
            created: Arc::new(AtomicUsize::new(0)),
            tasks,
        }),
        ops: vec![],
        sink: Arc::new(Signal::default()),
        deps: vec![],
        tasks,
    };
    let rq = Query::new(vec![p]).with_slice(slice).start(1).unwrap();
    // wait() consumes the query: read the yield counter after the work has finished by polling
    while log.lock().unwrap().len() < tasks * morsels as usize {
        std::thread::sleep(Duration::from_millis(1));
    }
    std::thread::sleep(Duration::from_millis(20)); // let the last Finished calls happen
    let y = rq.yields();
    rq.wait().unwrap();
    let l = log.lock().unwrap().clone();
    (l, y)
}

/// One worker, three tasks of 12 morsels, slice 3: the execution order is strict round robin in
/// runs of exactly 3, and the yield count is exact (each task yields before morsels 4, 7, 10, 13:
/// 4 times, the last one at the end-of-input check).
#[test]
fn round_robin_order_and_exact_yield_count() {
    let (log, yields) = logged_run(3, 12, SliceConfig::morsels(3));
    let expect: Vec<usize> = (0..4).flat_map(|_| (0..3).flat_map(|t| [t, t, t])).collect();
    assert_eq!(log, expect, "order of morsels");
    assert_eq!(yields, 3 * 4, "yields");
}

#[test]
fn slice_of_one_morsel_alternates() {
    let (log, yields) = logged_run(2, 5, SliceConfig::morsels(1));
    assert_eq!(log, vec![0, 1, 0, 1, 0, 1, 0, 1, 0, 1]);
    assert_eq!(yields, 2 * 5);
}

/// A lone task never pays for a round trip through the queue: its slices are renewed in place.
#[test]
fn lone_task_never_yields() {
    let (log, yields) = logged_run(1, 20, SliceConfig::morsels(2));
    assert_eq!(log.len(), 20);
    assert_eq!(yields, 0);
}

#[test]
fn unlimited_slice_runs_tasks_to_completion_in_turn() {
    let (log, yields) = logged_run(2, 4, SliceConfig::unlimited());
    assert_eq!(log, vec![0, 0, 0, 0, 1, 1, 1, 1]);
    assert_eq!(yields, 0);
}

// ---------------------------------------------------------------- a consumer fed by a channel

fn consumer(slice: SliceConfig, timeout_ms: u64) -> Option<Duration> {
    let s = ConsumerScenario {
        workers: 2,
        heavy: 4,
        heavy_morsel: Duration::from_millis(1),
        chunks: 20,
        gap: Duration::from_millis(1),
        per_chunk: Duration::from_micros(500),
        cap: 4,
        timeout: Duration::from_millis(timeout_ms),
    };
    run_consumer(&s, slice)
}

/// 20 chunks arrive 1 ms apart and cost 0.5 ms each; 4 heavy tasks hold the 2 workers. Worst case
/// (one chunk per wake-up): chunks x (ceil(K/W) x S + c + gap) with S = 2 morsels x 1 ms.
#[test]
fn consumer_fed_by_channel_makes_progress() {
    let slice = SliceConfig::morsels(2);
    let bound = Duration::from_millis(20 * (2 * 2 + 1 + 1));
    let l = consumer(slice, 3000).expect("STARVED: the consumer did not finish");
    eprintln!("consumer: all 20 chunks consumed after {l:?} (worst-case bound {bound:?})");
    assert!(l <= bound + tolerance(bound), "{l:?} > {bound:?}");
}

#[test]
fn control_consumer_starves_without_slicing() {
    assert_eq!(consumer(SliceConfig::unlimited(), 1000), None);
}
