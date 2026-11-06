//! Late materialization (register item P1, `ScanOptions::late_materialization`): decode the filter columns first, evaluate the exact filter, decode the
//! output columns only for the rows that pass. The read must give the rows of the default read, in the same order, for every shape of file, filter,
//! projection, thread count and limit, and it must decode less when a filter is selective. (Against DuckDB: the whole differential suite of this crate
//! and of the workspace runs with `NATA_LATE_MATERIALIZATION=1`, see the record `bench/results/20261106-slice4-carried-in.md`.)

mod common;
use common::faults::{Fault, FaultKind, FaultOpener};
use common::*;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::{
    ArrayRef, Date32Array, Decimal128Array, DictionaryArray, Float64Array, Int32Array, Int64Array,
    RecordBatch, StringArray, types::Int32Type,
};
use arrow_schema::{DataType, Field, Schema};
use nata_parquet::{
    ParallelConfig, ScanError, ScanOptions, ScanPlan, ScanReport, csv, run_parallel, run_serial,
};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

/// `rows` rows starting at id `first`: a sorted id, a nullable int (every 5th NULL), a dictionary-friendly text with NULLs, an exact decimal, a date, a
/// double with NaN and -0.0, and a column that is NULL in every row.
fn write_shapes(path: &Path, first: i64, rows: i64, group: usize) {
    let ids = || first..first + rows;
    let id: ArrayRef = Arc::new(Int64Array::from_iter_values(ids()));
    let a: ArrayRef = Arc::new(Int32Array::from_iter(
        ids().map(|i| (i % 5 != 0).then_some((i % 11) as i32)),
    ));
    let s: ArrayRef = Arc::new(StringArray::from_iter(
        ids().map(|i| (i % 9 != 0).then(|| format!("x{}", i % 6))),
    ));
    let d: ArrayRef = Arc::new(
        Decimal128Array::from_iter_values(ids().map(|i| i128::from(i) * 25 % 100_000))
            .with_precision_and_scale(12, 2)
            .expect("decimal"),
    );
    let dt: ArrayRef = Arc::new(Date32Array::from_iter_values(
        ids().map(|i| 18_000 + (i % 400) as i32),
    ));
    let f: ArrayRef = Arc::new(Float64Array::from_iter_values(ids().map(|i| {
        match i % 13 {
            0 => f64::NAN,
            1 => -0.0,
            k => (k as f64) * 0.5,
        }
    })));
    let n: ArrayRef = Arc::new(Int32Array::from(vec![None::<i32>; rows as usize]));
    let batch = RecordBatch::try_from_iter([
        ("id", id),
        ("a", a),
        ("s", s),
        ("d", d),
        ("dt", dt),
        ("f", f),
        ("n", n),
    ])
    .expect("batch");
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(group))
        .build();
    let mut w = ArrowWriter::try_new(
        File::create(path).expect("create"),
        batch.schema(),
        Some(props),
    )
    .expect("writer");
    w.write(&batch).expect("write");
    w.close().expect("close");
}

/// Four files of different sizes (one of 0 rows would have no row group at all: it has its own test), row groups of 37 rows with a short last one.
fn dataset(name: &str) -> (PathBuf, Vec<PathBuf>) {
    let dir = scratch(name);
    let mut files = Vec::new();
    let mut next = 0;
    for (i, rows) in [250i64, 1, 111, 400].into_iter().enumerate() {
        let p = dir.join(format!("s{i}.parquet"));
        write_shapes(&p, next, rows, 37);
        next += rows;
        files.push(p);
    }
    (dir, files)
}

struct Run {
    csv: String,
    report: Result<ScanReport, ScanError>,
    plan: Arc<ScanPlan>,
}

fn opts(columns: Option<&[&str]>, filters: &[&str], prune: bool, late: bool) -> ScanOptions {
    ScanOptions {
        columns: columns.map(|c| c.iter().map(|s| (*s).to_string()).collect()),
        filters: filters.iter().map(|s| (*s).to_string()).collect(),
        prune,
        late_materialization: late,
    }
}

