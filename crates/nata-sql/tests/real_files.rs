//! The front end against real Parquet files: the schema is read from footers, a glob is expanded on disk, a differing file is named, and the plan's scan
//! options drive the real scan of slice 3 (the pushed conjuncts as its filters) to the rows the query asks for.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::{ArrayRef, Date32Array, Decimal128Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use nata_parquet::{ScanPlan, run_serial};
use nata_sql::{ErrorClass, FsSource, plan_sql};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("nata-sql-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&d).ok();
    std::fs::create_dir_all(&d).expect("scratch dir");
    d
}

/// A small lineitem-like file: ids from `first`, `n` rows, 4 row groups, SECRET in the comments (it must never appear in an error).
fn write(path: &Path, first: i64, n: i64, price_type: DataType) {
    let DataType::Decimal128(precision, _) = price_type else {
        panic!("a decimal")
    };
    let schema = Arc::new(Schema::new(vec![
        Field::new("l_orderkey", DataType::Int64, false),
        Field::new("l_shipdate", DataType::Date32, true),
        Field::new("l_discount", price_type, true),
        Field::new("l_comment", DataType::Utf8, true),
    ]));
    let ids: ArrayRef = Arc::new(Int64Array::from_iter_values(first..first + n));
    let dates: ArrayRef = Arc::new(Date32Array::from_iter_values(
        (first..first + n).map(|i| i32::try_from(8000 + i).expect("day")),
    ));
    let disc: ArrayRef = Arc::new(
        Decimal128Array::from_iter_values((first..first + n).map(|i| i128::from(i % 10)))
            .with_precision_and_scale(precision, 2)
            .expect("decimal"),
    );
    let comments: ArrayRef = Arc::new(StringArray::from_iter_values(
        (first..first + n).map(|i| format!("SECRET-{i}")),
    ));
    let batch =
        RecordBatch::try_new(schema.clone(), vec![ids, dates, disc, comments]).expect("batch");
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(usize::try_from(n / 4).expect("rows")))
        .build();
    let mut w = ArrowWriter::try_new(
        std::fs::File::create(path).expect("create"),
        schema,
        Some(props),
    )
    .expect("writer");
    w.write(&batch).expect("write");
    w.close().expect("close");
}

fn rows_of(plan: &nata_sql::LogicalPlan) -> (u64, nata_parquet::ScanReport) {
    let scan = &plan.scan;
    let opts = scan.scan_options();
    let sp = ScanPlan::open(&scan.files, &opts).expect("the scan accepts the plan's options");
    let mut rows = 0u64;
    let report = run_serial(&sp, None, |b| {
        rows += b.num_rows() as u64;
        Ok(())
    })
    .expect("scan");
    (rows, report)
}

