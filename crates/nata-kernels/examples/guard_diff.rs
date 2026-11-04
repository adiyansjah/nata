//! Prints, for every (table, query, executor), the SQL for DuckDB and the outcome of our kernels.
//! `scripts/guard_diff.py` runs the SQL in DuckDB and compares. One tab-separated line per case:
//! `table  query  executor  setup_sql  query_sql  outcome`
//! where outcome is `rows:id=value;id=value;...` (NULL as `NULL`) or `error:division_by_zero|overflow`.
#![allow(
    clippy::arithmetic_side_effects,
    reason = "example code on small values"
)]
#[path = "../tests/guard_cases/mod.rs"]
mod guard_cases;

use guard_cases::{EXECS, queries, tables};

fn literal(v: Option<i64>) -> String {
    match v {
        None => "NULL::BIGINT".to_string(),
        Some(i64::MIN) => "(-9223372036854775807::BIGINT - 1)".to_string(),
        Some(v) => format!("({v})::BIGINT"),
    }
}

fn main() {
    for t in tables() {
        let values: Vec<String> = t
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| format!("({i}::BIGINT, {}, {})", literal(r.0), literal(r.1)))
            .collect();
        let setup = format!(
            "CREATE TABLE t AS SELECT * FROM (VALUES {}) v(id, a, b);",
            values.join(", ")
        );
        for q in queries() {
            for ex in EXECS {
                let outcome = match (q.run)(&t, ex) {
                    Ok(rows) => {
                        let cells: Vec<String> = rows
                            .iter()
                            .map(|(id, v)| {
                                format!("{id}={}", v.map_or("NULL".to_string(), |v| v.to_string()))
                            })
                            .collect();
                        format!("rows:{}", cells.join(";"))
                    }
                    Err(c) => format!("error:{}", c.word()),
                };
                println!(
                    "{}\t{}\t{:?}\t{}\t{}\t{}",
                    t.name, q.name, ex, setup, q.sql, outcome
                );
            }
        }
    }
}
