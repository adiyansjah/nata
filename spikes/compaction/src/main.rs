//! compaction-bench: `bench` (the grid) and `selftest` (the harness detects an injected cost).
//!
//!   compaction-bench bench [--rounds N] [--nulls PCT] [--types int64,dec64,dec128,utf8,dict] [--shapes random,runs,mask]
//!                          [--sels 0.1,1,10,50,90,100] [--sizes 1024,8192,65536] [--csv FILE]
//!   compaction-bench selftest [rounds]
//!
//! Markdown on stdout, progress on stderr. Exit status 1 if an arm disagrees with the reference or the self-test fails.

use std::process::ExitCode;

use compaction::data::{Kind, Shape};
use compaction::grid::{GridOpts, build_fixture, Cell, csv, report, run_grid};
use compaction::harness::{load1, selftest, Verdict};

fn list<T>(s: &str, parse: impl Fn(&str) -> Option<T>) -> Vec<T> {
    s.split(',').map(|x| parse(x.trim()).unwrap_or_else(|| panic!("cannot parse '{x}'"))).collect()
}

fn host() -> String {
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("model name") || l.starts_with("Model name") || l.starts_with("CPU part")).map(|l| l.split(':').nth(1).unwrap_or("").trim().to_string()))
        .unwrap_or_else(|| "?".into());
    format!("arch {}, cpu {}, {} logical CPUs", std::env::consts::ARCH, cpu, std::thread::available_parallelism().map_or(0, |n| n.get()))
}

fn cmd_bench(args: &[String]) -> Result<(), String> {
    let (mut rounds, mut nulls, mut csv_path) = (25usize, 10.0f64, None);
    let mut o = GridOpts::default_grid(nulls, rounds);
    let mut i = 0;
    while i < args.len() {
        let v = args.get(i + 1).ok_or_else(|| format!("{} needs a value", args[i]))?;
        match args[i].as_str() {
            "--rounds" => rounds = v.parse().map_err(|e| format!("--rounds: {e}"))?,
            "--nulls" => nulls = v.parse().map_err(|e| format!("--nulls: {e}"))?,
            "--types" => o.kinds = list(v, Kind::parse),
            "--shapes" => o.shapes = list(v, Shape::parse),
            "--sels" => o.sels = list(v, |x| x.parse().ok()),
            "--sizes" => o.sizes = list(v, |x| x.parse().ok()),
            "--csv" => csv_path = Some(v.clone()),
            a => return Err(format!("unknown option {a}")),
        }
        i += 2;
    }
    o.rounds = rounds;
    o.nulls_pct = nulls;
    o.sels.sort_by(f64::total_cmp);
    let load_start = load1();
    println!("# Compaction before arrow wrappers: what the copy costs\n");
    println!("Host: {}. Release build, overflow-checks on for this crate, off for dependencies. Data: in-cache batches, warm; compaction copies one column and allocates its output on every call.\n", host());
    let (results, load_max) = run_grid(&o)?;
    print!("{}", report(&results, rounds, nulls, load_start, load_max, load1()));
    if let Some(p) = csv_path {
        std::fs::write(&p, csv(&results)).map_err(|e| format!("{p}: {e}"))?;
    }
    Ok(())
}

fn cmd_selftest(args: &[String]) -> Result<(), String> {
    let rounds: usize = args.first().map_or(Ok(41), |s| s.parse().map_err(|e| format!("rounds: {e}")))?;
    let c = Cell { kind: Kind::Int64, shape: Shape::Random, sel_pct: 10.0, n: 8192, nulls_pct: 10.0 };
    let fx = build_fixture(&c, 7);
    let l0 = load1();
    let (rows, base, ok) = selftest(&fx, rounds);
    println!("## Harness self-test (Int64, 8192 rows, 10% random idx, 10% NULLs; arm A = {base:.0} ns per call; {rounds} rounds; load {l0:.2} -> {:.2})\n", load1());
    println!("An arm with a known extra cost injected per call must be detected as slower by that amount; an identical twin must come out the same.\n");
    println!("| arm | injected ns/call | measured difference to A (paired median) | tolerance | verdict | expected | ok |");
    println!("|---|---:|---:|---:|---|---|---|");
    let name = |v: Verdict| match v { Verdict::Slower => "slower", Verdict::Same => "same", Verdict::Faster => "faster" };
    for r in &rows {
        println!("| {} | {} | {:.0} | +-{:.0} | {} | {} | {} |", r.label, r.injected_ns, r.measured_delta_ns, r.tolerance_ns, name(r.verdict), name(r.expected), if r.ok { "yes" } else { "NO" });
    }
    if ok { println!("\nSELF-TEST PASSED"); Ok(()) } else { println!("\nSELF-TEST FAILED"); Err("self-test failed".into()) }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("bench") => cmd_bench(&args[1..]),
        Some("selftest") => cmd_selftest(&args[1..]),
        _ => Err("usage: compaction-bench bench [options] | selftest [rounds]".into()),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ERROR: {e}");
            ExitCode::from(1)
        }
    }
}
