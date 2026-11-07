//! The mutant sweep and the hostile-allocation cases of the object-store wrapper (needs `--features mutants`): `cargo run -p nata-objstore --features mutants
//! --release --bin objstore-sweep -- faults|cache|hostile|hostile-all|mutants [--mutant NAME] [--cache-mutant NAME]`.
//!
//! * `faults`, `cache`: the in-process cases (`cases/`); exit 1 if one fails. With `--mutant`, the wrapper has that one deliberate defect.
//! * `hostile NAME`: one case that could allocate without bound; `hostile-all` runs each in a CHILD under `ulimit -v` (the allocation bound is the thing
//!   under test: a missing bound kills the child, which is a FAIL).
//! * `mutants`: the clean control first (must pass), then every mutant in a child process: CAUGHT = at least one case fails, SURVIVED = all pass.

use std::process::{Command, Stdio};

#[path = "../../cases/mod.rs"]
mod cases;

use cases::{cache as cache_cases, fault};
use nata_objstore::policy::{CacheMutants, Mutants};

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn mutant_by_name(name: &str) -> Mutants {
    if name.is_empty() {
        return Mutants::default();
    }
    Mutants::all()
        .into_iter()
        .find(|(n, _)| *n == name)
        .map(|(_, m)| m)
        .unwrap_or_else(|| {
            eprintln!("unknown mutant {name}");
            std::process::exit(2);
        })
}

fn cache_mutant_by_name(name: &str) -> CacheMutants {
    if name.is_empty() {
        return CacheMutants::default();
    }
    CacheMutants::all()
        .into_iter()
        .find(|(n, _)| *n == name)
        .map(|(_, m)| m)
        .unwrap_or_else(|| {
            eprintln!("unknown cache mutant {name}");
            std::process::exit(2);
        })
}

fn print_case(c: &fault::CaseResult) {
    println!(
        "CASE {}: {} {}",
        c.name,
        if c.ok { "PASS" } else { "FAIL" },
        c.detail
    );
}

fn child(exe: &std::path::Path, a: &[&str]) -> (i32, Vec<String>) {
    let out = Command::new(exe)
        .args(a)
        .env("MALLOC_ARENA_MAX", "2")
        .stdin(Stdio::null())
        .output()
        .expect("child process");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let failed = text
        .lines()
        .filter(|l| l.starts_with("CASE ") && l.contains(": FAIL"))
        .map(|l| l[5..].split(':').next().unwrap_or("").to_string())
        .collect();
    (out.status.code().unwrap_or(-1), failed)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("runtime");
    match args.first().map(String::as_str).unwrap_or("") {
        "faults" => {
            let m = mutant_by_name(&arg(&args, "--mutant").unwrap_or_default());
            let res = rt.block_on(fault::all_cases(&m));
            res.iter().for_each(print_case);
            let bad = res.iter().filter(|c| !c.ok).count();
            println!("RESULT faults: {} cases, {} failed", res.len(), bad);
            std::process::exit(i32::from(bad != 0));
        }
        "cache" => {
            let m = mutant_by_name(&arg(&args, "--mutant").unwrap_or_default());
            let cm = cache_mutant_by_name(&arg(&args, "--cache-mutant").unwrap_or_default());
            let res = rt.block_on(cache_cases::cache_cases(&m, &cm));
            res.iter().for_each(print_case);
            let bad = res.iter().filter(|c| !c.ok).count();
            println!("RESULT cache: {} cases, {} failed", res.len(), bad);
            std::process::exit(i32::from(bad != 0));
        }
        "hostile" => {
            let m = mutant_by_name(&arg(&args, "--mutant").unwrap_or_default());
            let name = args.get(1).cloned().unwrap_or_default();
            let c = rt.block_on(fault::hostile(&name, &m));
            print_case(&c);
            std::process::exit(i32::from(!c.ok));
        }
        "hostile-all" => {
            let limit_kib: u64 = arg(&args, "--ulimit-mib")
                .and_then(|s| s.parse().ok())
                .unwrap_or(1536)
                * 1024;
            let m = arg(&args, "--mutant").unwrap_or_default();
            let exe = std::env::current_exe().expect("current exe");
            let mut bad = 0;
            for name in fault::HOSTILE_CASES {
                let script =
                    format!("ulimit -v {limit_kib}; exec \"$0\" hostile {name} --mutant '{m}'");
                let out = Command::new("sh")
                    .arg("-c")
                    .arg(&script)
                    .arg(&exe)
                    .env("MALLOC_ARENA_MAX", "2")
                    .stdin(Stdio::null())
                    .output()
                    .expect("sh");
                let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let line = if text.is_empty() {
                    format!(
                        "CASE {name}: FAIL process ended without a result (status {:?}: killed or allocation failure under ulimit -v {} MiB)",
                        out.status.code(),
                        limit_kib / 1024
                    )
                } else {
                    text
                };
                if !line.contains(": PASS") {
                    bad += 1;
                }
                println!("{line}");
            }
            println!(
                "RESULT hostile: {} cases, {} failed (ulimit -v {} MiB)",
                fault::HOSTILE_CASES.len(),
                bad,
                limit_kib / 1024
            );
            std::process::exit(i32::from(bad != 0));
        }
        "mutants" => {
            let exe = std::env::current_exe().expect("current exe");
            let mut invalid = false;
            for (label, a) in [
                ("faults", vec!["faults"]),
                ("cache", vec!["cache"]),
                ("hostile", vec!["hostile-all"]),
            ] {
                let (code, failed) = child(&exe, &a);
                println!(
                    "CONTROL {label}: {}",
                    if code == 0 {
                        "all pass".to_string()
                    } else {
                        format!("FAILED {failed:?} (exit {code})")
                    }
                );
                invalid |= code != 0;
            }
            if invalid {
                println!("RESULT mutants: INVALID (the unmutated control fails)");
                std::process::exit(3);
            }
            let (mut caught, mut survived) = (0, 0);
            for (name, _) in Mutants::all() {
                let mut by: Vec<String> = Vec::new();
                for a in [
                    vec!["faults", "--mutant", name],
                    vec!["cache", "--mutant", name],
                    vec!["hostile-all", "--mutant", name],
                ] {
                    let (code, failed) = child(&exe, &a);
                    if code != 0 {
                        by.push(if failed.is_empty() {
                            format!("{} (process died, exit {code})", a[0])
                        } else {
                            failed.join(",")
                        });
                    }
                }
                if by.is_empty() {
                    survived += 1;
                    println!("MUTANT {name}: SURVIVED");
                } else {
                    caught += 1;
                    println!("MUTANT {name}: CAUGHT by {}", by.join(" | "));
                }
            }
            for (name, _) in CacheMutants::all() {
                let (code, failed) = child(&exe, &["cache", "--cache-mutant", name]);
                if code != 0 {
                    caught += 1;
                    println!("MUTANT cache:{name}: CAUGHT by {}", failed.join(","));
                } else {
                    survived += 1;
                    println!("MUTANT cache:{name}: SURVIVED");
                }
            }
            println!("RESULT mutants: {caught} caught, {survived} survived");
            std::process::exit(i32::from(survived != 0));
        }
        _ => {
            eprintln!(
                "usage: objstore-sweep faults|cache|hostile NAME|hostile-all|mutants [--mutant NAME] [--cache-mutant NAME]"
            );
            std::process::exit(2);
        }
    }
}
