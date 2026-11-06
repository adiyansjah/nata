//! The executor on real Parquet files: the rules of `docs/dialect.md` sections 4 to 7 and `docs/decimal-rules.md` as running code, with the expected
//! rows computed in the test from the formulas that wrote the file (`common::standard`). The comparison with DuckDB is `nata-check` and the fuzzer.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "test data: small loop counters become small column values"
)]

mod common;
use common::*;

use nata_exec::ExecConfig;

const N: i32 = 20;

fn a(i: i32) -> Option<i32> {
    (i % 7 != 3).then_some(i % 5 - 2)
}

fn f(i: i32) -> Option<bool> {
    (i % 3 != 2).then_some(i % 2 == 0)
}

fn fixture(name: &str) -> String {
    let d = dir(name);
    standard(&d.join("t.parquet"));
    d.join("t.parquet").to_str().expect("path").to_string()
}

fn ids(v: impl Iterator<Item = i32>) -> Vec<String> {
    v.map(|i| i.to_string()).collect()
}

#[test]
fn rows_come_in_file_order_for_any_thread_count_and_window() {
    let p = fixture("order");
    let q = format!("SELECT id FROM '{p}' WHERE id >= 3 LIMIT 5 OFFSET 2");
    for threads in [1, 2, 3, 8] {
        for window in [1, 2, 7] {
            let cfg = ExecConfig {
                threads,
                window,
                ordered: true,
                opener: None,
                late_materialization: false,
            };
            let out = run_with(&q, &cfg).csv.expect("runs");
            assert_eq!(
                out,
                format!("id\n{}\n", ids(5..10).join("\n")),
                "threads {threads} window {window}"
            );
        }
    }
}

#[test]
fn three_valued_logic_keeps_a_row_only_when_the_condition_is_true() {
    let p = fixture("3vl");
    let expect = |keep: &dyn Fn(i32) -> bool| ids((0..N).filter(|i| keep(*i)));
    assert_eq!(
        rows(&format!("SELECT id FROM '{p}' WHERE a > 0")),
        expect(&|i| a(i).is_some_and(|x| x > 0))
    );
    assert_eq!(
        rows(&format!("SELECT id FROM '{p}' WHERE NOT (a > 0)")),
        expect(&|i| a(i).is_some_and(|x| x <= 0))
    );
    assert_eq!(
        rows(&format!("SELECT id FROM '{p}' WHERE a > 0 OR f")),
        expect(&|i| a(i).is_some_and(|x| x > 0) || f(i) == Some(true))
    );
    assert_eq!(
        rows(&format!("SELECT id FROM '{p}' WHERE NOT (a > 0 AND f)")),
        expect(&|i| {
            let (l, r) = (a(i).map(|x| x > 0), f(i));
            // NOT (l AND r): TRUE only when l AND r is FALSE
            l == Some(false) || r == Some(false)
        })
    );
    assert_eq!(
        rows(&format!("SELECT id FROM '{p}' WHERE a IS NULL")),
        expect(&|i| a(i).is_none())
    );
    assert_eq!(
        rows(&format!(
            "SELECT id FROM '{p}' WHERE a IS NOT NULL AND f IS NULL"
        )),
        expect(&|i| a(i).is_some() && f(i).is_none())
    );
    // x = NULL is NULL for every row
    assert!(rows(&format!("SELECT id FROM '{p}' WHERE a = NULL")).is_empty());
    assert!(rows(&format!("SELECT id FROM '{p}' WHERE NOT (a = NULL)")).is_empty());
    // the values themselves, in a projection: NULL AND FALSE is FALSE, NULL OR TRUE is TRUE
    assert_eq!(
        rows(&format!(
            "SELECT (a > 100) AND f AS x FROM '{p}' WHERE id < 3"
        )),
        vec!["false", "false", "false"]
    );
    assert_eq!(
        rows(&format!(
            "SELECT (a > 100) OR f AS x FROM '{p}' WHERE id IN (0, 2)"
        )),
        vec!["true", "NULL"]
    );
}

