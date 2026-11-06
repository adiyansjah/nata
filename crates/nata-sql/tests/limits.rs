//! The limits of dialect section 8 through the whole front end (parse, bind, plan, print, drop), on the default 2 MB stack of a test thread: the front end
//! brings its own 16 MB stack, and the tree it returns is dropped and printed without recursion that the caller's stack could not take.

mod common;
use common::*;

fn on_2mb<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("no stack overflow, no panic")
}

fn through(sql: String) -> String {
    on_2mb(move || {
        let p = plan(&sql).unwrap_or_else(|e| panic!("{e}"));
        let text = p.plan.explain_text();
        drop(p);
        text
    })
}

#[test]
fn chains_of_10000_terms_are_planned_printed_and_dropped_for_each_kind_of_operator() {
    for (name, sql, ty) in [
        (
            "+",
            format!("SELECT a{} AS r FROM 't.parquet'", " + a".repeat(9_999)),
            "INTEGER",
        ),
        (
            "-",
            format!("SELECT c{} AS r FROM 't.parquet'", " - b".repeat(9_999)),
            "BIGINT",
        ),
        (
            "*",
            format!(
                "SELECT d_10_2{} AS r FROM 't.parquet'",
                " * 1".repeat(9_999)
            ),
            "DECIMAL",
        ),
        (
            "/ then +",
            format!("SELECT a / 1{} AS r FROM 't.parquet'", " + 1".repeat(9_998)),
            "DECIMAL",
        ),
        (
            "//",
            format!("SELECT a{} AS r FROM 't.parquet'", " // 1".repeat(9_999)),
            "INTEGER",
        ),
        (
            "::",
            format!(
                "SELECT a{} AS r FROM 't.parquet'",
                "::INTEGER".repeat(9_999)
            ),
            "INTEGER",
        ),
        (
            "IS NULL",
            format!("SELECT a{} AS r FROM 't.parquet'", " IS NULL".repeat(9_999)),
            "BOOLEAN",
        ),
        (
            "AND",
            format!("SELECT bo{} AS r FROM 't.parquet'", " AND bo".repeat(9_999)),
            "BOOLEAN",
        ),
        (
            "OR",
            format!("SELECT bo{} AS r FROM 't.parquet'", " OR bo".repeat(9_999)),
            "BOOLEAN",
        ),
    ] {
        let t = std::time::Instant::now();
        let text = through(sql);
        assert!(
            text.contains(ty),
            "{name}: {}",
            &text[..text.len().min(200)]
        );
        assert!(t.elapsed().as_secs() < 20, "{name}: {:?}", t.elapsed());
    }
}

#[test]
fn nesting_of_1000_levels_is_planned_printed_and_dropped() {
    let n = 1000;
    for (name, sql) in [
        (
            "parentheses",
            format!(
                "SELECT {}a{} AS r FROM 't.parquet'",
                "(".repeat(n),
                ")".repeat(n)
            ),
        ),
        (
            "CASE",
            format!(
                "SELECT {}1{} AS r FROM 't.parquet'",
                "CASE WHEN bo THEN ".repeat(n),
                " ELSE 2 END".repeat(n)
            ),
        ),
        (
            "NOT",
            format!("SELECT {}bo AS r FROM 't.parquet'", "NOT ".repeat(n)),
        ),
        (
            "unary minus",
            format!("SELECT {}a AS r FROM 't.parquet'", "- ".repeat(n)),
        ),
        (
            "CAST(",
            format!(
                "SELECT {}a{} AS r FROM 't.parquet'",
                "CAST(".repeat(n),
                " AS INTEGER)".repeat(n)
            ),
        ),
        (
            "mixed",
            format!(
                "SELECT {}bo{} AS r FROM 't.parquet'",
                "(NOT CASE WHEN ".repeat(333),
                " THEN bo ELSE bo END)".repeat(333)
            ),
        ),
    ] {
        let text = through(sql);
        assert!(text.starts_with("Project"), "{name}");
    }
}

#[test]
fn a_long_in_list_and_a_long_select_list_are_planned() {
    let q = format!(
        "SELECT a FROM 't.parquet' WHERE a IN (1{})",
        ",1".repeat(400_000)
    );
    let t = std::time::Instant::now();
    let text = through(q);
    assert!(text.contains("Filter"), "an IN list is not pushed");
    assert!(t.elapsed().as_secs() < 20, "{:?}", t.elapsed());
    let q = format!("SELECT a{} FROM 't.parquet'", ", b".repeat(200_000));
    let text = through(q);
    assert!(text.starts_with("Project"));
}

#[test]
fn a_query_of_1_mb_with_a_long_comment_or_string_is_planned() {
    let q = format!(
        "SELECT a FROM 't.parquet' WHERE s = '{}'",
        "x".repeat(1_000_000)
    );
    let text = through(q);
    assert!(
        text.contains("pushed: s = 'xxxx"),
        "a long string literal is pushed"
    );
    let q = format!("SELECT a FROM 't.parquet' -- {}", "y".repeat(1_000_000));
    assert!(through(q).contains("Scan"));
}
