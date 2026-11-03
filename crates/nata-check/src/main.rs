//! `nata-check`: differential correctness checker.
//!
//! Runs every `*.sql` file in a directory through a reference engine
//! (DuckDB) and through Nata, then compares the output. A query passes only
//! if the outputs agree; anything else is reported, and the exit code is
//! non-zero.
//!
//! Conventions both engines must follow:
//! - Reference: `duckdb -csv -c <sql>`
//! - Candidate: `nata sql --csv -c <sql>`
//! - `{data}` in a query is replaced by the absolute data directory.
//!
//! Per-query directives, in `--` comment lines at the top of the `.sql` file:
//! - `-- expect-error`: both engines must fail; Nata must not answer with a result.
//! - `-- expect-error-nata: <text>`: only Nata is run, and it must fail with a
//!   message containing `<text>` (case-insensitive). For deliberate differences
//!   from DuckDB (see `docs/decimal-rules.md`).
//! - `-- tolerance: <relative>`: cells that parse as numbers are compared within
//!   `<relative>` of `max(1, |a|, |b|)`; `0` compares values and ignores formatting
//!   (`3.5` equals `3.500000`). Other cells must match exactly. Rows are sorted as
//!   text when there is no `ORDER BY`, so a tolerance query with several rows
//!   needs one.
//!
//! A malformed directive is an error, never a silently ignored comment.
//!
//! Known limits (not yet handled): column types (only the text of each cell is
//! compared), and queries with several statements.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output};

use anyhow::{Context, Result, bail};
use clap::Parser;

#[derive(Parser)]
#[command(version, about = "Compare Nata's query results with DuckDB's")]
struct Args {
    /// Directory holding the data files (substituted for `{data}` in queries).
    #[arg(long)]
    data: PathBuf,
    /// Directory of `*.sql` files, one query per file.
    #[arg(long)]
    queries: PathBuf,
    /// Reference engine binary.
    #[arg(long, default_value = "duckdb")]
    reference: PathBuf,
    /// Nata binary. Defaults to the `nata` next to this executable.
    #[arg(long)]
    candidate: Option<PathBuf>,
}

#[derive(Debug, PartialEq)]
enum Outcome {
    Pass,
    Mismatch(String),
    /// Nata answered `not supported yet`: the loud, correct failure.
    Unsupported(String),
    CandidateError(String),
    /// The reference engine failed, so the query itself is suspect.
    ReferenceError(String),
}

impl Outcome {
    fn label(&self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Mismatch(_) => "MISMATCH",
            Self::Unsupported(_) => "UNSUPPORTED",
            Self::CandidateError(_) => "ERROR",
            Self::ReferenceError(_) => "REFERENCE-ERROR",
        }
    }

    fn detail(&self) -> Option<&str> {
        match self {
            Self::Pass => None,
            Self::Mismatch(s)
            | Self::Unsupported(s)
            | Self::CandidateError(s)
            | Self::ReferenceError(s) => Some(s),
        }
    }
}

/// Per-query settings read from the leading comment lines.
#[derive(Debug, Default, PartialEq)]
struct Directives {
    expect_error: bool,
    expect_error_nata: Option<String>,
    tolerance: Option<f64>,
}

