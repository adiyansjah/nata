//! RELIABILITY (card 0002, row "Reliability"): random scenarios of `nata sql`'s executor with a printed, replayable seed. Each scenario is a random
//! query (filter, projection, limit and offset, an expression that fails in one chosen row) over six generated files, with a random thread count, window
//! and order, and ONE random disturbance: nothing, a consumer that fails (a closed stdout), an external cancel at a random moment (what Ctrl-C does),
//! an I/O error or a panic injected into the opener at a random file or row group, a data error in a random row (an error in another task), or only
//! random delays. After EVERY scenario the test checks, as slice 3's `scenarios.rs` does for the scan:
//!   * no file handle is alive (the fault opener counts them), no footer is held, no decoded byte is counted as alive;
//!   * no thread of the query is alive (the process's thread count is what it was before);
//!   * the output relates to the single-thread reference as it must: ordered = a prefix (equal when nothing disturbed it, equal to the single-thread
//!     output of the SAME faults when a fault fired); unordered = a sub-multiset;
//!   * the error is the one of the lowest (file, row group) that the single-thread run reports, with the same text.
//!
//! A watchdog aborts the process (printing the scenario) after 30 seconds on one scenario: no hang.
//!
//! `NATA_SCENARIOS` (default 600; the long run is 2500), `NATA_SCENARIO_SEED` (default 20261106); a failing scenario prints `scenario N of seed S: ...`
//! and `NATA_SCENARIO_ONLY=N` replays just that one. `NATA_COMPETITORS=1` starts a busy loop on every core for the whole run (the scheduler must not
//! depend on getting a core). Known-bad versions of the executor are caught: `mutate.py`.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "test data and a seeded generator"
)]

mod common;
#[path = "../../nata-parquet/tests/common/faults.rs"]
mod faults;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use faults::{Fault, FaultKind, FaultOpener};
use nata_exec::output::{Format, ResultWriter};
use nata_exec::{ExecConfig, Observed, execute_observed};
use nata_parquet::{ErrorKind, ScanCancel, ScanError};
use nata_sql::{FsSource, plan_sql};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

fn thread_count() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Threads:"))
                .and_then(|l| l.split_whitespace().nth(1).and_then(|n| n.parse().ok()))
        })
        .unwrap_or(0)
}

