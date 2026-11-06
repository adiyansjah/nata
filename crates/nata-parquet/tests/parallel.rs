//! The parallel scan against the single-thread scan, on generated files (parquet-rs as the writer, no DuckDB): identical ordered output for every thread
//! count and window, the same multiset unordered, an exact `--limit`, the lowest error, lazy footers and bounded handles, and nothing left behind.
//! (The same comparison against DuckDB, at every thread count, is `differential_lineitem`/`differential_edge`/`special_files` run with
//! `NATA_SCAN_THREADS` set: see `run_thread_matrix.sh`.)

mod common;
use common::faults::{Fault, FaultKind, FaultOpener};
use common::genfiles::{write_ids, write_many};
use common::*;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use nata_parquet::{
    ParallelConfig, ScanError, ScanOptions, ScanPlan, ScanReport, csv, run_parallel, run_serial,
};

struct Run {
    csv: String,
    result: Result<ScanReport, ScanError>,
    plan: Arc<ScanPlan>,
}

fn options(columns: Option<&[&str]>, filters: &[&str], prune: bool) -> ScanOptions {
    ScanOptions {
        columns: columns.map(|c| c.iter().map(|s| (*s).to_string()).collect()),
        filters: filters.iter().map(|s| (*s).to_string()).collect(),
        prune,
        late_materialization: late_default(),
        strict_schema: false,
    }
}

/// Run with `cfg` (`None` = the serial reference); the CSV printed so far is returned also when the scan fails.
fn run(
    files: &[PathBuf],
    opts: &ScanOptions,
    cfg: Option<ParallelConfig>,
    limit: Option<u64>,
    opener: Option<Arc<FaultOpener>>,
) -> Run {
    let plan = match opener {
        Some(o) => ScanPlan::open_with(files, opts, o),
        None => ScanPlan::open(files, opts),
    }
    .expect("plan");
    let plan = Arc::new(plan);
    let mut out: Vec<u8> = Vec::new();
    let names: Vec<String> = plan.out_schema().iter().map(|(n, _)| n.clone()).collect();
    csv::write_header(&mut out, &names).expect("header");
    let result = match cfg {
        None => run_serial(&plan, limit, |b| csv::write_batch(&mut out, b)),
        Some(c) => run_parallel(Arc::clone(&plan), c, limit, |b| {
            csv::write_batch(&mut out, b)
        }),
    };
    Run {
        csv: String::from_utf8(out).expect("utf-8"),
        result,
        plan,
    }
}

fn cfg(threads: usize, window: usize, ordered: bool) -> ParallelConfig {
    cfg_ahead(threads, window, ordered, ahead_default())
}

fn cfg_ahead(threads: usize, window: usize, ordered: bool, footers_ahead: usize) -> ParallelConfig {
    ParallelConfig {
        threads,
        window,
        ordered,
        footers_ahead,
    }
}

/// Nothing is left behind: no handle, no footer, no decoded byte counted as alive.
fn assert_clean(plan: &ScanPlan, what: &str) {
    let st = plan.stats();
    assert_eq!(st.handles.now(), 0, "{what}: file handles still open");
    assert_eq!(st.footers.now(), 0, "{what}: footers still held");
    assert_eq!(st.memory.now(), 0, "{what}: decoded bytes still held");
}

fn dataset(name: &str) -> (PathBuf, Vec<PathBuf>) {
    let dir = scratch(name);
    // 9 files of 200..~330 rows in row groups of 41 rows: 5 to 8 row groups per file, some with a short last one
    let files = write_many(&dir, 9, 200, 130, 41);
    (dir, files)
}

