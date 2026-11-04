//! The verification scenarios. Each returns a list of named [`Check`]s; each check that guards a behaviour is also run
//! against a KNOWN-BAD version of that behaviour (a [`Mutant`]) and must FAIL there, otherwise the check proves nothing.
//!
//!  * `selftest`: the harness sees an injected known latency, a known bandwidth cap, a known tail, and counts requests
//!    the way the server does;
//!  * `correctness`: results identical to the reference for every policy and both decode modes;
//!  * `membound`: peak queued bytes <= the stated bound with a slow consumer, and an unbounded control exceeds it;
//!  * `cancellation`: after a LIMIT or a cancel no new request starts, nothing completes, nothing stays in flight;
//!  * `faults`: transient and permanent errors, bounded retries with backoff, clean failure, no leaked tasks.
//!
//! All of it runs against the SIMULATED store (`sim.rs`), whose faults and latencies are scripted; none of it says how a
//! real store behaves.

use std::collections::HashMap;
use std::path::Path as FsPath;
use std::sync::Arc;
use std::time::{Duration, Instant};

use object_store::ObjectStore;
use tokio::runtime::{Builder, Runtime};

use crate::exec::*;
use crate::fetch::*;
use crate::scan::*;
use crate::sim::{LogEntry, Profile, SimCtl, SimServer};
use crate::store::StoreSpec;
use crate::util::{median, percentile, sorted};

// ------------------------------------------------------------------ checks

#[derive(Clone, Debug)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

pub fn chk(name: impl Into<String>, ok: bool, detail: impl Into<String>) -> Check {
    Check {
        name: name.into(),
        ok,
        detail: detail.into(),
    }
}

pub fn all_ok(v: &[Check]) -> bool {
    v.iter().all(|c| c.ok)
}

pub fn print_checks(title: &str, v: &[Check]) {
    println!("---- {title} ----");
    for c in v {
        println!("{} {}: {}", if c.ok { "PASS" } else { "FAIL" }, c.name, c.detail);
    }
}

/// A known-bad version run through a scenario: `caught` when at least one check failed.
#[derive(Clone, Debug)]
pub struct MutantRun {
    pub mutant: Mutant,
    pub scenario: String,
    pub failed_checks: Vec<String>,
}

impl MutantRun {
    pub fn caught(&self) -> bool {
        !self.failed_checks.is_empty()
    }
}

pub fn print_mutants(title: &str, v: &[MutantRun]) {
    println!("---- {title} ----");
    for m in v {
        if m.caught() {
            println!("CAUGHT {} in {}: failed [{}]", m.mutant.label(), m.scenario, m.failed_checks.join(", "));
        } else {
            println!("NOT CAUGHT {} in {}: every check passed on the known-bad version", m.mutant.label(), m.scenario);
        }
    }
}

// ------------------------------------------------------------------ environment

pub fn io_runtime(threads: usize) -> Runtime {
    Builder::new_multi_thread()
        .worker_threads(threads)
        .thread_name("nata-io")
        .enable_all()
        .build()
        .expect("tokio runtime")
}

/// An in-process simulated store (the tests; `gcp-check.sh` starts the server as a separate process instead).
pub struct SimHost {
    pub rt: Runtime,
    pub server: SimServer,
}

impl SimHost {
    pub fn start(root: &FsPath, bucket: &str, profile: Profile) -> SimHost {
        let rt = io_runtime(2);
        let server = rt
            .block_on(SimServer::start(root.to_path_buf(), bucket, profile, 42, 0))
            .expect("bind the simulated store");
        SimHost { rt, server }
    }
    pub fn addr(&self) -> String {
        self.server.addr.to_string()
    }
}

/// The client side of a simulated store.
pub struct SimEnv {
    pub ctl: SimCtl,
    pub spec: StoreSpec,
    pub store: Arc<dyn ObjectStore>,
    /// The same store with `object_store`'s own retries on (reference arm).
    pub store_builtin_retry: Arc<dyn ObjectStore>,
    pub rt: Runtime,
}