/// The thread count once the kernel has finished removing the threads that were joined.
fn settled_thread_count(expected: usize) -> usize {
    let t = Instant::now();
    loop {
        let n = thread_count();
        if n <= expected || t.elapsed() > Duration::from_secs(2) {
            return n;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[derive(Debug, Clone)]
enum Disturbance {
    None,
    SinkFails(usize),
    ExternalCancel(u64),
    /// A data error (division by zero) in the row with this id.
    DataError(i64),
    Faults(Vec<Fault>),
}

#[derive(Debug, Clone)]
struct Scenario {
    threads: usize,
    window: usize,
    ordered: bool,
    query: usize,
    delays: Vec<Fault>,
    disturbance: Disturbance,
}

/// `{F}` is the glob of the six files; `{E}` is an expression that is `0` and raises an error in the row `K` when the scenario asks for one.
const QUERIES: &[&str] = &[
    "SELECT id{E} FROM '{F}'",
    "SELECT id, cat FROM '{F}' WHERE id >= 150 AND id % 2 = 0",
    "SELECT id + 1{E} AS n, cat FROM '{F}' WHERE cat <> 'cat2' OR id < 50",
    "SELECT id{E} FROM '{F}' LIMIT 100 OFFSET 7",
    "SELECT id, CASE WHEN id % 5 = 0 THEN 'five' ELSE cat END AS c, id{E} AS z FROM '{F}' WHERE id < 600 LIMIT 333",
    "SELECT cat, id{E} AS z FROM '{F}' WHERE id BETWEEN 100 AND 400",
    "SELECT id{E} FROM '{F}' WHERE id IN (3, 77, 140, 590, 803) OR cat = 'cat4' LIMIT 5000",
    "SELECT 1 AS one, id{E} AS z FROM '{F}' WHERE id % 7 = 0",
    "SELECT id FROM '{F}' WHERE id > 200 AND id < 700 LIMIT 0",
];

fn make(rng: &mut Rng, files: usize, groups: &[usize]) -> Scenario {
    let threads = 1 + rng.below(8) as usize;
    let window = if rng.below(3) == 0 {
        1 + rng.below(2 * threads as u64) as usize
    } else {
        threads
    };
    let mut delays = vec![];
    for _ in 0..rng.below(4) {
        let file = rng.below(files as u64) as usize;
        let rg = if rng.below(4) == 0 {
            None
        } else {
            Some(rng.below(groups[file].max(1) as u64) as usize)
        };
        delays.push(Fault {
            file,
            rg,
            kind: FaultKind::DelayMs(rng.below(3)),
        });
    }
    let disturbance = match rng.below(10) {
        0 | 1 => Disturbance::None,
        2 | 3 => Disturbance::SinkFails(1 + rng.below(6) as usize),
        4 | 5 => Disturbance::ExternalCancel(rng.below(3000)),
        6 | 7 => Disturbance::DataError(rng.below(871) as i64),
        _ => {
            let mut fs = vec![];
            for _ in 0..1 + rng.below(3) {
                let file = rng.below(files as u64) as usize;
                let rg = if rng.below(4) == 0 {
                    None
                } else {
                    Some(rng.below(groups[file].max(1) as u64) as usize)
                };
                let kind = match rng.below(5) {
                    0 | 1 => FaultKind::FailOpen,
                    2 | 3 => FaultKind::FailReadsAfter(rng.below(2) * 40),
                    _ => FaultKind::Panic,
                };
                let kind = if rg.is_none() && matches!(kind, FaultKind::FailReadsAfter(_)) {
                    FaultKind::FailOpen
                } else {
                    kind
                };
                // the first footer is read when the plan is opened, before the scan: a fault there would fail `open`, not the scan
                let rg = if file == 0 && rg.is_none() {
                    Some(0)
                } else {
                    rg
                };
                fs.push(Fault { file, rg, kind });
            }
            Disturbance::Faults(fs)
        }
    };
    Scenario {
        threads,
        window,
        ordered: rng.below(4) != 0,
        query: rng.below(QUERIES.len() as u64) as usize,
        delays,
        disturbance,
    }
}

struct Run {
    csv: String,
    result: Result<(), ScanError>,
    stats: Option<Arc<nata_parquet::ScanStats>>,
}

fn run_one(
    sql: &str,
    cfg: &ExecConfig,
    sink_fails_after: Option<usize>,
    cancel: Option<&ScanCancel>,
) -> Run {
    let mut cfg = cfg.clone();
    cfg.late_materialization |= std::env::var("NATA_LATE_MATERIALIZATION").is_ok_and(|v| v == "1");
    if let Some(k) = std::env::var("NATA_FOOTERS_AHEAD")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        cfg.footers_ahead = cfg.footers_ahead.max(k);
    }
    let cfg = &cfg;
    let planned = plan_sql(sql, &FsSource).expect("the query plans");
    let names: Vec<String> = planned.output.iter().map(|c| c.name.clone()).collect();
    let types: Vec<_> = planned.output.iter().map(|c| c.ty).collect();
    let mut buf: Vec<u8> = Vec::new();
    let mut observed = Observed::default();
    let result = {
        let mut w = ResultWriter::new(&mut buf, Format::Csv, names, &types);
        let mut batches = 0usize;
        let r = execute_observed(Arc::new(planned.plan), cfg, cancel, &mut observed, |b| {
            batches += 1;
            if sink_fails_after.is_some_and(|n| batches > n) {
                return Err(ScanError::new(ErrorKind::Io(
                    "Broken pipe (os error 32)".into(),
                )));
            }
            w.write_batch(b)
        });
        match r {
            Ok(_) => w.finish(),
            Err(e) => {
                let _ = w.flush_partial();
                Err(e)
            }
        }
    };
    Run {
        csv: String::from_utf8(buf).expect("utf-8"),
        result,
        stats: observed.stats,
    }
}

fn lines(csv: &str) -> Vec<String> {
    csv.lines().skip(1).map(str::to_string).collect()
}

fn multiset(v: &[String]) -> HashMap<&str, usize> {
    let mut m = HashMap::new();
    for r in v {
        *m.entry(r.as_str()).or_insert(0) += 1;
    }
    m
}

fn is_sub_multiset(got: &[String], all: &[String]) -> bool {
    let a = multiset(all);
    multiset(got)
        .into_iter()
        .all(|(k, n)| a.get(k).is_some_and(|m| *m >= n))
}

static CURRENT: Mutex<Option<(Instant, String)>> = Mutex::new(None);
static DONE: AtomicBool = AtomicBool::new(false);

fn watchdog() {
    std::thread::spawn(|| {
        while !DONE.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(500));
            let cur = CURRENT.lock().map(|g| g.clone()).unwrap_or(None);
            if let Some((t, what)) = cur
                && t.elapsed() > Duration::from_secs(30)
            {
                eprintln!("HANG: {what}");
                std::process::abort();
            }
        }
    });
}

