//! The throughput and request-pattern measurement: layouts x queries x arms x rounds, the arms interleaved (the order
//! rotates every cell and round), with controls, checked against the reference result before and during the timing.
//!
//! Arms: `A`..`E` are the fetch policies (see `scan.rs`); `B-twin` is B again, to show the noise (the control twin);
//! `D-pool` is D with the decode on the CPU pool instead of on the I/O runtime; `local-B` is B reading the same files
//! from local disk (the CPU-bound ceiling: no network); `duckdb` is DuckDB over the same store (`duck.rs`).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use object_store::ObjectStore;
use tokio::runtime::Runtime;

use crate::duck::DuckCfg;
use crate::exec::{QuerySpec, normalize};
use crate::layouts::LayoutDef;
use crate::scan::*;
use crate::sim::SimCtl;
use crate::store::StoreSpec;
use crate::util::{load1, median, percentile, sorted};

#[derive(Clone)]
pub enum ArmKind {
    Ours { policy: Policy, decode: DecodeMode, local: bool },
    Duck,
}

#[derive(Clone)]
pub struct Arm {
    pub name: String,
    pub kind: ArmKind,
}

pub fn default_arms(with_duck: bool, with_local: bool) -> Vec<Arm> {
    let ours = |name: &str, p: Policy, d: DecodeMode, local: bool| Arm {
        name: name.into(),
        kind: ArmKind::Ours { policy: p, decode: d, local },
    };
    let mut v = vec![
        ours("A", Policy::a(), DecodeMode::Tokio, false),
        ours("B", Policy::b(), DecodeMode::Tokio, false),
        ours("B-twin", Policy::b(), DecodeMode::Tokio, false),
        ours("C", Policy::c(), DecodeMode::Tokio, false),
        ours("D", Policy::d(), DecodeMode::Tokio, false),
        ours("E", Policy::e(), DecodeMode::Tokio, false),
        ours("D-pool", Policy::d(), DecodeMode::Pool, false),
    ];
    if with_local {
        v.push(ours("local-B", Policy::b(), DecodeMode::Tokio, true));
    }
    if with_duck {
        v.push(Arm {
            name: "duckdb".into(),
            kind: ArmKind::Duck,
        });
    }
    v
}

pub struct BenchCfg {
    /// "SIMULATED" or "REAL": printed in every output.
    pub label: String,
    pub rounds: usize,
    pub layouts: Vec<&'static LayoutDef>,
    pub queries: Vec<&'static QuerySpec>,
    pub arms: Vec<Arm>,
    pub spec: StoreSpec,
    pub store: Arc<dyn ObjectStore>,
    pub ctl: Option<SimCtl>,
    pub duck: Option<DuckCfg>,
    /// Expected result text per query name.
    pub expected: HashMap<String, String>,
    pub local_dir: Option<PathBuf>,
    pub out_dir: PathBuf,
    /// The machine was not quiet before the timing phase (load >= 2): the report says SANITY ONLY.
    pub sanity_only: bool,
    pub nproc: usize,
}

#[derive(Clone, Debug)]
pub struct RunRec {
    pub round: usize,
    pub layout: String,
    pub query: String,
    pub arm: String,
    pub wall_s: f64,
    pub first_batch_s: f64,
    pub plan_s: f64,
    pub attempts: u64,
    pub data_requests: u64,
    pub footer_requests: u64,
    pub retries: u64,
    pub data_bytes: u64,
    pub footer_bytes: u64,
    pub needed_bytes: u64,
    pub waste_pct: f64,
    pub peak_inflight: usize,
    pub req_p50: f64,
    pub req_p99: f64,
    pub ttfb_p50: f64,
    pub ttfb_p99: f64,
    pub cpu_s: f64,
    pub avg_inflight: f64,
    /// -1 when there is no server-side counter (the real store).
    pub server_requests: i64,
    pub server_bytes: i64,
    pub ok: bool,
    pub note: String,
    pub load1: f64,
}

fn expect_text(cfg: &BenchCfg, q: &QuerySpec) -> String {
    if q.limit.is_some() {
        return format!("{} row(s)\n", q.limit.unwrap_or(0));
    }
    cfg.expected.get(q.name).cloned().unwrap_or_default()
}

