//! ic-spike: the commit-safety spike. Subcommands:
//!   suite    --backend rest|store [--url U] --work DIR [--seeds N] [--writers N] [--ops N] [--kills N]
//!            forced scenarios (correct writer must pass, each known-bad writer must be caught), the crash matrix, and the
//!            stress runs (N writer processes with injected faults and SIGKILLs). Exit 1 on any mismatch.
//!   writer   (internal) one writer process of the stress run
//!   mock     --work DIR            run the REST test double until killed (prints its URL)
//!   interop-table --backend rest --url U --work DIR   build a table with every operation; print the files each snapshot has (JSON)
//!   check    --backend rest|store [--url U] --table T [--dir D] --journal J    run the checker on an existing table
use iceberg_commit::catalog::{Catalog, RestCatalog, StoreCatalog};
use iceberg_commit::suite::{self, Backend, ScenarioResult, StressCfg};
use iceberg_commit::writer::{Isolation, Mode};
use iceberg_commit::{checker, mock};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

fn parse_args(args: &[String]) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    let mut i = 0;
    while i < args.len() {
        if let Some(k) = args[i].strip_prefix("--") {
            let v = args.get(i + 1).filter(|v| !v.starts_with("--")).cloned();
            match v {
                Some(v) => {
                    m.insert(k.to_string(), v);
                    i += 2;
                }
                None => {
                    m.insert(k.to_string(), "true".to_string());
                    i += 1;
                }
            }
        } else {
            i += 1;
        }
    }
    m
}

fn backend_of(a: &BTreeMap<String, String>) -> Result<Backend, String> {
    match a.get("backend").map(|s| s.as_str()) {
        Some("rest") => Ok(Backend::Rest(a.get("url").cloned().ok_or("--url is required for --backend rest")?)),
        Some("store") => Ok(Backend::Store),
        other => Err(format!("--backend rest|store required, got {other:?}")),
    }
}