impl SimEnv {
    pub fn connect(addr: &str, bucket: &str) -> Result<SimEnv, String> {
        let spec = StoreSpec::sim(addr, bucket);
        Ok(SimEnv {
            ctl: SimCtl::new(addr),
            store: spec.build(None)?,
            store_builtin_retry: spec.build(Some(4))?,
            spec,
            rt: io_runtime(2),
        })
    }
    pub fn dataset(&self, layout: &str, files: usize) -> Dataset {
        dataset(Arc::clone(&self.store), &self.spec, layout, files)
    }
}

fn norm(t: &str) -> String {
    normalize(t)
}

/// The result of `o` is the expected text.
fn result_matches(o: &ScanOutcome, expected: &str) -> (bool, String) {
    match (&o.text, &o.error) {
        (Some(t), None) if norm(t) == norm(expected) => (true, "identical".into()),
        (Some(t), None) => (
            false,
            format!("result differs: got {:?} expected {:?}", first_line(t), first_line(expected)),
        ),
        (_, Some(e)) => (false, format!("query failed: {e}")),
        _ => (false, "no result".into()),
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(120).collect()
}

fn base_cfg(policy: Policy, decode: DecodeMode) -> ScanConfig {
    let mut c = ScanConfig::new(policy, decode);
    c.pool_threads = 4;
    c
}

// ------------------------------------------------------------------ self-test of the harness

/// Can the measurement see what the simulated store is told to do? `file` is a relative key of a file of at least
/// 20 MiB in the store (`bw_file`) and any small file (`small_file`).
pub fn selftest(env: &SimEnv, small_file: &str, bw_file: &str) -> Vec<Check> {
    let mut out = Vec::new();
    let store = Arc::clone(&env.store);
    let mk = |conc: usize| Fetcher::new(Arc::clone(&store), FetchStats::new(), RetryPolicy::fast_tests(), Mutant::None, conc, env.spec.redactor());
    let small = env.spec.path(small_file);
    let big = env.spec.path(bw_file);

    // 1. a known first-byte latency is seen; a harness that sees none (zero latency) is flagged by the same check
    let measure = |profile: &str| -> (f64, f64) {
        env.ctl.clean().ok();
        env.ctl.command(&format!("profile {profile}")).ok();
        let f = mk(1);
        env.rt.block_on(async {
            for _ in 0..20 {
                let _ = f.get_range(&small, 0..1024, false).await;
            }
        });
        let recs = f.stats.records();
        (
            median(&recs.iter().map(|r| r.total_ms).collect::<Vec<_>>()),
            median(&recs.iter().map(|r| r.ttfb_ms).collect::<Vec<_>>()),
        )
    };
    let sees_50 = |total: f64, ttfb: f64| (45.0..=90.0).contains(&total) && ttfb >= 45.0;
    let (t50, f50) = measure("const50");
    let (t0, f0) = measure("zero");
    out.push(chk(
        "selftest/latency: an injected 50 ms first-byte latency is measured as ~50 ms",
        sees_50(t50, f50),
        format!("const50: median total {t50:.1} ms, first byte {f50:.1} ms"),
    ));
    out.push(chk(
        "selftest/latency: the same check FAILS on a store with no latency (known-bad)",
        !sees_50(t0, f0),
        format!("zero: median total {t0:.1} ms, first byte {f0:.1} ms"),
    ));

    // 2. a known bandwidth cap is seen
    let bw = |profile: &str| -> f64 {
        env.ctl.clean().ok();
        env.ctl.command(&format!("profile {profile}")).ok();
        let f = mk(4);
        let t = Instant::now();
        env.rt.block_on(async {
            let futs = (0..4u64).map(|i| {
                let (f, p) = (Arc::clone(&f), big.clone());
                async move { f.get_range(&p, i * 4_194_304..(i + 1) * 4_194_304, false).await }
            });
            futures::future::join_all(futs).await
        });
        16.0 / t.elapsed().as_secs_f64()
    };
    let sees_cap = |mibs: f64| (35.0..=60.0).contains(&mibs);
    let capped = bw("bw50");
    let free = bw("zero");
    out.push(chk(
        "selftest/bandwidth: an injected 50 MiB/s cap is measured as ~50 MiB/s",
        sees_cap(capped),
        format!("bw50: {capped:.1} MiB/s"),
    ));
    out.push(chk(
        "selftest/bandwidth: the same check FAILS on an uncapped store (known-bad)",
        !sees_cap(free),
        format!("zero: {free:.1} MiB/s"),
    ));

    // 3. a known tail is seen in p99 and absent from p50; with no tail injected p99 stays low
    let tail = |prob: f64| -> (f64, f64) {
        env.ctl.clean().ok();
        env.ctl.command(&format!("profile zero\nset median_ms 5\nset tail_prob {prob}\nset tail_min_ms 100\nset tail_cap_ms 100")).ok();
        let f = mk(1);
        env.rt.block_on(async {
            for _ in 0..120 {
                let _ = f.get_range(&small, 0..512, false).await;
            }
        });
        let v = sorted(f.stats.records().iter().map(|r| r.total_ms).collect());
        (percentile(&v, 50.0), percentile(&v, 99.0))
    };
    let (p50t, p99t) = tail(0.2);
    let (p50n, p99n) = tail(0.0);
    let sees_tail = |p50: f64, p99: f64| p50 < 40.0 && p99 >= 95.0;
    out.push(chk(
        "selftest/tail: a 20 percent tail of 100 ms shows in p99, not in p50",
        sees_tail(p50t, p99t),
        format!("with tail: p50 {p50t:.1} ms, p99 {p99t:.1} ms"),
    ));
    out.push(chk(
        "selftest/tail: the same check FAILS when no tail is injected (known-bad)",
        !sees_tail(p50n, p99n),
        format!("without tail: p50 {p50n:.1} ms, p99 {p99n:.1} ms"),
    ));

    // 4. requests are counted the way the server counts them, and concurrency is seen
    env.ctl.clean().ok();
    env.ctl.command("profile const50").ok();
    env.ctl.command("reset").ok();
    let f = mk(8);
    env.rt.block_on(async {
        let futs = (0..8u64).map(|i| {
            let (f, p) = (Arc::clone(&f), small.clone());
            async move { f.get_range(&p, i * 64..i * 64 + 64, false).await }
        });
        futures::future::join_all(futs).await
    });
    let server_req = env.ctl.stat("requests").unwrap_or(u64::MAX);
    let client_req = f.stats.started.load(std::sync::atomic::Ordering::SeqCst);
    out.push(chk(
        "selftest/count: client-side request count equals the server's",
        client_req == 8 && server_req == 8,
        format!("client {client_req}, server {server_req}"),
    ));
    let peak = f.stats.peak_inflight.load(std::sync::atomic::Ordering::SeqCst);
    out.push(chk(
        "selftest/concurrency: 8 requests issued together are seen as 8 in flight (>= 7)",
        peak >= 7,
        format!("peak in flight {peak}"),
    ));

    // 5. percentile arithmetic
    let v: Vec<f64> = (1..=100).map(f64::from).collect();
    out.push(chk(
        "selftest/percentile: nearest-rank p50 and p99 of 1..=100",
        percentile(&v, 50.0) == 50.0 && percentile(&v, 99.0) == 99.0,
        format!("p50 {} p99 {}", percentile(&v, 50.0), percentile(&v, 99.0)),
    ));
    env.ctl.clean().ok();
    out
}

// ------------------------------------------------------------------ correctness

/// Every policy and both decode modes give exactly the reference result.
pub fn correctness(env: &SimEnv, ds: &Dataset, expected: &dyn Fn(&'static QuerySpec) -> String, queries: &[&'static QuerySpec], label: &str) -> Vec<Check> {
    let mut out = Vec::new();
    env.ctl.clean().ok();
    env.ctl.command("profile zero").ok();
    for q in queries {
        let exp = expected(q);
        for pol in ["A", "B", "C", "D", "E"] {
            for decode in [DecodeMode::Tokio, DecodeMode::Pool] {
                let cfg = base_cfg(Policy::named(pol).unwrap_or_else(Policy::b), decode);
                let o = run_scan(&env.rt, ds, q, &cfg);
                let (ok, detail) = result_matches(&o, &exp);
                out.push(chk(
                    format!("correct/{label}/{}/policy {pol}/{decode:?}", q.name),
                    ok && o.alive_after == 0 && o.inflight_after == 0,
                    format!("{detail}; {} requests, {} bytes fetched", o.attempts, o.data_bytes + o.footer_bytes),
                ));
            }
        }
    }
    // the comparison itself can fail: a perturbed expectation must not match
    if let Some(q) = queries.iter().find(|q| q.limit.is_none()) {
        let exp = expected(q);
        let o = run_scan(&env.rt, ds, q, &base_cfg(Policy::b(), DecodeMode::Pool));
        let bad = format!("{exp}0");
        out.push(chk(
            "correct/the comparison fails on a wrong expected result (known-bad)",
            !result_matches(&o, &bad).0 || exp.is_empty(),
            "a perturbed expectation was rejected",
        ));
    }
    out
}

// ------------------------------------------------------------------ memory bound

fn mem_case(env: &SimEnv, ds: &Dataset, q: &'static QuerySpec, expected: &str, decode: DecodeMode, cap: usize, policy: Policy, mutant: Mutant) -> (ScanOutcome, Vec<Check>) {
    env.ctl.clean().ok();
    env.ctl.command("profile zero").ok();
    let prefetch = policy.prefetch;
    let mut cfg = base_cfg(policy, decode);
    cfg.cap = cap;
    cfg.batch_rows = 2048;
    cfg.consumer_delay = Duration::from_millis(2);
    cfg.mutant = mutant;
    let o = run_scan(&env.rt, ds, q, &cfg);
    let name = format!("{decode:?}/policy {}/cap {cap}", cfg.policy.name);
    let mut c = Vec::new();
    let bb = o.bridge_bound();
    let fb = o.fetch_bound(prefetch, decode == DecodeMode::Pool);
    c.push(chk(
        format!("membound/{name}: peak bytes queued in the bridge <= channels x (cap + 2) x largest item"),
        o.bridge_peak <= bb && o.bridge_peak > 0,
        format!("peak {} B <= bound {} B ({} channels x {} x {} B)", o.bridge_peak, bb, o.channels, cap + 2, o.max_item_bytes),
    ));
    c.push(chk(
        format!("membound/{name}: peak fetched bytes alive <= producers x units held x largest unit"),
        o.fetch_peak <= fb,
        format!("peak {} B <= bound {} B", o.fetch_peak, fb),
    ));
    let (ok, detail) = result_matches(&o, expected);
    c.push(chk(format!("membound/{name}: result correct under backpressure"), ok, detail));
    c.push(chk(
        format!("membound/{name}: no task or request left behind"),
        o.producers_joined && o.alive_after == 0 && o.inflight_after == 0,
        format!("joined {}, alive {}, in flight {}", o.producers_joined, o.alive_after, o.inflight_after),
    ));
    (o, c)
}

/// Peak queued bytes <= the stated bound, with a slow consumer, bounded; and the unbounded control exceeds it.
pub fn membound(env: &SimEnv, ds: &Dataset, q: &'static QuerySpec, expected: &str) -> (Vec<Check>, Vec<MutantRun>) {
    let mut checks = Vec::new();
    let mut mutants = Vec::new();
    for decode in [DecodeMode::Tokio, DecodeMode::Pool] {
        for cap in [1usize, 4] {
            for pol in [Policy::b(), Policy::d()] {
                let (_, c) = mem_case(env, ds, q, expected, decode, cap, pol, Mutant::None);
                checks.extend(c);
            }
        }
        // the control: the same scan with an unbounded channel must break the bound that holds above
        let (o, c) = mem_case(env, ds, q, expected, decode, 1, Policy::b(), Mutant::Unbounded);
        let failed: Vec<String> = c.iter().filter(|x| !x.ok).map(|x| x.name.clone()).collect();
        checks.push(chk(
            format!("membound/control {decode:?}: the unbounded channel exceeds the bound of cap 1"),
            o.bridge_peak > o.bridge_bound(),
            format!("peak {} B > bound {} B (bounded run above stays within it)", o.bridge_peak, o.bridge_bound()),
        ));
        mutants.push(MutantRun {
            mutant: Mutant::Unbounded,
            scenario: format!("membound/{decode:?}"),
            failed_checks: failed,
        });
    }
    (checks, mutants)
}

// ------------------------------------------------------------------ cancellation

#[derive(Clone, Copy, Debug)]
pub enum CancelKind {
    /// `LIMIT 1`: the sink finishes early.
    Limit,
    /// The query is cancelled from outside after a few requests.
    External,
}

fn cancel_case(env: &SimEnv, ds: &Dataset, kind: CancelKind, mutant: Mutant) -> Vec<Check> {
    let q: &'static QuerySpec = match kind {
        CancelKind::Limit => &Q_LIMIT1,
        CancelKind::External => &Q_SUM,
    };
    let mut cfg = base_cfg(Policy::b(), DecodeMode::Pool);
    cfg.cap = 2;
    cfg.mutant = mutant;
    // size the transfer so that one request takes about 800 ms: a request in flight at the cancel is then
    // unmistakable, whatever the data size
    env.ctl.clean().ok(); // zero latency while the footers are read for the plan
    let per_unit_mib = match plan_stats(&env.rt, ds, q, &cfg) {
        Ok(p) if p.units > 0 => (p.needed_bytes as f64 / p.units as f64 / 1_048_576.0).max(0.001),
        _ => 1.0,
    };
    let per_mb_ms = (800.0 / per_unit_mib).clamp(5.0, 200_000.0);
    env.ctl.clean().ok();
    env.ctl.command(&format!("profile slow\nset median_ms 10\nset per_mb_ms {per_mb_ms}")).ok();
    env.ctl.command("reset").ok();
    if let CancelKind::External = kind {
        cfg.cancel_after = Some(CancelTrigger::AfterRequests(6));
    }
    let o = run_scan(&env.rt, ds, q, &cfg);
    let name = format!("cancel/{kind:?}");
    let mut c = Vec::new();
    match kind {
        CancelKind::Limit => c.push(chk(
            format!("{name}: LIMIT 1 returns exactly one row"),
            o.text.as_deref() == Some("1 row(s)\n"),
            format!("{:?} {:?}", o.text.as_deref().map(first_line), o.error),
        )),
        CancelKind::External => c.push(chk(
            format!("{name}: a cancelled query reports 'cancelled'"),
            o.error.as_deref() == Some("cancelled"),
            format!("{:?}", o.error),
        )),
    }
    let nch = o.channels.max(1) as u64;
    c.push(chk(
        format!("{name}: the cancel was reached while requests were still outstanding"),
        o.cancel_at_s.is_some() && o.inflight_at_cancel > 0,
        format!("in flight at cancel {}, cancel at {:?} s", o.inflight_at_cancel, o.cancel_at_s),
    ));
    c.push(chk(
        format!("{name}: no new request starts after the cancel (<= one per producer in the race window)"),
        o.started_after_cancel <= nch,
        format!("{} started after cancel (limit {nch})", o.started_after_cancel),
    ));
    c.push(chk(
        format!("{name}: no request completes more than 50 ms after the cancel (in-flight requests are dropped)"),
        o.late_completions_after_cancel == 0,
        format!("{} completed late, {} completed in all after the cancel ({} B)", o.late_completions_after_cancel, o.completed_after_cancel, o.bytes_after_cancel),
    ));
    c.push(chk(
        format!("{name}: every producer has ended within 250 ms of the cancel"),
        o.producers_joined && o.quiesce_ms.is_some_and(|q| q <= 250.0),
        format!("quiesce {:?} ms, joined {}", o.quiesce_ms.map(|x| x.round()), o.producers_joined),
    ));
    c.push(chk(
        format!("{name}: nothing left in flight and no task leaked"),
        o.inflight_after == 0 && o.alive_after == 0,
        format!("in flight {}, alive tasks {}", o.inflight_after, o.alive_after),
    ));
    // the server sees the clients go away: its own in-flight count falls to zero
    let t = Instant::now();
    let mut srv = u64::MAX;
    while t.elapsed() < Duration::from_secs(3) {
        srv = env.ctl.stat("inflight").unwrap_or(u64::MAX);
        if srv == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    c.push(chk(
        format!("{name}: the server's in-flight count returns to zero within 3 s"),
        srv == 0,
        format!("server in flight {srv}"),
    ));
    env.ctl.clean().ok();
    c
}

pub fn cancellation(env: &SimEnv, ds: &Dataset) -> (Vec<Check>, Vec<MutantRun>) {
    let mut checks = Vec::new();
    let mut mutants = Vec::new();
    for kind in [CancelKind::Limit, CancelKind::External] {
        checks.extend(cancel_case(env, ds, kind, Mutant::None));
        for m in [Mutant::CancelSendOnly, Mutant::CancelIgnore] {
            let c = cancel_case(env, ds, kind, m);
            mutants.push(MutantRun {
                mutant: m,
                scenario: format!("cancel/{kind:?}"),
                failed_checks: c.iter().filter(|x| !x.ok).map(|x| x.name.clone()).collect(),
            });
        }
    }
    (checks, mutants)
}

// ------------------------------------------------------------------ retries and errors

#[derive(Clone, Debug)]
pub enum Expect {
    /// Succeeds with the right result; every request that is hit by the fault takes exactly `k + 1` attempts.
    Success { k: u32 },
    /// Fails cleanly; the failing requests take `attempts` attempts (the retry limit, or 1 when the error is permanent).
    Failure { attempts: u32 },
}

#[derive(Clone, Debug)]
pub struct FaultCase {
    pub name: &'static str,
    /// Control commands applied before the run.
    pub setup: Vec<String>,
    pub expect: Expect,
}

pub fn fault_cases(victim: &str) -> Vec<FaultCase> {
    let s = |x: &str| x.to_string();
    vec![
        FaultCase { name: "transient 503 x2 on every data range", setup: vec![s("rule - 503 2 data")], expect: Expect::Success { k: 2 } },
        FaultCase { name: "transient connection reset mid-body x1", setup: vec![s("rule - reset 1 data")], expect: Expect::Success { k: 1 } },
        FaultCase { name: "transient connection reset before the response x1", setup: vec![s("rule - reset_early 1 data")], expect: Expect::Success { k: 1 } },
        FaultCase { name: "transient hang (attempt timeout) x1", setup: vec![s("set hang_ms 1500"), s("rule - hang 1 data")], expect: Expect::Success { k: 1 } },
        FaultCase { name: "transient 503 x1 on the footer reads", setup: vec![s("rule - 503 1 footer")], expect: Expect::Success { k: 1 } },
        FaultCase { name: "permanent 503 on one file's data", setup: vec![format!("rule {victim} 503 perm data")], expect: Expect::Failure { attempts: 4 } },
        FaultCase { name: "permanent 403 on one file's data", setup: vec![format!("rule {victim} 403 perm data")], expect: Expect::Failure { attempts: 1 } },
    ]
}

fn group_keys(log: &[LogEntry]) -> HashMap<(String, String), Vec<LogEntry>> {
    let mut m: HashMap<(String, String), Vec<LogEntry>> = HashMap::new();
    for e in log {
        m.entry((e.path.clone(), e.range.clone())).or_default().push(e.clone());
    }
    for v in m.values_mut() {
        v.sort_by_key(|e| e.id);
    }
    m
}

pub fn fault_case(env: &SimEnv, ds: &Dataset, q: &'static QuerySpec, expected: &str, case: &FaultCase, mutant: Mutant, decode: DecodeMode) -> Vec<Check> {
    env.ctl.clean().ok();
    env.ctl.command("profile zero").ok();
    for line in &case.setup {
        if let Err(e) = env.ctl.command(line) {
            return vec![chk(format!("faults/{}: control command {line:?}", case.name), false, e)];
        }
    }
    env.ctl.command("reset").ok();
    let mut cfg = base_cfg(Policy::b(), decode);
    cfg.retry = RetryPolicy::fast_tests();
    cfg.mutant = mutant;
    cfg.hard_stop_after = Some(Duration::from_secs(8));
    let o = run_scan(&env.rt, ds, q, &cfg);
    // let the server finish writing its log for the last responses (a hung request is logged when it starts)
    let t = Instant::now();
    while env.ctl.stat("inflight").unwrap_or(0) > 0 && t.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let log = env.ctl.log().unwrap_or_default();
    let keys = group_keys(&log);
    let name = format!("faults/{}/{decode:?}", case.name);
    let mut c = Vec::new();
    let faulted: Vec<&Vec<LogEntry>> = keys.values().filter(|v| v.iter().any(|e| !e.fault.is_empty())).collect();
    let max_attempts = cfg.retry.max_attempts;
    c.push(chk(
        format!("{name}: the fault was injected"),
        !faulted.is_empty(),
        format!("{} request keys hit by a fault", faulted.len()),
    ));

    // backoff: the gap between attempt k and k+1 of a key is at least half the nominal backoff after k
    let mut worst: Option<(f64, f64)> = None;
    for v in keys.values() {
        for (k, w) in v.windows(2).enumerate() {
            let gap = w[1].t_start_ms - w[0].t_start_ms;
            let need = cfg.retry.min_backoff_after(k as u32 + 1).as_secs_f64() * 1e3 - 6.0;
            if gap < need && worst.is_none_or(|(g, n)| gap - need < g - n) {
                worst = Some((gap, need));
            }
        }
    }
    c.push(chk(
        format!("{name}: retries wait at least the backoff between attempts"),
        worst.is_none(),
        worst.map_or("all gaps >= half the nominal backoff".into(), |(g, n)| format!("a retry came after {g:.1} ms, backoff requires >= {n:.1} ms")),
    ));
    c.push(chk(
        format!("{name}: retries are bounded (never more than {max_attempts} attempts per request)"),
        keys.values().all(|v| v.len() as u32 <= max_attempts) && o.retries <= keys.len() as u64 * u64::from(max_attempts - 1),
        format!("max attempts on one request {}, {} retries over {} requests", keys.values().map(Vec::len).max().unwrap_or(0), o.retries, keys.len()),
    ));
    c.push(chk(
        format!("{name}: no task, request or connection left behind"),
        o.producers_joined && o.alive_after == 0 && o.inflight_after == 0,
        format!("joined {}, alive {}, in flight {}", o.producers_joined, o.alive_after, o.inflight_after),
    ));
    match &case.expect {
        Expect::Success { k } => {
            let (ok, detail) = result_matches(&o, expected);
            c.push(chk(format!("{name}: correct result"), ok, detail));
            let exact = faulted.iter().all(|v| v.len() as u32 == k + 1);
            c.push(chk(
                format!("{name}: every faulted request needed exactly {} attempts", k + 1),
                exact,
                format!("attempt counts of faulted requests: {:?}", {
                    let mut a: Vec<usize> = faulted.iter().map(|v| v.len()).collect();
                    a.sort_unstable();
                    a.dedup();
                    a
                }),
            ));
        }
        Expect::Failure { attempts } => {
            c.push(chk(
                format!("{name}: the query fails with an error and no result"),
                o.error.is_some() && o.text.is_none(),
                format!("error {:?}, result {:?}", o.error.as_deref().map(first_line), o.text.as_deref().map(first_line)),
            ));
            c.push(chk(
                format!("{name}: it fails within 3 s (no hang)"),
                o.wall <= Duration::from_secs(3),
                format!("{:.2} s", o.wall.as_secs_f64()),
            ));
            let counts: Vec<u32> = faulted.iter().map(|v| v.len() as u32).collect();
            let ok = if *attempts == 1 {
                !counts.is_empty() && counts.iter().all(|&n| n == 1)
            } else {
                counts.iter().max() == Some(attempts)
            };
            c.push(chk(
                format!("{name}: the failing request was tried {attempts} time(s)"),
                ok,
                format!("attempt counts of faulted requests: {counts:?}"),
            ));
        }
    }
    env.ctl.clean().ok();
    c
}

/// The reference arm: `object_store`'s own retries (4) instead of ours, same transient fault. Informational: it reports
/// what the crate does, it does not guard our code.
pub fn builtin_retry_reference(env: &SimEnv, ds_files: &[&str], layout: &str, q: &'static QuerySpec, expected: &str) -> Vec<Check> {
    let ds = Dataset {
        store: Arc::clone(&env.store_builtin_retry),
        files: ds_files.iter().map(|f| env.spec.path(&format!("{layout}/{f}"))).collect(),
        redact: env.spec.redactor(),
    };
    env.ctl.clean().ok();
    env.ctl.command("profile zero\nrule - 503 2 data").ok();
    env.ctl.command("reset").ok();
    let mut cfg = base_cfg(Policy::b(), DecodeMode::Pool);
    cfg.retry.max_attempts = 1; // our layer does not retry here
    let o = run_scan(&env.rt, &ds, q, &cfg);
    let srv = env.ctl.stat("requests").unwrap_or(0);
    let (ok, detail) = result_matches(&o, expected);
    env.ctl.clean().ok();
    vec![chk(
        "faults/reference: object_store's own retry (4) also completes a transient 503 x2",
        ok,
        format!("{detail}; the server saw {srv} requests for {} client-side calls (each retry is invisible to our counters)", o.attempts),
    )]
}

pub fn faults(env: &SimEnv, ds: &Dataset, q: &'static QuerySpec, expected: &str, victim: &str) -> (Vec<Check>, Vec<MutantRun>) {
    let mut checks = Vec::new();
    let mut mutants = Vec::new();
    let cases = fault_cases(victim);
    for case in &cases {
        for decode in [DecodeMode::Tokio, DecodeMode::Pool] {
            checks.extend(fault_case(env, ds, q, expected, case, Mutant::None, decode));
        }
    }
    // known-bad versions, each against the case that must expose it
    let find = |n: &str| cases.iter().find(|c| c.name.starts_with(n)).cloned();
    let plan: [(Mutant, &str); 4] = [
        (Mutant::RetryForever, "permanent 503"),
        (Mutant::RetryNoBackoff, "transient 503 x2"),
        (Mutant::RetryPermanent, "permanent 403"),
        (Mutant::SwallowError, "permanent 503"),
    ];
    for (m, case_name) in plan {
        if let Some(case) = find(case_name) {
            let c = fault_case(env, ds, q, expected, &case, m, DecodeMode::Pool);
            mutants.push(MutantRun {
                mutant: m,
                scenario: format!("faults/{}", case.name),
                failed_checks: c.iter().filter(|x| !x.ok).map(|x| x.name.clone()).collect(),
            });
        }
    }
    (checks, mutants)
}