#[test]
fn between_and_in_use_three_valued_logic() {
    let p = fixture("between");
    assert_eq!(
        rows(&format!("SELECT id FROM '{p}' WHERE a BETWEEN -1 AND 1")),
        ids((0..N).filter(|i| a(*i).is_some_and(|x| (-1..=1).contains(&x))))
    );
    assert_eq!(
        rows(&format!(
            "SELECT id FROM '{p}' WHERE a NOT BETWEEN -1 AND 1"
        )),
        ids((0..N).filter(|i| a(*i).is_some_and(|x| !(-1..=1).contains(&x))))
    );
    assert_eq!(
        rows(&format!(
            "SELECT 1 BETWEEN 3 AND 1 AS x FROM '{p}' WHERE id = 0"
        )),
        vec!["false"]
    );
    assert_eq!(
        rows(&format!(
            "SELECT a IN (1, NULL) AS x FROM '{p}' WHERE id IN (3, 7, 8)"
        )),
        vec!["NULL", "NULL", "true"]
    );
    assert_eq!(
        rows(&format!(
            "SELECT a NOT IN (1, NULL) AS x FROM '{p}' WHERE id IN (3, 7, 8)"
        )),
        vec!["NULL", "NULL", "false"]
    );
    assert_eq!(
        rows(&format!(
            "SELECT a NOT IN (1, 2) AS x FROM '{p}' WHERE id IN (3, 7, 8)"
        )),
        vec!["NULL", "true", "false"]
    );
}

#[test]
fn errors_are_raised_only_for_selected_valid_rows() {
    let p = fixture("errors");
    // a is 0 for ids 2, 7, 12 and NULL for 3, 10, 17
    let e = err(&format!("SELECT 10 // a AS q FROM '{p}'"));
    assert!(
        e.contains("division by zero") && e.contains("line 1, column 8"),
        "{e}"
    );
    assert!(
        e.contains(&p) && e.contains("row group"),
        "the error names the file and the row group: {e}"
    );
    assert_eq!(
        rows(&format!("SELECT 10 // a AS q FROM '{p}' WHERE a <> 0")).len(),
        (0..N).filter(|i| a(*i).is_some_and(|x| x != 0)).count()
    );
    // AND guards the division: the second operand runs for the rows the first left TRUE
    assert_eq!(
        rows(&format!(
            "SELECT id FROM '{p}' WHERE a <> 0 AND 10 // a > 1"
        )),
        ids((0..N).filter(|i| a(*i).is_some_and(|x| x != 0 && 10 / x > 1)))
    );
    // `a <> 0` alone would be pushed into the scan: written as an expression it stays in the executor's filter, where the AND narrows the rows
    assert_eq!(
        rows(&format!(
            "SELECT id FROM '{p}' WHERE a + 0 <> 0 AND 10 // a > 1"
        )),
        ids((0..N).filter(|i| a(*i).is_some_and(|x| x != 0 && 10 / x > 1)))
    );
    // so does OR (a NULL first operand leaves the row undecided: the division runs, and a NULL a gives NULL, not an error)
    assert_eq!(
        rows(&format!(
            "SELECT id FROM '{p}' WHERE a = 0 OR 10 // a > 100"
        )),
        vec!["2", "7", "12"]
    );
    // a NULL row is not an error: 10 // NULL is NULL
    assert_eq!(
        rows(&format!("SELECT 10 // a AS q FROM '{p}' WHERE id = 3")),
        vec!["NULL"]
    );
    // a row before the error in the same query still means an error for the whole query
    assert!(
        run(&format!(
            "SELECT 10 // (a - a) AS q FROM '{p}' WHERE id IN (1, 2)"
        ))
        .is_err()
    );
}

