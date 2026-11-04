//! Cost of the runtime abstraction on the REAL runtime (verification-plan item 23, deliverable 5).
//!
//! The existing in-process exchange + scheduler path (producer pipeline -> exchange -> consumer pipeline, one
//! tiny batch per morsel, so every batch crosses the exchange) is timed
//!   direct   : as it is today,
//!   generic  : through `HookedSink/HookedSource<RealHooks>` (static dispatch, a clock read and an event per batch per side),
//!   dyn      : the same through `dyn Hooks`,
//!   nohooks  : the wrapper with empty hooks (the indirection alone),
//!   control  : `direct` again (a second identical arm: the noise floor of the harness).
//! Arms are interleaved in rotating order, once per round, so drift hits every arm alike. The result of every
//! run is checked (row count and sum) before its time counts. Statistics are paired per round.
//!
//! Self-test: an arm with a KNOWN cost (`SlowHooks`: 2 x 100 ns per batch on one thread) must be detected, or
//! the whole benchmark is declared invalid (exit 1).
//!
//! Env: SIM_BENCH_ROUNDS (default 15), SIM_BENCH_BATCHES (default 200000).

use std::sync::Arc;
use std::time::Instant;

use nata_sim::api::*;
use nata_sim::batch::Batch;
use nata_sim::exchange::exchange;
use nata_sim::hooks::*;
use nata_sim::ops::{Collect, Generator};
use nata_sim::sched::Query;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Arm {
    Direct,
    Generic,
    Dyn,
    NoHooks,
    Control,
    Injected,
}

impl Arm {
    fn name(self) -> &'static str {
        match self {
            Arm::Direct => "direct",
            Arm::Generic => "generic",
            Arm::Dyn => "dyn",
            Arm::NoHooks => "nohooks",
            Arm::Control => "control",
            Arm::Injected => "injected",
        }
    }
}

const SPIN_NS: u64 = 100;

/// One query run; returns ns per batch. Panics if the result is wrong.
fn run_arm(arm: Arm, threads: usize, batches: usize) -> f64 {
    let tasks = threads;
    let gen_source = Generator::new(batches, |m| Batch::from_cols(vec![vec![m as i64]]));
    let (mut sink, mut source, _chan) = exchange(2 * tasks, tasks, tasks);
    match arm {
        Arm::Direct | Arm::Control => {}
        Arm::Generic => {
            let h = Arc::new(RealHooks::new());
            sink = Arc::new(HookedSink::<RealHooks>(sink, Arc::clone(&h)));
            source = Arc::new(HookedSource::<RealHooks>(source, h));
        }
        Arm::Dyn => {
            let h: Arc<dyn Hooks> = Arc::new(RealHooks::new());
            sink = Arc::new(HookedSink::<dyn Hooks>(sink, Arc::clone(&h)));
            source = Arc::new(HookedSource::<dyn Hooks>(source, h));
        }
        Arm::NoHooks => {
            let h = Arc::new(NoHooks);
            sink = Arc::new(HookedSink::<NoHooks>(sink, Arc::clone(&h)));
            source = Arc::new(HookedSource::<NoHooks>(source, h));
        }
        Arm::Injected => {
            let h = Arc::new(SlowHooks { spin_ns: SPIN_NS });
            sink = Arc::new(HookedSink::<SlowHooks>(sink, Arc::clone(&h)));
            source = Arc::new(HookedSource::<SlowHooks>(source, h));
        }
    }
    let collect = Collect::new();
    let producer = Pipeline {
        name: "producer".into(),
        source: Arc::new(gen_source),
        ops: vec![],
        sink,
        deps: vec![],
        tasks,
    };
    let consumer = Pipeline { name: "consumer".into(), source, ops: vec![], sink: Arc::new(collect.clone()), deps: vec![], tasks };
    let q = Query::new(vec![producer, consumer]);
    let t = Instant::now();
    q.run(threads).expect("query ran");
    let ns = t.elapsed().as_nanos() as f64 / batches as f64;
    let rows = collect.rows();
    assert_eq!(rows.len(), batches, "arm {arm:?}: wrong row count");
    let sum: i64 = rows.iter().map(|r| r[0]).sum();
    assert_eq!(sum, (batches as i64) * (batches as i64 - 1) / 2, "arm {arm:?}: wrong sum");
    ns
}

