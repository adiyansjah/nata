//! Tests of the spike's scan: types, morsels, threads and the reference SQL. Run against deliberately broken builds by mutants/run.sh.
use std::fs::File;
use std::sync::Arc;

use arrow::array::{ArrayRef, Date32Array, Decimal128Array, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::{EnabledStatistics, WriterProperties};
use pscan_lib::*;

const COLS: [&str; 7] = ["l_returnflag", "l_linestatus", "l_quantity", "l_extendedprice", "l_discount", "l_tax", "l_shipdate"];

/// A lineitem-shaped file with NULLs, row groups of `rg_rows`, with or without a page index (small pages when it has one).
fn make(dir: &std::path::Path, name: &str, rows: usize, rg_rows: usize, page_index: bool) -> String {
    let path = dir.join(name).to_string_lossy().to_string();
    let flags = ["A", "N", "R"];
    let status = ["F", "O"];
    let s1: Vec<Option<&str>> = (0..rows).map(|i| if i % 11 == 5 { None } else { Some(flags[i % 3]) }).collect();
    let s2: Vec<Option<&str>> = (0..rows).map(|i| Some(status[(i / 3) % 2])).collect();
    let dec = |m: i128, nulls_every: usize| -> ArrayRef {
        let v: Vec<Option<i128>> = (0..rows).map(|i| if nulls_every > 0 && i % nulls_every == 3 { None } else { Some((i as i128 * m) % 1_000_003 - 7) }).collect();
        Arc::new(Decimal128Array::from(v).with_precision_and_scale(15, 2).unwrap())
    };
    let date: Vec<Option<i32>> = (0..rows).map(|i| Some(8000 + (i % 2500) as i32)).collect();
    let fields = vec![
        Field::new("l_returnflag", DataType::Utf8, true),
        Field::new("l_linestatus", DataType::Utf8, true),
        Field::new("l_quantity", DataType::Decimal128(15, 2), true),
        Field::new("l_extendedprice", DataType::Decimal128(15, 2), true),
        Field::new("l_discount", DataType::Decimal128(15, 2), true),
        Field::new("l_tax", DataType::Decimal128(15, 2), true),
        Field::new("l_shipdate", DataType::Date32, true),
    ];
    let schema = Arc::new(Schema::new(fields));
    let batch = RecordBatch::try_new(schema.clone(), vec![Arc::new(StringArray::from(s1)), Arc::new(StringArray::from(s2)), dec(3, 0), dec(17, 0), dec(5, 7), dec(11, 13), Arc::new(Date32Array::from(date))]).unwrap();
    let mut b = WriterProperties::builder().set_max_row_group_row_count(Some(rg_rows));
    if page_index {
        b = b.set_data_page_row_count_limit(97).set_statistics_enabled(EnabledStatistics::Page);
    } else {
        b = b.set_offset_index_disabled(true).set_statistics_enabled(EnabledStatistics::Chunk);
    }
    let mut w = ArrowWriter::try_new(File::create(&path).unwrap(), schema, Some(b.build())).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    path
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("pscan-test-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_test_files_have_what_they_claim() {
    let d = tmp("claims");
    let with = Src::open(&make(&d, "with.parquet", 1000, 300, true), &COLS, Types::Engine);
    let without = Src::open(&make(&d, "without.parquet", 1000, 300, false), &COLS, Types::Engine);
    assert!(with.has_page_index && !without.has_page_index);
    assert_eq!(with.row_group_rows, vec![300, 300, 300, 100]);
    assert_eq!(with.total_rows(), 1000);
}

#[test]
fn plain_and_engine_types_return_the_same_result_with_nulls() {
    let d = tmp("types");
    for pi in [true, false] {
        let p = make(&d, &format!("t{pi}.parquet"), 2345, 500, pi);
        let (plain, engine) = (scan_all(&Src::open(&p, &COLS, Types::Plain)), scan_all(&Src::open(&p, &COLS, Types::Engine)));
        assert_eq!(plain, engine, "page index {pi}");
        assert_eq!(plain.rows, 2345);
        assert!(plain.sums.values().all(|v| *v != 0), "a column was not touched: {plain:?}");
    }
}

#[test]
fn engine_types_are_dictionary_strings_and_decimal64() {
    let d = tmp("dict");
    let p = make(&d, "d.parquet", 600, 300, true);
    let b = Src::open(&p, &COLS, Types::Engine).reader(0, None).next().unwrap().unwrap();
    let t = |n: &str| b.schema().field_with_name(n).unwrap().data_type().clone();
    assert!(matches!(t("l_returnflag"), DataType::Dictionary(_, _)) && matches!(t("l_linestatus"), DataType::Dictionary(_, _)));
    assert_eq!(t("l_quantity"), DataType::Decimal64(15, 2));
    let b = Src::open(&p, &COLS, Types::Plain).reader(0, None).next().unwrap().unwrap();
    assert_eq!(b.schema().field_with_name("l_returnflag").unwrap().data_type(), &DataType::Utf8);
}

#[test]
fn parts_of_every_row_group_cover_every_row_exactly_once() {
    let d = tmp("parts");
    // row groups of 1000, 1000, 1000, 37 rows and odd sizes that do not divide by 4; also tiny groups
    for (rows, rg) in [(3037usize, 1000usize), (1001, 250), (7, 3), (4, 4), (5, 1), (4097, 4096)] {
        let src = Src::open(&make(&d, &format!("p{rows}-{rg}.parquet"), rows, rg, true), &COLS, Types::Engine);
        for parts in 1..=8 {
            let m = by_parts(&src, parts);
            let mut seen = vec![0u32; src.row_group_rows.len()];
            let mut last_end = vec![0usize; src.row_group_rows.len()];
            for x in &m {
                let r = x.rows.clone().unwrap();
                assert_eq!(r.start, last_end[x.rg], "rows {rows} rg {rg} parts {parts}: ranges must be contiguous");
                last_end[x.rg] = r.end;
                seen[x.rg] += (r.end - r.start) as u32;
            }
            for (g, n) in src.row_group_rows.iter().enumerate() {
                assert_eq!(seen[g] as usize, *n, "rows {rows} rg {rg} parts {parts}: row group {g} is covered {} times its size", seen[g] as f64 / *n as f64);
            }
        }
    }
}

#[test]
fn a_scan_by_quarters_equals_a_scan_by_row_groups_with_and_without_a_page_index() {
    let d = tmp("quarters");
    for pi in [true, false] {
        let src = Src::open(&make(&d, &format!("q{pi}.parquet"), 4321, 1000, pi), &COLS, Types::Engine);
        let whole = run(&src, &by_row_group(&src), 1);
        for parts in [2, 3, 4, 7] {
            assert_eq!(run(&src, &by_parts(&src, parts), 1), whole, "page index {pi}, {parts} parts");
        }
    }
}

#[test]
fn the_number_of_threads_does_not_change_the_result() {
    let d = tmp("threads");
    let src = Src::open(&make(&d, "th.parquet", 6000, 1000, true), &COLS, Types::Engine);
    let want = scan_all(&src);
    for threads in 1..=8 {
        assert_eq!(run(&src, &by_row_group(&src), threads), want, "row groups, {threads} threads");
        assert_eq!(run(&src, &by_parts(&src, 4), threads), want, "quarters, {threads} threads");
    }
}

#[test]
fn the_reference_sql_agrees_with_the_scan_on_a_file_with_nulls() {
    // this tests the DuckDB side of the gate: the SQL, the unscaled decimals, the date arithmetic and the first-byte function
    let d = tmp("duck");
    let p = make(&d, "duck.parquet", 3000, 700, false);
    let acc = scan_all(&Src::open(&p, &COLS, Types::Engine));
    let (line, secs) = duck_run(&duck_sql(&p, &COLS)).expect("duckdb must be installed for this test");
    assert_eq!(line, acc_line(&acc));
    assert!(secs >= 0.0);
}

#[test]
fn duckdb_output_is_parsed() {
    let (l, s) = parse_duck("6001215|1|2\nRun Time (s): real 0.123 user 0.100 sys 0.010\n").unwrap();
    assert_eq!((l.as_str(), s), ("6001215|1|2", 0.123));
    assert!(parse_duck("1|2\n").is_err(), "no timer line must be an error");
    assert!(parse_duck("Run Time (s): real 0.5 user 0 sys 0\n").is_err(), "no result line must be an error");
}