fn run(
    files: &[PathBuf],
    o: &ScanOptions,
    threads: usize,
    limit: Option<u64>,
    opener: Option<Arc<FaultOpener>>,
) -> Run {
    let plan = Arc::new(
        match opener {
            Some(op) => ScanPlan::open_with(files, o, op),
            None => ScanPlan::open(files, o),
        }
        .expect("plan"),
    );
    let mut out: Vec<u8> = Vec::new();
    let names: Vec<String> = plan.out_schema().iter().map(|(n, _)| n.clone()).collect();
    csv::write_header(&mut out, &names).expect("header");
    let report = if threads == 0 {
        run_serial(&plan, limit, |b| csv::write_batch(&mut out, b))
    } else {
        let cfg = ParallelConfig {
            window: threads * 2,
            ..ParallelConfig::new(threads)
        };
        run_parallel(Arc::clone(&plan), cfg, limit, |b| {
            csv::write_batch(&mut out, b)
        })
    };
    Run {
        csv: String::from_utf8(out).expect("utf-8"),
        report,
        plan,
    }
}

const COLUMNS: &[Option<&[&str]>] = &[
    None,
    // the filter column is not selected
    Some(&["s"]),
    Some(&["id", "d"]),
    // the filter column is also selected, and the columns are not in file order
    Some(&["dt", "a", "id"]),
    // a text column that is dictionary encoded in the output
    Some(&["s", "a", "f"]),
];

const FILTERS: &[&[&str]] = &[
    &["id >= 100"],
    &["a IS NULL"],
    &["a IS NOT NULL", "s = 'x1'"],
    &["a = 3", "s = 'x2'", "id < 700"],
    &["f > 1.5"],
    &["f = nan"],
    &["f = 0.0"],
    &["n IS NULL"],
    &["n = 1"],
    &["id >= 0"],
    &["id < 0"],
    &["d >= 10.50", "dt < 2019-07-23"],
    &["s <> 'x3'"],
];

#[test]
fn late_and_default_give_the_same_rows_for_every_shape() {
    let (_d, files) = dataset("late-same");
    let mut cases = 0;
    for columns in COLUMNS {
        for filters in FILTERS {
            for prune in [true, false] {
                let base = run(
                    &files,
                    &opts(*columns, filters, prune, false),
                    0,
                    None,
                    None,
                );
                let want = base.report.as_ref().expect("default scan");
                for (threads, limit) in
                    [(0, None), (1, None), (3, None), (4, Some(7)), (2, Some(0))]
                {
                    let late = run(
                        &files,
                        &opts(*columns, filters, prune, true),
                        threads,
                        limit,
                        None,
                    );
                    let got = late
                        .report
                        .as_ref()
                        .unwrap_or_else(|e| panic!("late {columns:?} {filters:?}: {e}"));
                    let what = format!(
                        "{columns:?} {filters:?} prune {prune} threads {threads} limit {limit:?}"
                    );
                    match limit {
                        None => {
                            assert_eq!(late.csv, base.csv, "rows differ: {what}");
                            // everything but the bytes and the memory counts what the default read counts
                            for (a, b, n) in [
                                (
                                    got.files_opened as u64,
                                    want.files_opened as u64,
                                    "files_opened",
                                ),
                                (got.row_groups_total, want.row_groups_total, "total"),
                                (got.row_groups_skipped, want.row_groups_skipped, "skipped"),
                                (got.row_groups_read, want.row_groups_read, "read"),
                                (got.rows_read, want.rows_read, "rows_read"),
                                (got.rows_returned, want.rows_returned, "rows_returned"),
                            ] {
                                assert_eq!(a, b, "{n}: {what}");
                            }
                        }
                        Some(l) => {
                            // a limit gives the first `l` rows of the default result
                            let all: Vec<&str> = base.csv.lines().collect();
                            let take = usize::try_from(l).expect("limit").min(all.len() - 1);
                            assert_eq!(
                                late.csv.lines().collect::<Vec<_>>(),
                                all[..=take].to_vec(),
                                "limit: {what}"
                            );
                        }
                    }
                    assert_eq!(late.plan.stats().handles.now(), 0, "handles left: {what}");
                    assert_eq!(late.plan.stats().memory.now(), 0, "bytes left: {what}");
                    cases += 1;
                }
            }
        }
    }
    assert!(cases >= 650, "{cases}");
}