fn num<T: std::str::FromStr>(a: &BTreeMap<String, String>, k: &str, default: T) -> T {
    a.get(k).and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn print_results(title: &str, results: &[ScenarioResult], failures: &mut u32) {
    println!("\n== {title}");
    for r in results {
        println!("{}", r.line());
        if !r.ok() {
            *failures += 1;
            if let Some(e) = &r.error {
                println!("    error: {e}");
            }
            for l in r.report.render().lines() {
                println!("    {l}");
            }
            println!("    outcomes: {:?}", r.outcomes);
        } else if r.report.violations.is_empty() {
            let o = r.report.orphans.len();
            if o > 0 {
                println!("    ({} orphan file(s) left behind, expected after a crash or an aborted operation)", o);
            }
        } else {
            // a detected known-bad writer: show what the checker said, once
            for v in r.report.violations.iter().take(2) {
                println!("    checker says {}: {}", v.inv, v.msg);
            }
        }
    }
}

fn suite_cmd(a: &BTreeMap<String, String>) -> Result<bool, String> {
    let backend = backend_of(a)?;
    let work = PathBuf::from(a.get("work").ok_or("--work DIR required")?);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut failures = 0u32;

    let forced = suite::run_all_forced(&backend, &work);
    print_results("forced interleavings: correct writer must pass, each known-bad writer must be caught", &forced, &mut failures);
    let n_bad = forced.iter().filter(|r| matches!(r.expect, suite::Expect::Fail(_))).count();
    let n_bad_ok = forced.iter().filter(|r| matches!(r.expect, suite::Expect::Fail(_)) && r.ok()).count();

    let crash = suite::run_crash_matrix(&backend, &work);
    print_results("crash matrix: a writer dies at each step, the others carry on", &crash, &mut failures);

    println!("\n== stress: writer processes with lost responses/requests, duplicates, crash points and SIGKILL (correct writers: every seed must pass)");
    let seeds: u64 = num(a, "seeds", 6);
    let ops: u32 = num(a, "ops", 3);
    let kills: u32 = num(a, "kills", 1);
    let default_writers: u32 = num(a, "writers", 0);
    let sizes = [4u32, 8, 16];
    let mut stress_ok = 0;
    for s in 1..=seeds {
        let writers = if default_writers > 0 { default_writers } else { sizes[(s as usize - 1) % sizes.len()] };
        let cfg = StressCfg { writers, ops_per_writer: ops, seed: s, mode: Mode::Correct, isolation: Isolation::Serializable, kills, p_fault: 0.25, p_crash: 0.2 };
        let r = suite::run_stress(&backend, &work, &exe, &cfg);
        let ok = r.error.is_none() && r.report.ok();
        println!("{:<8} stress seed {s:<3} writers {writers:<3} ops {ops} kills {kills}: {}  [{}]", backend.name(), if ok { "OK" } else { "MISMATCH" },
                 r.report.render().lines().next().unwrap_or(""));
        if ok {
            stress_ok += 1;
        } else {
            failures += 1;
            if let Some(e) = &r.error {
                println!("    error: {e}");
            }
            for l in r.report.render().lines().skip(1) {
                println!("    {l}");
            }
            for (id, e) in &r.exits {
                println!("    writer {id}: {e}");
            }
        }
    }

    println!("\n== stress with the known-bad writers (informational: detection needs a race to happen; the forced scenarios above are the gate)");
    for mode in [Mode::BadNoCas, Mode::BadBlindRetry, Mode::BadSkipValidation] {
        let mut detected = 0;
        let runs = 3;
        for s in 1..=runs {
            let cfg = StressCfg { writers: 8, ops_per_writer: ops, seed: 100 + s, mode, isolation: Isolation::Serializable, kills: 0, p_fault: 0.4, p_crash: 0.0 };
            let r = suite::run_stress(&backend, &work, &exe, &cfg);
            if r.error.is_none() && !r.report.ok() {
                detected += 1;
            }
        }
        println!("{:<8} stress {:<20} the checker flagged {detected} of {runs} runs", backend.name(), mode.name());
    }

    println!("\nSUMMARY backend={} forced={} (known-bad writers caught {n_bad_ok}/{n_bad}) crash-matrix={} stress-correct={stress_ok}/{seeds} failures={failures}",
             backend.name(), forced.len(), crash.len());
    println!("{}", if failures == 0 { "SUITE PASS" } else { "SUITE FAIL" });
    Ok(failures == 0)
}

fn check_cmd(a: &BTreeMap<String, String>) -> Result<bool, String> {
    let backend = backend_of(a)?;
    let table = a.get("table").ok_or("--table required")?;
    let cat: Arc<dyn Catalog> = match &backend {
        Backend::Rest(u) => Arc::new(RestCatalog::connect(u, "nata", table)?),
        Backend::Store => Arc::new(StoreCatalog::new(PathBuf::from(a.get("dir").ok_or("--dir required")?))?),
    };
    let r = checker::check(cat.as_ref(), &PathBuf::from(a.get("journal").ok_or("--journal required")?));
    print!("{}", r.render());
    Ok(r.ok())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().cloned() else {
        eprintln!("usage: ic-spike suite|writer|mock|check ... (see src/main.rs)");
        return ExitCode::from(2);
    };
    let a = parse_args(&args[1..]);
    let res: Result<bool, String> = match cmd.as_str() {
        "suite" => suite_cmd(&a),
        "check" => check_cmd(&a),
        "writer" => suite::writer_process(&a).map(|_| true),
        "interop-table" => (|| {
            let backend = backend_of(&a)?;
            let work = PathBuf::from(a.get("work").ok_or("--work DIR required")?);
            std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
            println!("{}", suite::interop_table(&backend, &work)?);
            Ok(true)
        })(),
        "mock" => (|| {
            let work = PathBuf::from(a.get("work").ok_or("--work DIR required")?);
            let m = mock::MockServer::start(work)?;
            println!("{}", m.url);
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        })(),
        other => Err(format!("unknown subcommand {other}")),
    };
    match res {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}
