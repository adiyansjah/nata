//! Deterministic tests of the object-store read path against the SIMULATED store (in-process, tiny generated data).
//! They share one server whose profile and fault rules the tests change, so they are serialised by a lock.
//! Every check that guards a behaviour is also run against a known-bad version and must fail there.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use spike_object_store_scan::api::{SourceFactory, Waker};
use spike_object_store_scan::chan::channel;
use spike_object_store_scan::exec::*;
use spike_object_store_scan::fetch::*;
use spike_object_store_scan::scan::*;
use spike_object_store_scan::scenarios::*;
use spike_object_store_scan::sim::Profile;
use spike_object_store_scan::testdata::*;

struct Fx {
    host: SimHost,
    env: SimEnv,
    rows: Rows,
    dir: PathBuf,
}

static FX: OnceLock<Fx> = OnceLock::new();
static LOCK: Mutex<()> = Mutex::new(());

const T_BIG: (&str, usize, usize) = ("t_big", 1, 30_000); // 4 large row groups
const T_SMALL: (&str, usize, usize) = ("t_small", 1, 2_500); // 48 small row groups
const T_FILES: (&str, usize, usize) = ("t_files", 6, 0); // 6 files, one row group each

fn fx() -> (MutexGuard<'static, ()>, &'static Fx) {
    let g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let f = FX.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("objscan-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let rows = gen_rows(120_000, 7);
        for (name, files, rpg) in [T_BIG, T_SMALL, T_FILES] {
            write_layout(&dir, name, &rows, files, rpg).expect("write layout");
        }
        std::fs::write(dir.join("small.bin"), vec![7u8; 8192]).expect("small");
        std::fs::write(dir.join("big.bin"), vec![9u8; 20 * 1024 * 1024]).expect("big");
        let host = SimHost::start(&dir, "sim", Profile::zero());
        let env = SimEnv::connect(&host.addr(), "sim").expect("connect");
        Fx { host, env, rows, dir }
    });
    (g, f)
}

fn expected(f: &'static Fx) -> impl Fn(&'static QuerySpec) -> String {
    move |q| reference(q, &f.rows)
}

fn assert_all(title: &str, checks: &[Check]) {
    print_checks(title, checks);
    let bad: Vec<&Check> = checks.iter().filter(|c| !c.ok).collect();
    assert!(bad.is_empty(), "{title}: {} check(s) failed: {:?}", bad.len(), bad.iter().map(|c| &c.name).collect::<Vec<_>>());
}

fn assert_caught(title: &str, m: &[MutantRun]) {
    print_mutants(title, m);
    assert!(!m.is_empty(), "{title}: no known-bad version was run");
    let missed: Vec<String> = m.iter().filter(|x| !x.caught()).map(|x| format!("{} in {}", x.mutant.label(), x.scenario)).collect();
    assert!(missed.is_empty(), "{title}: known-bad versions NOT caught: {missed:?}");
}

#[test]
fn harness_sees_injected_latency_bandwidth_and_tail() {
    let (_g, f) = fx();
    assert_all("harness self-test", &selftest(&f.env, "small.bin", "big.bin"));
}

#[test]
fn every_policy_and_decode_mode_gives_the_reference_result() {
    let (_g, f) = fx();
    let exp = expected(f);
    let get = |q: &'static QuerySpec| exp(q);
    for (name, files, _) in [T_BIG, T_SMALL, T_FILES] {
        let ds = f.env.dataset(name, files);
        assert_all(name, &correctness(&f.env, &ds, &get, &[&Q_SUM, &Q_GRP, &Q_LIMIT1], name));
    }
}

#[test]
fn memory_bound_holds_with_a_slow_consumer_and_the_unbounded_control_exceeds_it() {
    let (_g, f) = fx();
    let ds = f.env.dataset(T_SMALL.0, T_SMALL.1);
    let (c, m) = membound(&f.env, &ds, &Q_GRP, &reference(&Q_GRP, &f.rows));
    assert_all("memory bound", &c);
    assert_caught("memory bound", &m);
}

#[test]
fn cancellation_stops_requests_promptly_and_the_known_bad_versions_are_caught() {
    let (_g, f) = fx();
    let ds = f.env.dataset(T_SMALL.0, T_SMALL.1);
    let (c, m) = cancellation(&f.env, &ds);
    assert_all("cancellation", &c);
    assert_caught("cancellation", &m);
}