fn dataset_for(cfg: &BenchCfg, layout: &LayoutDef, local: bool) -> Result<Dataset, String> {
    if local {
        let d = cfg.local_dir.as_ref().ok_or("the local arm needs --data")?;
        local_dataset(d, layout.name, layout.files)
    } else {
        Ok(dataset(Arc::clone(&cfg.store), &cfg.spec, layout.name, layout.files))
    }
}

fn server_counts(cfg: &BenchCfg) -> (i64, i64) {
    match &cfg.ctl {
        Some(c) => (
            c.stat("requests").map_or(-1, |v| v as i64),
            c.stat("bytes_sent").map_or(-1, |v| v as i64),
        ),
        None => (-1, -1),
    }
}

/// One run of one arm; returns the record and the per-request latency samples (data requests).
fn run_arm(rt: &Runtime, cfg: &BenchCfg, layout: &'static LayoutDef, q: &'static QuerySpec, arm: &Arm, round: usize, needed: u64) -> Result<(RunRec, Vec<f64>, Vec<f64>), String> {
    if let Some(c) = &cfg.ctl {
        c.command("reset")?;
    }
    let exp = expect_text(cfg, q);
    let mut rec = RunRec {
        round,
        layout: layout.name.into(),
        query: q.name.into(),
        arm: arm.name.clone(),
        wall_s: 0.0,
        first_batch_s: f64::NAN,
        plan_s: f64::NAN,
        attempts: 0,
        data_requests: 0,
        footer_requests: 0,
        retries: 0,
        data_bytes: 0,
        footer_bytes: 0,
        needed_bytes: needed,
        waste_pct: f64::NAN,
        peak_inflight: 0,
        req_p50: f64::NAN,
        req_p99: f64::NAN,
        ttfb_p50: f64::NAN,
        ttfb_p99: f64::NAN,
        cpu_s: f64::NAN,
        avg_inflight: f64::NAN,
        server_requests: -1,
        server_bytes: -1,
        ok: false,
        note: String::new(),
        load1: load1().unwrap_or(f64::NAN),
    };
    let (mut total, mut ttfb) = (Vec::new(), Vec::new());
    match &arm.kind {
        ArmKind::Ours { policy, decode, local } => {
            let ds = dataset_for(cfg, layout, *local)?;
            let mut sc = ScanConfig::new(policy.clone(), *decode);
            sc.pool_threads = cfg.nproc;
            let o = run_scan(rt, &ds, q, &sc);
            let (sr, sb) = server_counts(cfg);
            rec.wall_s = o.wall.as_secs_f64();
            rec.first_batch_s = o.first_batch.map_or(f64::NAN, |d| d.as_secs_f64());
            rec.plan_s = o.plan_time.as_secs_f64();
            rec.attempts = o.attempts;
            rec.data_requests = o.data_requests;
            rec.footer_requests = o.footer_requests;
            rec.retries = o.retries;
            rec.data_bytes = o.data_bytes;
            rec.footer_bytes = o.footer_bytes;
            rec.needed_bytes = o.needed_bytes;
            rec.waste_pct = o.waste_pct();
            rec.peak_inflight = o.peak_inflight;
            rec.req_p50 = o.total_ms_pct(50.0);
            rec.req_p99 = o.total_ms_pct(99.0);
            rec.ttfb_p50 = o.ttfb_ms_pct(50.0);
            rec.ttfb_p99 = o.ttfb_ms_pct(99.0);
            rec.cpu_s = o.cpu_s;
            rec.avg_inflight = o.req_total_ms.iter().sum::<f64>() / 1e3 / rec.wall_s.max(1e-9);
            if !*local {
                rec.server_requests = sr;
                rec.server_bytes = sb;
                if sr >= 0 && sr as u64 != o.attempts {
                    rec.note = format!("client counted {} attempts, server {}", o.attempts, sr);
                }
            }
            match (&o.text, &o.error) {
                (Some(t), None) if normalize(t) == normalize(&exp) => rec.ok = true,
                (Some(t), None) => rec.note = format!("WRONG RESULT: {:?}", t.lines().next().unwrap_or("")),
                (_, Some(e)) => rec.note = format!("FAILED: {e}"),
                _ => rec.note = "no result".into(),
            }
            total = o.req_total_ms;
            ttfb = o.req_ttfb_ms;
        }
        ArmKind::Duck => {
            let d = cfg.duck.as_ref().ok_or("the duckdb arm needs a duckdb configuration")?;
            let r = d.run(layout.name, layout.files, q);
            let (sr, sb) = server_counts(cfg);
            rec.server_requests = sr;
            rec.server_bytes = sb;
            match r {
                Ok(run) => {
                    rec.wall_s = run.seconds;
                    if normalize(&run.text) == normalize(&exp) {
                        rec.ok = true;
                    } else {
                        rec.note = format!("WRONG RESULT: {:?}", run.text.lines().next().unwrap_or(""));
                    }
                }
                Err(e) => rec.note = format!("FAILED: {e}"),
            }
            if sb >= 0 {
                rec.data_bytes = sb as u64;
                if needed > 0 {
                    rec.waste_pct = 100.0 * (sb as f64 - needed as f64) / needed as f64;
                }
            }
            rec.attempts = sr.max(0) as u64;
        }
    }
    Ok((rec, total, ttfb))
}

