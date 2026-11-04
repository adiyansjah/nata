//! madsim-trial sweep <first> <count> | hashes <first> <count> | probe-thread
#![cfg_attr(not(madsim), allow(unused))]

#[cfg(madsim)]
fn main() {
    use madsim_trial::run_seed;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let num = |i: usize, d: u64| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    nata_sim::engine::set_inline(true); // madsim forbids system threads (see probe-thread)
    match args.first().map(String::as_str) {
        Some("sweep") => {
            let (first, count) = (num(1, 0), num(2, 200));
            let (mut pass, mut fail, mut first_fail) = (0u64, 0u64, None);
            let t = std::time::Instant::now();
            for seed in first..first + count {
                let r = std::panic::catch_unwind(|| run_seed(seed));
                match r {
                    Ok(o) if o.fail.is_none() => pass += 1,
                    Ok(o) => {
                        fail += 1;
                        first_fail.get_or_insert(format!("seed={seed} {} (unmapped faults: {})", o.fail.unwrap_or_default(), o.scenario_features_unmapped));
                    }
                    Err(p) => {
                        fail += 1;
                        let m = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                        first_fail.get_or_insert(format!("seed={seed} PANIC {m}"));
                    }
                }
            }
            println!("MADSIM SWEEP seeds={count} pass={pass} fail={fail} wall_s={:.1}", t.elapsed().as_secs_f64());
            if let Some(f) = first_fail {
                println!("MADSIM FIRST_FAIL {f}");
            }
            std::process::exit(if fail > 0 { 1 } else { 0 });
        }
        Some("hashes") => {
            let (first, count) = (num(1, 0), num(2, 20));
            for seed in first..first + count {
                match std::panic::catch_unwind(|| run_seed(seed)) {
                    Ok(o) => println!("MADSIM HASH seed={seed} hash={:016x} events={}", o.hash, o.events),
                    Err(_) => println!("MADSIM HASH seed={seed} PANIC"),
                }
            }
        }
        Some("probe-thread") => {
            // Can the REAL scheduler (which spawns an OS thread per query) run inside a madsim node?
            nata_sim::engine::set_inline(false);
            let r = std::panic::catch_unwind(|| {
                let rt = madsim::runtime::Runtime::with_seed_and_config(1, madsim::Config::default());
                rt.block_on(async { nata_sim::engine::process(&(0..100).collect::<Vec<i64>>(), 8, 1).len() })
            });
            match r {
                Ok(n) => println!("MADSIM PROBE-THREAD ran the real scheduler inside the simulation, {n} rows (MADSIM_ALLOW_SYSTEM_THREAD={:?})", std::env::var("MADSIM_ALLOW_SYSTEM_THREAD").ok()),
                Err(p) => {
                    let m = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                    println!("MADSIM PROBE-THREAD the real scheduler could NOT run inside the simulation: {}", m.lines().next().unwrap_or(""));
                    std::process::exit(1);
                }
            }
        }
        _ => {
            eprintln!("usage: madsim-trial sweep <first> <count> | hashes <first> <count> | probe-thread");
            std::process::exit(2);
        }
    }
}

#[cfg(not(madsim))]
fn main() {
    eprintln!("madsim-trial: build with RUSTFLAGS=\"--cfg madsim\"");
    std::process::exit(2);
}