#[test]
fn retries_are_bounded_errors_fail_cleanly_and_the_known_bad_versions_are_caught() {
    let (_g, f) = fx();
    let ds = f.env.dataset(T_FILES.0, T_FILES.1);
    let exp = reference(&Q_SUM, &f.rows);
    let (c, m) = faults(&f.env, &ds, &Q_SUM, &exp, "part-0003");
    assert_all("faults", &c);
    assert_caught("faults", &m);
}

#[test]
fn object_stores_own_retry_is_a_working_reference() {
    let (_g, f) = fx();
    let names: Vec<String> = (0..T_FILES.1).map(spike_object_store_scan::layouts::file_name).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    assert_all("builtin retry", &builtin_retry_reference(&f.env, &refs, T_FILES.0, &Q_SUM, &reference(&Q_SUM, &f.rows)));
}

#[test]
fn request_pattern_follows_the_policy_and_the_waste_metric_can_be_non_zero() {
    let (_g, f) = fx();
    let ds = f.env.dataset(T_SMALL.0, T_SMALL.1);
    f.env.ctl.clean().unwrap();
    f.env.ctl.command("profile zero").unwrap();
    let run = |p: Policy| {
        f.env.ctl.command("reset").unwrap();
        let mut cfg = ScanConfig::new(p, DecodeMode::Pool);
        cfg.pool_threads = 4;
        let o = run_scan(&f.env.rt, &ds, &Q_SUM, &cfg);
        let srv = f.env.ctl.stat("requests").unwrap();
        assert!(o.ok(), "{:?}", o.error);
        assert_eq!(o.text.as_deref().map(normalize), Some(normalize(&reference(&Q_SUM, &f.rows))));
        assert_eq!(o.attempts, srv, "the client counts the requests the server sees");
        o
    };
    let a = run(Policy::a());
    let b = run(Policy::b());
    let big_gap = run(Policy { gap: Some(u64::MAX / 4), ..Policy::c() });
    assert_eq!(a.data_requests, b.data_requests, "A and B ask for the same ranges; only the concurrency differs");
    assert_eq!(a.peak_inflight, 1, "A never has more than one request in flight");
    assert!(b.peak_inflight > 1, "B overlaps requests (peak {})", b.peak_inflight);
    assert_eq!(b.data_bytes, b.needed_bytes, "without merging exactly the needed bytes are fetched (waste 0)");
    assert!(big_gap.data_requests < b.data_requests, "merging reduces the requests: {} vs {}", big_gap.data_requests, b.data_requests);
    assert!(big_gap.waste_pct() > 0.0, "merging across the columns in between fetches bytes that are not needed");
    assert!(big_gap.data_bytes > big_gap.needed_bytes);
    // span 8 merges neighbouring row groups: fewer requests than C
    let c = run(Policy::c());
    let e = run(Policy::e());
    assert!(e.data_requests <= c.data_requests, "E {} vs C {}", e.data_requests, c.data_requests);
}

#[test]
fn a_producer_that_ends_without_done_makes_the_query_fail_not_return_a_short_result() {
    let waker = Waker::new(|| {});
    let mk = || {
        let ctl = std::sync::Arc::new(ScanCtl { cancel: CancelToken::new(), stats: FetchStats::new() });
        let (tx, rx) = channel::<Item>(2);
        (tx, ScanSources::new(vec![rx], ctl, &Q_SUM).make())
    };
    // known-bad: the sender goes away without a Done marker
    let (tx, mut src) = mk();
    drop(tx);
    assert!(src.next(&waker).is_err(), "a vanished producer must be an error");
    // good: Done then close is a normal end
    let (tx, mut src) = mk();
    assert!(futures::executor::block_on(tx.send(Item::Done)).is_ok());
    drop(tx);
    assert!(matches!(src.next(&waker), Ok(spike_object_store_scan::api::SourceResult::Finished)));
}

#[test]
fn the_reference_and_the_generated_files_agree_with_duckdb() {
    let (_g, f) = fx();
    if std::process::Command::new("duckdb").arg("--version").output().is_err() {
        eprintln!("SKIPPED: duckdb is not on PATH (the GCP run has it)");
        return;
    }
    let glob = format!("read_parquet('{}/{}/*.parquet')", f.dir.display(), T_FILES.0);
    for q in [&Q_SUM, &Q_GRP] {
        let d = spike_object_store_scan::duck::expected_local(&glob, q).expect("duckdb");
        assert_eq!(normalize(&d), normalize(&reference(q, &f.rows)), "query {}", q.name);
    }
    let _ = &f.host;
}
