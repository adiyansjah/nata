//! `objscan`: the command-line front end used by `gcp-check.sh`. See the README block of that script.
//!
//!   serve      the SIMULATED object store (a separate process)
//!   ctl        send a control command to it / read its counters
//!   selftest   the harness sees an injected latency, bandwidth cap and tail (hermetic: its own server)
//!   expected   write the reference result of each query (DuckDB on the local base file)
//!   layouts    shape of the generated layouts, checked against what was asked for
//!   check      every policy and decode mode give the reference result (simulated store)
//!   membound   the bridge's memory bound with a slow consumer, and the unbounded control (simulated store)
//!   cancel     cancellation, LIMIT, and the known-bad variants (simulated store)
//!   faults     transient and permanent errors, retries, and the known-bad variants (simulated store)
//!   bench      the throughput / request-pattern measurement, simulated or real
//!   real-status / real-check / upload   the real store (coordinates ONLY from environment variables)
//!   redact / leakscan                   keep the real store's coordinates out of every file

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use spike_object_store_scan::bench::*;
use spike_object_store_scan::duck::{DuckCfg, expected_local};
use spike_object_store_scan::exec::*;
use spike_object_store_scan::layouts::*;
use spike_object_store_scan::scan::*;
use spike_object_store_scan::scenarios::*;
use spike_object_store_scan::sim::{Profile, SimCtl, SimServer};
use spike_object_store_scan::store::*;
use spike_object_store_scan::util::nproc;

struct Args(Vec<String>);

impl Args {
    fn opt(&self, k: &str) -> Option<String> {
        self.0.iter().position(|a| a == k).and_then(|i| self.0.get(i + 1)).cloned()
    }
    fn req(&self, k: &str) -> Result<String, String> {
        self.opt(k).ok_or_else(|| format!("missing {k}"))
    }
    fn has(&self, k: &str) -> bool {
        self.0.iter().any(|a| a == k)
    }
    fn num<T: std::str::FromStr>(&self, k: &str, default: T) -> T {
        self.opt(k).and_then(|v| v.parse().ok()).unwrap_or(default)
    }
    fn list(&self, k: &str) -> Option<Vec<String>> {
        self.opt(k).map(|v| v.split(',').map(str::to_string).collect())
    }
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = argv.first().cloned() else {
        eprintln!("usage: objscan <serve|ctl|selftest|expected|layouts|check|membound|cancel|faults|bench|real-status|real-check|upload|redact|leakscan> ...");
        return ExitCode::from(2);
    };
    let args = Args(argv[1..].to_vec());
    let r = match cmd.as_str() {
        "serve" => cmd_serve(&args),
        "ctl" => cmd_ctl(&args),
        "selftest" => cmd_selftest(&args),
        "expected" => cmd_expected(&args),
        "layouts" => cmd_layouts(&args),
        "check" => cmd_check(&args),
        "membound" => cmd_membound(&args),
        "cancel" => cmd_cancel(&args),
        "faults" => cmd_faults(&args),
        "bench" => cmd_bench(&args),
        "real-status" => cmd_real_status(),
        "real-check" => cmd_real_check(&args),
        "upload" => cmd_upload(&args),
        "real-readonly" => cmd_real_readonly(),
        "redact" => cmd_redact(),
        "leakscan" => cmd_leakscan(&args),
        c => Err(format!("unknown command {c}")),
    };
    match r {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            // anything that may have come from the real store was redacted where it was produced; redact again here
            eprintln!("error: {}", Redactor::from_env().apply(&e));
            ExitCode::from(2)
        }
    }
}

// ------------------------------------------------------------------ helpers

fn sim_env(a: &Args) -> Result<SimEnv, String> {
    SimEnv::connect(&a.req("--addr")?, &a.opt("--bucket").unwrap_or_else(|| "sim".into()))
}

fn read_expected(dir: &Path) -> Result<HashMap<String, String>, String> {
    let mut m = HashMap::new();
    for q in [&Q_SUM, &Q_GRP] {
        let p = dir.join(format!("{}.txt", q.name));
        m.insert(q.name.to_string(), std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?);
    }
    Ok(m)
}

fn layouts_from(a: &Args) -> Result<Vec<&'static LayoutDef>, String> {
    match a.list("--layouts") {
        None => Ok(LAYOUTS.iter().collect()),
        Some(v) => v.iter().map(|n| layout(n).ok_or_else(|| format!("unknown layout {n}"))).collect(),
    }
}