/// Read the directives from the leading `--` lines and return them with the
/// SQL that remains. Ordinary comments stay in the SQL.
fn parse_directives(sql: &str) -> Result<(Directives, String), String> {
    let mut d = Directives::default();
    let mut kept = Vec::new();
    let mut in_header = true;
    for line in sql.lines() {
        let t = line.trim();
        if in_header && t.starts_with("--") {
            let body = t.trim_start_matches('-').trim();
            if body == "expect-error" {
                d.expect_error = true;
                continue;
            }
            if let Some(rest) = body.strip_prefix("expect-error-nata:") {
                let wanted = rest.trim();
                if wanted.is_empty() {
                    return Err("`expect-error-nata:` needs the text to look for".into());
                }
                d.expect_error_nata = Some(wanted.to_string());
                continue;
            }
            if let Some(rest) = body.strip_prefix("tolerance:") {
                match rest.trim().parse::<f64>() {
                    Ok(v) if v >= 0.0 && v.is_finite() => d.tolerance = Some(v),
                    _ => return Err(format!("bad tolerance {:?}", rest.trim())),
                }
                continue;
            }
            if body.starts_with("expect") || body.starts_with("tolerance") {
                return Err(format!("unknown directive {body:?}"));
            }
            kept.push(line);
            continue;
        }
        if !t.is_empty() {
            in_header = false;
        }
        kept.push(line);
    }
    if d.expect_error && d.expect_error_nata.is_some() {
        return Err("`expect-error` and `expect-error-nata:` cannot be combined".into());
    }
    Ok((d, kept.join("\n")))
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("nata-check: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run(args: Args) -> Result<bool> {
    let data = args
        .data
        .canonicalize()
        .with_context(|| format!("data directory {}", args.data.display()))?;
    let candidate = match args.candidate {
        Some(c) => c,
        None => default_candidate()?,
    };

    let queries = list_queries(&args.queries)?;
    if queries.is_empty() {
        bail!("no *.sql files in {}", args.queries.display());
    }

    let mut counts = [0usize; 5];
    for path in &queries {
        let name = path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let sql =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let (directives, sql) =
            parse_directives(&sql).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        let sql = substitute_data(&sql, &data);
        let outcome = check_query(&args.reference, &candidate, &sql, &directives);

        let slot = match outcome {
            Outcome::Pass => 0,
            Outcome::Mismatch(_) => 1,
            Outcome::Unsupported(_) => 2,
            Outcome::CandidateError(_) => 3,
            Outcome::ReferenceError(_) => 4,
        };
        counts[slot] += 1;

        match outcome.detail() {
            Some(d) => println!("{:<16} {name}\n    {d}", outcome.label()),
            None => println!("{:<16} {name}", outcome.label()),
        }
    }

    let [pass, mismatch, unsupported, error, reference_error] = counts;
    println!(
        "\n{} queries: {pass} pass, {mismatch} mismatch, {unsupported} unsupported, \
         {error} error, {reference_error} reference-error",
        queries.len()
    );
    Ok(pass == queries.len())
}

fn default_candidate() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("locating own executable")?;
    let dir = exe.parent().context("executable has no parent directory")?;
    Ok(dir.join("nata"))
}

fn list_queries(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("queries directory {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "sql"))
        .collect();
    files.sort();
    Ok(files)
}

fn substitute_data(sql: &str, data: &Path) -> String {
    sql.replace("{data}", &data.to_string_lossy())
}

/// What one engine did with a query.
enum Run {
    Rows(String),
    Failed(String),
    Launch(String),
}

fn run_engine(program: &Path, args: &[&str]) -> Run {
    match exec(program, args) {
        Ok(out) if out.status.success() => {
            Run::Rows(String::from_utf8_lossy(&out.stdout).into_owned())
        }
        Ok(out) => Run::Failed(first_line(&out.stderr)),
        Err(e) => Run::Launch(format!("{e:#}")),
    }
}

fn check_query(reference: &Path, candidate: &Path, sql: &str, d: &Directives) -> Outcome {
    let run_candidate = || run_engine(candidate, &["sql", "--csv", "-c", sql]);

    // Deliberate difference: the reference is not consulted.
    if let Some(wanted) = &d.expect_error_nata {
        return match run_candidate() {
            Run::Rows(_) => Outcome::Mismatch(format!(
                "expected an error mentioning {wanted:?}, but the candidate returned a result"
            )),
            Run::Failed(m) if m.contains("not supported yet") => Outcome::Unsupported(m),
            Run::Failed(m) if m.to_lowercase().contains(&wanted.to_lowercase()) => Outcome::Pass,
            Run::Failed(m) => Outcome::Mismatch(format!("error does not mention {wanted:?}: {m}")),
            Run::Launch(m) => Outcome::CandidateError(m),
        };
    }

    let reference_run = run_engine(reference, &["-csv", "-c", sql]);

    // Both engines must fail.
    if d.expect_error {
        match reference_run {
            Run::Failed(_) => {}
            Run::Rows(_) => {
                return Outcome::ReferenceError(
                    "expected the reference to fail, but it returned a result".into(),
                );
            }
            Run::Launch(m) => return Outcome::ReferenceError(m),
        }
        return match run_candidate() {
            Run::Rows(_) => {
                Outcome::Mismatch("the reference fails but the candidate returned a result".into())
            }
            Run::Failed(m) if m.contains("not supported yet") => Outcome::Unsupported(m),
            Run::Failed(_) => Outcome::Pass,
            Run::Launch(m) => Outcome::CandidateError(m),
        };
    }

    let expected = match reference_run {
        Run::Rows(text) => text,
        Run::Failed(m) | Run::Launch(m) => return Outcome::ReferenceError(m),
    };
    let actual = match run_candidate() {
        Run::Rows(text) => text,
        Run::Failed(m) if m.contains("not supported yet") => return Outcome::Unsupported(m),
        Run::Failed(m) | Run::Launch(m) => return Outcome::CandidateError(m),
    };

    let ordered = has_top_level_order_by(sql);
    match compare(&expected, &actual, ordered, d.tolerance) {
        None => Outcome::Pass,
        Some(diff) => Outcome::Mismatch(diff),
    }
}

fn exec(program: &Path, args: &[&str]) -> Result<Output> {
    Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("running {}", program.display()))
}

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("(no message)")
        .to_string()
}

