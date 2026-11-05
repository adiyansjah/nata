//! DIFFERENTIAL against DuckDB: TPC-H `lineitem` (SF0.01, 60,175 rows) sorted by `l_shipdate` and written with 2,048-row groups (30 row groups) by
//! DuckDB, by parquet-rs in several styles and by pyarrow, then scanned with a matrix of projections x filters. Every case compares the rows IN ORDER
//! with DuckDB's, for the pruned and for the unpruned scan, and checks the skip count: nonzero where the case is designed to prune, ZERO where it is
//! designed not to. Prints `DIFFERENTIAL <test>: <n> cases` for the record.

mod common;
use common::*;
use std::path::PathBuf;

struct Q {
    name: &'static str,
    filters: Vec<Filter>,
    skip: Skip,
}

fn q(name: &'static str, filters: Vec<Filter>, skip: Skip) -> Q {
    Q {
        name,
        filters,
        skip,
    }
}

/// The filters of the matrix, with what the layout (sorted by `l_shipdate`, 30 row groups) says about pruning.
fn queries() -> Vec<Q> {
    let d = |op: &str, day: &str| flt("l_shipdate", op, day, &format!("DATE '{day}'"));
    let mut v = vec![
        // the card's example
        q(
            "year-1994",
            vec![d(">=", "1994-01-01"), d("<", "1995-01-01")],
            Skip::AtLeast(20),
        ),
        q(
            "year-1994-and-discount-and-quantity",
            vec![
                d(">=", "1994-01-01"),
                d("<", "1995-01-01"),
                flt("l_discount", ">=", "0.05", "0.05"),
                flt("l_quantity", "<", "24", "24"),
            ],
            Skip::AtLeast(20),
        ),
        // no cross-term reasoning: the one row group that straddles 1995-01-01 passes both terms on its own
        q(
            "empty-range",
            vec![d(">=", "1995-01-01"), d("<", "1995-01-01")],
            Skip::AtLeast(28),
        ),
        // l_shipdate runs from 1992-01-02 to 1998-12-01
        q("date-lt-min", vec![d("<", "1992-01-02")], Skip::All),
        q(
            "date-lt-min-plus",
            vec![d("<", "1992-01-03")],
            Skip::AtLeast(25),
        ),
        q(
            "date-le-min",
            vec![d("<=", "1992-01-02")],
            Skip::AtLeast(25),
        ),
        q("date-ge-min", vec![d(">=", "1992-01-02")], Skip::Zero),
        q("date-gt-max", vec![d(">", "1998-12-01")], Skip::All),
        q(
            "date-ge-max",
            vec![d(">=", "1998-12-01")],
            Skip::AtLeast(25),
        ),
        q("date-ge-above-max", vec![d(">=", "1998-12-02")], Skip::All),
        q("date-le-max", vec![d("<=", "1998-12-01")], Skip::Zero),
        q(
            "date-eq-middle",
            vec![d("=", "1995-06-17")],
            Skip::AtLeast(25),
        ),
        q(
            "date-eq-absent-below",
            vec![d("=", "1991-12-31")],
            Skip::All,
        ),
        q("date-ne-middle", vec![d("<>", "1995-06-17")], Skip::Zero),
        q("date-all-rows", vec![d(">=", "1900-01-01")], Skip::Zero),
        q("date-no-rows", vec![d(">", "2100-01-01")], Skip::All),
        // an integer column that is not sorted: pruning can only prove the extremes
        q(
            "orderkey-lt-0",
            vec![flt("l_orderkey", "<", "0", "0")],
            Skip::All,
        ),
        q(
            "orderkey-eq-0",
            vec![flt("l_orderkey", "=", "0", "0")],
            Skip::All,
        ),
        q(
            "orderkey-between",
            vec![
                flt("l_orderkey", ">=", "1000", "1000"),
                flt("l_orderkey", "<=", "2000", "2000"),
            ],
            Skip::Zero,
        ),
        q(
            "orderkey-huge-literal",
            vec![flt(
                "l_orderkey",
                "<",
                "99999999999999999999",
                "99999999999999999999",
            )],
            Skip::Zero,
        ),
        q("orderkey-is-null", vec![is_null("l_orderkey")], Skip::All),
        q(
            "orderkey-is-not-null",
            vec![is_not_null("l_orderkey")],
            Skip::Zero,
        ),
        // decimals, with literals at other scales (never rounded)
        q(
            "discount-gt-max",
            vec![flt("l_discount", ">", "0.10", "0.10")],
            Skip::All,
        ),
        q(
            "discount-gt-max-scale0",
            vec![flt("l_discount", ">", "1", "1")],
            Skip::All,
        ),
        q(
            "discount-ge-max",
            vec![flt("l_discount", ">=", "0.10", "0.10")],
            Skip::Zero,
        ),
        q(
            "discount-lt-min",
            vec![flt("l_discount", "<", "0.00", "0.00")],
            Skip::All,
        ),
        q(
            "discount-lt-min-scale0",
            vec![flt("l_discount", "<", "0", "0")],
            Skip::All,
        ),
        q(
            "discount-eq-scale3-absent",
            vec![flt("l_discount", "=", "0.055", "0.055")],
            Skip::Zero,
        ),
        q(
            "discount-eq-scale1",
            vec![flt("l_discount", "=", "0.1", "0.1")],
            Skip::Zero,
        ),
        q(
            "discount-le-just-below-min",
            vec![flt("l_discount", "<", "0.0000001", "0.0000001")],
            Skip::Zero,
        ),
        q(
            "discount-gt-just-below-max",
            vec![flt("l_discount", ">", "0.0999999", "0.0999999")],
            Skip::Zero,
        ),
        q(
            "quantity-ne",
            vec![flt("l_quantity", "<>", "24", "24")],
            Skip::Zero,
        ),
        q(
            "tax-ge-literal-wider-than-column",
            vec![flt("l_tax", ">=", "0.080000", "0.080000")],
            Skip::Zero,
        ),
        // the dictionary columns: l_returnflag is 'N' only in the late row groups
        q(
            "returnflag-eq-R",
            vec![flt("l_returnflag", "=", "'R'", "'R'")],
            Skip::AtLeast(5),
        ),
        // 'N' sorts between 'A' and 'R': the early groups hold only A and R, so the range [A, R] contains it
        q(
            "returnflag-eq-N",
            vec![flt("l_returnflag", "=", "'N'", "'N'")],
            Skip::Zero,
        ),
        q(
            "returnflag-eq-A",
            vec![flt("l_returnflag", "=", "'A'", "'A'")],
            Skip::AtLeast(5),
        ),
        q(
            "returnflag-eq-absent",
            vec![flt("l_returnflag", "=", "'Z'", "'Z'")],
            Skip::All,
        ),
        q(
            "returnflag-lt-A",
            vec![flt("l_returnflag", "<", "'A'", "'A'")],
            Skip::All,
        ),
        q(
            "returnflag-gt-A",
            vec![flt("l_returnflag", ">", "'A'", "'A'")],
            Skip::Any,
        ),
        q(
            "returnflag-ne-A",
            vec![flt("l_returnflag", "<>", "'A'", "'A'")],
            Skip::Any,
        ),
        q(
            "returnflag-and-linestatus",
            vec![
                flt("l_returnflag", "=", "'R'", "'R'"),
                flt("l_linestatus", "=", "'O'", "'O'"),
            ],
            Skip::AtLeast(5),
        ),
        q(
            "returnflag-is-null",
            vec![is_null("l_returnflag")],
            Skip::All,
        ),
        q(
            "shipmode-eq",
            vec![flt("l_shipmode", "=", "'AIR'", "'AIR'")],
            Skip::Zero,
        ),
        q(
            "shipmode-lt-min",
            vec![flt("l_shipmode", "<", "'AIR'", "'AIR'")],
            Skip::All,
        ),
        q(
            "comment-gt-above-max",
            vec![flt(
                "l_comment",
                ">",
                "'zzzzzzzzzzzzzzzzzzzzzzzzzzzz'",
                "'zzzzzzzzzzzzzzzzzzzzzzzzzzzz'",
            )],
            Skip::All,
        ),
        q(
            "comment-lt-space",
            vec![flt("l_comment", "<", "' '", "' '")],
            Skip::All,
        ),
        q(
            "comment-quote-literal",
            vec![flt("l_comment", "=", "'it''s'", "'it''s'")],
            Skip::Any,
        ),
        q(
            "comment-ne-empty",
            vec![flt("l_comment", "<>", "''", "''")],
            Skip::Zero,
        ),
        // two columns and a contradiction between them
        q(
            "shipdate-and-receiptdate-disjoint",
            vec![
                d("<", "1992-06-01"),
                flt("l_receiptdate", ">", "1993-01-01", "DATE '1993-01-01'"),
            ],
            Skip::All,
        ),
    ];
    // every operator on the date and on a decimal at three literals each
    for op in OPS_LOCAL {
        for day in ["1992-01-02", "1995-06-17", "1998-12-01"] {
            v.push(q("op-grid-date", vec![d(op, day)], Skip::Any));
        }
        for lit in ["0", "0.05", "0.1"] {
            v.push(q(
                "op-grid-discount",
                vec![flt("l_discount", op, lit, lit)],
                Skip::Any,
            ));
        }
    }
    v
}

