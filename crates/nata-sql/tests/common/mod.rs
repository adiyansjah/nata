//! Shared fixtures of the integration tests: in-memory sources, and helpers that read a plan back as text and types.
#![allow(dead_code, reason = "each test file uses a different part")]

use nata_parquet::schema::{Kind, Unit};
use nata_sql::plan::render_expr;
use nata_sql::source::MemSource;
use nata_sql::{ErrorClass, Planned, SqlError, plan_sql};

pub fn int(bits: u8) -> Result<Kind, &'static str> {
    Ok(Kind::Int { bits, signed: true })
}

pub fn dec(precision: u8, scale: u8) -> Result<Kind, &'static str> {
    Ok(Kind::Decimal { precision, scale })
}

/// `t.parquet`: one column of each type of the dialect, decimals named `d_<precision>_<scale>`.
pub fn t_columns() -> Vec<(&'static str, Result<Kind, &'static str>)> {
    vec![
        ("a", int(32)),
        ("b", int(32)),
        ("c", int(64)),
        ("ti", int(8)),
        ("si", int(16)),
        ("s", Ok(Kind::Utf8)),
        ("d", Ok(Kind::Float64)),
        ("f", Ok(Kind::Float32)),
        ("dt", Ok(Kind::Date)),
        ("bo", Ok(Kind::Bool)),
        ("d_15_2", dec(15, 2)),
        ("e_15_2", dec(15, 2)),
        ("d_5_3", dec(5, 3)),
        ("d_18_2", dec(18, 2)),
        ("d_38_2", dec(38, 2)),
        ("d_10_2", dec(10, 2)),
        ("d_10_9", dec(10, 9)),
        ("d_20_2", dec(20, 2)),
        ("d_20_20", dec(20, 20)),
        ("d_10_0", dec(10, 0)),
        ("d_19_0", dec(19, 0)),
        ("d_38_0", dec(38, 0)),
        ("d_10_8", dec(10, 8)),
        ("d_38_38", dec(38, 38)),
        ("d_1_0", dec(1, 0)),
        ("ts", Ok(Kind::Timestamp(Unit::Micros))),
        (
            "u",
            Ok(Kind::Int {
                bits: 32,
                signed: false,
            }),
        ),
        (
            "nested",
            Err("a nested or repeated column (structs, lists and maps)"),
        ),
        ("MixedCase", Ok(Kind::Utf8)),
        ("A", int(32)),
        ("with space", int(32)),
    ]
}

pub fn lineitem_columns() -> Vec<(&'static str, Result<Kind, &'static str>)> {
    vec![
        ("l_orderkey", int(64)),
        ("l_partkey", int(64)),
        ("l_suppkey", int(64)),
        ("l_linenumber", int(32)),
        ("l_quantity", dec(15, 2)),
        ("l_extendedprice", dec(15, 2)),
        ("l_discount", dec(15, 2)),
        ("l_tax", dec(15, 2)),
        ("l_returnflag", Ok(Kind::Utf8)),
        ("l_linestatus", Ok(Kind::Utf8)),
        ("l_shipdate", Ok(Kind::Date)),
        ("l_commitdate", Ok(Kind::Date)),
        ("l_receiptdate", Ok(Kind::Date)),
        ("l_shipinstruct", Ok(Kind::Utf8)),
        ("l_shipmode", Ok(Kind::Utf8)),
        ("l_comment", Ok(Kind::Utf8)),
    ]
}

pub fn src() -> MemSource {
    MemSource::new()
        .file("t.parquet", &t_columns())
        .file("lineitem.parquet", &lineitem_columns())
        .file(
            "nation.parquet",
            &[
                ("n_nationkey", int(32)),
                ("n_name", Ok(Kind::Utf8)),
                ("n_regionkey", int(32)),
                ("n_comment", Ok(Kind::Utf8)),
            ],
        )
}

pub fn plan(sql: &str) -> Result<Planned, SqlError> {
    plan_sql(sql, &src())
}

pub fn ok(sql: &str) -> Planned {
    plan(sql).unwrap_or_else(|e| panic!("{sql:?}: {e}"))
}

pub fn err(sql: &str) -> SqlError {
    match plan(sql) {
        Ok(p) => panic!("{sql:?} must fail, got\n{}", p.plan.explain_text()),
        Err(e) => e,
    }
}

/// The type of the single output column of `SELECT <expr> FROM 't.parquet'`.
pub fn ty(expr: &str) -> String {
    let p = ok(&format!("SELECT {expr} AS r FROM 't.parquet'"));
    assert_eq!(p.output.len(), 1);
    p.output[0].ty.to_string()
}

/// The error of `SELECT <expr> FROM 't.parquet'`.
pub fn ty_err(expr: &str) -> SqlError {
    err(&format!("SELECT {expr} AS r FROM 't.parquet'"))
}

/// The select-list expressions of a query as text (every operator parenthesised, columns by name).
pub fn exprs(sql: &str) -> Vec<String> {
    let p = ok(sql);
    let names: Vec<String> = p.plan.scan.columns.iter().map(|c| c.name.clone()).collect();
    p.plan
        .project
        .exprs
        .iter()
        .map(|e| render_expr(&e.expr, &names))
        .collect()
}

/// `SELECT <expr> AS r FROM 't.parquet'` as one text.
pub fn show(expr: &str) -> String {
    exprs(&format!("SELECT {expr} AS r FROM 't.parquet'")).remove(0)
}

/// The WHERE clause as the plan keeps it: (pushed conjuncts in SQL, the remaining filter or "-").
pub fn where_split(cond: &str) -> (Vec<String>, String) {
    let p = ok(&format!("SELECT a FROM 't.parquet' WHERE {cond}"));
    let names: Vec<String> = p.plan.scan.columns.iter().map(|c| c.name.clone()).collect();
    let pushed = p.plan.scan.pushed.iter().map(|t| t.sql()).collect();
    let rest = p
        .plan
        .filter
        .as_ref()
        .map_or_else(|| "-".to_string(), |f| render_expr(&f.predicate, &names));
    (pushed, rest)
}

pub fn class(e: &SqlError) -> ErrorClass {
    e.class
}
