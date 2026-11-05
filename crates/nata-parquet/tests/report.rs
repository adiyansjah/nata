//! The scan REPORT and the order and bounds of the reader, on files written by parquet-rs in the test (no DuckDB needed): files, row groups total, skipped
//! and read, bytes read, the order of the file list, `--limit`.

mod common;
use common::*;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::{ArrayRef, Int64Array, RecordBatch, StringArray};
use nata_parquet::{ScanOptions, ScanPlan, run_serial};
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::file::properties::WriterProperties;

/// `rows` rows with ids `first..first+rows` and a text column, in row groups of `group` rows.
fn write_ids(path: &Path, first: i64, rows: i64, group: usize) {
    let ids: ArrayRef = Arc::new(Int64Array::from_iter_values(first..first + rows));
    let txt: ArrayRef = Arc::new(StringArray::from_iter_values(
        (first..first + rows).map(|i| format!("v{i:06}")),
    ));
    let batch = RecordBatch::try_from_iter([("id", ids), ("txt", txt)]).expect("batch");
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

/// The sum of the byte lengths of the column chunks of `columns` in the row groups `groups`, from the footer: an independent count of what a scan reads.
fn chunk_bytes(path: &Path, columns: &[&str], groups: &[usize]) -> u64 {
    let b =
        ParquetRecordBatchReaderBuilder::try_new(File::open(path).expect("open")).expect("builder");
    let md = b.metadata();
    let mut total = 0;
    for &g in groups {
        for c in md.row_group(g).columns() {
            if columns.contains(&c.column_path().string().as_str()) {
                total += c.byte_range().1;
            }
        }
    }
    total
}

#[test]
fn the_report_counts_files_row_groups_rows_and_bytes() {
    let dir = scratch("report");
    let f = dir.join("a.parquet");
    write_ids(&f, 0, 5000, 1000); // 5 row groups of 1000 ids: [0,1000) [1000,2000) ...
    let s = scan(&[&f], Some(&["txt"]), &["id >= 3500"], true, None).expect("scan");
    let r = &s.report;
    assert_eq!(
        (
            r.files,
            r.row_groups_total,
            r.row_groups_skipped,
            r.row_groups_read
        ),
        (1, 5, 3, 2),
        "{}",
        r.line()
    );
    assert_eq!((r.rows_read, r.rows_returned), (2000, 1500));
    assert_eq!(r.columns_read, 2, "txt (projected) and id (filter only)");
    // bytes: exactly the column chunks of the two row groups read, both columns (the filter column is read too)
    assert_eq!(
        r.bytes_read,
        chunk_bytes(&f, &["id", "txt"], &[3, 4]),
        "{}",
        r.line()
    );
    // without pruning every row group is read
    let u = scan(&[&f], Some(&["txt"]), &["id >= 3500"], false, None)
        .expect("scan")
        .report;
    assert_eq!(
        (u.row_groups_total, u.row_groups_skipped, u.row_groups_read),
        (5, 0, 5)
    );
    assert_eq!(
        u.bytes_read,
        chunk_bytes(&f, &["id", "txt"], &[0, 1, 2, 3, 4])
    );
    assert_eq!(
        u.rows_returned, r.rows_returned,
        "same rows with and without pruning"
    );
    // only the projected column: fewer bytes
    let p = scan(&[&f], Some(&["id"]), &[], true, None)
        .expect("scan")
        .report;
    assert_eq!(p.bytes_read, chunk_bytes(&f, &["id"], &[0, 1, 2, 3, 4]));
    assert!(p.bytes_read < u.bytes_read);
    assert_eq!(
        p.line()
            .split_whitespace()
            .filter(|w| w.starts_with("morsel=row-group"))
            .count(),
        1,
        "the morsel size is in the report"
    );
    assert!(p.line().contains("pruning=none(no filter)"));
    assert!(r.line().contains("pruning=applied(1 of 1 terms)"));
    assert!(u.line().contains("pruning=off(--no-prune)"), "{}", u.line());
    assert!(
        r.line().contains("skipped=3 read=2 not_read=0"),
        "{}",
        r.line()
    );
}

#[test]
fn the_file_list_is_scanned_in_the_order_given() {
    let dir = scratch("order");
    let (a, b, c) = (
        dir.join("a.parquet"),
        dir.join("b.parquet"),
        dir.join("c.parquet"),
    );
    write_ids(&a, 0, 3, 2);
    write_ids(&b, 100, 3, 2);
    write_ids(&c, 200, 3, 2);
    let rows = |files: &[&Path]| -> Vec<String> {
        let s = scan(files, Some(&["id"]), &[], true, None).expect("scan");
        records(&s.csv)[1..].to_vec()
    };
    assert_eq!(
        rows(&[&a, &b, &c]),
        ["0", "1", "2", "100", "101", "102", "200", "201", "202"]
    );
    assert_eq!(
        rows(&[&c, &a, &b]),
        ["200", "201", "202", "0", "1", "2", "100", "101", "102"]
    );
    assert_eq!(
        rows(&[&b, &b]),
        ["100", "101", "102", "100", "101", "102"],
        "the same file twice is scanned twice"
    );
    // the row groups of one file come in file order, also when some are skipped
    let s = scan(&[&c, &a], Some(&["id"]), &["id <> 1"], true, None).expect("scan");
    assert_eq!(records(&s.csv)[1..], ["200", "201", "202", "0", "2"]);
    assert_eq!(s.report.files, 2);
    assert_eq!(s.report.row_groups_total, 4);
}

#[test]
fn limit_stops_early_and_reports_what_was_not_read() {
    let dir = scratch("limit");
    let f = dir.join("a.parquet");
    write_ids(&f, 0, 5000, 1000);
    let s = scan(&[&f], Some(&["id"]), &[], true, Some(10)).expect("scan");
    assert_eq!(records(&s.csv).len(), 11, "header and 10 rows");
    assert_eq!(records(&s.csv)[1], "0");
    assert_eq!(records(&s.csv)[10], "9");
    let r = &s.report;
    assert_eq!(
        (r.row_groups_total, r.row_groups_read, r.rows_returned),
        (5, 1, 10),
        "{}",
        r.line()
    );
    assert!(r.line().contains("not_read=4"), "{}", r.line());
    // a limit across a row-group boundary and across files
    let g = dir.join("b.parquet");
    write_ids(&g, 10_000, 5, 5);
    let s = scan(&[&f, &g], Some(&["id"]), &[], true, Some(5003)).expect("scan");
    let rows = records(&s.csv);
    assert_eq!(rows.len(), 5004);
    assert_eq!(rows[5000], "4999");
    assert_eq!(rows[5001], "10000");
    assert_eq!(s.report.rows_returned, 5003);
    // limit with a filter: the first rows that MATCH
    let s = scan(&[&f], Some(&["id"]), &["id >= 2500"], true, Some(3)).expect("scan");
    assert_eq!(records(&s.csv)[1..], ["2500", "2501", "2502"]);
    assert_eq!(s.report.row_groups_skipped, 2);
    assert_eq!(s.report.row_groups_read, 1);
    // limit 0: nothing is read at all
    let s = scan(&[&f], Some(&["id"]), &[], true, Some(0)).expect("scan");
    assert_eq!(records(&s.csv), ["id"]);
    assert_eq!((s.report.row_groups_read, s.report.bytes_read), (0, 0));
    // a limit above the row count returns everything
    let s = scan(&[&f], Some(&["id"]), &[], true, Some(1_000_000)).expect("scan");
    assert_eq!(records(&s.csv).len(), 5001);
}

#[test]
fn tasks_are_independent_row_group_morsels() {
    // the plan hands out one task per kept row group, and a task can be run alone, in any order, from another thread
    let dir = scratch("tasks");
    let f = dir.join("a.parquet");
    write_ids(&f, 0, 5000, 1000);
    let options = ScanOptions {
        columns: Some(vec!["id".into()]),
        filters: vec!["id < 2500".into()],
        prune: true,
    };
    let plan = ScanPlan::open(&[PathBuf::from(&f)], &options).expect("plan");
    let tasks: Vec<_> = plan.tasks().collect();
    assert_eq!(
        tasks.iter().map(|t| t.row_group()).collect::<Vec<_>>(),
        [0, 1, 2]
    );
    let handles: Vec<_> = tasks
        .into_iter()
        .rev()
        .map(|t| std::thread::spawn(move || (t.row_group(), t.run().expect("run"))))
        .collect();
    let mut by_group: Vec<_> = handles
        .into_iter()
        .map(|h| h.join().expect("thread"))
        .collect();
    by_group.sort_by_key(|(g, _)| *g);
    assert_eq!(
        by_group
            .iter()
            .map(|(_, o)| o.rows_returned)
            .collect::<Vec<_>>(),
        [1000, 1000, 500]
    );
    let serial = run_serial(&plan, None, |_| Ok(())).expect("serial");
    assert_eq!(serial.rows_returned, 2500);
    assert_eq!(
        serial.bytes_read,
        by_group.iter().map(|(_, o)| o.bytes_read).sum::<u64>()
    );
}

#[test]
fn schema_differences_between_files_name_the_file() {
    let dir = scratch("schema");
    let (a, b) = (dir.join("a.parquet"), dir.join("b.parquet"));
    write_ids(&a, 0, 3, 3);
    // b has `id` as text
    let ids: ArrayRef = Arc::new(StringArray::from(vec!["x", "y"]));
    let batch = RecordBatch::try_from_iter([("id", ids)]).expect("batch");
    let mut w = ArrowWriter::try_new(File::create(&b).expect("create"), batch.schema(), None)
        .expect("writer");
    w.write(&batch).expect("write");
    w.close().expect("close");
    let e = scan(&[&a, &b], Some(&["id"]), &[], true, None).expect_err("must fail");
    let text = e.to_string();
    assert!(
        text.contains("b.parquet")
            && text.contains("id")
            && text.contains("VARCHAR")
            && text.contains("BIGINT"),
        "{text}"
    );
    // b lacks `txt`
    let e = scan(&[&a, &b], Some(&["txt"]), &[], true, None).expect_err("must fail");
    assert!(
        e.to_string().contains("b.parquet") && e.to_string().contains("txt"),
        "{e}"
    );
    // and the first file does not have a column that is asked for
    let e = scan(&[&a], Some(&["nosuch"]), &[], true, None).expect_err("must fail");
    assert!(
        e.to_string().contains("a.parquet") && e.to_string().contains("no column named"),
        "{e}"
    );
}

#[test]
fn usage_errors() {
    let dir = scratch("usage");
    let a = dir.join("a.parquet");
    write_ids(&a, 0, 3, 3);
    for (cols, filters, want) in [
        (Some(vec!["id", "id"]), vec![], "listed twice"),
        (Some(vec![]), vec![], "at least one column"),
        (Some(vec!["id"]), vec!["nosuch = 1"], "no column named"),
        (Some(vec!["id"]), vec!["id = 1.5"], "never rounded"),
        (Some(vec!["id"]), vec!["id ~ 1"], "expected one of"),
        (Some(vec!["id"]), vec!["id >= abc"], "write an integer"),
        (Some(vec!["id"]), vec!["txt = plain"], "single quotes"),
        (Some(vec!["id"]), vec!["ID = 1"], "no column named"),
    ] {
        let c: Option<Vec<&str>> = cols;
        let e = scan(&[&a], c.as_deref(), &filters, true, None).expect_err(want);
        assert!(e.to_string().contains(want), "wanted {want:?} in {e}");
        if !filters.is_empty() {
            assert!(
                e.to_string().contains(filters[0]) || e.to_string().contains("no column named"),
                "the filter text is in the message: {e}"
            );
        }
    }
    let e = ScanPlan::open(&[], &ScanOptions::default()).expect_err("no files");
    assert!(e.to_string().contains("no file"));
}