const OPS_LOCAL: [&str; 6] = ["<", "<=", ">", ">=", "=", "<>"];

fn column_sets() -> Vec<Option<Vec<&'static str>>> {
    vec![
        None,
        Some(vec!["l_orderkey"]),
        Some(vec!["l_shipdate", "l_quantity"]),
        Some(vec!["l_returnflag", "l_linestatus", "l_extendedprice"]),
        Some(vec!["l_comment"]),
        // reversed and partial: the output order is the order asked for
        Some(vec!["l_tax", "l_discount", "l_orderkey"]),
    ]
}

fn run_matrix(duck: &str, file: &std::path::Path, label: &str, full: bool) -> usize {
    let mut specs = Vec::new();
    let sets = column_sets();
    let all = queries();
    for (si, cols) in sets.iter().enumerate() {
        // the all-columns scans are the slow ones: only a few filters
        for (qi, query) in all.iter().enumerate() {
            if cols.is_none() && !(qi < 12 || query.name == "returnflag-eq-R") {
                continue;
            }
            if !full && si > 1 {
                continue;
            }
            specs.push(Spec {
                columns: cols
                    .as_ref()
                    .map(|c| c.iter().map(|s| s.to_string()).collect()),
                filters: query.filters.clone(),
                expect: query.skip,
                what: format!("{label} columns#{si} {}", query.name),
            });
        }
    }
    check_cases(duck, &[file], &specs);
    specs.len()
}

