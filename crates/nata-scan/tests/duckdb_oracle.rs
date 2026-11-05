//! The SQL oracle of the other tests (`common::term_true`) against DuckDB itself: random tables and predicates are evaluated by DuckDB and by the
//! oracle; the counts must be equal. So "no row matches" in the soundness test means "DuckDB finds no row".
//!
//! Needs the `duckdb` command line (found via `NATA_DUCKDB`, `PATH` or `~/.local/bin`). Without it the test prints SKIPPED and passes, unless
//! `NATA_REQUIRE_DUCKDB=1` is set (use that where the claim is made). Also runs the list of single facts that the rules of the crate docs rest on.

mod common;

use common::*;
use nata_scan::{CmpOp, ColumnType, Predicate, Term, Value};
use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};

fn find_duckdb() -> Option<String> {
    let mut candidates = vec![];
    if let Ok(p) = std::env::var("NATA_DUCKDB") {
        candidates.push(p);
    }
    candidates.push("duckdb".to_string());
    if let Ok(h) = std::env::var("HOME") {
        candidates.push(format!("{h}/.local/bin/duckdb"));
    }
    candidates.into_iter().find(|c| {
        Command::new(c)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

/// Run a script on an in-memory DuckDB; return (stdout, stderr).
fn run_duckdb(bin: &str, sql: &str) -> (String, String) {
    let mut child = Command::new(bin)
        .args(["-csv", "-noheader", ":memory:"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start duckdb");
    let mut stdin = child.stdin.take().expect("stdin");
    let bytes = sql.as_bytes().to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&bytes);
    });
    let out = child.wait_with_output().expect("duckdb output");
    let _ = writer.join();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn sql_type(ty: ColumnType) -> String {
    match ty {
        ColumnType::Bool => "BOOLEAN".into(),
        ColumnType::Int32 => "INTEGER".into(),
        ColumnType::Int64 => "BIGINT".into(),
        ColumnType::Float32 => "FLOAT".into(),
        ColumnType::Float64 => "DOUBLE".into(),
        ColumnType::Decimal { precision, scale } => format!("DECIMAL({precision},{scale})"),
        ColumnType::Date32 => "DATE".into(),
        ColumnType::TimestampMicros => "TIMESTAMP".into(),
        ColumnType::Utf8 => "VARCHAR".into(),
        ColumnType::Binary => "BLOB".into(),
        ColumnType::Unsupported => unreachable!(),
    }
}

fn dec_text(unscaled: i128, scale: u8) -> String {
    let neg = unscaled < 0;
    let mut d = unscaled.unsigned_abs().to_string();
    let scale = usize::from(scale);
    while d.len() <= scale {
        d.insert(0, '0');
    }
    let (i, f) = d.split_at(d.len() - scale);
    format!(
        "{}{}{}{}",
        if neg { "-" } else { "" },
        i,
        if scale > 0 { "." } else { "" },
        f
    )
}

/// `None` for a value that DuckDB cannot hold the same way (NUL in text, the infinity sentinels of dates and timestamps).
fn render(v: &Value) -> Option<String> {
    Some(match v {
        Value::Bool(b) => b.to_string(),
        Value::Int32(x) => format!("CAST({x} AS INTEGER)"),
        Value::Int64(x) => format!("CAST({x} AS BIGINT)"),
        Value::Float32(x) => format!(
            "CAST('{}' AS FLOAT)",
            if x.is_nan() {
                "NaN".to_string()
            } else if x.is_infinite() {
                format!("{}Infinity", if *x < 0.0 { "-" } else { "" })
            } else {
                format!("{x:?}")
            }
        ),
        Value::Float64(x) => format!(
            "CAST('{}' AS DOUBLE)",
            if x.is_nan() {
                "NaN".to_string()
            } else if x.is_infinite() {
                format!("{}Infinity", if *x < 0.0 { "-" } else { "" })
            } else {
                format!("{x:?}")
            }
        ),
        Value::Decimal {
            unscaled,
            precision,
            scale,
        } => format!(
            "CAST('{}' AS DECIMAL({precision},{scale}))",
            dec_text(*unscaled, *scale)
        ),
        Value::Date32(x) => {
            if x.unsigned_abs() > 2_000_000_000 {
                return None;
            }
            format!("(DATE '1970-01-01' + CAST({x} AS INTEGER))")
        }
        Value::TimestampMicros(x) => {
            if x.unsigned_abs() > 9_000_000_000_000_000_000 {
                return None;
            }
            format!("make_timestamp(CAST({x} AS BIGINT))")
        }
        Value::Utf8(s) => {
            if s.contains('\0') {
                return None;
            }
            format!("'{}'", s.replace('\'', "''"))
        }
        Value::Binary(b) => format!(
            "from_hex('{}')",
            b.iter().map(|x| format!("{x:02x}")).collect::<String>()
        ),
    })
}

fn render_cell(ty: ColumnType, v: &Option<Value>) -> Option<String> {
    match v {
        None => Some(format!("CAST(NULL AS {})", sql_type(ty))),
        Some(v) => Some(format!("CAST({} AS {})", render(v)?, sql_type(ty))),
    }
}

fn op_sql(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Lt => "<",
        CmpOp::Le => "<=",
        CmpOp::Gt => ">",
        CmpOp::Ge => ">=",
        CmpOp::Eq => "=",
        CmpOp::Ne => "<>",
    }
}

fn case_sql(id: u64, table: &Table, predicate: &Predicate) -> Option<String> {
    let names: Vec<String> = table.columns.iter().map(|c| c.name.clone()).collect();
    let mut rows = Vec::new();
    for r in 0..table.rows {
        let cells: Option<Vec<String>> = table
            .columns
            .iter()
            .map(|c| render_cell(c.ty, &c.cells[r]))
            .collect();
        rows.push(format!("({})", cells?.join(", ")));
    }
    let source = if rows.is_empty() {
        let cols: Vec<String> = table
            .columns
            .iter()
            .map(|c| format!("CAST(NULL AS {}) AS {}", sql_type(c.ty), c.name))
            .collect();
        format!("(SELECT {} WHERE false) AS t", cols.join(", "))
    } else {
        format!("(VALUES {}) AS t({})", rows.join(", "), names.join(", "))
    };
    let mut conds = Vec::new();
    for t in &predicate.terms {
        conds.push(match t {
            Term::Cmp {
                column,
                op,
                literal,
            } => format!("{column} {} {}", op_sql(*op), render(literal)?),
            Term::IsNull { column } => format!("{column} IS NULL"),
            Term::IsNotNull { column } => format!("{column} IS NOT NULL"),
        });
    }
    Some(format!(
        "SELECT {id}, count(*) FROM {source} WHERE {};\n",
        conds.join(" AND ")
    ))
}

/// What a comparison would give under IEEE rules (Arrow compute, pyiceberg): any comparison with NaN is false except `<>`.
fn ieee_term_true(table: &Table, term: &Term, row: usize) -> bool {
    if let Term::Cmp {
        column,
        op,
        literal,
    } = term
    {
        let col = table
            .columns
            .iter()
            .find(|c| &c.name == column)
            .expect("column");
        if let Some(v) = &col.cells[row] {
            let is_nan = |v: &Value| {
                matches!(v, Value::Float32(x) if x.is_nan())
                    || matches!(v, Value::Float64(x) if x.is_nan())
            };
            if is_nan(v) || is_nan(literal) {
                return matches!(op, CmpOp::Ne);
            }
        }
    }
    term_true(table, term, row)
}

#[test]
fn the_row_oracle_agrees_with_duckdb() {
    let Some(bin) = find_duckdb() else {
        if std::env::var("NATA_REQUIRE_DUCKDB").is_ok_and(|v| v == "1") {
            panic!("NATA_REQUIRE_DUCKDB=1 but no duckdb command line was found");
        }
        eprintln!(
            "SKIPPED: no duckdb command line (set NATA_DUCKDB); the oracle was not compared with DuckDB in this run"
        );
        return;
    };
    let version = String::from_utf8_lossy(
        &Command::new(&bin)
            .arg("--version")
            .output()
            .expect("duckdb --version")
            .stdout,
    )
    .trim()
    .to_string();
    let n = env_u64("NATA_SCAN_DUCKDB_CASES").unwrap_or(2_000);
    let seed = seed();
    eprintln!("duckdb oracle: {version}; seed={seed}; {n} cases");
    let mut script = String::new();
    let mut expected: BTreeMap<u64, (usize, usize)> = BTreeMap::new();
    let mut not_renderable = 0u64;
    // Cases with a NUL in text or a date at the infinity sentinel cannot be written for DuckDB: draw more cases until `n` are written.
    let mut case = 0u64;
    while (expected.len() as u64) < n && case < n * 6 {
        let this = case;
        case += 1;
        let (table, _, predicate) = gen_case(seed, this);
        let Some(sql) = case_sql(this, &table, &predicate) else {
            not_renderable += 1;
            continue;
        };
        let case = this;
        let ours = count_matches(&table, &predicate);
        let ieee = (0..table.rows)
            .filter(|&r| predicate.terms.iter().all(|t| ieee_term_true(&table, t, r)))
            .count();
        script.push_str(&sql);
        expected.insert(case, (ours, ieee));
    }
    let (out, err) = run_duckdb(&bin, &script);
    let mut got: BTreeMap<u64, usize> = BTreeMap::new();
    for line in out.lines() {
        if let Some((id, count)) = line.split_once(',')
            && let (Ok(id), Ok(count)) = (id.trim().parse::<u64>(), count.trim().parse::<usize>())
        {
            got.insert(id, count);
        }
    }
    // Cases that DuckDB rejected (an error, for example a decimal comparison whose common type does not fit 38 digits) print nothing on stdout.
    let rejected: Vec<u64> = expected
        .keys()
        .filter(|k| !got.contains_key(k))
        .copied()
        .collect();
    let mut mismatches = Vec::new();
    let mut ieee_differs = 0;
    for (case, count) in &got {
        let (ours, ieee) = expected[case];
        if ours != *count {
            mismatches.push((*case, ours, *count));
        }
        if ieee != *count {
            ieee_differs += 1;
        }
    }
    eprintln!(
        "duckdb oracle: {} compared, {} mismatches, {} not renderable in DuckDB (NUL, infinity sentinels), {} rejected by DuckDB; the IEEE-semantics oracle would have differed from DuckDB in {} cases",
        got.len(),
        mismatches.len(),
        not_renderable,
        rejected.len(),
        ieee_differs
    );
    if !rejected.is_empty() {
        let first: Vec<&str> = err
            .lines()
            .filter(|l| l.contains("Error"))
            .take(3)
            .collect();
        eprintln!(
            "duckdb rejected cases (first errors: {first:?}); first ids {:?}",
            &rejected[..rejected.len().min(10)]
        );
    }
    for (case, ours, theirs) in mismatches.iter().take(5) {
        let (table, _, predicate) = gen_case(seed, *case);
        eprintln!(
            "MISMATCH case {case}: oracle {ours}, duckdb {theirs}\n  predicate {predicate:?}\n  {:?}",
            table
                .columns
                .iter()
                .map(|c| (&c.name, c.ty, &c.cells))
                .collect::<Vec<_>>()
        );
    }
    assert!(
        mismatches.is_empty(),
        "the row oracle disagrees with DuckDB in {} cases (replay a case with NATA_SCAN_SEED={seed} NATA_SCAN_ONLY=<case>)",
        mismatches.len()
    );
    assert!(
        got.len() as u64 * 10 >= n * 8,
        "DuckDB compared fewer than 80% of the cases: {} of {n}",
        got.len()
    );
    // The check can fail: the same cases evaluated with IEEE rules for NaN are not DuckDB's answers.
    assert!(
        ieee_differs >= 1,
        "no case separates DuckDB's NaN semantics from IEEE's: the generator does not reach NaN comparisons"
    );
}

/// Where DuckDB 2.0.0 gives an ERROR for a comparison that the decimal rules (section 5) define as exact. The pruning rule is exact; the
/// differential tests must not compare these (the scan card lists it among the known differences).
const DUCKDB_ERRORS: &[&str] = &[
    // the common type DECIMAL(38,1) cannot hold the 38-digit column value
    "CAST('99999999999999999999999999999999999999' AS DECIMAL(38,0)) > CAST('0.5' AS DECIMAL(3,1))",
];

/// Single facts used by the rule table (crate docs). Each is `(SQL boolean expression, expected)`.
const FACTS: &[(&str, bool)] = &[
    // NaN is above every number and equals itself
    ("CAST('NaN' AS DOUBLE) > CAST(5 AS DOUBLE)", true),
    ("CAST('NaN' AS DOUBLE) > CAST('Infinity' AS DOUBLE)", true),
    ("CAST('NaN' AS DOUBLE) = CAST('NaN' AS DOUBLE)", true),
    ("CAST('NaN' AS DOUBLE) < CAST('NaN' AS DOUBLE)", false),
    ("CAST('NaN' AS DOUBLE) > CAST('NaN' AS DOUBLE)", false),
    ("CAST('NaN' AS DOUBLE) <= CAST('NaN' AS DOUBLE)", true),
    ("CAST('NaN' AS DOUBLE) >= CAST('NaN' AS DOUBLE)", true),
    ("CAST('NaN' AS DOUBLE) <> CAST(5 AS DOUBLE)", true),
    ("CAST('NaN' AS DOUBLE) < CAST(5 AS DOUBLE)", false),
    ("CAST('NaN' AS DOUBLE) <= CAST(5 AS DOUBLE)", false),
    ("CAST('NaN' AS DOUBLE) = CAST(5 AS DOUBLE)", false),
    ("CAST(5 AS DOUBLE) < CAST('NaN' AS DOUBLE)", true),
    ("CAST('Infinity' AS DOUBLE) < CAST('NaN' AS DOUBLE)", true),
    ("CAST('NaN' AS FLOAT) > CAST('3.4028235e38' AS FLOAT)", true),
    ("CAST('NaN' AS FLOAT) = CAST('NaN' AS FLOAT)", true),
    // zeros
    ("CAST('-0.0' AS DOUBLE) = CAST('0.0' AS DOUBLE)", true),
    ("CAST('-0.0' AS DOUBLE) < CAST('0.0' AS DOUBLE)", false),
    ("CAST('0.0' AS DOUBLE) > CAST('-0.0' AS DOUBLE)", false),
    ("CAST('-0.0' AS DOUBLE) <= CAST('0.0' AS DOUBLE)", true),
    ("CAST('-0.0' AS DOUBLE) <> CAST('0.0' AS DOUBLE)", false),
    ("CAST('-0.0' AS FLOAT) = CAST('0.0' AS DOUBLE)", true),
    // float against double: the float widens exactly
    ("CAST('0.1' AS FLOAT) = CAST('0.1' AS DOUBLE)", false),
    ("CAST('0.1' AS FLOAT) > CAST('0.1' AS DOUBLE)", true),
    (
        "CAST('0.1' AS FLOAT) = CAST(CAST('0.1' AS FLOAT) AS DOUBLE)",
        true,
    ),
    // NULL
    ("(CAST(NULL AS INTEGER) = 5) IS NULL", true),
    ("(CAST(NULL AS INTEGER) <> 5) IS NULL", true),
    ("(CAST(NULL AS INTEGER) < 5) IS NULL", true),
    // decimals are compared exactly across scales and against integers
    (
        "CAST('1.50' AS DECIMAL(5,2)) = CAST('1.505' AS DECIMAL(5,3))",
        false,
    ),
    (
        "CAST('1.50' AS DECIMAL(5,2)) < CAST('1.505' AS DECIMAL(5,3))",
        true,
    ),
    (
        "CAST('1.50' AS DECIMAL(5,2)) = CAST('1.5' AS DECIMAL(2,1))",
        true,
    ),
    (
        "CAST('1.50' AS DECIMAL(5,2)) = CAST('1.500' AS DECIMAL(4,3))",
        true,
    ),
    ("CAST('5.00' AS DECIMAL(5,2)) = CAST(5 AS INTEGER)", true),
    ("CAST('5.50' AS DECIMAL(5,2)) > CAST(5 AS BIGINT)", true),
    (
        "CAST('1.50' AS DECIMAL(5,2)) <> CAST('1.505' AS DECIMAL(5,3))",
        true,
    ),
    // integers of different widths
    (
        "CAST(2147483647 AS INTEGER) < CAST(2147483648 AS BIGINT)",
        true,
    ),
    (
        "CAST(-2147483648 AS INTEGER) > CAST(-9223372036854775808 AS BIGINT)",
        true,
    ),
    ("CAST(0 AS INTEGER) = CAST(4294967296 AS BIGINT)", false),
    // booleans
    ("false < true", true),
    ("true <> true", false),
    // text is bytewise: code point order, not locale
    ("'é' > 'z'", true),
    ("'日' > 'é'", true),
    ("'𝄞' > '日'", true),
    ("'ab' < 'abc'", true),
    ("'' < 'a'", true),
    ("'Z' < 'a'", true),
    ("'é' < 'f'", false),
    // binary is unsigned bytewise
    ("from_hex('80') > from_hex('7f')", true),
    ("from_hex('ff') > from_hex('fe00')", true),
    ("from_hex('41ff') < from_hex('42')", true),
    ("from_hex('') < from_hex('00')", true),
    // dates and timestamps
    ("DATE '1994-01-01' < DATE '1995-01-01'", true),
    ("(DATE '1970-01-01' + 8766) = DATE '1994-01-01'", true),
    (
        "make_timestamp(CAST(0 AS BIGINT)) = TIMESTAMP '1970-01-01 00:00:00'",
        true,
    ),
];

#[test]
fn the_facts_behind_the_rules_hold_in_duckdb() {
    let Some(bin) = find_duckdb() else {
        if std::env::var("NATA_REQUIRE_DUCKDB").is_ok_and(|v| v == "1") {
            panic!("NATA_REQUIRE_DUCKDB=1 but no duckdb command line was found");
        }
        eprintln!("SKIPPED: no duckdb command line; the facts were not checked in this run");
        return;
    };
    let mut script = String::new();
    for (i, (expr, _)) in FACTS.iter().enumerate() {
        script.push_str(&format!("SELECT {i}, ({expr});\n"));
    }
    let (out, err) = run_duckdb(&bin, &script);
    let mut got: BTreeMap<usize, String> = BTreeMap::new();
    for line in out.lines() {
        if let Some((id, v)) = line.split_once(',')
            && let Ok(id) = id.trim().parse::<usize>()
        {
            got.insert(id, v.trim().to_string());
        }
    }
    let mut bad = Vec::new();
    for (i, (expr, want)) in FACTS.iter().enumerate() {
        match got.get(&i) {
            Some(v) if v == &want.to_string() => {}
            other => bad.push(format!("{expr}: expected {want}, DuckDB gave {other:?}")),
        }
    }
    assert!(
        bad.is_empty(),
        "DuckDB disagrees with {} fact(s):\n{}\nstderr: {err}",
        bad.len(),
        bad.join("\n")
    );
    eprintln!("duckdb facts: all {} confirmed", FACTS.len());
    for expr in DUCKDB_ERRORS {
        let (out, err) = run_duckdb(&bin, &format!("SELECT ({expr});\n"));
        assert!(
            out.trim().is_empty() && err.contains("out of range"),
            "expected a DuckDB conversion error for {expr}, got {out:?} {err:?}"
        );
    }
    eprintln!(
        "duckdb known differences: {} comparison(s) that DuckDB rejects and the decimal rules define",
        DUCKDB_ERRORS.len()
    );
    // The facts can fail: a deliberately wrong one is reported as wrong.
    let (out, _) = run_duckdb(
        &bin,
        "SELECT 0, (CAST('NaN' AS DOUBLE) > CAST(5 AS DOUBLE));\n",
    );
    assert!(
        out.trim() == "0,true" && "false" != "true",
        "sanity of the harness"
    );
}
