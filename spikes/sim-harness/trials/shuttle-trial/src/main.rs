//! shuttle-trial sweep <first> <count>   one randomized shuttle schedule per seed
//! shuttle-trial replay <seed>           the same seed again: the failure must be the same
//!
//! Scenarios (alternating by seed parity), both on the real scheduler (2 worker threads) and real exchange:
//!   even: producer -> exchange(capacity 1) -> consumer, 3 items; the consumer must see exactly [0, 1, 2]
//!   odd : the same query is cancelled right after it starts; it must end (Ok or "cancelled"), never hang
//! A deadlock or a failed assertion is a finding. Shuttle replays a failure from the seed.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::time::{Duration, Instant};

use shuttle_trial::api::*;
use shuttle_trial::batch::Batch;
use shuttle_trial::exchange::exchange;
use shuttle_trial::ops::{Collect, Generator};
use shuttle_trial::sched::Query;

fn query() -> (Query, Collect) {
    let gen_source = Generator::new(3, |m| Batch::from_cols(vec![vec![m as i64]]));
    let (sink, source, _chan) = exchange(1, 1, 1);
    let collect = Collect::new();
    let producer = Pipeline { name: "producer".into(), source: Arc::new(gen_source), ops: vec![], sink, deps: vec![], tasks: 1 };
    let consumer = Pipeline { name: "consumer".into(), source, ops: vec![], sink: Arc::new(collect.clone()), deps: vec![], tasks: 1 };
    (Query::new(vec![producer, consumer]), collect)
}

fn plain() {
    let (q, collect) = query();
    q.run(2).expect("query ran");
    assert_eq!(collect.rows(), vec![vec![0], vec![1], vec![2]], "consumer saw the wrong items");
}

fn cancelled() {
    let (q, _collect) = query();
    let running = q.start(2).expect("started");
    running.cancel();
    match running.wait() {
        Ok(()) => {}
        Err(e) => assert_eq!(e.0, "cancelled"),
    }
}

/// One shuttle execution for `seed`. Err(message) on a panic or a deadlock.
fn run_seed(seed: u64) -> std::result::Result<(), String> {
    let r = std::panic::catch_unwind(|| {
        if seed % 2 == 0 {
            shuttle::check_random_with_seed(plain, seed, 1);
        } else {
            shuttle::check_random_with_seed(cancelled, seed, 1);
        }
    });
    r.map_err(|p| {
        let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
        // keep the first line: the schedule dump that follows is long
        msg.lines().next().unwrap_or("").chars().take(200).collect()
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::panic::set_hook(Box::new(|_| {})); // failures are reported by this program, not by 10,000 panic messages
    let progress = Arc::new(AtomicU64::new(0));
    let p2 = Arc::clone(&progress);
    std::thread::spawn(move || {
        let (mut last, mut idle) = (0, 0);
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let now = p2.load(SeqCst);
            if now == last {
                idle += 1;
                if idle >= 120 {
                    eprintln!("WATCHDOG: no seed finished for 120 s; aborting");
                    std::process::exit(5);
                }
            } else {
                idle = 0;
                last = now;
            }
        }
    });
    let bug = cfg!(feature = "bug-lost-shutdown");
    match args.first().map(String::as_str) {
        Some("sweep") => {
            let first: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            let count: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1000);
            let t = Instant::now();
            let (mut fails, mut first_fail) = (0u64, None);
            for seed in first..first + count {
                progress.fetch_add(1, SeqCst);
                if let Err(m) = run_seed(seed) {
                    fails += 1;
                    if first_fail.is_none() {
                        first_fail = Some((seed, m));
                    }
                }
            }
            println!("SHUTTLE SWEEP bug_feature={bug} seeds={count} fail={fails} wall_s={:.1}", t.elapsed().as_secs_f64());
            if let Some((seed, m)) = first_fail {
                println!("SHUTTLE FIRST_FAIL seed={seed} msg={m}");
            }
            std::process::exit(if fails > 0 { 1 } else { 0 });
        }
        Some("replay") => {
            let seed: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            match run_seed(seed) {
                Ok(()) => println!("SHUTTLE REPLAY seed={seed} PASS"),
                Err(m) => {
                    println!("SHUTTLE REPLAY seed={seed} msg={m}");
                    std::process::exit(1);
                }
            }
        }
        _ => {
            eprintln!("usage: shuttle-trial sweep <first> <count> | replay <seed>");
            std::process::exit(2);
        }
    }
}