#[test]
fn ordered_parallel_output_is_the_serial_output_for_every_thread_count_and_window() {
    let (_d, files) = dataset("par-ordered");
    for (columns, filters) in [
        (None, vec![]),
        (Some(vec!["cat", "id"]), vec!["id >= 700"]),
        (Some(vec!["id"]), vec!["cat = 'cat3'", "id < 1500"]),
        (Some(vec!["x"]), vec!["id IS NULL"]),
    ] {
        for prune in [true, false] {
            let opts = options(columns.as_deref(), &filters, prune);
            let reference = run(&files, &opts, None, None, None);
            let want = reference.result.as_ref().expect("serial");
            for threads in [1, 2, 3, 4, 8] {
                for window in [1, 2, threads, 3 * threads] {
                    let r = run(&files, &opts, Some(cfg(threads, window, true)), None, None);
                    let got = r.result.as_ref().unwrap_or_else(|e| panic!("{e}"));
                    let what = format!(
                        "{columns:?} {filters:?} prune {prune} threads {threads} window {window}"
                    );
                    assert_eq!(r.csv, reference.csv, "{what}");
                    for (a, b, n) in [
                        (got.files_opened, want.files_opened, "files_opened"),
                        (
                            got.row_groups_total as usize,
                            want.row_groups_total as usize,
                            "total",
                        ),
                        (
                            got.row_groups_skipped as usize,
                            want.row_groups_skipped as usize,
                            "skipped",
                        ),
                        (
                            got.row_groups_read as usize,
                            want.row_groups_read as usize,
                            "read",
                        ),
                        (got.rows_read as usize, want.rows_read as usize, "rows_read"),
                        (
                            got.rows_returned as usize,
                            want.rows_returned as usize,
                            "rows_returned",
                        ),
                        (
                            got.bytes_read as usize,
                            want.bytes_read as usize,
                            "bytes_read",
                        ),
                    ] {
                        assert_eq!(a, b, "{what}: report field {n}");
                    }
                    assert_eq!(got.row_groups_abandoned, 0, "{what}");
                    assert!(
                        got.peak_open_files as usize <= threads + 1,
                        "{what}: {}",
                        got.line()
                    );
                    assert!(
                        got.peak_inflight_bytes <= window as u64 * got.largest_task_bytes,
                        "{what}: the memory bound window x largest task: {}",
                        got.line()
                    );
                    assert_clean(&r.plan, &what);
                }
            }
        }
    }
}

#[test]
fn unordered_parallel_output_is_the_same_multiset() {
    let (_d, files) = dataset("par-unordered");
    for filters in [vec![], vec!["id >= 700", "cat <> 'cat1'"]] {
        let opts = options(None, &filters, true);
        let reference = run(&files, &opts, None, None, None);
        for threads in [1, 2, 3, 4, 8] {
            let r = run(
                &files,
                &opts,
                Some(cfg(threads, threads, false)),
                None,
                None,
            );
            assert!(r.result.is_ok());
            assert_eq!(
                diff_unordered(&reference.csv, &r.csv),
                None,
                "threads {threads} {filters:?}"
            );
            assert_clean(&r.plan, "unordered");
        }
    }
}

#[test]
fn limit_returns_exactly_the_first_n_rows_in_ordered_mode_and_leaves_nothing_behind() {
    let (_d, files) = dataset("par-limit");
    let opts = options(Some(&["id", "cat"]), &["id >= 100"], true);
    let all = run(&files, &opts, None, None, None);
    let total = records(&all.csv).len() - 1;
    for limit in [
        0u64,
        1,
        40,
        41,
        42,
        500,
        1234,
        total as u64,
        total as u64 + 10,
    ] {
        let serial = run(&files, &opts, None, Some(limit), None);
        for threads in [1, 2, 4, 8] {
            let r = run(
                &files,
                &opts,
                Some(cfg(threads, threads, true)),
                Some(limit),
                None,
            );
            let rep = r.result.as_ref().expect("scan");
            assert_eq!(r.csv, serial.csv, "limit {limit} threads {threads}");
            assert_eq!(rep.rows_returned as usize, (limit as usize).min(total));
            assert_clean(&r.plan, &format!("limit {limit} threads {threads}"));
            if limit == 0 {
                assert_eq!(
                    (rep.files_opened, rep.bytes_read),
                    (0, 0),
                    "limit 0 opens nothing"
                );
            }
            // read-ahead is bounded by the window: at most `window` row groups beyond the ones the output needed were started
            let needed = serial.result.as_ref().expect("serial").row_groups_read;
            assert!(
                rep.row_groups_read + rep.row_groups_abandoned <= needed + threads as u64,
                "limit {limit} threads {threads}: {}",
                rep.line()
            );
        }
        // unordered: N rows, each of them a row of the full output
        let u = run(&files, &opts, Some(cfg(4, 4, false)), Some(limit), None);
        let rows = records(&u.csv);
        assert_eq!(rows.len() - 1, (limit as usize).min(total));
        let full: std::collections::HashSet<String> = records(&all.csv).into_iter().collect();
        assert!(rows.iter().all(|r| full.contains(r)));
        assert_clean(&u.plan, "unordered limit");
    }
}