fn quantile(sorted: &[f64], p: f64) -> f64 {
    let pos = p * (sorted.len() - 1) as f64;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}

fn quartiles(v: &[f64]) -> (f64, f64, f64, f64, f64) {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    (s[0], quantile(&s, 0.25), quantile(&s, 0.5), quantile(&s, 0.75), s[s.len() - 1])
}

fn load_avg() -> f64 {
    std::fs::read_to_string("/proc/loadavg").ok().and_then(|s| s.split_whitespace().next().and_then(|x| x.parse().ok())).unwrap_or(-1.0)
}

fn main() {
    let rounds: usize = std::env::var("SIM_BENCH_ROUNDS").ok().and_then(|s| s.parse().ok()).unwrap_or(15);
    let batches: usize = std::env::var("SIM_BENCH_BATCHES").ok().and_then(|s| s.parse().ok()).unwrap_or(200_000);
    println!("BENCH arch={} profile={} rounds={rounds} batches={batches} loadavg_start={:.2}", std::env::consts::ARCH, if cfg!(debug_assertions) { "debug" } else { "release" }, load_avg());
    let mut invalid = false;
    for threads in [1usize, 4] {
        let arms: Vec<Arm> = if threads == 1 {
            vec![Arm::Direct, Arm::Generic, Arm::Dyn, Arm::NoHooks, Arm::Control, Arm::Injected]
        } else {
            vec![Arm::Direct, Arm::Generic, Arm::Dyn, Arm::NoHooks, Arm::Control]
        };
        let mut times: Vec<Vec<f64>> = vec![Vec::new(); arms.len()];
        // warm-up: one run of the direct arm, discarded
        let _ = run_arm(Arm::Direct, threads, batches / 4);
        for round in 0..rounds {
            for k in 0..arms.len() {
                let ai = (round + k) % arms.len(); // rotate the order every round
                times[ai].push(run_arm(arms[ai], threads, batches));
            }
        }
        println!("THREADS {threads}: ns per batch (min q1 median q3 max), {rounds} interleaved rounds");
        for (i, a) in arms.iter().enumerate() {
            let (mn, q1, md, q3, mx) = quartiles(&times[i]);
            println!("  {:<9} {mn:8.1} {q1:8.1} {md:8.1} {q3:8.1} {mx:8.1}", a.name());
        }
        let base = arms.iter().position(|a| *a == Arm::Direct).expect("direct arm");
        println!("  paired difference against `direct` per round (ns per batch): median [q1, q3]");
        let mut floor = f64::NAN;
        for (i, a) in arms.iter().enumerate() {
            if i == base {
                continue;
            }
            let d: Vec<f64> = times[i].iter().zip(&times[base]).map(|(x, y)| x - y).collect();
            let (_, q1, md, q3, _) = quartiles(&d);
            let pct = 100.0 * md / quartiles(&times[base]).2;
            println!("  {:<9} {md:+8.1} [{q1:+.1}, {q3:+.1}]  ({pct:+.1}% of direct's median)", a.name());
            if *a == Arm::Control {
                floor = (q3 - q1).max(md.abs());
            }
            if *a == Arm::Injected {
                let expected = 2.0 * SPIN_NS as f64;
                let noise = (q3 - q1).max(floor);
                let detected = md > 0.5 * expected && md > noise;
                println!(
                    "  SELFTEST injected cost {expected:.0} ns/batch (2 x {SPIN_NS} ns): measured {md:+.1}, noise floor {noise:.1} -> {}",
                    if detected { "DETECTED" } else { "NOT DETECTED (benchmark INVALID)" }
                );
                invalid |= !detected;
            }
        }
        println!("  NOISE_FLOOR threads={threads} ns_per_batch={floor:.1} (control arm vs direct: max(|median|, IQR))");
    }
    println!("BENCH loadavg_end={:.2}", load_avg());
    if invalid {
        println!("BENCH INVALID");
        std::process::exit(1);
    }
    println!("BENCH OK");
}