#[test]
fn case_evaluates_only_the_branch_a_row_takes() {
    let p = fixture("case");
    let q = format!(
        "SELECT CASE WHEN a = 0 THEN 0 ELSE 100 // a END AS q FROM '{p}' WHERE a IS NOT NULL"
    );
    assert_eq!(rows(&q).len(), 17);
    // the same with the failing expression in the THEN branch: it runs only for the rows whose condition is TRUE
    assert_eq!(
        rows(&format!(
            "SELECT CASE WHEN a <> 0 THEN 100 // a ELSE 0 END AS q FROM '{p}' WHERE a IS NOT NULL"
        ))
        .len(),
        17
    );
    assert_eq!(
        rows(&format!(
            "SELECT CASE a WHEN 0 THEN 0 WHEN 1 THEN 100 // (a - 1 + 1) ELSE 100 // (a - a + 1) END AS q FROM '{p}' WHERE id IN (2, 8, 4)"
        )),
        vec!["0", "100", "100"]
    );
    // the ELSE branch runs for the rows no WHEN took, also when it would fail for the others
    assert!(
        run(&format!(
            "SELECT CASE WHEN id = 1000 THEN 0 ELSE 100 // a END AS q FROM '{p}'"
        ))
        .is_err()
    );
    // a NULL condition is not TRUE: the row falls through
    assert_eq!(
        rows(&format!(
            "SELECT CASE WHEN a > 0 THEN 'pos' WHEN a <= 0 THEN 'nonpos' ELSE 'null' END AS x FROM '{p}' WHERE id IN (0, 1, 3, 4)"
        )),
        vec!["nonpos", "nonpos", "null", "pos"]
    );
    // the simple form compares with =: a NULL operand matches nothing, not even WHEN NULL
    assert_eq!(
        rows(&format!(
            "SELECT CASE a WHEN 0 THEN 'zero' WHEN NULL THEN 'n' ELSE 'e' END AS x FROM '{p}' WHERE id IN (2, 3, 4)"
        )),
        vec!["zero", "e", "e"]
    );
    // results of different widths: one type
    assert_eq!(
        rows(&format!(
            "SELECT CASE WHEN id < 1 THEN ti ELSE b END AS x FROM '{p}' WHERE id IN (0, 2)"
        )),
        vec!["0", "2000000000000"]
    );
}

#[test]
fn integer_arithmetic_is_checked_against_the_type() {
    let p = fixture("ints");
    assert_eq!(
        rows(&format!(
            "SELECT a * 2 + 1 AS x FROM '{p}' WHERE id IN (0, 1, 3)"
        )),
        vec!["-3", "-1", "NULL"]
    );
    assert_eq!(
        rows(&format!(
            "SELECT -7 % 3 AS x, -7 // 2 AS y, 7 % -3 AS z FROM '{p}' WHERE id = 0"
        )),
        vec!["-1,-3,1"]
    );
    // an INTEGER overflows at 32 bits, a BIGINT at 64
    let e = err(&format!(
        "SELECT a + 2147483647 AS x FROM '{p}' WHERE id = 4"
    ));
    assert!(
        e.contains("integer overflow") && e.contains("INTEGER") && e.contains("line 1, column 8"),
        "{e}"
    );
    assert_eq!(
        rows(&format!(
            "SELECT b + 9000000000000000000 AS x FROM '{p}' WHERE id = 0"
        )),
        vec!["9000000000000000000"]
    );
    assert!(
        err(&format!(
            "SELECT b + 9223372036854775807 AS x FROM '{p}' WHERE id = 10"
        ))
        .contains("integer overflow")
    );
    // TINYINT arithmetic with an INTEGER literal is INTEGER here (dialect section 9)
    assert_eq!(
        rows(&format!("SELECT ti + 1 AS x FROM '{p}' WHERE id = 19")),
        vec!["20"]
    );
    assert!(
        err(&format!("SELECT -(-2147483647 - 1) AS x FROM '{p}'")).contains("integer overflow")
    );
}

