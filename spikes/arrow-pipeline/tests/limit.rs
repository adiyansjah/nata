//! LIMIT and early finish on Arrow batches (ADR 0001, "Early finish travels three ways": up and sideways here;
//! across an exchange is in tests/exchange.rs).
//! Properties checked against a scalar reference on many random cases:
//!  (a) the output has exactly min(k, n) rows and every row belongs to the full filtered result;
//!  (b) a small k with many morsels reads FEWER morsels than the total (early finish reaches the source), and
//!      tasks parked on their source are finished (the query terminates, every task calls Source::finish);
//!  (c) no hang under contention: tests/stress.rs.
mod common;

use std::sync::atomic::Ordering::SeqCst;
use std::time::Duration;

use common::*;
use spike_arrow_pipeline::sources::Chaos;
use spike_arrow_pipeline::synth::*;

fn cases() -> u64 {
    std::env::var("LIMIT_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(1500)
}

#[test]
fn a_output_is_exactly_min_k_n_rows_of_the_filtered_result() {
    let (mut with_rows, mut zero_k, mut exact, mut over, mut empty_input) = (0, 0, 0, 0, 0);
    for seed in 0..cases() {
        let case = random_case(seed);
        let run = run_case(&case, watchdog());
        check_run(&case, &run);
        let n = filtered(&run.all, &case.filters).len();
        with_rows += (!run.rows.is_empty()) as u32;
        zero_k += (case.k == 0) as u32;
        exact += (case.k == n && n > 0) as u32;
        over += (case.k > n) as u32;
        empty_input += run.all.is_empty() as u32;
    }
    // The generator really produced the interesting cases (otherwise the property would be vacuous).
    println!("cases {}: with output rows {with_rows}, k=0 {zero_k}, k=n {exact}, k>n {over}, empty input {empty_input}", cases());
    assert!(with_rows > 200 && zero_k > 50 && exact > 50 && over > 100 && empty_input > 20);
}

#[test]
fn b_early_finish_reaches_the_source_with_a_provable_bound() {
    // 2000 morsels, each ONE batch with exactly m matching rows. The LIMIT is satisfied after ceil(k/m) batches
    // have passed it; each task may have one batch in flight and may claim one more morsel between the pipeline's
    // stop flag being set and being observed, so claimed <= ceil(k/m) + 2*tasks.
    let total = 2000;
    let mut checked = 0;
    for &(m, k) in &[(1usize, 1usize), (3, 5), (8, 17), (8, 100), (64, 70), (2, 0)] {
        for &threads in &[1usize, 4, 8] {
            for &tasks in &[threads, 2 * threads] {
                for &sink_limit in &[false, true] {
                    for rep in 0..6u64 {
                        let mut data = SynthCfg::random(rep);
                        data.morsels = total;
                        data.stripe = Some((64, m));
                        let chaos = if rep % 2 == 0 { Chaos::default() } else { Chaos { seed: rep, delay_per_mille: 200, max_delay_us: 20, ..Chaos::default() } };
                        let case = Case { data, filters: vec![F::A(1, 1)], project: false, k, threads, tasks, sink_limit, chaos, sink_chaos: None };
                        let run = run_case(&case, watchdog());
                        check_run(&case, &run);
                        let claimed = run.stats.claimed.load(SeqCst);
                        let bound = k.div_ceil(m) + 2 * tasks;
                        assert!(claimed <= bound, "claimed {claimed} morsels > bound {bound}: early finish did not reach the source (m={m} k={k} threads={threads} tasks={tasks} sink_limit={sink_limit})");
                        assert!(claimed < total, "scan read everything");
                        checked += 1;
                    }
                }
            }
        }
    }
    println!("{checked} configurations: morsels claimed stayed within ceil(k/m) + 2*tasks of {total}");
}

#[test]
fn b_parked_tasks_are_woken_and_finished_by_early_finish() {
    // tasks 1.. are blocked on their source forever and never call their waker. Only the scheduler's
    // "pipeline stopped early -> wake the parked tasks" can end them.
    let mut ran = 0;
    for seed in 0..cases() {
        let Some(case) = stuck_case(seed) else { continue };
        let run = run_case(&case, watchdog());
        check_run(&case, &run);
        assert!(run.stats.blocked.load(SeqCst) >= case.tasks - 1 || run.stats.finished.load(SeqCst) == case.tasks);
        ran += 1;
    }
    println!("{ran} cases with stuck tasks all terminated with every task finished");
    assert!(ran > 300);
}

#[test]
fn b_cancel_mid_flight_wakes_parked_tasks_and_terminates() {
    use spike_arrow_pipeline::proto::sched::Query;
    let mut cancelled_err = 0;
    for seed in 0..300u64 {
        // Every task stuck: the query can only end through cancellation.
        let mut case = random_case(seed);
        case.chaos.stuck_from_task = Some(if seed % 3 == 0 { 0 } else { 1 });
        case.data.morsels = case.data.morsels.max(30);
        let (p, scan, _sink, _all) = build(&case);
        let q = Query::new(vec![p]).start(case.threads).expect("start");
        std::thread::sleep(Duration::from_micros(seed % 7 * 300));
        q.cancel();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(q.wait());
        });
        let r = rx.recv_timeout(watchdog()).unwrap_or_else(|_| panic!("TIMEOUT: wait() hung after cancel: a parked task was never woken ({case:?})"));
        // A cancelled query may still finish before the cancel lands if the live task drained all the data and
        // the other tasks were not stuck; with stuck tasks it cannot, so it must report the cancellation.
        let e = r.expect_err("a query with stuck tasks can only end by cancellation");
        assert_eq!(e.0, "cancelled");
        assert_eq!(scan.stats.finished.load(SeqCst), case.tasks, "every task must end and call Source::finish after cancel");
        cancelled_err += 1;
    }
    println!("{cancelled_err} cancelled queries terminated with every task finished");
}

