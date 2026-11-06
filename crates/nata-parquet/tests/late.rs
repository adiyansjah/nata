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
        strict_schema: false,
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

/// `clean` with the footer's row count of row group `rg` raised by `extra`: the pages hold fewer rows than the footer says (a damaged or lying footer).
fn with_inflated_row_count(clean: &[u8], rg: usize, extra: i64) -> Vec<u8> {
    use parquet::file::metadata::{ParquetMetaDataReader, ParquetMetaDataWriter};
    let n = clean.len();
    let footer_len = u32::from_le_bytes(clean[n - 8..n - 4].try_into().expect("4 bytes")) as usize;
    let body_end = n - 8 - footer_len;
    let meta = ParquetMetaDataReader::new()
        .parse_and_finish(&bytes::Bytes::copy_from_slice(clean))
        .expect("footer");
    let groups: Vec<_> = meta
        .row_groups()
        .iter()
        .enumerate()
        .map(|(i, g)| {
            if i == rg {
                g.clone()
                    .into_builder()
                    .set_num_rows(g.num_rows() + extra)
                    .build()
                    .expect("row group")
            } else {
                g.clone()
            }
        })
        .collect();
    let new = meta.into_builder().set_row_groups(groups).build();
    let mut out = clean[..body_end].to_vec();
    ParquetMetaDataWriter::new(&mut out, &new)
        .finish()
        .expect("footer written");
    out
}

#[test]
fn a_row_group_that_holds_fewer_rows_than_its_footer_says_is_an_error_in_both_reads() {
    let dir = scratch("late-footer-rows");
    let clean = dir.join("clean.parquet");
    write_shapes(&clean, 0, 200, 50);
    let lying = dir.join("lying.parquet");
    std::fs::write(
        &lying,
        with_inflated_row_count(&std::fs::read(&clean).expect("read"), 2, 5),
    )
    .expect("write");
    // the footer of the clean file is rewritten faithfully: the same rows
    let files = vec![lying.clone()];
    for late in [false, true] {
        for filters in [vec!["id >= 0"], vec!["id >= 120", "a IS NOT NULL"]] {
            let r = run(
                &files,
                &opts(Some(&["id", "s"]), &filters, false, late),
                2,
                None,
                None,
            );
            let e = r
                .report
                .as_ref()
                .expect_err("the footer lies about row group 2");
            assert_eq!(e.row_group, Some(2), "late {late}: {e}");
            assert!(
                e.to_string().contains("yielded") && e.to_string().contains("footer says"),
                "late {late}: {e}"
            );
        }
    }
}

#[test]
fn a_cancel_is_looked_at_between_the_batches_of_both_steps() {
    // 5 batches per step: the cancellation closure is asked once before the handle is opened and once per batch of step 1 and of step 2. A cancel that
    // arrives at ANY of those points ends the task as cancelled; one that never arrives lets it finish.
    use nata_parquet::TaskResult;
    use std::cell::Cell;
    let dir = scratch("late-cancel-points");
    let p = dir.join("five.parquet");
    common::genfiles::write_ids(&p, 0, 8192 * 5, 8192 * 5);
    let files = vec![p];
    for late in [false, true] {
        // every row passes the filter, so step 2 has no selection and reads all five batches
        let plan = ScanPlan::open(
            &files,
            &opts(Some(&["cat", "x"]), &["id >= 0"], false, late),
        )
        .expect("plan");
        let task = plan
            .open_file(0)
            .expect("footer")
            .tasks()
            .pop()
            .expect("one row group");
        let asked = Cell::new(0usize);
        let done = task
            .run_cancellable(&|| {
                asked.set(asked.get() + 1);
                false
            })
            .expect("runs");
        assert!(matches!(done, TaskResult::Done(_)));
        let total = asked.get();
        assert_eq!(
            total,
            if late { 1 + 5 + 5 } else { 1 + 5 },
            "late {late}: how often the cancel was asked"
        );
        for k in 1..=total {
            let n = Cell::new(0usize);
            let r = task
                .run_cancellable(&|| {
                    n.set(n.get() + 1);
                    n.get() >= k
                })
                .expect("runs");
            assert!(
                matches!(r, TaskResult::Cancelled),
                "late {late}: a cancel at question {k} of {total} must stop the task"
            );
        }
    }
}

#[test]
fn the_filter_columns_being_decoded_in_step_1_are_counted_even_when_nothing_is_kept() {
    // no row passes, so step 2 never runs and the output holds nothing: the only decoded bytes are the batches of step 1, and they must be on the gauge
    let dir = scratch("late-transient");
    let f = dir.join("a.parquet");
    common::genfiles::write_ids(&f, 0, 20_000, 20_000);
    let files = vec![f];
    for threads in [0, 2] {
        let r = run(
            &files,
            &opts(Some(&["cat"]), &["id IS NULL"], false, true),
            threads,
            None,
            None,
        );
        let rep = r.report.expect("scan");
        assert_eq!(rep.rows_returned, 0);
        assert_eq!(rep.rows_read, 20_000);
        assert!(
            rep.largest_task_bytes >= 8192 * 8 && rep.peak_inflight_bytes >= 8192 * 8,
            "a batch of 8,192 ids is at least 64 KiB: {}",
            rep.line()
        );
        assert_eq!(r.plan.stats().memory.now(), 0);
    }
}

