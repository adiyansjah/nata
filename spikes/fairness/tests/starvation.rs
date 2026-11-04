//! Reproduction of the starvation found by the async-bridge spike, using only the scheduler's
//! default API. K CPU-heavy tasks (each ~60 s of work, 1 ms morsels) on W pool threads and one
//! small task that needs 3 ms of CPU. The small task must complete within a budget that is tiny
//! next to the heavy tasks' runtime. On a scheduler without time slicing it never does.
use std::time::Duration;

use spike_fairness::load::*;

fn scenario(workers: usize, heavy: usize) -> Scenario {
    Scenario {
        workers,
        heavy,
        heavy_morsel: Duration::from_millis(1),
        heavy_total: Duration::from_secs(60),
        small_morsels: 3,
        small_morsel: Duration::from_millis(1),
        timeout: Duration::from_secs(2),
    }
}

fn check(workers: usize, heavy: usize) {
    let o = run_starvation(&scenario(workers, heavy), |q, w| q.start(w).unwrap());
    eprintln!("workers={workers} heavy={heavy}: small task latency = {:?}", o.latency);
    let l = o
        .latency
        .unwrap_or_else(|| panic!("STARVED: the 3 ms task did not complete within 2 s (workers={workers}, heavy={heavy})"));
    assert!(l < Duration::from_secs(1), "latency {l:?}");
}

#[test]
fn one_worker_two_heavy() {
    check(1, 2);
}

#[test]
fn one_worker_four_heavy() {
    check(1, 4);
}

#[test]
fn two_workers_two_heavy() {
    check(2, 2);
}

#[test]
fn two_workers_four_heavy() {
    check(2, 4);
}

#[test]
fn two_workers_eight_heavy() {
    check(2, 8);
}

/// Positive control: with a free worker (1 heavy task, 2 workers) the small task completes at once,
/// even without time slicing. It shows the harness measures completion correctly.
#[test]
fn control_free_worker_completes() {
    check(2, 1);
}