#[test]
fn decimals_are_exact_and_follow_the_result_types_of_the_rules_page() {
    let p = fixture("decimals");
    // price is DECIMAL(10,2): id * 1.25
    assert_eq!(
        rows(&format!("SELECT price * 3 AS v FROM '{p}' WHERE id < 3")),
        vec!["0.00", "3.75", "7.50"]
    );
    assert_eq!(
        rows(&format!(
            "SELECT price + price AS v FROM '{p}' WHERE id IN (1, 19)"
        )),
        vec!["2.50", "47.50"]
    );
    assert_eq!(
        rows(&format!(
            "SELECT price - 1 AS v FROM '{p}' WHERE id IN (0, 1)"
        )),
        vec!["-1.00", "0.25"]
    );
    assert_eq!(
        rows(&format!(
            "SELECT price * price AS v FROM '{p}' WHERE id = 3"
        )),
        vec!["14.0625"]
    );
    // division: scale s1 + 6, rounded half away from zero
    assert_eq!(
        rows(&format!(
            "SELECT price / 3 AS v FROM '{p}' WHERE id IN (1, 2)"
        )),
        vec!["0.41666667", "0.83333333"]
    );
    assert_eq!(
        rows(&format!(
            "SELECT 7 / 2 AS v, -7 / 2 AS w FROM '{p}' WHERE id = 0"
        )),
        vec!["3.500000,-3.500000"]
    );
    assert_eq!(
        rows(&format!(
            "SELECT 1 / 3 AS v, 2 / 3 AS w, -2 / 3 AS x FROM '{p}' WHERE id = 0"
        )),
        vec!["0.333333,0.666667,-0.666667"]
    );
    // a remainder takes the sign of the dividend
    assert_eq!(
        rows(&format!("SELECT -price % 2 AS v FROM '{p}' WHERE id = 3")),
        vec!["-1.75"]
    );
    // comparisons are exact across scales, widths and integers
    assert_eq!(
        rows(&format!(
            "SELECT id FROM '{p}' WHERE price = 2.5 OR price = 5"
        )),
        vec!["2", "4"]
    );
    assert_eq!(
        rows(&format!(
            "SELECT id FROM '{p}' WHERE price > 23.749999999999999999"
        )),
        vec!["19"]
    );
    // overflow of the result type
    let e = err(&format!(
        "SELECT price * price * price * price * price * price AS v FROM '{p}' WHERE id = 19"
    ));
    assert!(
        e.contains("decimal overflow") && e.contains("DECIMAL("),
        "{e}"
    );
    // rounding of casts
    assert_eq!(
        rows(&format!(
            "SELECT CAST(price AS DECIMAL(10,1)) AS v, CAST(price AS INTEGER) AS w FROM '{p}' WHERE id IN (1, 2, 3)"
        )),
        vec!["1.3,1", "2.5,3", "3.8,4"]
    );
    assert_eq!(
        rows(&format!(
            "SELECT CAST(-price AS INTEGER) AS w FROM '{p}' WHERE id = 2"
        )),
        vec!["-3"]
    );
    // DECIMAL(p,p) prints without the leading zero, as DuckDB
    assert_eq!(
        rows(&format!(
            "SELECT CAST(price AS DECIMAL(3,3)) AS v FROM '{p}' WHERE id = 0"
        )),
        vec![".000"]
    );
    assert_eq!(
        rows(&format!(
            "SELECT CAST(price AS VARCHAR) AS v FROM '{p}' WHERE id = 3"
        )),
        vec!["3.75"]
    );
}

#[test]
fn floats_compare_with_nan_above_everything_and_negative_zero_equal_to_zero() {
    let p = fixture("floats");
    let q = |w: &str| rows(&format!("SELECT id FROM '{p}' WHERE {w}"));
    assert_eq!(q("d = CAST('NaN' AS DOUBLE)"), vec!["7"]);
    assert_eq!(q("d >= CAST('Infinity' AS DOUBLE)"), vec!["7", "10"]);
    assert_eq!(q("d > CAST('Infinity' AS DOUBLE)"), vec!["7"]);
    assert_eq!(q("d = CAST(0 AS DOUBLE)"), vec!["0", "8", "9"]);
    assert_eq!(q("d < CAST(0 AS DOUBLE)"), Vec::<String>::new());
    assert_eq!(
        q("d <> CAST('NaN' AS DOUBLE) AND d <= CAST(1 AS DOUBLE)"),
        vec!["0", "1", "2", "8", "9"]
    );
}

