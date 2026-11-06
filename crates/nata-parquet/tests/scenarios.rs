//! RELIABILITY (card row "Reliability"): random scenarios with a printed, replayable seed. Each scenario is a small scan (random files, projection,
//! filter, thread count, window, order) with ONE random disturbance: a `LIMIT`, a consumer that fails (a closed stdout), an external cancel at a random
//! moment, an I/O error or a panic in a random file or row group, or only random delays. After EVERY scenario the test checks:
//!   * no file handle is alive (the fault opener counts them, `Tracked` pattern), no footer is held, no decoded byte is counted as alive;
//!   * no thread of the scan is alive (the process's thread count is what it was before);
//!   * the output relates to the reference as it must: ordered = a prefix of the single-thread output (equal when nothing disturbed it, equal to the
//!     single-thread output of the SAME faults when a fault fired); unordered = a sub-multiset;
//!   * the error is the one of the lowest (file, row group) that the single-thread scan reports, with the same text.
//!
//! A watchdog aborts the process (printing the scenario) if one scenario takes more than 30 seconds: no hang.
//!
//!
//! `NATA_SCENARIOS` (default 5000), `NATA_SCENARIO_SEED` (default 20261106); a failing scenario prints `scenario N of seed S: ...`, and
//! `NATA_SCENARIO_ONLY=N` replays just that one. Known-bad versions of the code (a handle forgotten on cancel) are caught: see `mutate.py`.

mod common;
use common::faults::{Fault, FaultKind, FaultOpener};
use common::genfiles::write_ids;
use common::*;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nata_parquet::{
    ErrorKind, ParallelConfig, ScanCancel, ScanError, ScanOptions, ScanPlan, csv,
    run_parallel_with, run_serial,
};

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

    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[self.below(v.len() as u64) as usize]
    }
}

/// The thread count once the kernel has finished removing the threads that were joined (a joined thread can show in `/proc` for a few
/// microseconds); a thread that is really alive stays and the wait runs out.
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

#[derive(Debug, Clone)]
enum Disturbance {
    None,
    Limit(u64),
    SinkFails(usize),
    ExternalCancel(u64),
    Faults(Vec<Fault>),
}

#[derive(Debug, Clone)]
struct Scenario {
    threads: usize,
    window: usize,
    ordered: bool,
    prune: bool,
    columns: Option<Vec<&'static str>>,
    filters: Vec<&'static str>,
    delays: Vec<Fault>,
    disturbance: Disturbance,
}

fn make(rng: &mut Rng, files: usize, groups: &[usize]) -> Scenario {
    let threads = 1 + rng.below(8) as usize;
    let window = if rng.below(3) == 0 {
        1 + rng.below(2 * threads as u64) as usize
    } else {
        threads
    };
    let columns = rng
        .pick(&[
            None,
            Some(vec!["id"]),
            Some(vec!["cat", "id"]),
            Some(vec!["x", "cat"]),
        ])
        .clone();
    let filters = rng
        .pick(&[
            vec![],
            vec!["id >= 150"],
            vec!["id < 300", "cat <> 'cat2'"],
            vec!["cat = 'cat4'"],
            vec!["id IS NULL"],
        ])
        .clone();
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
        2..=4 => Disturbance::Limit(*rng.pick(&[0, 1, 2, 50, 100, 333, 1000, 5000])),
        5 => Disturbance::SinkFails(1 + rng.below(6) as usize),
        6 => Disturbance::ExternalCancel(rng.below(3000)),
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
                // FailReadsAfter at a footer has no effect (the footer is read whole): make it an open failure
                let kind = if rg.is_none() && matches!(kind, FaultKind::FailReadsAfter(_)) {
                    FaultKind::FailOpen
                } else {
                    kind
                };
                // the first footer is read when the plan is opened, before any scan: faults there would make `open` fail, not the scan
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
        prune: rng.below(4) != 0,
        columns,
        filters,
        delays,
        disturbance,
    }
}