/// A busy loop on every core until the returned flag is set.
fn competitors() -> Option<(Arc<AtomicBool>, Vec<std::thread::JoinHandle<()>>)> {
    if std::env::var("NATA_COMPETITORS").is_err() {
        return None;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let n = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let hs = (0..n)
        .map(|_| {
            let s = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut x = 1u64;
                while !s.load(Ordering::Relaxed) {
                    x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                    std::hint::black_box(x);
                }
            })
        })
        .collect();
    Some((stop, hs))
}

#[test]
fn random_cancel_error_and_delay_scenarios_of_nata_sql_leave_nothing_behind() {
    std::panic::set_hook(Box::new(|info| {
        let injected = info
            .payload()
            .downcast_ref::<&str>()
            .is_some_and(|m| m.contains("injected panic"));
        if !injected {
            eprintln!("{info}");
        }
    }));
    let seed: u64 = std::env::var("NATA_SCENARIO_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_261_106);
    let count: u64 = std::env::var("NATA_SCENARIOS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(600);
    let only: Option<u64> = std::env::var("NATA_SCENARIO_ONLY")
        .ok()
        .and_then(|v| v.parse().ok());
    eprintln!(
        "scenarios: seed {seed}, count {count}, only {only:?} (replay one with NATA_SCENARIO_SEED={seed} NATA_SCENARIO_ONLY=<n>)"
    );

    let dir = common::dir("scenarios");
    // six files of 40, 80, 120, 200, 271 and 160 rows in row groups of 40: 1, 2, 3, 5, 7 and 4 row groups, ids 0..870 in file order
    let sizes: [i64; 6] = [40, 80, 120, 200, 271, 160];
    let mut files: Vec<PathBuf> = vec![];
    let mut groups = vec![];
    let mut first = 0;
    for (i, rows) in sizes.iter().enumerate() {
        let p = dir.join(format!("s{i}.parquet"));
        write_ids(&p, first, *rows, 40);
        first += rows;
        groups.push((*rows as usize).div_ceil(40));
        files.push(p);
    }
    let pattern = dir.join("s*.parquet").display().to_string();
    watchdog();
    let comp = competitors();
    let threads_before = thread_count();
    let mut classes: HashMap<&'static str, u64> = HashMap::new();
    let mut ran = 0;
    for n in 0..count {
        if only.is_some_and(|o| o != n) {
            continue;
        }
        let mut rng = Rng(seed ^ n.wrapping_mul(0xD6E8_FEB8_6659_FD93));
        let s = make(&mut rng, files.len(), &groups);
        let what = format!("scenario {n} of seed {seed}: {s:?}");
        *CURRENT.lock().expect("lock") = Some((Instant::now(), what.clone()));
        ran += 1;
        let e = match &s.disturbance {
            Disturbance::DataError(k) => format!(" + 100 // (id - {k}) - 100 // (id - {k})"),
            _ => String::new(),
        };
        let sql = QUERIES[s.query].replace("{F}", &pattern).replace("{E}", &e);
        let faults: Vec<Fault> = match &s.disturbance {
            Disturbance::Faults(f) => f.clone(),
            _ => vec![],
        };
        let sink_fails = match s.disturbance {
            Disturbance::SinkFails(k) => Some(k),
            _ => None,
        };
        let serial = |opener: Arc<FaultOpener>, sink_fails: Option<usize>| {
            let cfg = ExecConfig {
                threads: 1,
                window: 1,
                ordered: true,
                opener: Some(opener),
                late_materialization: false,
                footers_ahead: 1,
            };
            run_one(&sql, &cfg, sink_fails, None)
        };
        // the references: the single-thread run with the same faults, and a clean run of the same query
        let reference = serial(FaultOpener::new(&files, faults.clone()), sink_fails);
        let clean_sql = sql.replace(&e, "");
        let clean = {
            let cfg = ExecConfig {
                threads: 1,
                window: 1,
                ordered: true,
                opener: Some(FaultOpener::new(&files, vec![])),
                late_materialization: false,
                footers_ahead: 1,
            };
            run_one(&clean_sql, &cfg, None, None)
        };
        assert!(clean.result.is_ok(), "{what}: {:?}", clean.result);
        let clean_rows = lines(&clean.csv);
        // every row the query could return, whatever the limit: what an unordered run with a limit may return a part of
        let universe = if clean_sql.contains(" LIMIT ") {
            let unlimited = clean_sql
                .split(" LIMIT ")
                .next()
                .unwrap_or(&clean_sql)
                .to_string();
            let cfg = ExecConfig {
                threads: 1,
                window: 1,
                ordered: true,
                opener: Some(FaultOpener::new(&files, vec![])),
                late_materialization: false,
                footers_ahead: 1,
            };
            lines(&run_one(&unlimited, &cfg, None, None).csv)
        } else {
            clean_rows.clone()
        };

        // the scenario
        let mut all = s.delays.clone();
        all.extend(faults.clone());
        let opener = FaultOpener::new(&files, all);
        let cancel = ScanCancel::new();
        let canceller = if let Disturbance::ExternalCancel(us) = s.disturbance {
            let c = cancel.clone();
            Some(std::thread::spawn(move || {
                std::thread::sleep(Duration::from_micros(us));
                c.cancel();
            }))
        } else {
            None
        };
        let cfg = ExecConfig {
            threads: s.threads,
            window: s.window,
            ordered: s.ordered,
            opener: Some(opener.clone()),
            late_materialization: false,
            footers_ahead: 1,
        };
        let got = run_one(&sql, &cfg, sink_fails, Some(&cancel));
        if let Some(c) = canceller {
            c.join().expect("canceller");
        }

        // 1. nothing is left behind
        assert_eq!(
            opener.live_handles(),
            0,
            "{what}: a file handle was not closed"
        );
        let st = got.stats.as_ref().expect("the scan was opened");
        assert_eq!(st.handles.now(), 0, "{what}: handle gauge");
        assert_eq!(st.footers.now(), 0, "{what}: footers still held");
        assert_eq!(
            st.memory.now(),
            0,
            "{what}: decoded bytes still counted alive"
        );
        assert_eq!(
            settled_thread_count(threads_before),
            threads_before,
            "{what}: a thread of the query is still alive"
        );

        // 2. the output and the error
        let rows = lines(&got.csv);
        let ref_rows = lines(&reference.csv);
        let class: &'static str = match (&s.disturbance, &got.result) {
            (Disturbance::None, r) => {
                assert_eq!(r.as_ref().err().map(ToString::to_string), None, "{what}");
                if s.ordered {
                    assert_eq!(
                        rows, clean_rows,
                        "{what}: ordered output equals the single-thread output"
                    );
                } else {
                    assert!(
                        rows.len() == clean_rows.len() && is_sub_multiset(&rows, &universe),
                        "{what}: unordered multiset"
                    );
                }
                "complete"
            }
            (Disturbance::SinkFails(_), r) => {
                if reference.result.is_ok() {
                    assert!(r.is_ok(), "{what}: {r:?}");
                    if s.ordered {
                        assert_eq!(rows, ref_rows, "{what}");
                    }
                    "sink-not-reached"
                } else {
                    let msg = r.as_ref().expect_err(&what).to_string();
                    assert!(msg.contains("Broken pipe"), "{what}: {msg}");
                    if s.ordered {
                        assert_eq!(
                            rows, ref_rows,
                            "{what}: delivered rows before the failing sink"
                        );
                    } else {
                        assert!(is_sub_multiset(&rows, &universe), "{what}");
                    }
                    "sink-failed"
                }
            }
            (Disturbance::ExternalCancel(_), r) => match r {
                Ok(()) => {
                    if s.ordered {
                        assert_eq!(rows, clean_rows, "{what}: finished before the cancel");
                    }
                    "cancel-too-late"
                }
                Err(e) => {
                    assert!(matches!(e.kind, ErrorKind::Cancelled(_)), "{what}: {e}");
                    if s.ordered {
                        assert!(
                            rows.len() <= clean_rows.len() && rows == clean_rows[..rows.len()],
                            "{what}: delivered rows are a prefix"
                        );
                    } else {
                        assert!(is_sub_multiset(&rows, &universe), "{what}");
                    }
                    "cancelled"
                }
            },
            (Disturbance::DataError(_) | Disturbance::Faults(_), r)
                if !s.ordered && sql.contains("LIMIT") =>
            {
                // unordered with a limit: which row groups arrive before the limit is met is timing, so the single-thread run is no reference; what
                // must hold is that the rows are rows of the query and that an error is a task error
                assert!(is_sub_multiset(&rows, &universe), "{what}");
                if let Err(e) = r {
                    assert!(!matches!(e.kind, ErrorKind::Cancelled(_)), "{what}: {e}");
                }
                "unordered-limit-with-fault"
            }
            (Disturbance::DataError(_) | Disturbance::Faults(_), r) => match (&reference.result, r)
            {
                (Err(want), Err(e)) => {
                    assert_eq!(
                        e.to_string(),
                        want.to_string(),
                        "{what}: the error of the lowest (file, row group)"
                    );
                    assert!(!matches!(e.kind, ErrorKind::Cancelled(_)), "{what}");
                    if s.ordered {
                        assert_eq!(
                            rows, ref_rows,
                            "{what}: every row before the error, none after"
                        );
                    } else {
                        assert!(is_sub_multiset(&rows, &universe), "{what}");
                    }
                    match &e.kind {
                        ErrorKind::Internal(_) => "fault-panic",
                        ErrorKind::Eval { .. } => "data-error",
                        _ => "fault-error",
                    }
                }
                (Ok(()), Ok(())) => {
                    // the error or the fault was never reached (a limit, a filter, pruning)
                    if s.ordered {
                        assert_eq!(rows, ref_rows, "{what}");
                    }
                    "not-reached"
                }
                (a, b) => panic!("{what}: serial {a:?} but parallel {b:?}"),
            },
        };
        *classes.entry(class).or_insert(0) += 1;
        *CURRENT.lock().expect("lock") = None;
    }
    DONE.store(true, Ordering::SeqCst);
    if let Some((stop, hs)) = comp {
        stop.store(true, Ordering::SeqCst);
        for h in hs {
            h.join().expect("competitor");
        }
    }
    let mut report: Vec<_> = classes.into_iter().collect();
    report.sort();
    eprintln!("scenarios run: {ran}; outcome classes: {report:?}");
    if only.is_none() && count >= 500 {
        for k in [
            "complete",
            "sink-failed",
            "cancelled",
            "fault-error",
            "fault-panic",
            "data-error",
            "not-reached",
        ] {
            assert!(
                report.iter().any(|(c, n)| *c == k && *n > 0),
                "the scenario mix never produced the class {k}: {report:?}"
            );
        }
    }
}

fn write_ids(path: &std::path::Path, first: i64, rows: i64, group: usize) {
    use arrow_array::{ArrayRef, Int64Array, RecordBatch, StringArray};
    let ids: ArrayRef = Arc::new(Int64Array::from_iter_values(first..first + rows));
    let cat: ArrayRef = Arc::new(StringArray::from_iter_values(
        (first..first + rows).map(|i| format!("cat{}", i % 7)),
    ));
    let batch = RecordBatch::try_from_iter([("id", ids), ("cat", cat)]).expect("batch");
    common::write_batch(path, &batch, group);
}