#[test]
fn b_k_zero_and_empty_input_terminate_without_hanging() {
    for sink_limit in [false, true] {
        for morsels in [0usize, 1, 7] {
            for threads in [1usize, 4] {
                let mut data = SynthCfg::random(1);
                data.morsels = morsels;
                data.stripe = Some((10, 3)); // every morsel has one non-empty batch, so the LIMIT sees data at once
                let case = Case { data, filters: vec![], project: false, k: 0, threads, tasks: threads, sink_limit, chaos: Chaos::default(), sink_chaos: None };
                let run = run_case(&case, watchdog());
                check_run(&case, &run);
                assert!(run.rows.is_empty());
                // k = 0 over a non-empty input: a task reads at most one morsel before the LIMIT says Finished
                assert!(run.stats.claimed.load(SeqCst) <= threads.max(1) * 2 + 1, "k=0 read {} morsels", run.stats.claimed.load(SeqCst));
            }
        }
    }
}

#[test]
fn a_batch_already_admitted_by_the_limit_survives_a_sideways_stop_while_the_sink_is_blocked() {
    // Regression for a protocol gap this spike found: task A's LIMIT admits the row, A's sink answers Blocked and
    // A parks holding it; task B then sees the LIMIT exhausted, stops the pipeline and wakes A. With a pipeline-wide
    // `stop: bool`, A saw "stopped" and ended WITHOUT pushing the admitted row: LIMIT 1 returned 0 rows.
    use spike_arrow_pipeline::sinks::SinkChaos;
    let mut lost = Vec::new();
    for rep in 0..600u64 {
        let mut data = SynthCfg::random(rep);
        data.morsels = 40;
        data.stripe = Some((10, 3));
        let sink_chaos = Some(SinkChaos { seed: rep, block_per_mille: 700, max_block_us: 40 });
        let case = Case { data, filters: vec![F::A(1, 1)], project: false, k: 1 + (rep % 3) as usize, threads: 4, tasks: 4, sink_limit: false, chaos: Chaos::default(), sink_chaos };
        let run = run_case(&case, watchdog());
        if run.result.is_ok() && run.rows.len() != case.k {
            lost.push((rep, run.rows.len(), case.k));
        }
    }
    assert!(lost.is_empty(), "LIMIT lost admitted rows in {} of 600 runs (rep, got, want): {:?}", lost.len(), &lost[..lost.len().min(5)]);
}

#[test]
fn limit_is_exact_under_heavy_contention() {
    // Many tiny batches and more tasks than threads, so several tasks reach the LIMIT at once.
    for round in 0..200u64 {
        let mut data = SynthCfg::random(round);
        data.morsels = 3000;
        data.max_rows = 8;
        let case = Case { data, filters: vec![], project: false, k: 400, threads: 8, tasks: 8, sink_limit: round % 2 == 0, chaos: Chaos::default(), sink_chaos: None };
        let run = run_case(&case, watchdog());
        check_run(&case, &run);
    }
}