#[test]
fn lineitem_written_by_duckdb() {
    let Some(duck) = duckdb_or_skip("lineitem_written_by_duckdb") else {
        return;
    };
    let dir = scratch("li-duck");
    let Some(file) = data::lineitem_sorted(&duck, &dir) else {
        return;
    };
    let n = run_matrix(&duck, &file, "duckdb", true);
    eprintln!("DIFFERENTIAL lineitem_written_by_duckdb: {n} cases");
    for d in pushdown_differences() {
        eprintln!("DUCKDB-PUSHDOWN-DIFFERENCE {d}");
    }
}

/// A projection that does not contain the filter column, and the filter columns of every type that are not projected.
#[test]
fn filter_columns_that_are_not_projected() {
    let Some(duck) = duckdb_or_skip("filter_columns_that_are_not_projected") else {
        return;
    };
    let dir = scratch("li-nonproj");
    let Some(file) = data::lineitem_sorted(&duck, &dir) else {
        return;
    };
    let mut n = 0;
    for (proj, f) in [
        (
            vec!["l_orderkey"],
            flt("l_shipdate", ">=", "1998-01-01", "DATE '1998-01-01'"),
        ),
        (
            vec!["l_orderkey", "l_linenumber"],
            flt("l_discount", "=", "0.05", "0.05"),
        ),
        (vec!["l_shipdate"], flt("l_returnflag", "=", "'R'", "'R'")),
        (vec!["l_quantity"], flt("l_comment", ">", "'x'", "'x'")),
        (vec!["l_returnflag"], flt("l_orderkey", "<", "1000", "1000")),
    ] {
        let r = check_case(
            &duck,
            &[&file],
            Some(&proj),
            std::slice::from_ref(&f),
            Skip::Any,
            "non-projected filter column",
        );
        // the filter column is read in addition to the projection
        assert_eq!(
            r.columns_read,
            proj.len() + 1,
            "columns read: the projection plus the filter column"
        );
        n += 1;
    }
    // the filter column that is also projected is read once
    let r = check_case(
        &duck,
        &[&file],
        Some(&["l_shipdate"]),
        &[flt("l_shipdate", ">=", "1998-01-01", "DATE '1998-01-01'")],
        Skip::AtLeast(20),
        "projected filter column",
    );
    assert_eq!(r.columns_read, 1);
    eprintln!(
        "DIFFERENTIAL filter_columns_that_are_not_projected: {} cases",
        n + 1
    );
}

