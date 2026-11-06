//! The abstract-event traces of the trace-validation check (verification-plan item 24, code gate). With `IC_TRACE` set the
//! suite scenarios must behave exactly as without it, and every run must leave a well-formed trace. This file sets the
//! variable for its own process only (the other test binaries run with tracing off). The check of the traces against the
//! TLA+ model is `model/trace_check.py`; this test is about the instrumentation.
use iceberg_commit::catalog::{Catalog, FaultPlan, FaultyCatalog};
use iceberg_commit::suite::{self, Backend, Expect};
use iceberg_commit::writer::{Isolation, Journal, Kind, Mode, Outcome, SyncCfg, Writer};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// One trace directory for the whole process: the variable is process-wide and the two tests run in parallel threads.
fn trace_dir() -> &'static PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let d = work("traces");
        // Only this test binary runs with the variable set; it is set once, before any table exists.
        unsafe { std::env::set_var("IC_TRACE", &d) };
        d
    })
}

fn work(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ic-trace-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn events(dir: &Path) -> BTreeMap<String, Vec<Value>> {
    let mut out = BTreeMap::new();
    for e in std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()) {
        let p = e.path();
        if p.extension().map(|x| x == "jsonl").unwrap_or(false) {
            let evs: Vec<Value> = std::fs::read_to_string(&p).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
            out.insert(p.file_stem().unwrap().to_string_lossy().to_string(), evs);
        }
    }
    out
}

struct RemoveEmpty(PathBuf);
impl Drop for RemoveEmpty {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0); // only succeeds once the other test has cleaned up too
    }
}

