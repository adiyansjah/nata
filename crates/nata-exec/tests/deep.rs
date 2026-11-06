//! R11, deep expressions (register item R11, `docs/slices/0002-select-where-limit.md` part 5): the parser allows 10,000 terms in an operator chain and 1,000
//! levels of nesting (docs/dialect.md); the EXECUTOR must evaluate exactly those, on the default 2 MiB stack of a thread, without overflowing it. Finding of step
//! 4a: it already does, because `exec::execute` sizes a stack for the deepest expression (`DEEP`/`big_stack`) and evaluates on a thread that has it, in the scan's
//! workers and on the consuming thread. What was missing was the test AT the limits and for every construct, run from a thread of exactly 2 MiB (the bound of
//! the cost: a stack overflow aborts the process, so the known-bad build — `DEEP` raised so that no big stack is made — fails this file by crashing it).

mod common;
use common::*;

use std::sync::Arc;

use nata_exec::{ExecConfig, execute};
use nata_sql::{FsSource, plan_sql};

/// Run `f` on a thread with a stack of exactly 2 MiB (the default of a spawned thread).
fn on_2mb<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("the thread must not overflow its stack or panic")
}

fn path() -> String {
    static P: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    P.get_or_init(|| {
        let d = dir("deep-limits");
        standard(&d.join("t.parquet"));
        d.join("t.parquet").to_str().expect("path").to_string()
    })
    .clone()
}

/// The rows (without the header) of a query, run from a 2 MiB thread, with `threads` workers and the given option set.
fn rows_on_2mb(sql: String, threads: usize) -> Result<Vec<String>, String> {
    on_2mb(move || {
        let mut cfg = ExecConfig::new(threads);
        cfg.window = 2 * threads;
        run_with(&sql, &cfg)
            .csv
            .map(|c| c.lines().skip(1).map(str::to_string).collect())
    })
}

#[test]
fn an_operator_chain_of_10000_terms_is_evaluated_on_a_2_mb_thread_for_each_operator() {
    let p = path();
    // each chain keeps the value of `id`: + 0, - 0, * 1, // 1, % 1000 (id < 1000), then the boolean connectives over the whole file
    for (op, unit) in [
        ("+", "0"),
        ("-", "0"),
        ("*", "1"),
        ("//", "1"),
        ("%", "1000"),
    ] {
        let chain = format!("id{}", format!(" {op} {unit}").repeat(9_999));
        for threads in [1, 4] {
            let got = rows_on_2mb(
                format!("SELECT {chain} AS x FROM '{p}' WHERE id IN (3, 17)"),
                threads,
            )
            .unwrap_or_else(|e| panic!("{op}, {threads} threads: {e}"));
            assert_eq!(got, vec!["3", "17"], "{op}, {threads} threads");
            // the same chain in WHERE (not pushable) and under a LIMIT (the projection then runs on the consuming thread)
            let got = rows_on_2mb(format!("SELECT id FROM '{p}' WHERE {chain} >= 18"), threads)
                .unwrap_or_else(|e| panic!("{op} in WHERE, {threads} threads: {e}"));
            assert_eq!(got, vec!["18", "19"], "{op} in WHERE, {threads} threads");
            let got = rows_on_2mb(
                format!("SELECT {chain} AS x FROM '{p}' WHERE id >= 5 LIMIT 2 OFFSET 1"),
                threads,
            )
            .unwrap_or_else(|e| panic!("{op} under LIMIT, {threads} threads: {e}"));
            assert_eq!(got, vec!["6", "7"], "{op} under LIMIT, {threads} threads");
        }
    }
    for (op, expect) in [("AND", vec!["0", "1", "2"]), ("OR", vec!["0", "1", "2"])] {
        // AND: id >= 0 AND ... AND id < 3; OR: id < 3 OR id < 3 ...
        let chain = if op == "AND" {
            format!("(id < 3){}", " AND (id >= 0)".repeat(9_999))
        } else {
            format!("(id < 3){}", " OR (id < 3)".repeat(9_999))
        };
        for threads in [1, 4] {
            let got = rows_on_2mb(format!("SELECT id FROM '{p}' WHERE {chain}"), threads)
                .unwrap_or_else(|e| panic!("{op}, {threads} threads: {e}"));
            assert_eq!(got, expect, "{op}, {threads} threads");
        }
    }
}

