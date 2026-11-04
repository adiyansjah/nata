//! turmoil-trial sweep <first> <count> | hashes <first> <count>
use turmoil_trial::run_seed;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let num = |i: usize, d: u64| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);
    nata_sim::engine::set_inline(false); // turmoil does not forbid system threads (nor does it make them deterministic)
    match args.first().map(String::as_str) {
        Some("sweep") => {
            let (first, count) = (num(1, 0), num(2, 200));
            let (mut pass, mut fail, mut first_fail) = (0u64, 0u64, None);
            let t = std::time::Instant::now();
            for seed in first..first + count {
                match std::panic::catch_unwind(|| run_seed(seed)) {
                    Ok(o) if o.fail.is_none() => pass += 1,
                    Ok(o) => {
                        fail += 1;
                        first_fail.get_or_insert(format!("seed={seed} {} (unmapped faults: {})", o.fail.unwrap_or_default(), o.unmapped));
                    }
                    Err(p) => {
                        fail += 1;
                        let m = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                        first_fail.get_or_insert(format!("seed={seed} PANIC {m}"));
                    }
                }
            }
            println!("TURMOIL SWEEP seeds={count} pass={pass} fail={fail} wall_s={:.1}", t.elapsed().as_secs_f64());
            if let Some(f) = first_fail {
                println!("TURMOIL FIRST_FAIL {f}");
            }
            std::process::exit(if fail > 0 { 1 } else { 0 });
        }
        Some("hashes") => {
            let (first, count) = (num(1, 0), num(2, 20));
            for seed in first..first + count {
                match std::panic::catch_unwind(|| run_seed(seed)) {
                    Ok(o) => println!("TURMOIL HASH seed={seed} hash={:016x} events={}", o.hash, o.events),
                    Err(_) => println!("TURMOIL HASH seed={seed} PANIC"),
                }
            }
        }
        _ => {
            eprintln!("usage: turmoil-trial sweep <first> <count> | hashes <first> <count>");
            std::process::exit(2);
        }
    }
}
