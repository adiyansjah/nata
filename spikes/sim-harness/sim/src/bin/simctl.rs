//! Command line of the harness. Plain text output, stable line formats (the GCP script greps them).
//!
//!   simctl info
//!   simctl run <seed> [--log]                 one seed; prints its PASS/FAIL line (and the trace with --log)
//!   simctl replay <seed>                      same as run, printed as REPLAY so a script can compare
//!   simctl sweep <first> <count> [--jobs N] [--require-coverage]
//!   simctl hashes <first> <count>             trace hash of every seed, each run twice in-process
//!   simctl real [--kill]                      the toy protocol on the REAL runtime
//!
//! Exit codes: 0 all good, 1 a seed failed (sweep/run/real), 2 usage, 3 coverage requirement not met,
//! 4 in-process nondeterminism found.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::time::{Duration, Instant};

use nata_sim::real::{RealPlan, run_real};
use nata_sim::scenario::{FAMILIES, FAMILY_NAMES, Outcome, Scenario, run_seed};

/// Event kinds that a control sweep of at least one full family cycle must have exercised, or the checks
/// that depend on them could never fail.
const REQUIRED_COVERAGE: &[&str] = &[
    "retry",
    "retransmit",
    "dup_frame",
    "stale_data",
    "cancel_seen",
    "worker_restart",
    "store_retry",
    "bad_frame",
    "net_drop",
    "net_dup",
    "crash",
    "partition",
    "task_dropped_stale",
];

fn arg_u64(args: &[String], i: usize, what: &str) -> u64 {
    args.get(i).and_then(|s| s.parse().ok()).unwrap_or_else(|| {
        eprintln!("expected a number for {what}");
        std::process::exit(2)
    })
}

fn flag_value(args: &[String], flag: &str) -> Option<u64> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok())
}

fn watchdog(seconds: u64) -> std::sync::Arc<AtomicU64> {
    let progress = std::sync::Arc::new(AtomicU64::new(0));
    let p = std::sync::Arc::clone(&progress);
    std::thread::spawn(move || {
        let mut last = 0;
        let mut idle = 0;
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let now = p.load(SeqCst);
            if now == last {
                idle += 1;
                if idle >= seconds {
                    eprintln!("WATCHDOG: no seed finished for {seconds} s (last progress counter {now}); aborting");
                    std::process::exit(5);
                }
            } else {
                idle = 0;
                last = now;
            }
        }
    });
    progress
}

fn cmd_sweep(args: &[String]) -> i32 {
    let first = arg_u64(args, 0, "first seed");
    let count = arg_u64(args, 1, "seed count");
    let jobs = flag_value(args, "--jobs").unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get() as u64)).max(1);
    let require_cov = args.iter().any(|a| a == "--require-coverage");
    let progress = watchdog(120);
    let best = AtomicU64::new(u64::MAX); // lowest failing seed so far
    let fails = AtomicU64::new(0);
    let passes = AtomicU64::new(0);
    let first_fail: Mutex<Option<String>> = Mutex::new(None);
    let kinds: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());
    let cover: Mutex<BTreeMap<&'static str, u64>> = Mutex::new(BTreeMap::new());
    let per_family: Mutex<BTreeMap<u32, (u64, u64)>> = Mutex::new(BTreeMap::new());
    let started = Instant::now();
    std::thread::scope(|s| {
        for j in 0..jobs {
            let (best, fails, passes, first_fail, kinds, cover, per_family, progress) =
                (&best, &fails, &passes, &first_fail, &kinds, &cover, &per_family, &progress);
            s.spawn(move || {
                let mut i = j;
                while i < count {
                    let seed = first + i;
                    i += jobs;
                    let o: Outcome = run_seed(seed, false);
                    progress.fetch_add(1, SeqCst);
                    {
                        let mut c = cover.lock().unwrap();
                        for (k, v) in &o.stats {
                            if *v > 0 {
                                *c.entry(k).or_insert(0) += 1; // seeds in which the event happened
                            }
                        }
                        let mut pf = per_family.lock().unwrap();
                        let e = pf.entry(o.family).or_insert((0, 0));
                        e.0 += 1;
                        e.1 += o.fail.is_some() as u64;
                    }
                    match &o.fail {
                        None => {
                            passes.fetch_add(1, SeqCst);
                        }
                        Some(f) => {
                            fails.fetch_add(1, SeqCst);
                            *kinds.lock().unwrap().entry(f.kind.to_string()).or_insert(0) += 1;
                            let prev = best.fetch_min(seed, SeqCst);
                            if seed < prev {
                                *first_fail.lock().unwrap() = Some(o.line());
                            }
                        }
                    }
                }
            });
        }
    });
    let (p, f) = (passes.load(SeqCst), fails.load(SeqCst));
    // `first_fail` can briefly hold a higher seed if two threads race; recompute it from the lowest seed.
    let lowest = best.load(SeqCst);
    let first_line = if lowest != u64::MAX { Some(run_seed(lowest, false).line()) } else { first_fail.lock().unwrap().clone() };
    println!("SWEEP first={first} count={count} jobs={jobs} pass={p} fail={f} wall_s={:.1}", started.elapsed().as_secs_f64());
    for (fam, (n, bad)) in per_family.lock().unwrap().iter() {
        println!("FAMILY {} seeds={n} failing={bad}", FAMILY_NAMES[*fam as usize]);
    }
    for (k, n) in kinds.lock().unwrap().iter() {
        println!("FAILKIND {k} seeds={n}");
    }
    let cov = cover.lock().unwrap();
    for (k, n) in cov.iter() {
        println!("COVERAGE {k} seeds={n}");
    }
    let mut missing = Vec::new();
    for k in REQUIRED_COVERAGE {
        if !cov.contains_key(k) {
            missing.push(*k);
        }
    }
    if let Some(line) = first_line {
        println!("FIRST_FAIL {line}");
    }
    if require_cov && count >= FAMILIES {
        if missing.is_empty() {
            println!("COVERAGE_REQUIRED ok ({} kinds)", REQUIRED_COVERAGE.len());
        } else {
            println!("COVERAGE_REQUIRED MISSING {}", missing.join(","));
            return 3;
        }
    }
    if f > 0 { 1 } else { 0 }
}

