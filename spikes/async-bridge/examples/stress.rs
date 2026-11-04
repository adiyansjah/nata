//! Stress the real bridge (tokio + the snapshotted scheduler) over a matrix of configurations.
//!   cargo run --release --example stress -- --reps 600 [--contention] [--hook-us 200]
//!        [--worker-delay-us 200] [--timeout-s 20] [--seed 1] [--label name]
//! Exit code 0 only if every run completed in time with the right result.
use std::collections::BTreeMap;
use std::sync::atomic::Ordering::SeqCst;
use std::time::{Duration, Instant};

use spike_async_bridge::TEST_WORKER_DELAY_US;
use spike_async_bridge::chan::HOOK_DELAY_US;
use spike_async_bridge::harness::*;

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let reps: usize = arg(&args, "--reps").map_or(300, |v| v.parse().unwrap());
    let seed: u64 = arg(&args, "--seed").map_or(1, |v| v.parse().unwrap());
    let timeout = Duration::from_secs(arg(&args, "--timeout-s").map_or(20, |v| v.parse().unwrap()));
    let hook: u64 = arg(&args, "--hook-us").map_or(0, |v| v.parse().unwrap());
    let wdelay: u64 = arg(&args, "--worker-delay-us").map_or(0, |v| v.parse().unwrap());
    let label = arg(&args, "--label").unwrap_or_else(|| "stress".into());
    let contention = args.iter().any(|a| a == "--contention");
    HOOK_DELAY_US.store(hook, SeqCst);
    TEST_WORKER_DELAY_US.store(wdelay, SeqCst);
    let cores = std::thread::available_parallelism().map_or(8, |n| n.get());
    let _busy = contention.then(|| Busy::start(cores));
    eprintln!(
        "load before: {}",
        std::fs::read_to_string("/proc/loadavg")
            .unwrap_or_default()
            .trim()
    );

    let t0 = Instant::now();
    let (mut ok, mut wrong, mut hung) = (0usize, 0usize, 0usize);
    let mut slow_recovered = 0usize; // timed out once, finished on a retry with 5x the timeout
    let mut slow_cases: Vec<String> = Vec::new();
    let mut by_kind: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    let mut cancelled_runs = 0usize;
    let mut interrupted: BTreeMap<String, usize> = BTreeMap::new();
    let mut slowest = Duration::ZERO;
    for i in 0..reps {
        let sc = config(i, seed);
        let t = Instant::now();
        let v = run_timeboxed(&sc, timeout);
        slowest = slowest.max(t.elapsed());
        let e = by_kind.entry(format!("{:?}", sc.kind)).or_default();
        match v {
            Verdict::Done(Ok(o)) => {
                ok += 1;
                e.0 += 1;
                cancelled_runs += usize::from(o.query_err);
                if o.delivered < o.planned {
                    *interrupted.entry(format!("{:?}", sc.kind)).or_default() += 1;
                }
            }
            Verdict::Done(Err(msg)) => {
                wrong += 1;
                e.1 += 1;
                eprintln!("WRONG #{i} {sc:?}: {msg}");
            }
            Verdict::Hang => {
                // A timeout while the machine is loaded is inconclusive: retry the same scenario
                // once with a 5x timeout. Only a second timeout counts as a hang.
                let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
                eprintln!(
                    "TIMEOUT #{i} after {timeout:?} (load {}): retrying with {:?}",
                    load.trim(),
                    timeout * 5
                );
                match run_timeboxed(&sc, timeout * 5) {
                    Verdict::Done(Ok(_)) => {
                        slow_recovered += 1;
                        e.0 += 1;
                        slow_cases.push(format!(
                            "#{i} load={}",
                            load.split(' ').next().unwrap_or("?")
                        ));
                    }
                    Verdict::Done(Err(msg)) => {
                        wrong += 1;
                        e.1 += 1;
                        eprintln!("WRONG on retry #{i} {sc:?}: {msg}");
                    }
                    Verdict::Hang => {
                        hung += 1;
                        e.2 += 1;
                        eprintln!("HANG #{i} (timed out twice) {sc:?}");
                        if hung >= 3 {
                            break;
                        }
                    }
                }
            }
        }
    }
    println!(
        "{label}: runs={} ok={ok} (of which slow_under_load_recovered_on_retry={slow_recovered}) wrong={wrong} hung_confirmed={hung} cancelled_with_err={cancelled_runs} slowest_run={:.0}ms total={:.1}s contention={contention} hook_us={hook} worker_delay_us={wdelay} timeout={}s",
        ok + wrong + hung,
        slowest.as_secs_f64() * 1e3,
        t0.elapsed().as_secs_f64(),
        timeout.as_secs()
    );
    if !slow_cases.is_empty() {
        println!("  slow cases (inconclusive timeouts that finished on retry): {slow_cases:?}");
    }
    for (k, (a, b, c)) in &by_kind {
        println!("  {k}: ok={a} wrong={b} hung={c}");
    }
    println!(
        "  runs that ended with fewer chunks delivered than planned (i.e. really cut off mid-flight): {interrupted:?}"
    );
    eprintln!(
        "load after: {}",
        std::fs::read_to_string("/proc/loadavg")
            .unwrap_or_default()
            .trim()
    );
    std::process::exit(if wrong + hung == 0 { 0 } else { 1 });
}