#[test]
fn footers_are_read_when_the_scan_reaches_the_file() {
    let (_d, files) = dataset("par-lazy");
    let opts = options(Some(&["id"]), &[], true);
    // a limit inside the first file: with one thread and a window of one, only the first footer is ever read (with footers read ahead, at most `ahead`)
    let r = run(&files, &opts, Some(cfg(1, 1, true)), Some(10), None);
    let rep = r.result.expect("scan");
    assert_eq!(rep.files, 9);
    assert!(
        (1..=ahead_default()).contains(&rep.files_opened),
        "{}",
        rep.line()
    );
    assert!(
        rep.row_groups_total < 12 * ahead_default() as u64,
        "only the opened file's row groups are counted: {}",
        rep.line()
    );
    // a complete scan opens every file once
    let r = run(&files, &opts, Some(cfg(4, 4, true)), None, None);
    let rep = r.result.expect("scan");
    assert_eq!(rep.files_opened, 9);
    // the plan itself read exactly one footer
    let plan = ScanPlan::open(&files, &opts).expect("plan");
    assert_eq!(ScanStats_files_opened(&plan), 0);
    assert_eq!(plan.stats().handles.peak(), 1);
}

#[allow(non_snake_case)]
fn ScanStats_files_opened(plan: &ScanPlan) -> u64 {
    plan.stats()
        .files_opened
        .load(std::sync::atomic::Ordering::SeqCst)
}

#[test]
fn the_error_of_the_lowest_file_and_row_group_is_reported_for_every_thread_count() {
    let (_d, files) = dataset("par-error");
    let opts = options(Some(&["id"]), &[], true);
    // file 5 row group 2 and file 3 row group 4 fail on read, file 7 fails at the footer
    let faults = vec![
        Fault {
            file: 5,
            rg: Some(2),
            kind: FaultKind::FailOpen,
        },
        Fault {
            file: 3,
            rg: Some(4),
            kind: FaultKind::FailReadsAfter(0),
        },
        Fault {
            file: 7,
            rg: None,
            kind: FaultKind::FailOpen,
        },
    ];
    let reference_rows: Vec<String> = {
        // the rows the ordered scan must deliver before the error: all row groups before (file 3, row group 4)
        let ok = run(&files, &opts, None, None, None);
        let per_file_groups: Vec<usize> = files
            .iter()
            .map(|f| {
                let b = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(
                    std::fs::File::open(f).expect("open"),
                )
                .expect("footer");
                b.metadata().num_row_groups()
            })
            .collect();
        let mut rows_before = 0usize;
        for (i, g) in per_file_groups.iter().enumerate().take(3) {
            let _ = (i, g);
        }
        for f in files.iter().take(3) {
            let b = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(
                std::fs::File::open(f).expect("open"),
            )
            .expect("footer");
            rows_before += usize::try_from(b.metadata().file_metadata().num_rows()).expect("rows");
        }
        let b = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(
            std::fs::File::open(&files[3]).expect("open"),
        )
        .expect("footer");
        for g in 0..4 {
            rows_before += usize::try_from(b.metadata().row_group(g).num_rows()).expect("rows");
        }
        records(&ok.csv)[..=rows_before].to_vec()
    };
    for threads in [1, 2, 3, 4, 8] {
        for ordered in [true, false] {
            for _ in 0..4 {
                let opener = FaultOpener::new(&files, faults.clone());
                let r = run(
                    &files,
                    &opts,
                    Some(cfg(threads, threads, ordered)),
                    None,
                    Some(opener.clone()),
                );
                let e = r.result.as_ref().expect_err("must fail");
                let text = e.to_string();
                assert!(
                    text.contains("a00003.parquet") && text.contains("row group 4"),
                    "threads {threads} ordered {ordered}: the lowest failing (file, row group) must be named: {text}"
                );
                if ordered {
                    assert_eq!(
                        records(&r.csv),
                        reference_rows,
                        "threads {threads}: every row before the error and none after"
                    );
                }
                assert_eq!(
                    opener.live_handles(),
                    0,
                    "handles closed: threads {threads}"
                );
                assert_clean(&r.plan, "error");
            }
        }
    }
    // an error in a footer alone
    let opener = FaultOpener::new(
        &files,
        vec![Fault {
            file: 6,
            rg: None,
            kind: FaultKind::FailOpen,
        }],
    );
    let r = run(&files, &opts, Some(cfg(4, 4, true)), None, Some(opener));
    let text = r.result.expect_err("must fail").to_string();
    assert!(
        text.contains("a00006.parquet") && !text.contains("row group"),
        "{text}"
    );
}

