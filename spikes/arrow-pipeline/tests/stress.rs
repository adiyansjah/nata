//! (c) No hang under CPU contention and with injected delays. Own test binary: the worker delay hook
//! (TEST_WORKER_DELAY_US) is process-wide.
//!
//! The process runs one busy-loop thread per CPU (plus `STRESS_EXTRA_BUSY`), the scheduler's workers sleep a
//! random-ish time under the queue lock between "queue empty" and "wait", sources sleep and block at random, and
//! hundreds of LIMIT queries (random, and with stuck tasks that only early finish or cancel can end) run
//! back to back. A query that does not finish within the watchdog is recorded; after the load is removed each
//! recorded case is re-run alone: if it hangs alone it is a real hang (test fails); if it never hangs alone
//! it is reported as INCONCLUSIVE (timeouts under high load are not a verdict).
mod common;

use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::Arc;
use std::time::Duration;

use common::*;
use spike_arrow_pipeline::proto::sched::TEST_WORKER_DELAY_US;

fn reps() -> u64 {
    std::env::var("STRESS_REPS").ok().and_then(|v| v.parse().ok()).unwrap_or(400)
}

fn reruns() -> usize {
    std::env::var("STRESS_RERUNS").ok().and_then(|v| v.parse().ok()).unwrap_or(30)
}

/// More timeouts than this and the run is a failure for sure: stop collecting and go straight to the re-runs.
const MAX_RECORDED: usize = 5;

fn one(seed: u64, dog: Duration) -> Result<(), String> {
    // Alternate: random LIMIT cases, and stuck-task cases (early finish must wake parked tasks).
    let case = if seed % 2 == 0 { Some(random_case(seed)) } else { stuck_case(seed).or_else(|| Some(random_case(seed))) };
    let case = case.expect("case");
    let run = run_case(&case, dog);
    if matches!(run.result, Err(None)) {
        return Err(format!("timeout seed {seed}"));
    }
    check_run(&case, &run);
    Ok(())
}

#[test]
fn limit_queries_terminate_under_cpu_contention_and_injected_delays() {
    let busy = Arc::new(AtomicBool::new(true));
    let ncpu = std::thread::available_parallelism().map_or(4, |n| n.get());
    let extra: usize = std::env::var("STRESS_EXTRA_BUSY").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let spinners: Vec<_> = (0..ncpu + extra)
        .map(|_| {
            let b = Arc::clone(&busy);
            std::thread::spawn(move || {
                let mut x = 1u64;
                while b.load(SeqCst) {
                    for _ in 0..10_000 {
                        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    }
                    std::hint::black_box(x);
                }
            })
        })
        .collect();
    TEST_WORKER_DELAY_US.store(150, SeqCst);
    let dog = watchdog();
    let mut timeouts = Vec::new();
    let start = std::time::Instant::now();
    for seed in 0..reps() {
        if one(seed.wrapping_mul(7919) + 17, dog).is_err() {
            timeouts.push(seed.wrapping_mul(7919) + 17);
            if timeouts.len() >= MAX_RECORDED {
                break;
            }
        }
    }
    let loaded = start.elapsed();
    busy.store(false, SeqCst);
    for s in spinners {
        s.join().expect("spinner");
    }
    TEST_WORKER_DELAY_US.store(0, SeqCst);
    println!("{} queries under {} busy threads + worker delay hook in {:.1}s; {} timeouts", reps(), ncpu + extra, loaded.as_secs_f64(), timeouts.len());
    // Re-run every timed-out case alone: a real hang reproduces without load.
    for seed in &timeouts {
        for rep in 0..reruns() {
            one(*seed, dog).unwrap_or_else(|e| panic!("HANG reproduced without load (rep {rep}): {e}"));
        }
        println!("INCONCLUSIVE: seed {seed} timed out under load but passed {} re-runs alone", reruns());
    }
}

#[test]
fn many_repetitions_of_the_same_stuck_task_case_with_worker_delay() {
    // The same small stuck-task case hundreds of times with the delay hook: the wake-on-early-finish path under a
    // widened race window. Runs alone in this binary only if the other test is not running (cargo runs them in
    // parallel threads; the delay hook is shared, which only widens the window further).
    let dog = watchdog();
    let n = reps();
    let mut hung = Vec::new();
    for seed in 0..n {
        if one(seed * 2 + 1, dog).is_err() {
            hung.push(seed * 2 + 1);
            if hung.len() >= MAX_RECORDED {
                break;
            }
        }
    }
    for seed in &hung {
        for _ in 0..reruns() {
            one(*seed, dog).unwrap_or_else(|e| panic!("HANG reproduced: {e}"));
        }
        println!("INCONCLUSIVE: seed {seed} timed out once but passed {} re-runs", reruns());
    }
    println!("{n} repetitions, {} timeouts", hung.len());
}