#[test]
fn strings_dates_and_booleans() {
    let p = fixture("text");
    assert_eq!(
        rows(&format!(
            "SELECT id FROM '{p}' WHERE s = 'alpha' AND id < 9"
        )),
        vec!["0", "4", "8"]
    );
    // a comparison that stays in the executor (the CASE keeps it from being pushed into the scan): bytewise, not case-insensitive
    assert_eq!(
        rows(&format!(
            "SELECT id FROM '{p}' WHERE CASE WHEN id >= 0 THEN s END < 'b' AND id < 6"
        )),
        vec!["0", "1", "4", "5"]
    );
    assert_eq!(
        rows(&format!(
            "SELECT id FROM '{p}' WHERE CASE WHEN id >= 0 THEN s END = 'beta' AND id < 6"
        )),
        Vec::<String>::new()
    );
    assert_eq!(
        rows(&format!(
            "SELECT id FROM '{p}' WHERE s IN ('alpha', 'Beta') AND id < 6"
        )),
        vec!["0", "1", "4", "5"]
    );
    assert_eq!(
        rows(&format!("SELECT id FROM '{p}' WHERE s < 'b' AND id < 6")),
        vec!["0", "1", "4", "5"],
        "bytewise: uppercase sorts before lowercase"
    );
    assert_eq!(
        rows(&format!("SELECT s AS x FROM '{p}' WHERE id IN (3, 7)")),
        vec!["\"g,3\"", "\"g,7\""],
        "a comma is quoted as in DuckDB's CSV"
    );
    assert_eq!(
        rows(&format!(
            "SELECT s IS NULL AS x FROM '{p}' WHERE id IN (1, 2)"
        )),
        vec!["false", "true"]
    );
    assert_eq!(
        rows(&format!(
            "SELECT CAST(dt AS VARCHAR) AS x FROM '{p}' WHERE id = 0"
        )),
        vec!["1994-01-01"]
    );
    assert_eq!(
        rows(&format!(
            "SELECT id FROM '{p}' WHERE dt >= DATE '1994-04-11' AND dt < DATE '1994-07-20'"
        )),
        vec!["1"]
    );
    assert_eq!(
        rows(&format!(
            "SELECT id FROM '{p}' WHERE dt <= DATE '1994-07-20'"
        )),
        vec!["0", "1", "2"]
    );
    assert_eq!(
        rows(&format!("SELECT f AS x FROM '{p}' WHERE id < 4")),
        vec!["true", "false", "NULL", "false"]
    );
    assert_eq!(
        rows(&format!(
            "SELECT CASE WHEN f THEN s ELSE 'none' END AS x FROM '{p}' WHERE id < 4"
        )),
        vec!["alpha", "none", "none", "none"]
    );
}

#[test]
fn a_limit_stops_the_scan_and_the_report_says_how_far_it_got() {
    let p = fixture("limit");
    let cfg = ExecConfig {
        threads: 1,
        window: 1,
        ordered: true,
        opener: None,
        late_materialization: false,
    };
    for q in [
        format!("SELECT id FROM '{p}' LIMIT 3"),
        format!("SELECT id FROM '{p}' WHERE id + 0 >= 0 LIMIT 3"),
        format!("SELECT id + 1 AS x FROM '{p}' LIMIT 3"),
    ] {
        let o = run_with(&q, &cfg);
        assert_eq!(o.csv.expect("runs").lines().count(), 4, "{q}");
        let r = o.report.expect("report");
        assert_eq!(r.scan.row_groups_total, 4);
        assert!(r.scan.row_groups_read <= 2, "{q}: {r:?}");
        assert_eq!(r.ops.rows_out, 3);
    }
    // offset counts the rows the filter keeps, in file order
    assert_eq!(
        rows(&format!(
            "SELECT id FROM '{p}' WHERE id >= 4 LIMIT 2 OFFSET 3"
        )),
        vec!["7", "8"]
    );
    // LIMIT 0 reads no row group
    let o = run_with(&format!("SELECT id FROM '{p}' LIMIT 0"), &cfg);
    assert_eq!(o.csv.expect("runs"), "id\n");
    assert_eq!(o.report.expect("report").scan.rows_read, 0);
    // an offset beyond the end is an empty result
    assert!(rows(&format!("SELECT id FROM '{p}' LIMIT 5 OFFSET 100")).is_empty());
}

#[test]
fn a_limit_shields_the_rows_beyond_it_from_projection_errors_but_not_the_filter() {
    let p = fixture("shield");
    // id 2 is the first row with a = 0: LIMIT 2 never projects it, LIMIT 3 does
    assert_eq!(
        rows(&format!("SELECT 10 // a AS q FROM '{p}' LIMIT 2")),
        vec!["-5", "-10"]
    );
    assert!(run(&format!("SELECT 10 // a AS q FROM '{p}' LIMIT 3")).is_err());
    // a row the OFFSET drops is still projected (DuckDB: the same)
    assert!(run(&format!("SELECT 10 // a AS q FROM '{p}' LIMIT 1 OFFSET 2")).is_err());
    // the filter decides which rows count, so its errors are never skipped
    assert!(run(&format!("SELECT id FROM '{p}' WHERE 10 // a > 0 LIMIT 1")).is_err());
}