#[test]
fn a_closed_consumer_cancels_the_scan_and_the_error_is_the_sinks() {
    let (_d, files) = dataset("par-sink");
    let opts = options(None, &[], true);
    for threads in [1, 4, 8] {
        let plan = Arc::new(ScanPlan::open(&files, &opts).expect("plan"));
        let mut calls = 0;
        let r = run_parallel(Arc::clone(&plan), cfg(threads, threads, true), None, |_| {
            calls += 1;
            if calls == 3 {
                Err(ScanError::new(nata_parquet::ErrorKind::Io(
                    "Broken pipe (os error 32)".into(),
                )))
            } else {
                Ok(())
            }
        });
        let e = r.expect_err("sink error");
        assert!(e.to_string().contains("Broken pipe"), "{e}");
        assert_clean(&plan, "closed consumer");
    }
}

#[test]
fn many_small_files_hold_few_handles_and_footers() {
    let dir = scratch("par-many");
    let files = write_many(&dir, 400, 3, 5, 100);
    let opts = options(Some(&["id"]), &[], true);
    let want = run(&files, &opts, None, None, None);
    for threads in [1, 8] {
        let opener = FaultOpener::new(&files, vec![]);
        let r = run(
            &files,
            &opts,
            Some(cfg(threads, threads, true)),
            None,
            Some(opener.clone()),
        );
        let rep = r.result.expect("scan");
        assert_eq!(r.csv, want.csv);
        assert_eq!(rep.files_opened, 400);
        assert!(
            rep.peak_open_files as usize <= threads + 1,
            "{}",
            rep.line()
        );
        assert!(
            rep.peak_footers as usize <= threads + ahead_default() + 1,
            "footers held: {}",
            rep.line()
        );
        assert_eq!(opener.live_handles(), 0);
        assert_clean(&r.plan, "many files");
    }
    let _ = Path::new("");
}

#[test]
fn a_task_error_never_carries_a_cell_value() {
    let dir = scratch("par-redact");
    let f = dir.join("a.parquet");
    write_ids(&f, 0, 300, 100);
    let files = vec![f];
    let opener = FaultOpener::new(
        &files,
        vec![Fault {
            file: 0,
            rg: Some(1),
            kind: FaultKind::FailReadsAfter(10),
        }],
    );
    let r = run(
        &files,
        &options(None, &[], true),
        Some(cfg(2, 2, true)),
        None,
        Some(opener),
    );
    let text = r.result.expect_err("fails").to_string();
    assert!(
        text.contains("a.parquet") && text.contains("row group 1"),
        "{text}"
    );
    assert!(!text.contains("cat3"), "{text}");
}

