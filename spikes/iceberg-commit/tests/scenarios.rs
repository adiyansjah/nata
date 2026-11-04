//! The whole harness against the test double (mock REST catalog) and against the conditional-create object store:
//! the correct writer must pass the checker in every scenario, every known-bad writer must be caught, the crash matrix and
//! a few stress seeds must pass, and the checker must see tampering with the files of a finished table.
//! The mock is not evidence about Iceberg (see src/mock.rs); this test is evidence that the HARNESS can tell good from bad.
use iceberg_commit::catalog::{Catalog, StoreCatalog};
use iceberg_commit::mock::MockServer;
use iceberg_commit::suite::{self, Backend, Expect, StressCfg};
use iceberg_commit::writer::{Isolation, Mode};
use iceberg_commit::checker;
use std::path::PathBuf;

fn work(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ic-scn-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn assert_all_ok(results: &[suite::ScenarioResult]) {
    let bad: Vec<String> = results.iter().filter(|r| !r.ok()).map(|r| format!("{}\n{}", r.line(), r.report.render())).collect();
    assert!(bad.is_empty(), "scenarios that did not behave as expected:\n{}", bad.join("\n"));
}

fn bad_writer_expectations(results: &[suite::ScenarioResult]) {
    // every known-bad mode was exercised at least once and detected
    for mode in [Mode::BadNoCas, Mode::BadBlindRetry, Mode::BadSkipValidation] {
        let detected = results.iter().filter(|r| r.mode == mode && matches!(r.expect, Expect::Fail(_)) && r.ok()).count();
        assert!(detected >= 1, "{} was never caught", mode.name());
    }
}

#[test]
fn mock_rest_forced_scenarios_and_crash_matrix() {
    let w = work("rest");
    let mock = MockServer::start(w.join("warehouse")).unwrap();
    let backend = Backend::Rest(mock.url.clone());
    let forced = suite::run_all_forced(&backend, &w);
    assert_all_ok(&forced);
    bad_writer_expectations(&forced);
    assert_all_ok(&suite::run_crash_matrix(&backend, &w));
    mock.stop();
}

#[test]
fn store_forced_scenarios_and_crash_matrix() {
    let w = work("store");
    let backend = Backend::Store;
    let forced = suite::run_all_forced(&backend, &w);
    assert_all_ok(&forced);
    bad_writer_expectations(&forced);
    assert_all_ok(&suite::run_crash_matrix(&backend, &w));
}

#[test]
fn stress_with_writer_processes_passes_for_the_correct_writer() {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_ic-spike"));
    let w = work("stress");
    let mock = MockServer::start(w.join("warehouse")).unwrap();
    for backend in [Backend::Rest(mock.url.clone()), Backend::Store] {
        for seed in 1..=2u64 {
            let cfg = StressCfg { writers: 6, ops_per_writer: 3, seed, mode: Mode::Correct, isolation: Isolation::Serializable, kills: 1, p_fault: 0.3, p_crash: 0.2 };
            let r = suite::run_stress(&backend, &w, &exe, &cfg);
            assert!(r.error.is_none(), "{:?}", r.error);
            assert!(r.report.ok(), "{} seed {seed}:\n{}\n{:?}", backend.name(), r.report.render(), r.exits);
            assert!(r.report.snapshots_checked >= 2);
        }
    }
    mock.stop();
}

/// The checker must fail on a finished, healthy table that is then damaged in the ways the invariants name.
#[test]
fn checker_sees_tampering() {
    let w = work("tamper");
    let backend = Backend::Store;
    let make = |label: &str| {
        let t = suite::new_table(&backend, &w, label).unwrap();
        suite::seed_table(&t, 6).unwrap();
        for (i, k) in [iceberg_commit::writer::Kind::Append, iceberg_commit::writer::Kind::Delete, iceberg_commit::writer::Kind::Append].into_iter().enumerate() {
            let id = 1 + i as u32;
            let j = std::sync::Arc::new(iceberg_commit::writer::Journal::open(&t.journal_dir, &format!("w{id}")).unwrap());
            let cfg = suite::writer_cfg(id, 0, k, Mode::Correct, Isolation::Serializable, &["a"], 2, id as u64);
            iceberg_commit::writer::Writer::new(cfg, t.catalog.clone(), j).run();
        }
        assert!(checker::check(t.catalog.as_ref(), &t.journal_dir).ok(), "the healthy table must pass");
        t
    };
    let latest = |dir: &PathBuf| -> (PathBuf, serde_json::Value) {
        let mut best = (0u64, PathBuf::new());
        for e in std::fs::read_dir(dir.join("metadata")).unwrap().filter_map(|e| e.ok()) {
            let n = e.file_name().to_string_lossy().to_string();
            if let Some(v) = n.strip_prefix('v').and_then(|s| s.strip_suffix(".metadata.json")).and_then(|s| s.parse::<u64>().ok()) {
                if v > best.0 {
                    best = (v, e.path());
                }
            }
        }
        let v = serde_json::from_str(&std::fs::read_to_string(&best.1).unwrap()).unwrap();
        (best.1, v)
    };

    // I1: a data file disappears
    let t = make("tamper-file");
    let victim = std::fs::read_dir(t.dir.join("data")).unwrap().filter_map(|e| e.ok()).find(|e| e.file_name().to_string_lossy().ends_with(".data.avro")).unwrap();
    std::fs::remove_file(victim.path()).unwrap();
    assert!(checker::check(t.catalog.as_ref(), &t.journal_dir).has("I1"));

    // I6: two snapshots share a sequence number
    let t = make("tamper-seq");
    let (p, mut m) = latest(&t.dir);
    let snaps = m["snapshots"].as_array_mut().unwrap();
    let first_seq = snaps[1]["sequence-number"].clone();
    snaps[2]["sequence-number"] = first_seq;
    std::fs::write(&p, m.to_string()).unwrap();
    assert!(checker::check(t.catalog.as_ref(), &t.journal_dir).has("I6"));

    // I7: a committed snapshot is dropped from the latest metadata
    let t = make("tamper-drop");
    let (p, mut m) = latest(&t.dir);
    let snaps = m["snapshots"].as_array_mut().unwrap();
    snaps.remove(1);
    std::fs::write(&p, m.to_string()).unwrap();
    assert!(checker::check(t.catalog.as_ref(), &t.journal_dir).has("I7"));

    // I3: a snapshot's record changes between metadata versions
    let t = make("tamper-record");
    let (p, mut m) = latest(&t.dir);
    m["snapshots"][0]["timestamp-ms"] = serde_json::json!(1);
    std::fs::write(&p, m.to_string()).unwrap();
    assert!(checker::check(t.catalog.as_ref(), &t.journal_dir).has("I3"));

    // I2: an acknowledged commit is not in the table (the journal says acked, the snapshot is gone from every version)
    let t = make("tamper-journal");
    let j = t.journal_dir.join("w1.jsonl");
    let text = std::fs::read_to_string(&j).unwrap().replace("\"op\":\"w1o0\"", "\"op\":\"w9o9\"");
    std::fs::write(&j, text).unwrap();
    let r = checker::check(t.catalog.as_ref(), &t.journal_dir);
    assert!(r.has("I2"), "{}", r.render());

    // and a store without conflict detection: two creates of the same version, the second must lose
    let c = StoreCatalog::new(w.join("store-two")).unwrap();
    c.create_table().unwrap();
    let base = c.load().unwrap();
    let mk = |id: i64| iceberg_commit::catalog::NewSnapshot { id, parent: None, seq: 1, manifest_list: "x".into(), summary: Default::default() };
    let first = c.commit(&iceberg_commit::catalog::CommitReq { base: &base, snapshot: mk(1), require_cas: true });
    let second = c.commit(&iceberg_commit::catalog::CommitReq { base: &base, snapshot: mk(2), require_cas: true });
    assert!(first.is_ok());
    assert!(matches!(second, Err(iceberg_commit::catalog::CommitError::Conflict(_))), "{second:?}");
}