#[test]
fn nesting_of_1000_levels_is_evaluated_on_a_2_mb_thread_for_each_construct() {
    let p = path();
    let n = 1_000usize;
    // (name, an expression of exactly 1,000 levels around `id`, the same value)
    let parens = format!("{}id{}", "(".repeat(n), ")".repeat(n));
    let neg = format!("{}id", "- ".repeat(n)); // an even number of minus signs: id
    let case_else = format!(
        "{}id{}",
        "CASE WHEN id < 0 THEN 1 ELSE ".repeat(n),
        " END".repeat(n)
    );
    let casts = format!("{}id{}", "CAST(".repeat(n), " AS INTEGER)".repeat(n));
    for (name, expr) in [
        ("parentheses", parens),
        ("unary minus", neg),
        ("CASE", case_else),
        ("CAST", casts),
    ] {
        for threads in [1, 4] {
            let got = rows_on_2mb(
                format!("SELECT {expr} AS x FROM '{p}' WHERE id IN (2, 9)"),
                threads,
            )
            .unwrap_or_else(|e| panic!("{name}, {threads} threads: {e}"));
            assert_eq!(got, vec!["2", "9"], "{name}, {threads} threads");
            let got = rows_on_2mb(format!("SELECT id FROM '{p}' WHERE {expr} >= 18"), threads)
                .unwrap_or_else(|e| panic!("{name} in WHERE, {threads} threads: {e}"));
            assert_eq!(got, vec!["18", "19"], "{name} in WHERE, {threads} threads");
            let got = rows_on_2mb(
                format!("SELECT {expr} AS x FROM '{p}' WHERE id >= 5 LIMIT 2"),
                threads,
            )
            .unwrap_or_else(|e| panic!("{name} under LIMIT, {threads} threads: {e}"));
            assert_eq!(got, vec!["5", "6"], "{name} under LIMIT, {threads} threads");
        }
    }
    // NOT, 1,000 levels deep (an even number): the boolean of `id < 3`
    let not = format!("{}id < 3", "NOT ".repeat(n));
    let got = rows_on_2mb(format!("SELECT id FROM '{p}' WHERE {not}"), 4).expect("NOT");
    assert_eq!(got, vec!["0", "1", "2"]);
}

#[test]
fn explain_and_explain_analyze_of_the_deepest_queries_run_on_a_2_mb_thread() {
    let p = path();
    let chain = format!("id{}", " + 0".repeat(9_999));
    let nested = format!("{}id{}", "(".repeat(1000), ")".repeat(1000));
    for expr in [chain, nested] {
        let sql = format!("EXPLAIN SELECT {expr} AS x FROM '{p}' WHERE id < 3");
        let text = on_2mb(move || {
            plan_sql(&sql, &FsSource)
                .expect("plans")
                .plan
                .explain_text()
        });
        assert!(
            text.contains("Scan") || text.contains("scan"),
            "{}",
            &text[..text.len().min(300)]
        );
        let sql = format!("SELECT {expr} AS x FROM '{p}' WHERE id < 3");
        let n = on_2mb(move || {
            let planned = plan_sql(&sql, &FsSource).expect("plans");
            let plan = Arc::new(planned.plan);
            let report =
                execute(Arc::clone(&plan), &ExecConfig::new(2), None, |_| Ok(())).expect("runs");
            let text = nata_exec::explain::analyze_text(&plan, &report);
            drop(plan);
            (report.ops.rows_out, text.len())
        });
        assert_eq!(n.0, 3);
    }
}

#[test]
fn a_chain_of_10001_terms_and_1001_levels_are_still_positioned_errors_not_crashes() {
    let p = path();
    let chain = format!("id{}", " + 0".repeat(10_000));
    let e = run_with(&format!("SELECT {chain} FROM '{p}'"), &ExecConfig::new(2))
        .csv
        .expect_err("one term too many");
    assert!(e.contains("line 1, column"), "{}", &e[..e.len().min(300)]);
    let nested = format!("{}id{}", "(".repeat(1001), ")".repeat(1001));
    let e = run_with(&format!("SELECT {nested} FROM '{p}'"), &ExecConfig::new(2))
        .csv
        .expect_err("one level too many");
    assert!(e.contains("line 1, column"), "{}", &e[..e.len().min(300)]);
}