fn finish(title: &str, checks: &[Check], mutants: &[MutantRun]) -> u8 {
    print_checks(title, checks);
    if !mutants.is_empty() {
        print_mutants(&format!("{title}: known-bad versions (each must be caught)"), mutants);
    }
    let bad = checks.iter().filter(|c| !c.ok).count();
    let missed = mutants.iter().filter(|m| !m.caught()).count();
    println!("RESULT {title}: {} checks, {bad} failed; {} known-bad versions, {missed} not caught", checks.len(), mutants.len());
    u8::from(bad > 0 || missed > 0)
}

// ------------------------------------------------------------------ commands

fn cmd_serve(a: &Args) -> Result<u8, String> {
    let root = PathBuf::from(a.req("--root")?);
    let bucket = a.opt("--bucket").unwrap_or_else(|| "sim".into());
    let profile = Profile::named(&a.opt("--profile").unwrap_or_else(|| "gcs".into())).ok_or("unknown profile")?;
    let rt = io_runtime(4);
    let server = rt
        .block_on(SimServer::start(root, &bucket, profile, a.num("--seed", 42u64), a.num("--port", 0u16)))
        .map_err(|e| format!("cannot start the simulated store: {e}"))?;
    let addr = server.addr.to_string();
    println!("LISTEN {addr} (SIMULATED object store)");
    std::io::stdout().flush().ok();
    if let Some(f) = a.opt("--port-file") {
        std::fs::write(&f, format!("{addr}\n")).map_err(|e| e.to_string())?;
    }
    while !server.state.shutdown.load(std::sync::atomic::Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(0)
}

fn cmd_ctl(a: &Args) -> Result<u8, String> {
    let addr = a.req("--addr")?;
    let ctl = SimCtl::new(addr);
    if a.has("--stats") {
        let mut v: Vec<_> = ctl.stats()?.into_iter().collect();
        v.sort();
        for (k, x) in v {
            println!("{k}={x}");
        }
        return Ok(0);
    }
    let cmds = a.req("--cmd")?.replace(';', "\n");
    ctl.command(&cmds)?;
    Ok(0)
}

fn cmd_selftest(_a: &Args) -> Result<u8, String> {
    let dir = std::env::temp_dir().join(format!("objscan-selftest-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("small.bin"), vec![7u8; 8192]).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("big.bin"), vec![9u8; 20 * 1024 * 1024]).map_err(|e| e.to_string())?;
    let host = SimHost::start(&dir, "sim", Profile::zero());
    let env = SimEnv::connect(&host.addr(), "sim")?;
    let checks = selftest(&env, "small.bin", "big.bin");
    drop(env);
    drop(host);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(finish("harness self-test", &checks, &[]))
}

fn cmd_expected(a: &Args) -> Result<u8, String> {
    let li = a.req("--lineitem")?;
    let out = PathBuf::from(a.req("--out")?);
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    for q in [&Q_SUM, &Q_GRP] {
        let t = expected_local(&format!("read_parquet('{li}')"), q)?;
        std::fs::write(out.join(format!("{}.txt", q.name)), &t).map_err(|e| e.to_string())?;
        println!("expected {}: {}", q.name, t.lines().next().unwrap_or(""));
    }
    Ok(0)
}

fn cmd_layouts(a: &Args) -> Result<u8, String> {
    use parquet::file::reader::{FileReader, SerializedFileReader};
    let data = PathBuf::from(a.req("--data")?);
    let mut md = String::from("| layout | files | row groups | rows | MiB | rows per row group (avg) | shape check |\n|---|---|---|---|---|---|---|\n");
    let mut bad = 0;
    let mut rows_seen: Option<i64> = None;
    for l in layouts_from(a)? {
        let (mut rgs, mut rows, mut bytes) = (0usize, 0i64, 0u64);
        let mut nfiles = 0;
        for i in 0..l.files {
            let p = data.join(rel_key(l.name, i));
            let f = std::fs::File::open(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            bytes += f.metadata().map_err(|e| e.to_string())?.len();
            let r = SerializedFileReader::new(f).map_err(|e| e.to_string())?;
            rgs += r.metadata().num_row_groups();
            rows += r.metadata().file_metadata().num_rows();
            nfiles += 1;
        }
        let avg = rows as f64 / rgs.max(1) as f64;
        let shape_ok = nfiles == l.files
            && rows_seen.is_none_or(|r| r == rows)
            && match l.name {
                "big1" => (2..=8).contains(&rgs),
                "files100" => rgs == l.files,
                _ => avg >= l.rows_per_rg as f64 * 0.5 && avg <= l.rows_per_rg as f64 * 1.05,
            };
        rows_seen.get_or_insert(rows);
        if !shape_ok {
            bad += 1;
        }
        md.push_str(&format!(
            "| {} | {nfiles} | {rgs} | {rows} | {:.1} | {avg:.0} | {} |\n",
            l.name,
            bytes as f64 / 1_048_576.0,
            if shape_ok { "ok" } else { "WRONG SHAPE" }
        ));
    }
    println!("{md}");
    if let Some(o) = a.opt("--out") {
        std::fs::write(o, &md).map_err(|e| e.to_string())?;
    }
    Ok(u8::from(bad > 0))
}

fn cmd_check(a: &Args) -> Result<u8, String> {
    let env = sim_env(a)?;
    let expected = read_expected(&PathBuf::from(a.req("--expected")?))?;
    let get = move |q: &'static QuerySpec| -> String {
        if q.limit.is_some() { "1 row(s)\n".to_string() } else { expected.get(q.name).cloned().unwrap_or_default() }
    };
    let mut all = Vec::new();
    for l in layouts_from(a)? {
        let ds = env.dataset(l.name, l.files);
        all.extend(correctness(&env, &ds, &get, &[&Q_SUM, &Q_GRP, &Q_LIMIT1], l.name));
        if !a.has("--no-duck") {
            // DuckDB over the same store must also equal the reference
            let d = DuckCfg::from_spec(&env.spec, nproc());
            for q in [&Q_SUM, &Q_GRP] {
                let r = d.run(l.name, l.files, q);
                let (ok, detail) = match r {
                    Ok(x) => (normalize(&x.text) == normalize(&get(q)), format!("{:.2} s", x.seconds)),
                    Err(e) => (false, e),
                };
                all.push(chk(format!("correct/{}/{}/duckdb over the simulated store", l.name, q.name), ok, detail));
            }
        }
    }
    Ok(finish("correctness against the reference", &all, &[]))
}

fn cmd_membound(a: &Args) -> Result<u8, String> {
    let env = sim_env(a)?;
    let expected = read_expected(&PathBuf::from(a.req("--expected")?))?;
    let l = layout(&a.opt("--layout").unwrap_or_else(|| "small1".into())).ok_or("unknown layout")?;
    let ds = env.dataset(l.name, l.files);
    let (c, m) = membound(&env, &ds, &Q_GRP, &expected["group"]);
    Ok(finish(&format!("memory bound ({})", l.name), &c, &m))
}

fn cmd_cancel(a: &Args) -> Result<u8, String> {
    let env = sim_env(a)?;
    let l = layout(&a.opt("--layout").unwrap_or_else(|| "mid1".into())).ok_or("unknown layout")?;
    let ds = env.dataset(l.name, l.files);
    let (c, m) = cancellation(&env, &ds);
    Ok(finish(&format!("cancellation ({})", l.name), &c, &m))
}

fn cmd_faults(a: &Args) -> Result<u8, String> {
    let env = sim_env(a)?;
    let expected = read_expected(&PathBuf::from(a.req("--expected")?))?;
    let l = layout(&a.opt("--layout").unwrap_or_else(|| "files100".into())).ok_or("unknown layout")?;
    let victim = a.opt("--victim").unwrap_or_else(|| if l.files > 1 { "part-0042".into() } else { l.name.into() });
    let ds = env.dataset(l.name, l.files);
    let (mut c, m) = faults(&env, &ds, &Q_SUM, &expected["sum"], &victim);
    let names: Vec<String> = (0..l.files).map(file_name).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    c.extend(builtin_retry_reference(&env, &refs, l.name, &Q_SUM, &expected["sum"]));
    Ok(finish(&format!("retries and errors ({})", l.name), &c, &m))
}

fn cmd_bench(a: &Args) -> Result<u8, String> {
    let mode = a.req("--mode")?;
    let rounds: usize = a.num("--rounds", 5);
    let out = PathBuf::from(a.req("--out")?);
    let expected = read_expected(&PathBuf::from(a.req("--expected")?))?;
    let data = a.opt("--data").map(PathBuf::from);
    let queries: Vec<&'static QuerySpec> = match a.list("--queries") {
        None => vec![&Q_SUM, &Q_GRP],
        Some(v) => v.iter().map(|n| query(n).ok_or_else(|| format!("unknown query {n}"))).collect::<Result<_, _>>()?,
    };
    let with_duck = !a.has("--no-duck");
    let with_local = !a.has("--no-local") && data.is_some();
    let rt = io_runtime(2);
    let (label, spec, ctl) = match mode.as_str() {
        "sim" => {
            let addr = a.req("--addr")?;
            let pname = a.opt("--profile").unwrap_or_else(|| "gcs".into());
            let p = Profile::named(&pname).ok_or("unknown profile")?;
            let ctl = SimCtl::new(addr.clone());
            ctl.clean()?;
            ctl.command(&format!("profile {pname}"))?;
            let mut spec = StoreSpec::sim(&addr, &a.opt("--bucket").unwrap_or_else(|| "sim".into()));
            spec.label = format!(
                "SIMULATED local server on loopback, profile '{pname}': first byte {} ms median (log-normal sigma {}), tail {} percent from {} ms (Pareto shape {}, cap {} ms), {} ms per MiB per connection, total cap {} MiB/s (0 = none)",
                p.median_ms, p.sigma, p.tail_prob * 100.0, p.tail_min_ms, p.tail_alpha, p.tail_cap_ms, p.per_mb_ms, p.bandwidth_mb_s
            );
            ("SIMULATED", spec, Some(ctl))
        }
        "real" => match StoreSpec::real_from_env() {
            RealConfig::Configured(s) => ("REAL", s, None),
            RealConfig::NotConfigured => {
                println!("REAL STORE NOT MEASURED: no {ENV_ENDPOINT}, {ENV_BUCKET} and {ENV_PREFIX} in the environment");
                return Ok(3);
            }
            RealConfig::Incomplete(m) => return Err(format!("the real store is half configured; missing: {}", m.join(", "))),
        },
        m => return Err(format!("unknown --mode {m}")),
    };
    let store = spec.build(None)?;
    let cfg = BenchCfg {
        label: label.into(),
        rounds,
        layouts: layouts_from(a)?,
        queries,
        arms: default_arms(with_duck, with_local),
        duck: with_duck.then(|| DuckCfg::from_spec(&spec, nproc())),
        spec,
        store,
        ctl,
        expected,
        local_dir: data,
        out_dir: out,
        sanity_only: a.has("--sanity-only"),
        nproc: nproc(),
    };
    match run_bench(&rt, &cfg) {
        Ok(recs) => {
            let bad = recs.iter().filter(|r| !r.ok).count();
            println!("RESULT bench {label}: {} runs, {bad} not ok", recs.len());
            Ok(u8::from(bad > 0))
        }
        Err(e) => {
            println!("RESULT bench {label}: {e}");
            Ok(3)
        }
    }
}

fn cmd_real_status() -> Result<u8, String> {
    match StoreSpec::real_from_env() {
        RealConfig::Configured(s) => {
            println!("configured ({}; credentials: {})", s.label, if s.gcs { "the service account of this VM (metadata server), none configured" } else if s.has_credentials() { "HMAC from the environment" } else { "none, anonymous reads" });
            Ok(0)
        }
        RealConfig::NotConfigured => {
            println!("not-configured");
            Ok(3)
        }
        RealConfig::Incomplete(m) => {
            println!("incomplete: missing {}", m.join(", "));
            Ok(2)
        }
    }
}

fn cmd_real_check(a: &Args) -> Result<u8, String> {
    let RealConfig::Configured(spec) = StoreSpec::real_from_env() else {
        return Err("the real store is not configured".into());
    };
    let store = spec.build(None)?;
    let rt = io_runtime(2);
    let mut missing = 0;
    for l in layouts_from(a)? {
        let ds = dataset(Arc::clone(&store), &spec, l.name, l.files);
        let cfg = ScanConfig::new(Policy::b(), DecodeMode::Pool);
        match plan_stats(&rt, &ds, &Q_SUM, &cfg) {
            Ok(p) => println!("layout {}: {} file(s) readable, {} row groups", l.name, p.files, p.row_groups),
            Err(e) => {
                missing += 1;
                println!("layout {}: NOT READABLE ({})", l.name, spec.redactor().apply(&e));
            }
        }
    }
    Ok(if missing > 0 { 4 } else { 0 })
}

fn cmd_upload(a: &Args) -> Result<u8, String> {
    use object_store::{ObjectStoreExt, PutPayload};
    let RealConfig::Configured(spec) = StoreSpec::real_from_env() else {
        return Err("the real store is not configured".into());
    };
    if !spec.has_credentials() {
        return Err(format!("uploading needs {ENV_KEY_ID} and {ENV_SECRET} with write access to the test prefix"));
    }
    let data = PathBuf::from(a.req("--data")?);
    let store = spec.build(None)?;
    let rt = io_runtime(2);
    let mut n = 0;
    for l in layouts_from(a)? {
        for i in 0..l.files {
            let bytes = std::fs::read(data.join(rel_key(l.name, i))).map_err(|e| e.to_string())?;
            let path = spec.path(&rel_key(l.name, i));
            rt.block_on(store.put(&path, PutPayload::from(bytes))).map_err(|e| spec.redactor().apply(&e.to_string()))?;
            n += 1;
        }
        println!("uploaded layout {}", l.name);
    }
    println!("uploaded {n} file(s)");
    Ok(0)
}

/// The identity the VM runs with must be READ-ONLY. Tries to write a probe object under the test prefix and requires that the store refuses; if the write is
/// accepted the probe is deleted again and the exit status is 7. (Added 2026-11-05: the service account could not be tested from outside, acting as it needs
/// a role on the operator's own account, so the VM tests itself.)
fn cmd_real_readonly() -> Result<u8, String> {
    use object_store::{ObjectStoreExt, PutPayload};
    let RealConfig::Configured(spec) = StoreSpec::real_from_env() else {
        return Err("the real store is not configured".into());
    };
    let store = spec.build(None)?;
    let rt = io_runtime(2);
    let probe = spec.path(".write-probe-must-be-refused");
    match rt.block_on(store.put(&probe, PutPayload::from(b"probe".to_vec()))) {
        Err(e) => {
            let msg = spec.redactor().apply(&e.to_string());
            println!("READ-ONLY OK: the write was refused ({})", msg.chars().take(120).collect::<String>());
            Ok(0)
        }
        Ok(_) => {
            let _ = rt.block_on(store.delete(&probe));
            println!("READ-ONLY VIOLATED: the VM identity can WRITE to the test prefix (the probe object was deleted again)");
            Ok(7)
        }
    }
}

fn cmd_redact() -> Result<u8, String> {
    let r = Redactor::from_env();
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s).map_err(|e| e.to_string())?;
    print!("{}", r.apply(&s));
    Ok(0)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
}

fn leak_scan(dir: &Path, r: &Redactor) -> Vec<PathBuf> {
    let mut files = Vec::new();
    walk(dir, &mut files);
    files
        .into_iter()
        .filter(|p| std::fs::read(p).is_ok_and(|b| r.leaks(&String::from_utf8_lossy(&b))))
        .collect()
}

fn cmd_leakscan(a: &Args) -> Result<u8, String> {
    if a.has("--selftest") {
        // the scan must find a planted value and must not flag a clean directory
        let r = Redactor::new(vec!["planted-secret-value-123".into()]);
        let d = std::env::temp_dir().join(format!("objscan-leak-{}", std::process::id()));
        std::fs::create_dir_all(d.join("sub")).map_err(|e| e.to_string())?;
        std::fs::write(d.join("sub/clean.txt"), "nothing here").map_err(|e| e.to_string())?;
        let clean = leak_scan(&d, &r).is_empty();
        std::fs::write(d.join("sub/dirty.txt"), "an error mentioning planted-secret-value-123 in the middle").map_err(|e| e.to_string())?;
        let found = leak_scan(&d, &r).len() == 1;
        let _ = std::fs::remove_dir_all(&d);
        println!("{} leakscan self-test: clean directory not flagged: {clean}; planted value found: {found}", if clean && found { "PASS" } else { "FAIL" });
        return Ok(u8::from(!(clean && found)));
    }
    let dir = PathBuf::from(a.req("--dir")?);
    let r = Redactor::from_env();
    let leaks = leak_scan(&dir, &r);
    if leaks.is_empty() {
        println!("leakscan: none of the configured endpoint, bucket, prefix or key values appears under the results directory");
        Ok(0)
    } else {
        for p in &leaks {
            println!("LEAK in {}", p.display());
        }
        Ok(1)
    }
}