#[test]
fn late_materialization_without_a_filter_is_the_default_read() {
    let (_d, files) = dataset("late-nofilter");
    let a = run(
        &files,
        &opts(Some(&["id", "s"]), &[], true, false),
        2,
        None,
        None,
    );
    let b = run(
        &files,
        &opts(Some(&["id", "s"]), &[], true, true),
        2,
        None,
        None,
    );
    assert_eq!(a.csv, b.csv);
    let (ra, rb) = (a.report.expect("a"), b.report.expect("b"));
    assert_eq!(
        ra.bytes_read, rb.bytes_read,
        "no filter: nothing to do late"
    );
}

#[test]
fn a_filter_that_keeps_nothing_decodes_only_the_filter_column() {
    let (_d, files) = dataset("late-nothing");
    // pruning off: every row group is read; `id < 0` keeps no row
    let early = run(
        &files,
        &opts(Some(&["s", "d", "dt", "f"]), &["id < 0"], false, false),
        1,
        None,
        None,
    );
    let late = run(
        &files,
        &opts(Some(&["s", "d", "dt", "f"]), &["id < 0"], false, true),
        1,
        None,
        None,
    );
    assert_eq!(early.csv, late.csv);
    let (e, l) = (early.report.expect("early"), late.report.expect("late"));
    assert_eq!((e.rows_returned, l.rows_returned), (0, 0));
    assert_eq!(e.rows_read, l.rows_read, "both examine every row");
    assert!(
        l.bytes_read * 3 < e.bytes_read,
        "late read {} bytes, the default {}: the output columns must not be read when no row passes",
        l.bytes_read,
        e.bytes_read
    );
    assert!(l.largest_task_bytes < e.largest_task_bytes, "{l:?} {e:?}");
}

#[test]
fn a_selective_filter_holds_no_more_in_memory_per_task() {
    let dir = scratch("late-selective");
    let p = dir.join("big.parquet");
    // 2 row groups of 20,000 rows: an id and a 100-byte text; the filter keeps 1 row in 100
    let n = 40_000i64;
    let id: ArrayRef = Arc::new(Int64Array::from_iter_values(0..n));
    let text: ArrayRef = Arc::new(StringArray::from_iter_values(
        (0..n).map(|i| format!("{i:0>100}")),
    ));
    let batch = RecordBatch::try_from_iter([("id", id), ("t", text)]).expect("batch");
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(20_000))
        .build();
    let mut w = ArrowWriter::try_new(
        File::create(&p).expect("create"),
        batch.schema(),
        Some(props),
    )
    .expect("w");
    w.write(&batch).expect("write");
    w.close().expect("close");
    let files = vec![p];
    // `id % 100` cannot be a filter; use a range that keeps 1 row in 100 of the row group by being narrow: ids 5000..=5199 (200 rows)
    let f = ["id >= 5000", "id < 5200"];
    let early = run(&files, &opts(Some(&["t"]), &f, false, false), 1, None, None);
    let late = run(&files, &opts(Some(&["t"]), &f, false, true), 1, None, None);
    assert_eq!(early.csv, late.csv);
    assert_eq!(early.csv.lines().count(), 201);
    let (e, l) = (early.report.expect("early"), late.report.expect("late"));
    // not smaller by the selectivity: parquet-rs sizes the buffers of a string batch for the batch size, not for the rows selected (a finding, see the
    // record); it must at least not hold more than the default read does
    assert!(
        l.largest_task_bytes <= e.largest_task_bytes,
        "late task held {} bytes, the default {}",
        l.largest_task_bytes,
        e.largest_task_bytes
    );
}

#[test]
fn an_empty_file_and_a_one_row_file_scan_the_same() {
    let dir = scratch("late-edge");
    let empty = dir.join("empty.parquet");
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, true),
        Field::new("s", DataType::Utf8, true),
    ]));
    let w = ArrowWriter::try_new(File::create(&empty).expect("create"), schema, None).expect("w");
    w.close().expect("close");
    let one = dir.join("one.parquet");
    write_shapes(&one, 41, 1, 10);
    for (file, filters) in [
        (&empty, vec!["id > 3"]),
        (&one, vec!["id > 3"]),
        (&one, vec!["id > 99"]),
    ] {
        let cols: Option<&[&str]> = if *file == empty {
            Some(&["s"])
        } else {
            Some(&["s", "id"])
        };
        let files = vec![file.clone()];
        let a = run(&files, &opts(cols, &filters, true, false), 2, None, None);
        let b = run(&files, &opts(cols, &filters, true, true), 2, None, None);
        assert_eq!(a.csv, b.csv, "{file:?} {filters:?}");
        assert_eq!(
            a.report.expect("a").rows_read,
            b.report.expect("b").rows_read
        );
    }
}