/// Compare two CSV outputs. Returns `None` when they agree, otherwise a
/// short description of the first difference. Row order only matters when
/// the query has a top-level `ORDER BY`.
fn compare(expected: &str, actual: &str, ordered: bool, tolerance: Option<f64>) -> Option<String> {
    let mut exp = split_records(expected);
    let mut act = split_records(actual);

    if exp.first() != act.first() {
        return Some(format!(
            "header differs: expected {:?}, got {:?}",
            exp.first(),
            act.first()
        ));
    }
    // Headers agree (both present, or both absent); drop them.
    if !exp.is_empty() {
        exp.remove(0);
        act.remove(0);
    }

    if !ordered {
        exp.sort();
        act.sort();
    }
    if exp.len() != act.len() {
        return Some(format!(
            "row count differs: expected {}, got {}",
            exp.len(),
            act.len()
        ));
    }
    exp.iter()
        .zip(&act)
        .position(|(e, a)| !rows_agree(e, a, tolerance))
        .map(|i| {
            format!(
                "row {} differs: expected {:?}, got {:?}",
                i + 1,
                exp[i],
                act[i]
            )
        })
}

/// Rows agree when they are equal, or, with a tolerance, when every cell is
/// equal or both parse as numbers within the relative tolerance.
fn rows_agree(expected: &str, actual: &str, tolerance: Option<f64>) -> bool {
    if expected == actual {
        return true;
    }
    let Some(tol) = tolerance else { return false };
    let (e, a) = (split_cells(expected), split_cells(actual));
    e.len() == a.len() && e.iter().zip(&a).all(|(x, y)| cells_agree(x, y, tol))
}

fn cells_agree(expected: &str, actual: &str, tol: f64) -> bool {
    if expected == actual {
        return true;
    }
    match (expected.trim().parse::<f64>(), actual.trim().parse::<f64>()) {
        (Ok(x), Ok(y)) => (x - y).abs() <= tol * x.abs().max(y.abs()).max(1.0),
        _ => false,
    }
}

/// Split one CSV record into cells, keeping commas that sit inside quotes.
fn split_cells(record: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in record.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            ',' if !in_quotes => cells.push(std::mem::take(&mut current)),
            _ => current.push(ch),
        }
    }
    cells.push(current);
    cells
}

/// Split CSV text into records, keeping newlines that sit inside quotes.
fn split_records(text: &str) -> Vec<String> {
    let mut records = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in text.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            '\n' if !in_quotes => records.push(std::mem::take(&mut current)),
            '\r' if !in_quotes => {}
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        records.push(current);
    }
    records
}