#[test]
fn a_query_without_a_column_to_read_still_counts_rows() {
    let p = fixture("nocol");
    assert_eq!(rows(&format!("SELECT 1 AS one FROM '{p}'")).len(), 20);
    assert_eq!(
        rows(&format!(
            "SELECT 1 AS one, 'x' AS s FROM '{p}' WHERE id < 3"
        )),
        vec!["1,x"; 3]
    );
    assert_eq!(
        rows(&format!("SELECT 1 AS one FROM '{p}' LIMIT 4 OFFSET 17")),
        vec!["1"; 3]
    );
}

#[test]
fn an_empty_result_has_a_header_and_no_rows() {
    let p = fixture("empty");
    assert_eq!(
        run(&format!("SELECT id, a FROM '{p}' WHERE id < 0")).expect("runs"),
        "id,a\n"
    );
    assert_eq!(
        run(&format!("SELECT id FROM '{p}' WHERE id + 0 < 0")).expect("runs"),
        "id\n"
    );
}

#[test]
fn a_chain_of_10000_terms_is_evaluated_on_a_big_stack() {
    let p = fixture("deep");
    let chain = vec!["id"; 10_000].join(" + ");
    // 10,000 additions of an INTEGER column overflow for id >= 215_000: here the values are 0..19, so 19 * 10_000
    let out = rows(&format!(
        "SELECT {chain} AS x FROM '{p}' WHERE id IN (1, 19)"
    ));
    assert_eq!(out, vec!["10000", "190000"]);
    let nested = format!("{}1{}", "(".repeat(900), ")".repeat(900));
    assert_eq!(
        rows(&format!(
            "SELECT id + {nested} AS x FROM '{p}' WHERE id = 0"
        )),
        vec!["1"]
    );
    let guard = vec!["id >= 0"; 3000].join(" AND ");
    assert_eq!(
        rows(&format!("SELECT id FROM '{p}' WHERE {guard} AND id < 2")),
        vec!["0", "1"]
    );
}

#[test]
fn errors_quote_the_query_and_never_a_cell_value() {
    let p = fixture("secret");
    for q in [
        format!("SELECT b * 1000000000 AS x FROM '{p}'"),
        format!("SELECT 1 // (id - id) AS x FROM '{p}'"),
        format!("SELECT CAST(s AS INTEGER) AS x FROM '{p}' WHERE id = 3"),
        format!("SELECT CAST(b AS INTEGER) AS x FROM '{p}'"),
        format!("SELECT CAST(d AS DECIMAL(5,2)) AS x FROM '{p}' WHERE id = 7"),
        format!("SELECT price * price * price * price * price * price AS x FROM '{p}'"),
    ] {
        let e = err(&q);
        for secret in [
            "g,3",
            "3000000000000",
            "19000000000000",
            "23.75",
            "NaN",
            "alpha",
            "Beta",
            "1000000000000",
        ] {
            assert!(!e.contains(secret), "{q}\n{e}\ncontains the value {secret}");
        }
        assert!(e.contains("line 1, column"), "{e}");
    }
}

#[test]
fn casts_between_text_and_numbers() {
    let p = fixture("casts");
    assert_eq!(
        rows(&format!(
            "SELECT CAST('12' AS INTEGER) AS a, CAST(' -7 ' AS BIGINT) AS b, CAST('1.005' AS DECIMAL(5,2)) AS c, CAST('2024-02-29' AS DATE) AS d, CAST('TRUE' AS BOOLEAN) AS e FROM '{p}' WHERE id = 0"
        )),
        vec!["12,-7,1.01,2024-02-29,true"]
    );
    assert_eq!(
        rows(&format!(
            "SELECT CAST('1e2' AS DECIMAL(6,1)) AS a FROM '{p}' WHERE id = 0"
        )),
        vec!["100.0"]
    );
    for bad in [
        "CAST('1.5' AS INTEGER)",
        "CAST('abc' AS BIGINT)",
        "CAST('2023-02-29' AS DATE)",
        "CAST('yes' AS BOOLEAN)",
        "CAST('1000' AS DECIMAL(5,2))",
        "CAST('2147483648' AS INTEGER)",
    ] {
        assert!(
            run(&format!("SELECT {bad} AS x FROM '{p}'")).is_err(),
            "{bad}"
        );
    }
    // nothing is raised for an unselected row or a NULL
    assert_eq!(
        rows(&format!(
            "SELECT CAST(s AS INTEGER) AS x FROM '{p}' WHERE id IN (2, 6)"
        )),
        vec!["NULL", "NULL"]
    );
}