#[test]
fn a_limit_stops_a_slow_task_at_the_next_batch_boundary_and_drops_its_handle() {
    // two files of 40 batches each; file 1 is read slowly. The limit is satisfied by file 0 alone; the task of file 1 is in flight and must stop long
    // before it could have finished, close its handle, and be reported as abandoned.
    let dir = scratch("par-slow");
    let files = vec![dir.join("a.parquet"), dir.join("b.parquet")];
    write_ids(&files[0], 0, 8192 * 4, 8192 * 4);
    write_ids(&files[1], 1_000_000, 8192 * 40, 8192 * 40);
    let opts = options(Some(&["id"]), &[], true);
    let slow = |files: &[PathBuf]| {
        FaultOpener::new(
            files,
            vec![Fault {
                file: 1,
                rg: Some(0),
                kind: FaultKind::SlowReadsMs(15),
            }],
        )
    };
    // how long the slow file takes when it is read to the end
    let t = std::time::Instant::now();
    let full = run(
        &files,
        &opts,
        Some(cfg(2, 2, true)),
        None,
        Some(slow(&files)),
    );
    let full_time = t.elapsed();
    assert!(full.result.is_ok());
    let opener = slow(&files);
    let t = std::time::Instant::now();
    let r = run(
        &files,
        &opts,
        Some(cfg(2, 2, true)),
        Some(100),
        Some(opener.clone()),
    );
    let cut_time = t.elapsed();
    let rep = r.result.expect("scan");
    assert_eq!(rep.rows_returned, 100);
    assert_eq!(
        rep.row_groups_abandoned,
        1,
        "the slow task was cancelled: {}",
        rep.line()
    );
    assert!(
        rep.rows_read < 8192 * 4 + 8192 * 40,
        "the slow task did not decode its whole row group: {}",
        rep.line()
    );
    assert!(
        cut_time < full_time / 2,
        "the cancelled scan ({cut_time:?}) must be much faster than the full one ({full_time:?})"
    );
    assert_eq!(opener.live_handles(), 0);
    assert_clean(&r.plan, "slow limit");
}

#[test]
fn the_batch_being_filtered_is_counted_even_when_nothing_is_kept() {
    // no row survives the filter, so the output holds nothing: the only decoded bytes are the batch being filtered, and they must be on the gauge
    let dir = scratch("par-transient");
    let f = dir.join("a.parquet");
    write_ids(&f, 0, 20_000, 20_000);
    let files = vec![f];
    let opts = options(Some(&["id"]), &["id IS NULL"], false);
    for c in [None, Some(cfg(2, 2, true))] {
        let r = run(&files, &opts, c, None, None);
        let rep = r.result.expect("scan");
        assert_eq!(rep.rows_returned, 0);
        assert_eq!(rep.rows_read, 20_000);
        assert!(
            rep.largest_task_bytes >= 8192 * 8 && rep.peak_inflight_bytes >= 8192 * 8,
            "a batch of 8,192 ids is at least 64 KiB: {}",
            rep.line()
        );
        assert_clean(&r.plan, "transient");
    }
}

// ------------------------------------------------------------------------------------------------------------------------------ footers ahead (P2)

#[test]
fn footers_ahead_give_the_serial_scan_for_every_thread_count_window_and_ahead() {
    let (_d, files) = dataset("par-ahead-ordered");
    for (columns, filters) in [
        (None, vec![]),
        (Some(vec!["cat", "id"]), vec!["id >= 700"]),
        (Some(vec!["x"]), vec!["id IS NULL"]),
    ] {
        let opts = options(columns.as_deref(), &filters, true);
        let reference = run(&files, &opts, None, None, None);
        let want = reference.result.as_ref().expect("serial");
        for threads in [1, 2, 4, 8] {
            for window in [1, threads, 3 * threads] {
                for ahead in [2, 3, 16] {
                    let r = run(
                        &files,
                        &opts,
                        Some(cfg_ahead(threads, window, true, ahead)),
                        None,
                        None,
                    );
                    let got = r.result.as_ref().unwrap_or_else(|e| panic!("{e}"));
                    let what = format!(
                        "{columns:?} {filters:?} threads {threads} window {window} ahead {ahead}"
                    );
                    assert_eq!(r.csv, reference.csv, "{what}");
                    assert_eq!(got.rows_returned, want.rows_returned, "{what}");
                    assert_eq!(got.rows_read, want.rows_read, "{what}");
                    assert_eq!(got.row_groups_total, want.row_groups_total, "{what}");
                    assert_eq!(got.row_groups_skipped, want.row_groups_skipped, "{what}");
                    assert_eq!(got.files_opened, want.files_opened, "{what}");
                    assert_clean(&r.plan, &what);
                }
            }
        }
    }
}

