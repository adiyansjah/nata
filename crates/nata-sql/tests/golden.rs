//! Golden plans: 50 queries, the text of their plan (what EXPLAIN will print) or of their error, with the pushdown decisions visible. The expected text is
//! in `tests/golden/plans.txt`; `NATA_BLESS=1 cargo test --test golden` rewrites it (then read the diff: it is the review).

mod common;
use common::*;

const QUERIES: &[&str] = &[
    // the example of the card
    "SELECT l_orderkey, l_extendedprice * (1 - l_discount) AS net FROM 'lineitem.parquet' WHERE l_shipdate >= DATE '1994-01-01' AND l_shipdate < DATE '1995-01-01' AND l_discount BETWEEN 0.05 AND 0.07 LIMIT 10",
    "EXPLAIN SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_shipdate >= DATE '1994-01-01'",
    "EXPLAIN ANALYZE SELECT * FROM 'nation.parquet' WHERE n_regionkey = 2",
    // pushed: column <op> literal, in each type
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_orderkey = 1",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE 5 < l_orderkey",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_orderkey <> 5 AND l_orderkey <= 100 AND l_orderkey > 0 AND l_orderkey >= 1",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_shipdate < DATE '1995-01-01'",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_returnflag = 'R'",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_comment = 'it''s'",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_discount > 0",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_discount = 0.055",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_quantity < 24 AND l_extendedprice >= 1e3",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_orderkey > 3000000000",
    "SELECT a FROM 't.parquet' WHERE ti < 5",
    "SELECT a FROM 't.parquet' WHERE bo = TRUE",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_comment IS NULL AND l_shipdate IS NOT NULL",
    // BETWEEN is two pushed conjuncts, unless negated or not literal
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_shipdate BETWEEN DATE '1994-01-01' AND DATE '1994-12-31'",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_discount NOT BETWEEN 0.05 AND 0.07",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_discount BETWEEN l_tax AND 0.07",
    // not pushed: OR, NOT, expressions, two columns, IN, casts, NULL literal, no column
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_orderkey = 1 OR l_orderkey = 2",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE NOT (l_orderkey > 1)",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE NOT l_comment IS NULL",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_orderkey + 1 > 5",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_commitdate < l_receiptdate",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_shipmode IN ('AIR', 'MAIL')",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE CAST(l_orderkey AS INTEGER) = 1",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_orderkey = NULL",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE 1 = 1",
    "SELECT a FROM 't.parquet' WHERE bo",
    "SELECT a FROM 't.parquet' WHERE d = CAST(1.5 AS DOUBLE)",
    // literals that are not typed exactly to the column are not pushed
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_orderkey < 10.5",
    "SELECT l_linenumber FROM 'lineitem.parquet' WHERE l_linenumber < 3000000000",
    "SELECT a FROM 't.parquet' WHERE ti < 1000",
    // pushed and remaining side by side, in the order written
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_discount > 0.05 AND l_quantity * 2 > 10 AND l_shipdate >= DATE '1994-01-01' AND (l_tax < 0.01 OR l_tax > 0.07)",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE (l_orderkey > 1 AND (l_orderkey < 9 AND l_partkey = 3)) AND l_suppkey IN (1, 2)",
    // which columns the scan reads
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_shipdate >= DATE '1994-01-01' AND l_discount < 0.01 AND l_tax > 0 AND l_quantity * 2 > 1",
    "SELECT l_tax, l_orderkey, l_tax FROM 'lineitem.parquet' WHERE l_partkey = 7",
    "SELECT 1 FROM 'lineitem.parquet'",
    "SELECT 1 AS one, l_orderkey FROM 'lineitem.parquet' LIMIT 5",
    // LIMIT and OFFSET
    "SELECT * FROM 'nation.parquet' LIMIT 3 OFFSET 2",
    "SELECT n_name FROM 'nation.parquet' WHERE n_regionkey = 1 LIMIT 0",
    // projections: names, types, casts, CASE
    "SELECT n_nationkey + 1 AS next, n_name AS \"Nation Name\", n_nationkey / 2 FROM 'nation.parquet'",
    "SELECT CASE WHEN l_discount > 0.05 THEN l_extendedprice * 0.9 ELSE l_extendedprice END AS p, CAST(l_quantity AS BIGINT) AS q, l_orderkey::VARCHAR FROM 'lineitem.parquet' WHERE CASE WHEN l_tax > 0 THEN l_discount < l_tax ELSE FALSE END",
    "SELECT l_extendedprice * (1 - l_discount) * (1 + l_tax) AS charge, -l_discount, l_orderkey // 2, l_orderkey % 7 FROM 'lineitem.parquet'",
    "SELECT \"with space\", \"A\", \"MixedCase\" FROM 't.parquet'",
    // a glob
    "SELECT l_orderkey FROM 'li-*.parquet' WHERE l_orderkey = 1",
    // errors, with the class and the position
    "SELECT l_orderky FROM 'lineitem.parquet'",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_shipdate = '1994-01-01'",
    "SELECT l_orderkey FROM 'lineitem.parquet' GROUP BY l_orderkey",
    "SELECT d_15_2 / d_15_2 * d_38_2 FROM 't.parquet'",
    "SELECT * FROM 't.parquet'",
    "SELECT l_orderkey FROM 'lineitem.parquet' WHERE l_orderkey = 1 LIMIT ALL",
];

fn render(sql: &str) -> String {
    match plan(sql) {
        Ok(p) => format!("mode: {:?}\n{}", p.mode, p.plan.explain_text()),
        Err(e) => format!("{e}\n"),
    }
}

#[test]
fn the_plans_match_the_golden_file() {
    assert!(QUERIES.len() >= 40, "{} queries", QUERIES.len());
    let mut all = String::new();
    for (i, q) in QUERIES.iter().enumerate() {
        all.push_str(&format!("### {}\n{q}\n---\n{}\n", i + 1, render(q)));
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/plans.txt");
    if std::env::var("NATA_BLESS").is_ok_and(|v| v == "1") {
        std::fs::write(path, &all).expect("write the golden file");
        return;
    }
    let want = std::fs::read_to_string(path)
        .expect("tests/golden/plans.txt (run with NATA_BLESS=1 to create it)");
    if want != all {
        // show the first block that differs
        let (w, a): (Vec<&str>, Vec<&str>) =
            (want.split("### ").collect(), all.split("### ").collect());
        for (i, (x, y)) in w.iter().zip(&a).enumerate() {
            assert_eq!(
                x, y,
                "golden block {i} differs (NATA_BLESS=1 rewrites the file; read the diff)"
            );
        }
        assert_eq!(w.len(), a.len(), "the number of golden blocks differs");
    }
}