/// `clean` with the footer of column `col` of row group `rg` cut to `keep` fifths of its bytes and of its values: the column chunk (five equal data pages, no
/// dictionary, no statistics) yields fewer rows than the others (a damaged footer; parquet-rs reads a chunk by its byte range).
fn with_short_column(clean: &[u8], rg: usize, col: usize, keep: i64) -> Vec<u8> {
    use parquet::file::metadata::{ParquetMetaDataReader, ParquetMetaDataWriter};
    let n = clean.len();
    let footer_len = u32::from_le_bytes(clean[n - 8..n - 4].try_into().expect("4 bytes")) as usize;
    let body_end = n - 8 - footer_len;
    let meta = ParquetMetaDataReader::new()
        .parse_and_finish(&bytes::Bytes::copy_from_slice(clean))
        .expect("footer");
    let groups: Vec<_> = meta
        .row_groups()
        .iter()
        .enumerate()
        .map(|(i, g)| {
            if i != rg {
                return g.clone();
            }
            let columns: Vec<_> = g
                .columns()
                .iter()
                .enumerate()
                .map(|(c, cc)| {
                    if c != col {
                        return cc.clone();
                    }
                    let (bytes, values) = (cc.compressed_size(), cc.num_values());
                    assert_eq!((bytes % 5, values % 5), (0, 0), "five equal pages");
                    cc.clone()
                        .into_builder()
                        .set_num_values(values / 5 * keep)
                        .set_total_compressed_size(bytes / 5 * keep)
                        .set_total_uncompressed_size(bytes / 5 * keep)
                        .build()
                        .expect("column")
                })
                .collect();
            g.clone()
                .into_builder()
                .set_column_metadata(columns)
                .build()
                .expect("row group")
        })
        .collect();
    let new = meta.into_builder().set_row_groups(groups).build();
    let mut out = clean[..body_end].to_vec();
    ParquetMetaDataWriter::new(&mut out, &new)
        .finish()
        .expect("footer written");
    out
}

#[test]
fn an_output_column_shorter_than_the_filter_column_is_an_error_in_the_late_read_never_fewer_rows() {
    // 5,000 rows in one row group, five equal pages of 1,000 values per column. The footer is cut so that `v` has 4 of its 5 pages (4,000 values): the filter column `id` yields 5,000 rows in
    // step 1, the output column 4,000 in step 2, and the rows decoded for the selection are not the rows the filter selected. The check of that equality
    // is what turns it into an error (without it the scan would return 4,000 rows for a filter that selected 5,000).
    let dir = scratch("late-short-column");
    let clean = dir.join("clean.parquet");
    let n = 5_000i64;
    let id: ArrayRef = Arc::new(Int64Array::from_iter_values(0..n));
    let v: ArrayRef = Arc::new(Int64Array::from_iter_values((0..n).map(|i| i * 3)));
    let batch = RecordBatch::try_from_iter([("id", id), ("v", v)]).expect("batch");
    let props = WriterProperties::builder()
        .set_data_page_size_limit(8_000)
        .set_write_batch_size(1_000)
        .set_dictionary_enabled(false)
        .set_statistics_enabled(parquet::file::properties::EnabledStatistics::None)
        .build();
    let mut w = ArrowWriter::try_new(
        File::create(&clean).expect("create"),
        batch.schema(),
        Some(props),
    )
    .expect("writer");
    w.write(&batch).expect("write");
    w.close().expect("close");
    let short = dir.join("short.parquet");
    std::fs::write(
        &short,
        with_short_column(&std::fs::read(&clean).expect("read"), 0, 1, 4),
    )
    .expect("write");
    // the clean file reads the same in both modes, and a filter that keeps every row, or a 90% selection, makes step 2 read the short column
    for filter in ["id >= 0", "id >= 500"] {
        let ok = run(
            std::slice::from_ref(&clean),
            &opts(Some(&["v"]), &[filter], false, true),
            1,
            None,
            None,
        );
        assert!(ok.report.is_ok(), "{filter}");
        let r = run(
            std::slice::from_ref(&short),
            &opts(Some(&["v"]), &[filter], false, true),
            1,
            None,
            None,
        );
        let e = r.report.as_ref().expect_err(&format!(
            "{filter}: the output column is shorter than the filter column"
        ));
        assert_eq!(e.row_group, Some(0), "{e}");
        assert!(
            e.to_string().contains("not the rows the filter selected"),
            "{filter}: {e}"
        );
    }
}