#[test]
fn many_small_files_with_footers_ahead_hold_the_slice_3_bounds_plus_the_footers_ahead() {
    let dir = scratch("par-many-ahead");
    let files = write_many(&dir, 400, 3, 5, 100);
    let opts = options(Some(&["id"]), &[], true);
    let want = run(&files, &opts, None, None, None);
    for (threads, ahead) in [(1, 4), (4, 3), (8, 8), (8, 16)] {
        let opener = FaultOpener::new(&files, vec![]);
        let r = run(
            &files,
            &opts,
            Some(cfg_ahead(threads, 2 * threads, true, ahead)),
            None,
            Some(opener.clone()),
        );
        let rep = r.result.expect("scan");
        assert_eq!(r.csv, want.csv, "threads {threads} ahead {ahead}");
        assert_eq!(rep.files_opened, 400);
        // one handle per worker: a worker either runs a row group or reads a footer
        assert!(
            rep.peak_open_files as usize <= threads + 1,
            "handles: {}",
            rep.line()
        );
        // the footers waiting (at most `ahead`) and the footers of the row groups that run (at most one per worker)
        assert!(
            rep.peak_footers as usize <= threads + ahead + 1,
            "footers held: {}",
            rep.line()
        );
        assert_eq!(opener.live_handles(), 0);
        assert_clean(&r.plan, "many files ahead");
    }
}

#[test]
fn footers_ahead_really_read_footers_at_the_same_time() {
    // slow footers (the fault opener sleeps in every footer open): with ahead 1 the reads are one after the other, with ahead 4 they overlap
    let dir = scratch("par-overlap");
    let files = write_many(&dir, 24, 3, 5, 100);
    let faults: Vec<Fault> = (0..24)
        .map(|i| Fault {
            file: i,
            rg: None,
            kind: FaultKind::DelayMs(15),
        })
        .collect();
    let opts = options(Some(&["id"]), &[], true);
    let mut secs = Vec::new();
    for ahead in [1usize, 4] {
        let opener = FaultOpener::new(&files, faults.clone());
        let t = std::time::Instant::now();
        let r = run(
            &files,
            &opts,
            Some(cfg_ahead(4, 8, true, ahead)),
            None,
            Some(opener),
        );
        r.result.as_ref().expect("scan");
        secs.push(t.elapsed().as_secs_f64());
    }
    // 24 footers of 15 ms: about 0.36 s one at a time, about 0.09 s four at a time; the bound is far from both
    assert!(
        secs[1] * 2.0 < secs[0],
        "ahead 4 took {:.3} s, ahead 1 took {:.3} s: the footer reads did not overlap",
        secs[1],
        secs[0]
    );
}