#[test]
fn the_files_of_a_glob_are_read_in_name_order() {
    let d = dir("glob");
    for (i, name) in ["b.parquet", "a.parquet", "c.parquet"].iter().enumerate() {
        use arrow_array::Int32Array;
        let ids: arrow_array::ArrayRef = std::sync::Arc::new(Int32Array::from_iter_values([
            i as i32 * 10,
            i as i32 * 10 + 1,
        ]));
        let batch = arrow_array::RecordBatch::try_from_iter([("id", ids)]).expect("batch");
        write_batch(&d.join(name), &batch, 1);
    }
    let pattern = d.join("*.parquet");
    // a.parquet (written second: ids 10, 11), b (0, 1), c (20, 21)
    assert_eq!(
        rows(&format!("SELECT id FROM '{}'", pattern.display())),
        vec!["10", "11", "0", "1", "20", "21"]
    );
    assert_eq!(
        rows(&format!(
            "SELECT id FROM '{}' LIMIT 3 OFFSET 1",
            pattern.display()
        )),
        vec!["11", "0", "1"]
    );
}

#[test]
fn min_by_minus_one_is_an_error_for_a_remainder_and_an_integer_division() {
    let p = fixture("minus_one");
    for q in [
        "-2147483648 % -1",
        "-2147483648 // -1",
        "-9223372036854775808 // -1",
        "-9223372036854775808 % -1",
    ] {
        let e = err(&format!("SELECT {q} AS x FROM '{p}' WHERE id = 0"));
        assert!(e.contains("integer overflow"), "{q}: {e}");
    }
    assert_eq!(
        rows(&format!(
            "SELECT -2147483647 % -1 AS x, -7 // -1 AS y FROM '{p}' WHERE id = 0"
        )),
        vec!["0,7"]
    );
}

#[test]
fn a_projection_evaluates_the_operands_of_and_or_and_between_only_where_they_matter() {
    let p = fixture("projection_guards");
    // the division runs only for the rows the first operand left undecided (a is 0 for ids 2, 7, 12; NULL for 3, 10, 17)
    assert_eq!(
        rows(&format!(
            "SELECT (a <> 0 AND 10 // a > 1) AS x FROM '{p}' WHERE id IN (2, 3, 4)"
        )),
        vec!["false", "NULL", "true"]
    );
    assert_eq!(
        rows(&format!(
            "SELECT (a = 0 OR 10 // a > 100) AS x FROM '{p}' WHERE id IN (2, 3, 4)"
        )),
        vec!["true", "NULL", "false"]
    );
    // BETWEEN: the upper bound runs only for the rows the lower bound did not decide as FALSE (a = 0 < 1 is decided)
    assert_eq!(
        rows(&format!(
            "SELECT id FROM '{p}' WHERE a BETWEEN 1 AND 10 // a"
        )),
        ids((0..N).filter(|i| a(*i).is_some_and(|x| x == 1 || x == 2)))
    );
    assert_eq!(
        rows(&format!(
            "SELECT (a BETWEEN 1 AND 10 // a) AS x FROM '{p}' WHERE id IN (2, 3, 4)"
        )),
        vec!["false", "NULL", "true"]
    );
    // in a WHERE only the rows whose lower bound is TRUE reach the upper bound: id 3 has a NULL a, so its upper bound (a division by zero) never runs;
    // as a value the upper bound runs for it (the lower bound is NULL, not FALSE) and the projection fails
    assert_eq!(
        rows(&format!(
            "SELECT id FROM '{p}' WHERE a BETWEEN 1 AND 10 // (id - 3)"
        )),
        ids((0..N).filter(|i| a(*i).is_some_and(|x| x >= 1 && *i != 3 && x <= 10 / (*i - 3))))
    );
    assert!(
        run(&format!(
            "SELECT (a BETWEEN 1 AND 10 // (id - 3)) AS x FROM '{p}'"
        ))
        .is_err()
    );
    assert_eq!(
        rows(&format!(
            "SELECT id FROM '{p}' WHERE a NOT BETWEEN 2 AND 10 // (id - 2)"
        )),
        ids((0..N).filter(|i| a(*i).is_some_and(|x| x < 2 || x > 10 / (*i - 2))))
    );
}