type Samples = HashMap<(String, String, String), (Vec<f64>, Vec<f64>)>;

/// Run the whole measurement. Returns the records; `Err` when the verification pass before the timing fails.
pub fn run_bench(rt: &Runtime, cfg: &BenchCfg) -> Result<Vec<RunRec>, String> {
    std::fs::create_dir_all(&cfg.out_dir).map_err(|e| e.to_string())?;
    if let Some(c) = &cfg.ctl {
        c.command("logging off")?;
    }
    // 1. correctness before timing: policy B (both decode modes) and DuckDB, each cell, against the reference
    let mut needed: HashMap<(String, String), u64> = HashMap::new();
    for l in &cfg.layouts {
        for q in &cfg.queries {
            let exp = expect_text(cfg, q);
            let ds = dataset_for(cfg, l, false)?;
            for decode in [DecodeMode::Tokio, DecodeMode::Pool] {
                let mut sc = ScanConfig::new(Policy::b(), decode);
                sc.pool_threads = cfg.nproc;
                let o = run_scan(rt, &ds, q, &sc);
                let ok = o.text.as_deref().is_some_and(|t| normalize(t) == normalize(&exp));
                if !ok {
                    return Err(format!(
                        "verification failed before timing: layout {} query {} {:?}: {:?} / {:?}",
                        l.name, q.name, decode, o.text.as_deref().map(|t| t.lines().next().unwrap_or("").to_string()), o.error
                    ));
                }
                needed.insert((l.name.into(), q.name.into()), o.needed_bytes);
            }
            if let Some(d) = &cfg.duck {
                let r = d.run(l.name, l.files, q).map_err(|e| format!("duckdb verification run failed: {e}"))?;
                if normalize(&r.text) != normalize(&exp) {
                    return Err(format!("verification failed before timing: duckdb over the store differs on layout {} query {}", l.name, q.name));
                }
            }
        }
    }
    // 2. timing, interleaved
    let mut recs = Vec::new();
    let mut samples: Samples = HashMap::new();
    let n = cfg.arms.len();
    for round in 0..cfg.rounds {
        for (li, l) in cfg.layouts.iter().enumerate() {
            for (qi, q) in cfg.queries.iter().enumerate() {
                let cell = li * cfg.queries.len() + qi;
                let need = needed.get(&(l.name.to_string(), q.name.to_string())).copied().unwrap_or(0);
                for k in 0..n {
                    let arm = &cfg.arms[(k + round + cell) % n];
                    let (rec, total, ttfb) = run_arm(rt, cfg, l, q, arm, round, need)?;
                    eprintln!(
                        "[{}] round {} {:>8} {:>6} {:>8}: {:8.3} s {}{}",
                        cfg.label,
                        round + 1,
                        l.name,
                        q.name,
                        arm.name,
                        rec.wall_s,
                        if rec.ok { "ok" } else { "NOT OK" },
                        if rec.note.is_empty() { String::new() } else { format!(" ({})", rec.note) }
                    );
                    let e = samples.entry((l.name.into(), q.name.into(), arm.name.clone())).or_default();
                    e.0.extend(total);
                    e.1.extend(ttfb);
                    recs.push(rec);
                }
            }
        }
    }
    write_csv(cfg, &recs).map_err(|e| e.to_string())?;
    std::fs::write(cfg.out_dir.join(format!("{}-summary.md", cfg.label.to_lowercase())), summary(cfg, &recs, &samples))
        .map_err(|e| e.to_string())?;
    Ok(recs)
}