#[test]
fn a_corrupt_later_file_is_the_same_error_with_footers_ahead_and_the_earlier_rows_come_first() {
    let dir = scratch("par-ahead-corrupt");
    let mut files = write_many(&dir, 8, 20, 5, 7);
    std::fs::write(&files[5], b"this is not a parquet file at all").expect("corrupt");
    let _ = &mut files;
    let opts = options(Some(&["id"]), &[], true);
    let reference = run(&files, &opts, Some(cfg_ahead(3, 6, true, 1)), None, None);
    let want_err = reference.result.as_ref().expect_err("fails").to_string();
    assert!(want_err.contains("a00005.parquet"), "{want_err}");
    for threads in [1, 3, 8] {
        for ahead in [2, 4, 16] {
            let r = run(
                &files,
                &opts,
                Some(cfg_ahead(threads, 2 * threads, true, ahead)),
                None,
                None,
            );
            let e = r.result.as_ref().expect_err("fails").to_string();
            assert_eq!(e, want_err, "threads {threads} ahead {ahead}");
            assert_eq!(
                r.csv, reference.csv,
                "the rows before the file come first: threads {threads} ahead {ahead}"
            );
            assert!(!e.contains("a00006") && !e.contains("a00007"), "{e}");
            assert_clean(&r.plan, "corrupt later file");
        }
    }
}

#[test]
fn a_later_file_with_another_schema_is_found_when_the_scan_reaches_it_with_strict_schema() {
    use arrow_array::{ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray};
    // file 3 of 6 gets another schema; every variant must be refused by name, and the rows of files 0 to 2 delivered first
    let variants: [(&str, &str); 4] = [
        ("fewer columns", "1 columns here, 3 in the first file"),
        ("another type", "column x"),
        ("another name", "column y"),
        ("another order", "column x"),
    ];
    for (variant, why) in variants {
        let dir = scratch("par-strict");
        let files = write_many(&dir, 6, 20, 5, 7);
        let ids = |n: i64| -> ArrayRef { Arc::new(Int64Array::from_iter_values(0..n)) };
        let cat = |n: i64| -> ArrayRef {
            Arc::new(StringArray::from_iter_values(
                (0..n).map(|i| format!("cat{i}")),
            ))
        };
        let x = |n: i64| -> ArrayRef {
            Arc::new(Float64Array::from_iter_values((0..n).map(|i| i as f64)))
        };
        let batch = match variant {
            "fewer columns" => RecordBatch::try_from_iter([("id", ids(5))]),
            // x is a string here, a double in the first file
            "another type" => {
                RecordBatch::try_from_iter([("id", ids(5)), ("cat", cat(5)), ("x", cat(5))])
            }
            "another name" => {
                RecordBatch::try_from_iter([("id", ids(5)), ("cat", cat(5)), ("y", x(5))])
            }
            _ => RecordBatch::try_from_iter([("id", ids(5)), ("x", x(5)), ("cat", cat(5))]),
        }
        .expect("batch");
        let mut w = parquet::arrow::ArrowWriter::try_new(
            std::fs::File::create(&files[3]).expect("create"),
            batch.schema(),
            None,
        )
        .expect("writer");
        w.write(&batch).expect("write");
        w.close().expect("close");
        for strict in [true, false] {
            // only `id` is read: a scan that is not strict accepts every variant, which still has `id`
            let mut opts = options(Some(&["id"]), &[], true);
            opts.strict_schema = strict;
            for ahead in [1usize, 4] {
                let r = run(
                    &files,
                    &opts,
                    Some(cfg_ahead(3, 6, true, ahead)),
                    None,
                    None,
                );
                let what = format!("{variant}, strict {strict}, ahead {ahead}");
                if strict {
                    let e = r
                        .result
                        .as_ref()
                        .expect_err("the schema differs")
                        .to_string();
                    assert!(
                        e.contains("a00003.parquet")
                            && e.contains("must have the same schema")
                            && e.contains(why),
                        "{what}: {e}"
                    );
                    assert!(
                        !e.contains("a00004") && !e.contains("a00005"),
                        "{what}: {e}"
                    );
                    // the rows of files 0..3 come first (their ids, and a header)
                    let n: u64 = (0..3).map(|i| 20 + (i * 13) % 5).sum();
                    assert_eq!(r.csv.lines().count() as u64, n + 1, "{what}");
                } else {
                    r.result.as_ref().unwrap_or_else(|e| panic!("{what}: {e}"));
                }
                assert_clean(&r.plan, &what);
            }
        }
    }
}