#[test]
fn lineitem_written_by_parquet_rs_in_every_style() {
    let Some(duck) = duckdb_or_skip("lineitem_written_by_parquet_rs") else {
        return;
    };
    let dir = scratch("li-prs");
    let Some(file) = data::lineitem_sorted(&duck, &dir) else {
        return;
    };
    let mut n = 0;
    let styles = writers::styles();
    for style in &styles {
        let f = data::rewrite_parquet_rs(&file, &dir, "li", style);
        // the same file layout (row groups of the style): the full matrix on the narrow column sets
        let tag = format!("parquet-rs {}", style.name);
        let year = vec![
            flt("l_shipdate", ">=", "1994-01-01", "DATE '1994-01-01'"),
            flt("l_shipdate", "<", "1995-01-01", "DATE '1995-01-01'"),
        ];
        check_case(
            &duck,
            &[&f],
            None,
            &year,
            Skip::AtLeast(10),
            &format!("{tag} year, all columns"),
        );
        check_case(
            &duck,
            &[&f],
            Some(&["l_returnflag", "l_extendedprice"]),
            &[flt("l_returnflag", "=", "'R'", "'R'")],
            Skip::AtLeast(1),
            &format!("{tag} returnflag"),
        );
        check_case(
            &duck,
            &[&f],
            Some(&["l_orderkey", "l_discount"]),
            &[flt("l_discount", ">=", "0.05", "0.05")],
            Skip::Zero,
            &format!("{tag} discount"),
        );
        check_case(
            &duck,
            &[&f],
            Some(&["l_comment"]),
            &[flt("l_comment", ">=", "'m'", "'m'")],
            Skip::Zero,
            &format!("{tag} comment"),
        );
        n += 4;
        if matches!(style.name, "snappy-dict-v2" | "delta-v2" | "truncate4") {
            n += run_matrix(&duck, &f, &tag, false);
        }
    }
    eprintln!(
        "DIFFERENTIAL lineitem_written_by_parquet_rs_in_every_style: {n} cases over {} styles",
        styles.len()
    );
}

#[test]
fn lineitem_written_by_pyarrow() {
    let Some(duck) = duckdb_or_skip("lineitem_written_by_pyarrow") else {
        return;
    };
    let dir = scratch("li-pa");
    let Some(file) = data::lineitem_sorted(&duck, &dir) else {
        return;
    };
    let mut n = 0;
    let mut files: Vec<PathBuf> = Vec::new();
    for variant in data::PYARROW_VARIANTS {
        match data::rewrite_pyarrow(&file, &dir, "li", variant) {
            Some(f) => files.push(f),
            None => {
                eprintln!("SKIPPED lineitem_written_by_pyarrow: pyarrow is not available");
                return;
            }
        }
    }
    for (variant, f) in data::PYARROW_VARIANTS.iter().zip(&files) {
        n += run_matrix(&duck, f, &format!("pyarrow {variant}"), false);
        let year = vec![
            flt("l_shipdate", ">=", "1994-01-01", "DATE '1994-01-01'"),
            flt("l_shipdate", "<", "1995-01-01", "DATE '1995-01-01'"),
        ];
        check_case(
            &duck,
            &[f],
            None,
            &year,
            Skip::AtLeast(20),
            &format!("pyarrow {variant} year, all columns"),
        );
        n += 1;
    }
    eprintln!(
        "DIFFERENTIAL lineitem_written_by_pyarrow: {n} cases over {} files",
        files.len()
    );
}
