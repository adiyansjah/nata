//! Can the harness see a difference of a known size? Arms with a deliberately injected extra cost (a busy-wait of a known
//! number of nanoseconds after every batch of the hand-fused control) are run interleaved with the control and its twin,
//! exactly as the real cells are. The measured paired difference per batch must equal the spin that was injected, within a
//! tolerance, and must not be flagged as noise by the rule used for every cell. The twin (identical code) must show NO
//! difference. If either fails, no number from this run is evidence and the benchmark stops.

use std::sync::Arc;
use std::time::Instant;

use crate::arms::*;
use crate::bench::interleave;
use crate::kernels::*;
use crate::stats::*;

/// Injected costs, nanoseconds per batch: about 0.3x, 1.3x and 5x the cost of one control batch of 1024 rows.
pub const SPINS_NS: [u64; 3] = [100, 400, 1600];

#[derive(Clone, Copy, Debug)]
pub struct Judgement {
    pub expected: f64,
    pub summary: Summary,
    pub tolerance: f64,
    pub accurate: bool,
    pub resolved: bool,
}

impl Judgement {
    pub fn pass(&self) -> bool {
        self.accurate && self.resolved
    }
}

/// `deltas`: paired per-round differences in ns per batch per worker. Accurate = the median is within
/// `max(25% of expected, 15 ns)` of the injected cost; resolved = the interquartile range is not wider than the median.
pub fn judge(expected: f64, deltas: &[f64]) -> Judgement {
    let summary = summarize(deltas);
    let tolerance = (0.25 * expected).max(15.0);
    Judgement {
        expected,
        summary,
        tolerance,
        accurate: (summary.median - expected).abs() <= tolerance,
        resolved: !is_noisy(&summary),
    }
}

/// The twin has identical code: its median difference must be within `max(10% of a control batch, 20 ns)` of zero.
pub fn judge_twin(deltas: &[f64], control_ns_per_batch: f64) -> (Summary, f64, bool) {
    let s = summarize(deltas);
    let tol = (0.10 * control_ns_per_batch).max(20.0);
    (s, tol, s.median.abs() <= tol)
}

/// The real average duration of `spin_ns(ns)` in nanoseconds (it overshoots by about the cost of reading the clock), taken
/// from the clock on one thread: the injected cost against which the harness is judged.
pub fn calibrate_spin(ns: u64) -> f64 {
    for _ in 0..1_000 {
        spin_ns(ns);
    }
    let n = 20_000u32;
    let t = Instant::now();
    for _ in 0..n {
        spin_ns(ns);
    }
    t.elapsed().as_nanos() as f64 / n as f64
}