#[test]
fn dictionary_strings_stay_dictionaries_in_both_steps() {
    // the output type is part of the contract of the scan (ADR 0003): the late read must hand out the same Arrow types
    let (_d, files) = dataset("late-types");
    for late in [false, true] {
        let plan = Arc::new(
            ScanPlan::open(&files, &opts(Some(&["s", "id"]), &["a = 3"], true, late))
                .expect("plan"),
        );
        let mut types = Vec::new();
        run_parallel(Arc::clone(&plan), ParallelConfig::new(2), None, |b| {
            types.push(b.schema().field(0).data_type().clone());
            Ok(())
        })
        .expect("scan");
        assert!(!types.is_empty());
        let want = DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8));
        assert!(types.iter().all(|t| *t == want), "late {late}: {types:?}");
        let _ = DictionaryArray::<Int32Type>::new(
            Int32Array::from(vec![0]),
            Arc::new(StringArray::from(vec!["x"])),
        );
    }
}

#[test]
fn a_failing_read_in_either_step_is_an_error_naming_the_file_and_the_row_group() {
    let (_d, files) = dataset("late-fault");
    // reads fail after k bytes: k small = inside the filter column's step, k larger = inside the output columns' step
    let o = opts(Some(&["s", "d", "f"]), &["id >= 0"], false, true);
    let mut seen = std::collections::BTreeSet::new();
    for k in [1u64, 100, 300, 600, 1200, 2500] {
        let opener = FaultOpener::new(
            &files,
            vec![Fault {
                file: 0,
                rg: Some(1),
                kind: FaultKind::FailReadsAfter(k),
            }],
        );
        let r = run(&files, &o, 3, None, Some(Arc::clone(&opener)));
        // an `Ok` means the fault was not reached: the row group is smaller than k
        if let Err(e) = &r.report {
            assert_eq!(e.row_group, Some(1), "k={k}: {e}");
            assert_eq!(e.file.as_deref(), Some(files[0].as_path()), "k={k}: {e}");
            seen.insert(k);
        }
        assert_eq!(opener.live_handles(), 0, "k={k}: a handle was left open");
        assert_eq!(r.plan.stats().memory.now(), 0, "k={k}: bytes left");
        assert_eq!(r.plan.stats().handles.now(), 0, "k={k}: handles left");
    }
    assert!(
        seen.len() >= 2,
        "the fault must be reached at several points: {seen:?}"
    );
}

#[test]
fn cancel_at_every_point_leaves_nothing_behind() {
    // a limit stops the scan while tasks are in either step; nothing is left (the scenario test of the crate runs this with random delays)
    let (_d, files) = dataset("late-cancel");
    for limit in [1u64, 5, 40, 300] {
        for threads in [1, 4, 8] {
            let r = run(
                &files,
                &opts(Some(&["s", "d", "id"]), &["a IS NOT NULL"], false, true),
                threads,
                Some(limit),
                None,
            );
            r.report.as_ref().expect("scan");
            assert_eq!(r.csv.lines().count() as u64, limit + 1);
            assert_eq!(r.plan.stats().handles.now(), 0);
            assert_eq!(r.plan.stats().memory.now(), 0);
        }
    }
}

#[test]
fn the_decimal_and_date_columns_survive_the_selection() {
    let (_d, files) = dataset("late-values");
    let f = ["d >= 10.50", "dt < 2019-07-23"];
    let a = run(
        &files,
        &opts(Some(&["id", "d", "dt"]), &f, true, false),
        1,
        None,
        None,
    );
    let b = run(
        &files,
        &opts(Some(&["id", "d", "dt"]), &f, true, true),
        1,
        None,
        None,
    );
    assert!(a.csv.lines().count() > 20, "{}", a.csv.lines().count());
    assert_eq!(a.csv, b.csv);
}