fn cmd_run(args: &[String], label: &str) -> i32 {
    let seed = arg_u64(args, 0, "seed");
    let log = args.iter().any(|a| a == "--log");
    let _wd = watchdog(120);
    let o = run_seed(seed, log);
    println!("{label} {}", o.line());
    if log {
        let sc = Scenario::from_seed(seed);
        println!(
            "SCENARIO files={} window={} batch_rows={} net={:?} store={:?} timed={:?} crash_points={}",
            sc.files.len(),
            sc.cfg.window,
            sc.cfg.batch_rows,
            sc.net,
            sc.store,
            sc.timed,
            sc.crash_points.len()
        );
        for l in &o.log {
            println!("  {l}");
        }
    }
    if o.fail.is_some() { 1 } else { 0 }
}

fn cmd_hashes(args: &[String]) -> i32 {
    let first = arg_u64(args, 0, "first seed");
    let count = arg_u64(args, 1, "seed count");
    let progress = watchdog(120);
    let mut bad = 0;
    let mut seen: BTreeMap<u64, u64> = BTreeMap::new();
    for seed in first..first + count {
        progress.fetch_add(1, SeqCst);
        let a = run_seed(seed, false);
        // another seed in between, so that any state leaking through globals or thread-locals shows up
        let _other = run_seed(seed ^ 0x55AA, false);
        let b = run_seed(seed, false);
        if a.hash != b.hash || a.line() != b.line() {
            println!("INPROCESS_MISMATCH seed={seed} {:016x} {:016x}", a.hash, b.hash);
            bad += 1;
        }
        println!("HASH seed={seed} hash={:016x} events={}", a.hash, a.events);
        if let Some(prev) = seen.insert(a.hash, seed) {
            println!("COLLISION seeds {prev} and {seed} have the same trace hash");
            bad += 1;
        }
    }
    println!("HASHES first={first} count={count} distinct={} inprocess_mismatches={bad}", seen.len());
    if bad > 0 { 4 } else { 0 }
}

fn cmd_real(args: &[String]) -> i32 {
    let kill = args.iter().any(|a| a == "--kill");
    let sc = Scenario::from_seed(0); // family 0 (calm): only its workload is used
    let plan = RealPlan { kill: if kill { Some((2, 25, None)) } else { None } };
    let o = run_real(7, &sc.cfg, &sc.files, &plan, 20_000);
    match &o.fail {
        None => println!("REAL PASS kill={kill} status={} elapsed_ms={}", o.status, o.elapsed_ms),
        Some(f) => println!("REAL FAIL kill={kill} kind={} msg={} elapsed_ms={}", f.kind, f.msg, o.elapsed_ms),
    }
    if o.fail.is_some() { 1 } else { 0 }
}

fn cmd_info() -> i32 {
    println!("INFO arch={} os={} profile={} pointer_width={}", std::env::consts::ARCH, std::env::consts::OS, if cfg!(debug_assertions) { "debug" } else { "release" }, usize::BITS);
    println!("INFO families={} names={}", FAMILIES, FAMILY_NAMES.join(","));
    0
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("info") => cmd_info(),
        Some("run") => cmd_run(&args[1..], "RUN"),
        Some("replay") => cmd_run(&args[1..], "REPLAY"),
        Some("sweep") => cmd_sweep(&args[1..]),
        Some("hashes") => cmd_hashes(&args[1..]),
        Some("real") => cmd_real(&args[1..]),
        _ => {
            eprintln!("usage: simctl info | run <seed> [--log] | replay <seed> | sweep <first> <count> [--jobs N] [--require-coverage] | hashes <first> <count> | real [--kill]");
            2
        }
    };
    std::process::exit(code);
}