fn write_csv(cfg: &BenchCfg, recs: &[RunRec]) -> std::io::Result<()> {
    let mut s = String::from(
        "store,round,layout,query,arm,wall_s,first_batch_s,plan_s,attempts,data_requests,footer_requests,retries,data_bytes,footer_bytes,needed_bytes,waste_pct,peak_inflight,req_p50_ms,req_p99_ms,ttfb_p50_ms,ttfb_p99_ms,cpu_s,avg_inflight,server_requests,server_bytes,ok,load1,note\n",
    );
    for r in recs {
        let _ = writeln!(
            s,
            "{},{},{},{},{},{:.4},{:.4},{:.4},{},{},{},{},{},{},{},{:.2},{},{:.2},{:.2},{:.2},{:.2},{:.3},{:.2},{},{},{},{:.2},\"{}\"",
            cfg.label, r.round + 1, r.layout, r.query, r.arm, r.wall_s, r.first_batch_s, r.plan_s, r.attempts, r.data_requests,
            r.footer_requests, r.retries, r.data_bytes, r.footer_bytes, r.needed_bytes, r.waste_pct, r.peak_inflight, r.req_p50,
            r.req_p99, r.ttfb_p50, r.ttfb_p99, r.cpu_s, r.avg_inflight, r.server_requests, r.server_bytes, r.ok, r.load1,
            r.note.replace('"', "'").replace('\n', " ")
        );
    }
    std::fs::write(cfg.out_dir.join(format!("{}-runs.csv", cfg.label.to_lowercase())), s)
}

fn med<F: Fn(&RunRec) -> f64>(rs: &[&RunRec], f: F) -> f64 {
    let v: Vec<f64> = rs.iter().map(|r| f(r)).filter(|x| x.is_finite()).collect();
    if v.is_empty() { f64::NAN } else { median(&v) }
}

fn min_max<F: Fn(&RunRec) -> f64>(rs: &[&RunRec], f: F) -> (f64, f64) {
    let v: Vec<f64> = rs.iter().map(|r| f(r)).filter(|x| x.is_finite()).collect();
    (v.iter().copied().fold(f64::INFINITY, f64::min), v.iter().copied().fold(f64::NEG_INFINITY, f64::max))
}