#[test]
fn a_glob_is_expanded_in_file_order_and_the_pushed_filter_drives_the_real_scan() {
    let d = scratch("glob");
    write(
        &d.join("part-2.parquet"),
        200,
        100,
        DataType::Decimal128(15, 2),
    );
    write(
        &d.join("part-1.parquet"),
        0,
        100,
        DataType::Decimal128(15, 2),
    );
    write(
        &d.join("part-3.parquet"),
        100,
        100,
        DataType::Decimal128(15, 2),
    );
    std::fs::write(d.join("notes.txt"), "not parquet").expect("write");
    let pat = format!("{}/part-*.parquet", d.display());
    let p = plan_sql(
        &format!("SELECT l_orderkey, l_discount * 2 AS d2 FROM '{pat}' WHERE l_shipdate >= DATE '1992-02-10' AND l_discount < 0.05 AND l_orderkey % 2 = 0 LIMIT 5"),
        &FsSource,
    )
    .expect("plan");
    let names: Vec<String> = p
        .plan
        .scan
        .files
        .iter()
        .map(|f| f.file_name().expect("name").to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        ["part-1.parquet", "part-2.parquet", "part-3.parquet"],
        "byte order, not directory order"
    );
    assert_eq!(p.plan.scan.pushed.len(), 2);
    assert!(p.plan.filter.is_some(), "the modulo stays in the filter");
    // DATE '1992-02-10' is day 8076: shipdate = 8000 + id, so ids from 76; discounts are 0.00 to 0.09 (id % 10 hundredths): < 0.05 keeps id % 10 in 0..4
    let (rows, report) = rows_of(&p.plan);
    let expected = (76..300i64).filter(|i| i % 10 < 5).count() as u64;
    assert_eq!(
        rows, expected,
        "the scan applies the pushed conjuncts exactly"
    );
    assert!(
        report.row_groups_skipped > 0,
        "the date conjunct prunes whole row groups of part-1: {}",
        report.line()
    );
    // a non-glob path, and the same query through the explain text
    let one = format!("{}/part-1.parquet", d.display());
    let p = plan_sql(
        &format!("EXPLAIN SELECT * FROM '{one}' WHERE l_orderkey = 7"),
        &FsSource,
    )
    .expect("plan");
    assert_eq!(p.plan.scan.files.len(), 1);
    let (rows, _) = rows_of(&p.plan);
    assert_eq!(rows, 1);
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_file_with_another_schema_is_named_and_no_cell_value_appears_in_an_error() {
    let d = scratch("mismatch");
    write(&d.join("a.parquet"), 0, 40, DataType::Decimal128(15, 2));
    write(&d.join("b.parquet"), 40, 40, DataType::Decimal128(15, 2));
    write(&d.join("c.parquet"), 80, 40, DataType::Decimal128(12, 2)); // l_discount differs
    write(&d.join("d.parquet"), 120, 40, DataType::Decimal128(12, 2));
    let e = plan_sql(
        &format!("SELECT l_orderkey FROM '{}/*.parquet'", d.display()),
        &FsSource,
    )
    .expect_err("schemas differ");
    assert_eq!(e.class, ErrorClass::Bind);
    assert!(
        e.message.contains("c.parquet") && !e.message.contains("d.parquet"),
        "the first differing file: {e}"
    );
    assert!(e.message.contains("l_discount"), "{e}");
    assert!(!e.to_string().contains("SECRET"), "{e}");
    // every other error of the binder is also free of cell values
    for q in [
        "SELECT l_commen FROM 'FILE'",
        "SELECT l_comment + 1 FROM 'FILE'",
        "SELECT l_comment FROM 'FILE' WHERE l_comment = 1",
    ] {
        let q = q.replace("FILE", &format!("{}/a.parquet", d.display()));
        let e = plan_sql(&q, &FsSource).expect_err("fails");
        assert!(!e.to_string().contains("SECRET"), "{e}");
    }
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn missing_corrupt_and_empty_sources_are_errors_with_a_position() {
    let d = scratch("bad");
    let junk = d.join("junk.parquet");
    std::fs::write(
        &junk,
        b"this is certainly not a parquet file, but it is long enough",
    )
    .expect("write");
    // a file that is missing or damaged is a failure of the data (class execution, exit code 1 of `nata sql`); no match and an empty path are the query's
    for (q, frag, class) in [
        (
            format!("SELECT a FROM '{}/missing.parquet'", d.display()),
            "missing.parquet",
            ErrorClass::Execution,
        ),
        (
            format!("SELECT a FROM '{}'", junk.display()),
            "junk.parquet",
            ErrorClass::Execution,
        ),
        (
            format!("SELECT a FROM '{}/nothing-*.parquet'", d.display()),
            "no file matches",
            ErrorClass::Bind,
        ),
        ("SELECT a FROM ''".to_string(), "empty", ErrorClass::Bind),
    ] {
        let e = plan_sql(&q, &FsSource).expect_err(&q);
        assert_eq!(e.class, class, "{q}: {e}");
        assert!(e.message.contains(frag), "{q}: {e}");
        assert_eq!((e.pos.line, e.pos.col), (1, 15), "{q}: {e}");
    }
    let e = plan_sql("SELECT a FROM 's3://bucket/x.parquet'", &FsSource).expect_err("remote");
    assert_eq!(e.class, ErrorClass::NotSupported);
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn the_front_end_plans_in_well_under_a_millisecond() {
    // card 0002, quality row "Performance": parsing and binding of the test-set queries, median per query under 1 ms (in-memory schema: the footer
    // read of a file is the scan's cost, not the planner's)
    use nata_sql::source::MemSource;
    let s = MemSource::new().file(
        "lineitem.parquet",
        &[
            (
                "l_orderkey",
                Ok(nata_parquet::schema::Kind::Int {
                    bits: 64,
                    signed: true,
                }),
            ),
            (
                "l_extendedprice",
                Ok(nata_parquet::schema::Kind::Decimal {
                    precision: 15,
                    scale: 2,
                }),
            ),
            (
                "l_discount",
                Ok(nata_parquet::schema::Kind::Decimal {
                    precision: 15,
                    scale: 2,
                }),
            ),
            ("l_shipdate", Ok(nata_parquet::schema::Kind::Date)),
        ],
    );
    let q = "SELECT l_orderkey, l_extendedprice * (1 - l_discount) AS net FROM 'lineitem.parquet' WHERE l_shipdate >= DATE '1994-01-01' AND l_shipdate < DATE '1995-01-01' AND l_discount BETWEEN 0.05 AND 0.07 LIMIT 10";
    let mut times: Vec<std::time::Duration> = (0..200)
        .map(|_| {
            let t = std::time::Instant::now();
            plan_sql(q, &s).expect("plan");
            t.elapsed()
        })
        .collect();
    times.sort();
    let median = times[times.len() / 2];
    eprintln!("plan_sql median over 200 runs (this build): {median:?}");
    assert!(
        median < std::time::Duration::from_millis(1),
        "median {median:?}"
    );
}