/// Runs the self-test and prints its table. Returns true if the harness passed.
pub fn run(rounds: usize, total_rows: usize, ncpu: usize) -> bool {
    println!("## Harness self-test\n");
    println!(
        "Arms: `control`, `control-twin` (same code), and `control+spinNns` = the control plus a busy-wait of N ns after every batch. \
Cell: cheap kernel, 1 stage, 1024 rows per batch, {} rows per run, {rounds} interleaved rounds. \
Expected difference = the spin measured on its own on one thread. PASS needs: |median - expected| <= max(25% of expected, 15 ns), \
interquartile range not wider than the median, and the twin within max(10% of a control batch, 20 ns) of zero.\n",
        total_rows
    );
    let thread_list: Vec<usize> = if ncpu >= 4 { vec![1, 4] } else { vec![1] };
    if ncpu < 4 {
        println!("(only {ncpu} CPU(s): the 4-thread self-test is skipped)\n");
    }
    let measured: Vec<f64> = SPINS_NS.iter().map(|&n| calibrate_spin(n)).collect();
    let mut all_pass = true;
    for &threads in &thread_list {
        let cell = Cell { kind: Kind::Cheap, len: 1, batch: 1024, threads, total_rows };
        let tmpl = Arc::new(template(cell.batch));
        let want = reference(cell.kind, cell.len, cell.batch, cell.morsels(), &tmpl);
        let mut arms = vec![Arm::Control, Arm::ControlTwin];
        arms.extend(SPINS_NS.iter().map(|&n| Arm::ControlSpin(n)));
        let names: Vec<String> = arms.iter().map(|a| a.name()).collect();
        let times = interleave(&names, rounds, want, |a| run_chain(arms[a], &cell, &tmpl));
        let m = cell.morsels();
        let ctl_ns_batch = summarize(&times[0]).median * 1e9 * threads as f64 / m as f64;
        println!("### {threads} thread(s): control = {ctl_ns_batch:.0} ns per batch per worker\n");
        println!("| arm | injected (nominal ns) | injected (measured ns) | measured difference ns/batch, median [q1, q3] | tolerance | result |");
        println!("|---|---|---|---|---|---|");
        let d = paired_delta_ns_per_batch(&times[1], &times[0], threads, m);
        let (ts, ttol, tok) = judge_twin(&d, ctl_ns_batch);
        println!(
            "| control-twin | 0 | 0 | {:+.1} [{:+.1}, {:+.1}] | ±{ttol:.0} | {} |",
            ts.median,
            ts.q1,
            ts.q3,
            if tok { "PASS (no difference seen)" } else { "FAIL (difference seen between identical arms)" }
        );
        all_pass &= tok;
        for (i, &nominal) in SPINS_NS.iter().enumerate() {
            let d = paired_delta_ns_per_batch(&times[2 + i], &times[0], threads, m);
            let j = judge(measured[i], &d);
            println!(
                "| {} | {nominal} | {:.0} | {:+.1} [{:+.1}, {:+.1}] | ±{:.0} | {} |",
                names[2 + i],
                measured[i],
                j.summary.median,
                j.summary.q1,
                j.summary.q3,
                j.tolerance,
                if j.pass() {
                    "PASS".to_string()
                } else {
                    format!("FAIL (accurate: {}, resolved: {})", j.accurate, j.resolved)
                }
            );
            all_pass &= j.pass();
        }
        println!();
    }
    println!(
        "**Harness self-test: {}**\n",
        if all_pass {
            "PASS - the harness measures an injected cost of 100 to 1600 ns per batch correctly"
        } else {
            "FAIL - no number from this run is evidence"
        }
    );
    all_pass
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn judge_accepts_the_right_size_and_rejects_the_wrong_one() {
        // injected 400 ns, measured around 400 with a tight spread
        let good: Vec<f64> = (0..25).map(|i| 400.0 + (i % 5) as f64 - 2.0).collect();
        assert!(judge(400.0, &good).pass());
        // the harness reports nothing (difference 0): not accurate
        let none: Vec<f64> = (0..25).map(|i| (i % 5) as f64 - 2.0).collect();
        let j = judge(400.0, &none);
        assert!(!j.accurate && !j.pass());
        // it reports three times too much
        let big: Vec<f64> = (0..25).map(|i| 1200.0 + (i % 5) as f64).collect();
        assert!(!judge(400.0, &big).accurate);
        // right median but so noisy that the interquartile range is wider than the difference
        let noisy: Vec<f64> = (0..25).map(|i| if i % 2 == 0 { 100.0 } else { 700.0 }).collect();
        let j = judge(400.0, &noisy);
        assert!(!j.resolved && !j.pass());
    }

    #[test]
    fn twin_check_flags_a_difference_between_identical_arms() {
        let quiet: Vec<f64> = (0..25).map(|i| (i % 5) as f64 - 2.0).collect();
        assert!(judge_twin(&quiet, 300.0).2);
        let off: Vec<f64> = (0..25).map(|i| 80.0 + (i % 5) as f64).collect();
        assert!(!judge_twin(&off, 300.0).2);
    }

    #[test]
    fn calibration_reports_at_least_the_requested_spin() {
        let m = calibrate_spin(200);
        assert!(m >= 200.0 && m < 20_000.0, "{m}");
    }
}