/// Removes the trace files (and their lock and counter files) of the runs whose table name passes `which`.
fn forget(dir: &Path, which: impl Fn(&str) -> bool) {
    let _guard = RemoveEmpty(dir.to_path_buf());
    for e in std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()) {
        let name = e.file_name().to_string_lossy().to_string();
        let table = name.split('.').next().unwrap_or("");
        if which(table) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

fn kinds(evs: &[Value]) -> Vec<String> {
    evs.iter().map(|e| e["a"].as_str().unwrap().to_string()).collect()
}

#[test]
fn traced_runs_behave_as_untraced_runs_and_leave_well_formed_traces() {
    let w = work("all");
    let tr = trace_dir().clone();
    let backend = Backend::Store;
    let forced = suite::run_all_forced(&backend, &w);
    let crash = suite::run_crash_matrix(&backend, &w);
    let bad: Vec<String> = forced.iter().chain(crash.iter()).filter(|r| !r.ok()).map(|r| r.line()).collect();
    assert!(bad.is_empty(), "tracing changed what the scenarios do:\n{}", bad.join("\n"));
    assert_eq!(forced.iter().filter(|r| matches!(r.expect, Expect::Fail(_)) && r.ok()).count(), 5, "5 known-bad writers must still be caught");

    let all: BTreeMap<String, Vec<Value>> = events(&tr).into_iter().filter(|(t, _)| !t.contains("finding")).collect();
    assert_eq!(all.len(), forced.len() + crash.len(), "one trace per run");
    for (table, evs) in &all {
        let k = kinds(evs);
        assert_eq!(k.iter().filter(|a| *a == "meta").count(), 1, "{table}: one meta event");
        assert_eq!(k.iter().filter(|a| *a == "final").count(), 1, "{table}: one final event");
        assert!(evs.iter().any(|e| e["a"] == "plan" && e["op"] == "w0o0"), "{table}: the seed commit is traced");
        // sequence numbers: an event has its own, except the two lines of one catalog call and a decision placed at a read
        let mut seen: BTreeMap<u64, Vec<&str>> = BTreeMap::new();
        for e in evs {
            seen.entry(e["seq"].as_u64().unwrap()).or_default().push(e["a"].as_str().unwrap());
        }
        for (seq, group) in &seen {
            let n = group.iter().filter(|a| **a != "deliver-intent" && **a != "deliver").count() + group.iter().any(|a| *a == "deliver") as usize;
            assert!(n == 1, "{table}: sequence number {seq} is shared by {group:?}");
        }
        // every request that reached the catalog is recorded with its outcome
        let intents = k.iter().filter(|a| *a == "deliver-intent").count();
        let delivered = k.iter().filter(|a| *a == "deliver").count();
        assert_eq!(intents, delivered, "{table}: every intent has its outcome (no writer was SIGKILLed here)");
    }
    // the scenarios that exercise a fault leave its event
    let find = |needle: &str| all.iter().find(|(t, _)| t.contains(needle)).map(|(_, e)| kinds(e)).unwrap_or_default();
    assert!(find("lost_request_correct").contains(&"drop".to_string()), "drop-request leaves a drop event");
    assert!(find("unknown_outcome_correct").contains(&"timeout".to_string()));
    assert!(find("unknown_outcome_correct").contains(&"recover-done".to_string()));
    assert!(find("duplicate_request_correct").iter().filter(|a| *a == "deliver").count() == 3, "a duplicate request is delivered twice (plus the seed)");
    assert!(find("crash_append_after_commit").contains(&"crash".to_string()));
    assert!(find("lost_update_correct").contains(&"recover-retry".to_string()));
    forget(&tr, |t| !t.contains("finding"));
    let _ = std::fs::remove_dir_all(&w);
}

/// A deterministic run of the one place where the code and the model were found to differ (see the record, section on trace
/// validation): two deleters of the same rows commit one after the other, the second one a redundant delete file; a
/// compaction that planned between the two finds a delete on its input files and aborts. The code aborts (Iceberg's
/// validateNoNewDeletesForDataFiles does the same). The first model counted only pairs that were not yet deleted and demanded
/// a retry; since 2026-11-06 the model conflicts with any delete file naming an input (`delTargets`), so the trace of this run,
/// the fixture `model/testdata/traces/finding_redundant_delete.jsonl`, is VALID (expectation `pass`).
#[test]
fn redundant_delete_makes_the_compaction_abort() {
    let w = work("finding");
    let tr = trace_dir().clone();
    let t = suite::new_table(&Backend::Store, &w, "finding-redundant-delete").unwrap();
    suite::trace_meta(&t, "finding-redundant-delete", Mode::Correct, "pass");
    suite::seed_table(&t, 6).unwrap();
    let (dir_a, dir_b) = (w.join("sync-a"), w.join("sync-b"));
    let sync = |dir: &Path, rank: u32| Some(SyncCfg { dir: dir.to_path_buf(), rank, n: 2, timeout: Duration::from_secs(20) });
    let spawn = |id: u32, kind: Kind, sync: Option<SyncCfg>, faults: &str| {
        let mut cfg = suite::writer_cfg(id, 0, kind, Mode::Correct, Isolation::Serializable, &["b"], 0, 300 + id as u64);
        cfg.sync = sync;
        let cat: Arc<dyn Catalog> = Arc::new(FaultyCatalog::new(t.catalog.clone(), FaultPlan::parse(faults).unwrap()));
        let journal = Arc::new(Journal::open(&t.journal_dir, &format!("w{id}")).unwrap());
        std::thread::spawn(move || Writer::new(cfg, cat, journal).run())
    };
    // D2 (id 2) commits first; D1 (id 1) planned before that, waits its turn and is held back 600 ms before its first commit
    let d2 = spawn(2, Kind::Delete, sync(&dir_a, 0), "");
    let d1 = spawn(1, Kind::Delete, sync(&dir_a, 1), "delay@1=600");
    let t0 = std::time::Instant::now();
    while !dir_a.join("done-0").exists() {
        assert!(t0.elapsed() < Duration::from_secs(20), "D2 never committed");
        std::thread::sleep(Duration::from_millis(5));
    }
    // C plans on D2's table (after D2, before D1's retry) and waits until D1 is done
    std::fs::create_dir_all(&dir_b).unwrap();
    std::fs::write(dir_b.join("ready-0"), b"").unwrap();
    let c = spawn(3, Kind::Compact, sync(&dir_b, 1), "");
    let (o2, o1) = (d2.join().unwrap(), d1.join().unwrap());
    std::fs::write(dir_b.join("done-0"), b"").unwrap();
    let o3 = c.join().unwrap();
    assert!(matches!(o2, Outcome::Acked(_)), "{o2:?}");
    assert!(matches!(o1, Outcome::Acked(_)), "D1 retries after the conflict and commits: {o1:?}");
    match &o3 {
        Outcome::Aborted(m) => assert!(m.contains("added deletes"), "{m}"),
        other => panic!("the compaction must abort on D1's delete file, got {other:?}"),
    }
    let rep = iceberg_commit::checker::check(t.catalog.as_ref(), &t.journal_dir);
    assert!(rep.ok(), "{}", rep.render());
    let evs: BTreeMap<String, Vec<Value>> = events(&tr).into_iter().filter(|(t, _)| t.contains("finding")).collect();
    assert_eq!(evs.len(), 1);
    let k = kinds(evs.values().next().unwrap());
    assert!(k.contains(&"recover-abort".to_string()) && k.contains(&"recover-retry".to_string()));
    if let Ok(keep) = std::env::var("IC_TRACE_KEEP_FINDING") {
        // refresh the fixture: IC_TRACE_KEEP_FINDING=<file> cargo test --release --test trace
        std::fs::copy(tr.join(format!("{}.jsonl", evs.keys().next().unwrap())), keep).unwrap();
    }
    forget(&tr, |t| t.contains("finding"));
    let _ = std::fs::remove_dir_all(&w);
}