pub fn summary(cfg: &BenchCfg, recs: &[RunRec], samples: &Samples) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "# Throughput and request pattern: {} store\n", cfg.label);
    if cfg.label == "SIMULATED" {
        let _ = writeln!(s, "**SIMULATED**: a local server on loopback with a scripted latency profile. It checks the client logic and");
        let _ = writeln!(s, "the shape of the policies' behaviour; it is not a measurement of any real object store.\n");
        let _ = writeln!(s, "Store: {}.\n", cfg.spec.label);
    } else {
        let _ = writeln!(s, "Store: {} (endpoint, bucket and prefix are deliberately not recorded).\n", cfg.spec.label);
    }
    if cfg.sanity_only {
        let _ = writeln!(s, "**TIMING IS SANITY ONLY: the 1-minute load average was >= 2 before the timing phase started.**\n");
    }
    let bad = recs.iter().filter(|r| !r.ok).count();
    let _ = writeln!(s, "{} runs, {} rounds, {} not ok. Order of the arms rotated every cell and round. Wall time of an `ours` arm = footers + planning + fetch + decode + aggregation on the CPU pool, from the start of the query to its last row; DuckDB's is its own `.timer` for the statement (process start and extension load excluded; footer reads included).\n", recs.len(), cfg.rounds, bad);
    let mismatches = recs.iter().filter(|r| r.note.starts_with("client counted")).count();
    if mismatches > 0 {
        let _ = writeln!(s, "WARNING: in {mismatches} runs the client-side request count differs from the server's; see the `note` column of the CSV.\n");
    }
    let loads: Vec<f64> = recs.iter().map(|r| r.load1).filter(|x| x.is_finite()).collect();
    if !loads.is_empty() {
        let _ = writeln!(s, "1-minute load average at the start of each run (it includes the benchmark's own threads): min {:.1}, median {:.1}, max {:.1}.\n",
            loads.iter().copied().fold(f64::INFINITY, f64::min), median(&loads), loads.iter().copied().fold(f64::NEG_INFINITY, f64::max));
    }
    for l in &cfg.layouts {
        for q in &cfg.queries {
            let cell: Vec<&RunRec> = recs.iter().filter(|r| r.layout == l.name && r.query == q.name).collect();
            if cell.is_empty() {
                continue;
            }
            let need = cell.iter().map(|r| r.needed_bytes).max().unwrap_or(0);
            let _ = writeln!(s, "## {} / {} ({})\n", l.name, q.name, l.note);
            let _ = writeln!(s, "Bytes the query needs (projected column chunks): {:.1} MiB.\n", need as f64 / 1_048_576.0);
            let _ = writeln!(s, "| arm | wall s median (min-max) | MiB/s useful | requests | MiB fetched | waste % | first batch s | req total p50 / p99 ms | first byte p50 / p99 ms | peak in flight | avg in flight | cores used |");
            let _ = writeln!(s, "|---|---|---|---|---|---|---|---|---|---|---|---|");
            let mut arm_names: Vec<&str> = Vec::new();
            for r in &cell {
                if !arm_names.contains(&r.arm.as_str()) {
                    arm_names.push(&r.arm);
                }
            }
            for a in arm_names {
                let rs: Vec<&RunRec> = cell.iter().copied().filter(|r| r.arm == a && r.ok).collect();
                if rs.is_empty() {
                    let _ = writeln!(s, "| {a} | NO OK RUN | | | | | | | | | | |");
                    continue;
                }
                let (lo, hi) = min_max(&rs, |r| r.wall_s);
                let w = med(&rs, |r| r.wall_s);
                let (p, f) = samples.get(&(l.name.to_string(), q.name.to_string(), a.to_string())).cloned().unwrap_or_default();
                let (p, f) = (sorted(p), sorted(f));
                let reqs = if rs[0].server_requests >= 0 { med(&rs, |r| r.server_requests as f64) } else { med(&rs, |r| r.attempts as f64) };
                let fmt_pc = |v: &[f64]| if v.is_empty() { "n/a".to_string() } else { format!("{:.0} / {:.0}", percentile(v, 50.0), percentile(v, 99.0)) };
                let _ = writeln!(
                    s,
                    "| {a} | {w:.3} ({lo:.3}-{hi:.3}) | {:.0} | {:.0} | {:.1} | {:.0} | {:.3} | {} | {} | {:.0} | {:.1} | {:.1} |",
                    need as f64 / 1_048_576.0 / w,
                    reqs,
                    med(&rs, |r| r.data_bytes as f64) / 1_048_576.0,
                    med(&rs, |r| r.waste_pct),
                    med(&rs, |r| r.first_batch_s),
                    fmt_pc(&p),
                    fmt_pc(&f),
                    med(&rs, |r| r.peak_inflight as f64),
                    med(&rs, |r| r.avg_inflight),
                    med(&rs, |r| r.cpu_s / r.wall_s.max(1e-9)),
                );
            }
            // the noise: B against its own twin
            let b: Vec<&RunRec> = cell.iter().copied().filter(|r| r.arm == "B" && r.ok).collect();
            let t: Vec<&RunRec> = cell.iter().copied().filter(|r| r.arm == "B-twin" && r.ok).collect();
            if !b.is_empty() && !t.is_empty() {
                let _ = writeln!(s, "\nNoise: median wall of B / B-twin = {:.3} (the same configuration twice; differences between arms smaller than this are not results).", med(&b, |r| r.wall_s) / med(&t, |r| r.wall_s));
            }
            let _ = writeln!(s);
        }
    }
    let _ = writeln!(s, "Notes: 'requests' for DuckDB and, on the simulated store, for every arm are the SERVER's count (all methods, retries included); on the real store 'requests' of our arms are the client's count and DuckDB's is not available. 'MiB/s useful' = bytes the query needs / wall. 'waste' = bytes fetched over bytes needed minus 1 (footer bytes not included; for DuckDB it is the server's bytes sent). 'avg in flight' = sum of per-request times / wall. 'cores used' = process CPU seconds / wall (ours only; includes the I/O runtime, the CPU pool and the decode).");
    s
}