fn lines(csv_text: &str) -> Vec<String> {
    records(csv_text)
}

fn multiset(v: &[String]) -> HashMap<&str, usize> {
    let mut m = HashMap::new();
    for r in v {
        *m.entry(r.as_str()).or_insert(0) += 1;
    }
    m
}

/// `got` is a sub-multiset of `all`.
fn is_sub_multiset(got: &[String], all: &[String]) -> bool {
    let a = multiset(all);
    multiset(got)
        .into_iter()
        .all(|(k, n)| a.get(k).is_some_and(|m| *m >= n))
}

struct Outcome {
    csv: String,
    result: Result<(), ScanError>,
    plan: Arc<ScanPlan>,
}

fn run_one(
    files: &[PathBuf],
    s: &Scenario,
    parallel: bool,
    limit: Option<u64>,
    sink_fails_after: Option<usize>,
    cancel: Option<&ScanCancel>,
    opener: &Arc<FaultOpener>,
) -> Outcome {
    let opts = ScanOptions {
        columns: s
            .columns
            .as_ref()
            .map(|c| c.iter().map(|x| (*x).to_string()).collect()),
        filters: s.filters.iter().map(|f| (*f).to_string()).collect(),
        prune: s.prune,
    };
    let plan = Arc::new(
        ScanPlan::open_with(files, &opts, opener.clone())
            .expect("the first footer is never faulty here"),
    );
    let mut out: Vec<u8> = Vec::new();
    let names: Vec<String> = plan.out_schema().iter().map(|(n, _)| n.clone()).collect();
    csv::write_header(&mut out, &names).expect("header");
    let mut batches = 0usize;
    let mut sink = |b: &arrow_array::RecordBatch| -> Result<(), ScanError> {
        batches += 1;
        if sink_fails_after.is_some_and(|n| batches > n) {
            return Err(ScanError::new(ErrorKind::Io(
                "Broken pipe (os error 32)".into(),
            )));
        }
        csv::write_batch(&mut out, b)
    };
    let result = if parallel {
        let cfg = ParallelConfig {
            threads: s.threads,
            window: s.window,
            ordered: s.ordered,
        };
        run_parallel_with(Arc::clone(&plan), cfg, limit, cancel, &mut sink).map(|_| ())
    } else {
        run_serial(&plan, limit, &mut sink).map(|_| ())
    };
    Outcome {
        csv: String::from_utf8(out).expect("utf-8"),
        result,
        plan,
    }
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

#[test]
fn random_cancel_error_and_delay_scenarios_leave_nothing_behind() {
    // panics injected into the opener must not print: the scan reports them as errors
    // the panics injected into the opener are expected: the scan must report them as errors. Any other panic is printed.
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
        .unwrap_or(5000);
    let only: Option<u64> = std::env::var("NATA_SCENARIO_ONLY")
        .ok()
        .and_then(|v| v.parse().ok());
    eprintln!(
        "scenarios: seed {seed}, count {count}, only {only:?} (replay one with NATA_SCENARIO_SEED={seed} NATA_SCENARIO_ONLY=<n>)"
    );

    let dir = scratch("scenarios");
    // 6 files: row groups of 40 rows; the sizes give 1, 2, 3, 5, 7 and 4 row groups, one of them with a short last group
    let sizes: [(i64, usize); 6] = [
        (40, 40),
        (80, 40),
        (120, 40),
        (200, 40),
        (271, 40),
        (160, 40),
    ];
    let mut files = vec![];
    let mut groups = vec![];
    let mut first = 0;
    for (i, (rows, g)) in sizes.iter().enumerate() {
        let p = dir.join(format!("s{i}.parquet"));
        write_ids(&p, first, *rows, *g);
        first += rows;
        groups.push((*rows as usize).div_ceil(*g));
        files.push(p);
    }
    watchdog();
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

        // the reference: the single-thread scan, with the same faults (delays only change timing)
        let faults: Vec<Fault> = match &s.disturbance {
            Disturbance::Faults(f) => f.clone(),
            _ => vec![],
        };
        let all_faults = || {
            let mut v = s.delays.clone();
            v.extend(faults.clone());
            v
        };
        let ref_opener = FaultOpener::new(&files, faults.clone());
        let limit = match s.disturbance {
            Disturbance::Limit(l) => Some(l),
            _ => None,
        };
        let sink_fails = match s.disturbance {
            Disturbance::SinkFails(k) => Some(k),
            _ => None,
        };
        let reference = run_one(&files, &s, false, limit, sink_fails, None, &ref_opener);
        let clean_opener = FaultOpener::new(&files, vec![]);
        let clean = run_one(&files, &s, false, None, None, None, &clean_opener);
        let clean_rows = lines(&clean.csv);
        assert!(clean.result.is_ok(), "{what}");

        // the scenario
        let opener = FaultOpener::new(&files, all_faults());
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
        let got = run_one(&files, &s, true, limit, sink_fails, Some(&cancel), &opener);
        if let Some(c) = canceller {
            c.join().expect("canceller");
        }

        // 1. nothing is left behind
        let st = got.plan.stats();
        assert_eq!(
            opener.live_handles(),
            0,
            "{what}: a file handle was not closed"
        );
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
            "{what}: a thread of the scan is still alive"
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
                        "{what}: ordered output must equal the single-thread output"
                    );
                } else {
                    assert!(
                        rows.len() == clean_rows.len() && is_sub_multiset(&rows, &clean_rows),
                        "{what}: unordered multiset"
                    );
                }
                "complete"
            }
            (Disturbance::Limit(_), Ok(())) => {
                if s.ordered {
                    assert_eq!(
                        rows, ref_rows,
                        "{what}: ordered limit = the first rows of the single-thread output"
                    );
                } else {
                    assert_eq!(rows.len(), ref_rows.len(), "{what}: limit row count");
                    assert!(
                        is_sub_multiset(&rows[1..], &clean_rows[1..]),
                        "{what}: unordered limit rows come from the full output"
                    );
                }
                "limit"
            }
            (Disturbance::SinkFails(_), r) => {
                if reference.result.is_ok() {
                    // fewer batches than the sink allows (an empty result): the sink never failed
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
                        assert!(is_sub_multiset(&rows[1..], &clean_rows[1..]), "{what}");
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
                        assert!(is_sub_multiset(&rows[1..], &clean_rows[1..]), "{what}");
                    }
                    "cancelled"
                }
            },
            (Disturbance::Faults(_), r) => match (&reference.result, r) {
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
                        assert!(is_sub_multiset(&rows[1..], &clean_rows[1..]), "{what}");
                    }
                    if matches!(e.kind, ErrorKind::Internal(_)) {
                        "fault-panic"
                    } else {
                        "fault-error"
                    }
                }
                (Ok(()), Ok(())) => {
                    // no injected fault was reached (pruned away, or behind nothing)
                    if s.ordered {
                        assert_eq!(rows, ref_rows, "{what}");
                    }
                    "fault-not-reached"
                }
                (a, b) => panic!("{what}: serial {a:?} but parallel {b:?}"),
            },
            (d, r) => panic!("{what}: unexpected {d:?} {r:?}"),
        };
        *classes.entry(class).or_insert(0) += 1;
        *CURRENT.lock().expect("lock") = None;
    }
    DONE.store(true, Ordering::SeqCst);
    let mut report: Vec<_> = classes.into_iter().collect();
    report.sort();
    eprintln!("scenarios run: {ran}; outcome classes: {report:?}");
    let need = |k: &str| report.iter().any(|(c, n)| *c == k && *n > 0);
    if only.is_none() && count >= 1000 {
        for k in [
            "complete",
            "limit",
            "sink-failed",
            "cancelled",
            "fault-error",
            "fault-panic",
            "fault-not-reached",
        ] {
            assert!(
                need(k),
                "the scenario mix never produced the class {k}: {report:?}"
            );
        }
    }
}