/// True if the statement has an `ORDER BY` outside any parentheses and
/// string literals. An `ORDER BY` inside `OVER (...)` or a subquery does
/// not order the final result.
fn has_top_level_order_by(sql: &str) -> bool {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut top_level = String::new();
    for ch in sql.chars() {
        match ch {
            '\'' => in_string = !in_string,
            _ if in_string => {}
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => top_level.push(ch.to_ascii_lowercase()),
            _ => {}
        }
    }
    let words: Vec<&str> = top_level.split_whitespace().collect();
    words.windows(2).any(|w| w[0] == "order" && w[1] == "by")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_outputs_agree() {
        assert_eq!(
            compare("a,b\n1,2\n3,4\n", "a,b\n1,2\n3,4\n", true, None),
            None
        );
    }

    #[test]
    fn unordered_queries_ignore_row_order() {
        assert_eq!(compare("a\n1\n2\n", "a\n2\n1\n", false, None), None);
    }

    #[test]
    fn ordered_queries_notice_row_order() {
        let diff = compare("a\n1\n2\n", "a\n2\n1\n", true, None);
        assert!(diff.is_some_and(|d| d.contains("row 1 differs")));
    }

    #[test]
    fn header_difference_is_reported() {
        let diff = compare("a\n1\n", "b\n1\n", false, None);
        assert!(diff.is_some_and(|d| d.contains("header differs")));
    }

    #[test]
    fn row_count_difference_is_reported() {
        let diff = compare("a\n1\n2\n", "a\n1\n", false, None);
        assert!(diff.is_some_and(|d| d.contains("row count differs")));
    }

    #[test]
    fn empty_result_with_header_agrees() {
        assert_eq!(compare("a\n", "a\n", false, None), None);
    }

    #[test]
    fn null_text_must_match() {
        assert!(compare("a\nNULL\n", "a\n\n", false, None).is_some());
    }

    #[test]
    fn quoted_newlines_stay_in_one_record() {
        let records = split_records("a\n\"x\ny\"\n");
        assert_eq!(records, vec!["a".to_string(), "\"x\ny\"".to_string()]);
    }

    #[test]
    fn order_by_detection() {
        assert!(has_top_level_order_by("SELECT a FROM t ORDER BY a"));
        assert!(has_top_level_order_by("select a from t\norder  by a"));
        assert!(!has_top_level_order_by("SELECT a FROM t"));
        assert!(!has_top_level_order_by(
            "SELECT row_number() OVER (ORDER BY a) FROM t"
        ));
        assert!(!has_top_level_order_by("SELECT 'order by' FROM t"));
    }

    #[test]
    fn tolerance_accepts_close_numbers_and_rejects_far_ones() {
        let exact = "v\n0.33333333333\n";
        let rounded = "v\n0.33333333\n";
        assert_eq!(compare(exact, rounded, false, Some(1e-7)), None);
        assert!(compare(exact, rounded, false, Some(1e-9)).is_some());
        assert!(compare(exact, rounded, false, None).is_some());
    }

    #[test]
    fn zero_tolerance_ignores_formatting_only() {
        assert_eq!(compare("v\n3.5\n", "v\n3.500000\n", false, Some(0.0)), None);
        assert!(compare("v\n3.5\n", "v\n3.6\n", false, Some(0.0)).is_some());
    }

    #[test]
    fn tolerance_does_not_apply_to_text_cells() {
        assert!(compare("v\nabc\n", "v\nabd\n", false, Some(1.0)).is_some());
        assert_eq!(
            compare("n,v\nabc,1.0\n", "n,v\nabc,1.00\n", false, Some(0.0)),
            None
        );
    }

    #[test]
    fn cells_split_outside_quotes_only() {
        assert_eq!(split_cells("a,\"b,c\",d"), vec!["a", "\"b,c\"", "d"]);
        assert_eq!(split_cells(""), vec![""]);
    }

    #[test]
    fn directives_are_parsed_and_removed() {
        let sql = "-- expect-error\n-- note for humans\nSELECT 1";
        let (d, rest) = parse_directives(sql).expect("valid");
        assert!(d.expect_error);
        assert_eq!(rest, "-- note for humans\nSELECT 1");

        let (d, _) =
            parse_directives("-- expect-error-nata: division by zero\nSELECT 1").expect("valid");
        assert_eq!(d.expect_error_nata.as_deref(), Some("division by zero"));

        let (d, _) = parse_directives("\n-- tolerance: 1e-7\nSELECT 1").expect("valid");
        assert_eq!(d.tolerance, Some(1e-7));
    }

    #[test]
    fn directives_after_the_first_statement_line_are_ordinary_comments() {
        let (d, rest) = parse_directives("SELECT 1\n-- expect-error\n").expect("valid");
        assert_eq!(d, Directives::default());
        assert!(rest.contains("-- expect-error"));
    }

    #[test]
    fn malformed_directives_are_errors() {
        assert!(parse_directives("-- tolerance: abc\nSELECT 1").is_err());
        assert!(parse_directives("-- tolerance: -1\nSELECT 1").is_err());
        assert!(parse_directives("-- expect-error-nata:\nSELECT 1").is_err());
        assert!(parse_directives("-- expect-eror\nSELECT 1").is_err());
        assert!(parse_directives("-- expect-error\n-- expect-error-nata: x\nSELECT 1").is_err());
    }

    #[test]
    fn data_placeholder_is_substituted() {
        let sql = substitute_data("SELECT * FROM '{data}/t.parquet'", Path::new("/d"));
        assert_eq!(sql, "SELECT * FROM '/d/t.parquet'");
    }
}
