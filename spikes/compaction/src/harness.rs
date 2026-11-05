//! Interleaved timing harness: arms are timed in rotating order, round after round, in the same process and session
//! (docs/way-of-working.md section 6). Reports per-round samples; the statistics are in the functions below.

use std::hint::black_box;
use std::time::Instant;

pub struct Arm<'a> {
    pub id: String,
    pub run: Box<dyn Fn() -> u64 + 'a>,
}

pub struct ArmStats {
    pub id: String,
    /// Calls per timed sample (chosen so one sample lasts at least `min_sample_ns`).
    pub reps: u64,
    /// Checksum returned by the first call.
    pub check: u64,
    /// ns per call, one entry per round, in round order (so entry i of two arms was taken in the same round).
    pub ns_per_call: Vec<f64>,
}

pub fn quantile(v: &[f64], q: f64) -> f64 {
    assert!(!v.is_empty());
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    s[((s.len() - 1) as f64 * q).round() as usize]
}

pub fn median(v: &[f64]) -> f64 {
    quantile(v, 0.5)
}

/// Element-wise `num / den` (paired by round).
pub fn ratios(num: &[f64], den: &[f64]) -> Vec<f64> {
    num.iter().zip(den).map(|(a, b)| a / b).collect()
}

/// Element-wise `a - b` (paired by round).
pub fn diffs(a: &[f64], b: &[f64]) -> Vec<f64> {
    a.iter().zip(b).map(|(a, b)| a - b).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Slower,
    Same,
    Faster,
}

/// The smallest difference the verdict will call a difference: 1% of the time. Without it a systematic bias of a few
/// nanoseconds (an identical twin measured 2 ns slower in 75% of rounds, found by the first GCP run) is "Slower".
pub const MIN_EFFECT: f64 = 0.01;

/// From the paired per-round ratios x/y: `Slower` if at least 75% of rounds had x slower by more than `MIN_EFFECT`
/// (lower quartile above 1.01), `Faster` if at least 75% had it faster by more than that (upper quartile below 0.99),
/// otherwise within noise.
pub fn verdict(ratio: &[f64]) -> Verdict {
    if quantile(ratio, 0.25) > 1.0 + MIN_EFFECT {
        Verdict::Slower
    } else if quantile(ratio, 0.75) < 1.0 - MIN_EFFECT {
        Verdict::Faster
    } else {
        Verdict::Same
    }
}

fn sample(a: &Arm<'_>, reps: u64) -> f64 {
    let t = Instant::now();
    let mut s = 0u64;
    for _ in 0..reps {
        s ^= black_box((a.run)());
    }
    let dt = t.elapsed();
    black_box(s);
    dt.as_nanos() as f64 / reps as f64
}

/// Time all arms. Per arm: one call for the checksum and warm-up, then the repetition count is calibrated so that a
/// sample lasts at least `min_sample_ns` (a clock read costs about 25 ns). Then `rounds` rounds; in round r the arms run in
/// the order rotated by r, so no arm always runs first or last. One extra warm-up round is run and discarded.
pub fn measure(arms: &[Arm<'_>], rounds: usize, min_sample_ns: f64) -> Vec<ArmStats> {
    let mut stats: Vec<ArmStats> = Vec::new();
    for a in arms {
        let check = (a.run)();
        let mut reps = 1u64;
        loop {
            let t = Instant::now();
            let mut s = 0u64;
            for _ in 0..reps {
                s ^= black_box((a.run)());
            }
            black_box(s);
            let dt = t.elapsed().as_nanos() as f64;
            if dt >= min_sample_ns || reps >= 1 << 24 {
                break;
            }
            reps = if dt < 2000.0 { reps * 8 } else { ((reps as f64 * min_sample_ns * 1.25 / dt).ceil() as u64).max(reps + 1) };
        }
        stats.push(ArmStats { id: a.id.clone(), reps, check, ns_per_call: Vec::with_capacity(rounds) });
    }
    let m = arms.len();
    for round in 0..=rounds {
        for off in 0..m {
            let i = (off + round) % m;
            let ns = sample(&arms[i], stats[i].reps);
            if round > 0 {
                stats[i].ns_per_call.push(ns);
            }
        }
    }
    stats
}

/// 1-minute load average from /proc/loadavg (0.0 if unreadable).
pub fn load1() -> f64 {
    std::fs::read_to_string("/proc/loadavg").ok().and_then(|s| s.split_whitespace().next().and_then(|x| x.parse().ok())).unwrap_or(0.0)
}

/// Busy-wait for `ns` nanoseconds (a deadline, not a calibrated loop, so the injected cost is what it says).
pub fn spin_ns(ns: u64) {
    if ns == 0 {
        return;
    }
    let t = Instant::now();
    while (t.elapsed().as_nanos() as u64) < ns {
        std::hint::spin_loop();
    }
}

// ------------------------------------------------------------------------------------------------ self-test

pub struct SelfTestRow {
    pub label: String,
    pub injected_ns: u64,
    pub measured_delta_ns: f64,
    pub tolerance_ns: f64,
    pub verdict: Verdict,
    pub expected: Verdict,
    pub ok: bool,
}

/// Harness self-test: the arm `A` of a real cell next to (a) an identical twin (nothing injected: must come out
/// "same", difference near 0) and (b) copies of `A` with a known extra cost `X` per call injected (must come out "slower"
/// by `X`, within 10% + 60 ns). Uses the same `measure` and the same paired-ratio statistics as the benchmark. Returns
/// the rows and whether all passed.
pub fn selftest(fx: &crate::arms::Fixture, rounds: usize) -> (Vec<SelfTestRow>, f64, bool) {
    use crate::arms::run_arm;
    const INJECT: [u64; 3] = [1_000, 5_000, 20_000];
    let mut arms: Vec<Arm<'_>> = vec![Arm { id: "A".into(), run: Box::new(|| run_arm(fx, "A")) }, Arm { id: "A-twin".into(), run: Box::new(|| run_arm(fx, "A")) }];
    for x in INJECT {
        arms.push(Arm {
            id: format!("A+{x}ns"),
            run: Box::new(move || {
                let r = run_arm(fx, "A");
                spin_ns(x);
                r
            }),
        });
    }
    let st = measure(&arms, rounds, 50_000.0);
    let base = &st[0];
    let base_med = median(&base.ns_per_call);
    let mut rows = Vec::new();
    let mut all_ok = true;
    for (i, s) in st.iter().enumerate().skip(1) {
        let injected = if i == 1 { 0 } else { INJECT[i - 2] };
        let delta = median(&diffs(&s.ns_per_call, &base.ns_per_call));
        let tol = if injected == 0 { 0.03 * base_med + 40.0 } else { 0.10 * injected as f64 + 60.0 };
        let v = verdict(&ratios(&s.ns_per_call, &base.ns_per_call));
        let expected = if injected == 0 { Verdict::Same } else { Verdict::Slower };
        let ok = (delta - injected as f64).abs() <= tol && v == expected && s.check == base.check;
        all_ok &= ok;
        rows.push(SelfTestRow { label: s.id.clone(), injected_ns: injected, measured_delta_ns: delta, tolerance_ns: tol, verdict: v, expected, ok });
    }
    (rows, base_med, all_ok)
}
