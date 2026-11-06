//! Shared helpers of the integration tests: DuckDB and pyarrow as writers and as the oracle, a scan to CSV text, a CSV comparison like nata-check's.
#![allow(dead_code, reason = "each test file uses a different part")]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

pub mod data;
pub mod faults;
pub mod genfiles;
pub mod writers;

use std::cell::Cell;
use std::sync::Arc;

use nata_parquet::csv;
use nata_parquet::{
    ParallelConfig, ScanError, ScanOptions, ScanPlan, ScanReport, run_parallel, run_serial,
};

/// How `scan` runs. `threads == 0`: the single-thread reference (`run_serial`). Otherwise `run_parallel` with that many workers.
#[derive(Clone, Copy, Debug)]
pub struct Mode {
    pub threads: usize,
    pub unordered: bool,
}

thread_local! {
    static MODE: Cell<Option<Mode>> = const { Cell::new(None) };
}

/// The mode of this test thread: set by [`with_mode`], else from `NATA_SCAN_THREADS` (a number; 0 or unset = serial) and `NATA_SCAN_UNORDERED=1`.
pub fn mode() -> Mode {
    MODE.get().unwrap_or_else(|| Mode {
        threads: std::env::var("NATA_SCAN_THREADS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        unordered: std::env::var("NATA_SCAN_UNORDERED").is_ok_and(|v| v == "1"),
    })
}

pub fn with_mode<R>(m: Mode, f: impl FnOnce() -> R) -> R {
    let old = MODE.replace(Some(m));
    let r = f();
    MODE.set(old);
    r
}

pub fn find_duckdb() -> Option<String> {
    static FOUND: OnceLock<Option<String>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
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
        })
        .clone()
}

/// The `duckdb` command line, or a panic when `NATA_REQUIRE_DUCKDB=1`; `None` (the test prints SKIPPED) otherwise.
pub fn duckdb_or_skip(test: &str) -> Option<String> {
    let d = find_duckdb();
    if d.is_none() {
        assert!(
            std::env::var("NATA_REQUIRE_DUCKDB").is_err(),
            "NATA_REQUIRE_DUCKDB is set and there is no duckdb command line"
        );
        eprintln!("SKIPPED {test}: no duckdb command line (NATA_DUCKDB, PATH, ~/.local/bin)");
    }
    d
}

/// Run SQL in DuckDB (`-csv`, with header), return stdout; panics with the SQL and stderr if it fails.
pub fn duckdb_csv(bin: &str, sql: &str) -> String {
    let out = Command::new(bin)
        .args(["-csv", "-c", sql])
        .output()
        .expect("start duckdb");
    assert!(
        out.status.success(),
        "duckdb failed on {sql}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn duckdb_try(bin: &str, sql: &str) -> Result<String, String> {
    let out = Command::new(bin)
        .args(["-csv", "-c", sql])
        .output()
        .expect("start duckdb");
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

/// A fresh directory under the system temp dir, removed first. Tests use their own name so that they can run in parallel.
pub fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("nata-parquet-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// A directory that survives between runs (generated data that is slow to make); keyed by name only.
pub fn cache_dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("nata-parquet-cache-{name}"));
    std::fs::create_dir_all(&dir).expect("create cache dir");
    dir
}

#[derive(Debug)]
pub struct Scan {
    pub csv: String,
    pub report: ScanReport,
}

/// Scan with the library and render the CSV like `nata scan --csv` does.
pub fn scan(
    files: &[&Path],
    columns: Option<&[&str]>,
    filters: &[&str],
    prune: bool,
    limit: Option<u64>,
) -> Result<Scan, ScanError> {
    let options = ScanOptions {
        columns: columns.map(|c| c.iter().map(|s| (*s).to_string()).collect()),
        filters: filters.iter().map(|s| (*s).to_string()).collect(),
        prune,
    };
    let paths: Vec<PathBuf> = files.iter().map(|p| p.to_path_buf()).collect();
    let plan = ScanPlan::open(&paths, &options)?;
    let mut out: Vec<u8> = Vec::new();
    let names: Vec<String> = plan.out_schema().iter().map(|(n, _)| n.clone()).collect();
    csv::write_header(&mut out, &names).expect("header");
    let m = mode();
    let report = if m.threads == 0 {
        run_serial(&plan, limit, |b| csv::write_batch(&mut out, b))?
    } else {
        let cfg = ParallelConfig {
            ordered: !m.unordered,
            ..ParallelConfig::new(m.threads)
        };
        run_parallel(Arc::new(plan), cfg, limit, |b| {
            csv::write_batch(&mut out, b)
        })?
    };
    Ok(Scan {
        csv: String::from_utf8(out).expect("csv is utf-8"),
        report,
    })
}

/// Split CSV text into records (newlines inside quotes stay), like nata-check.
pub fn records(text: &str) -> Vec<String> {
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

/// None if the two outputs hold the same rows (sorted) and the same header; otherwise the first difference.
pub fn diff_unordered(expected: &str, actual: &str) -> Option<String> {
    let (mut e, mut a) = (records(expected), records(actual));
    if e.first() != a.first() {
        return Some(format!(
            "header differs: expected {:?}, got {:?}",
            e.first(),
            a.first()
        ));
    }
    if !e.is_empty() {
        e.remove(0);
        a.remove(0);
    }
    e.sort();
    a.sort();
    if e.len() != a.len() {
        return Some(format!(
            "row count differs: expected {}, got {}",
            e.len(),
            a.len()
        ));
    }
    e.iter().zip(&a).position(|(x, y)| x != y).map(|i| {
        format!(
            "row {} (sorted) differs: expected {:?}, got {:?}",
            i + 1,
            e[i],
            a[i]
        )
    })
}

pub fn diff_ordered(expected: &str, actual: &str) -> Option<String> {
    let (e, a) = (records(expected), records(actual));
    if e.len() != a.len() {
        return Some(format!(
            "record count differs: expected {}, got {}",
            e.len(),
            a.len()
        ));
    }
    e.iter().zip(&a).position(|(x, y)| x != y).map(|i| {
        format!(
            "record {} differs: expected {:?}, got {:?}",
            i + 1,
            e[i],
            a[i]
        )
    })
}

/// pyarrow through uv (`uvx --offline --with pyarrow python`); `None` if it is not available. The script is passed as a file path, with `-I` (isolated).
pub fn pyarrow_run(script_dir: &Path, script: &str, args: &[&str]) -> Option<Result<(), String>> {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    let available = *AVAILABLE.get_or_init(|| {
        Command::new("uvx")
            .args([
                "--offline",
                "--with",
                "pyarrow",
                "python",
                "-I",
                "-c",
                "import pyarrow",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    });
    if !available {
        assert!(
            std::env::var("NATA_REQUIRE_PYARROW").is_err(),
            "NATA_REQUIRE_PYARROW is set and pyarrow is not available through uvx"
        );
        return None;
    }
    let path = script_dir.join("script.py");
    std::fs::write(&path, script).expect("write script");
    let out = Command::new("uvx")
        .args(["--offline", "--with", "pyarrow", "python", "-I"])
        .arg(&path)
        .args(args)
        .output()
        .expect("start uvx");
    Some(if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    })
}

// ------------------------------------------------------------------------------------------------------------------------------ the case matrix

/// What a case expects of pruning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    /// Not asserted (the scan must still agree with DuckDB).
    Any,
    /// Designed NOT to prune: exactly zero row groups skipped.
    Zero,
    /// Designed to prune: at least this many skipped.
    AtLeast(u64),
    Exactly(u64),
    /// Everything pruned.
    All,
}

/// A filter as the scan command takes it, and the same filter as DuckDB SQL.
#[derive(Clone, Debug)]
pub struct Filter {
    pub scan: String,
    pub sql: String,
}

pub fn flt(column: &str, op: &str, scan_literal: &str, sql_literal: &str) -> Filter {
    Filter {
        scan: format!("{column} {op} {scan_literal}"),
        sql: format!("{column} {op} {sql_literal}"),
    }
}

pub fn is_null(column: &str) -> Filter {
    Filter {
        scan: format!("{column} IS NULL"),
        sql: format!("{column} IS NULL"),
    }
}

pub fn is_not_null(column: &str) -> Filter {
    Filter {
        scan: format!("{column} IS NOT NULL"),
        sql: format!("{column} IS NOT NULL"),
    }
}

pub const OPS: [&str; 6] = ["<", "<=", ">", ">=", "=", "<>"];

/// Run many queries in ONE duckdb process (starting it and opening the files costs more than the queries) and split the output by markers.
/// Falls back to one process per query if the batch fails, so that the failing SQL is named.
pub fn duckdb_batch(bin: &str, sqls: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(sqls.len());
    for chunk in sqls.chunks(150) {
        let mut script = String::new();
        for (i, q) in chunk.iter().enumerate() {
            script.push_str(&format!("SELECT '@@NATA-CASE-{i}@@' AS m;\n{q};\n"));
        }
        let o = Command::new(bin)
            .args(["-csv", "-c", &script])
            .output()
            .expect("start duckdb");
        let text = String::from_utf8_lossy(&o.stdout).into_owned();
        let ok = o.status.success()
            && (0..chunk.len()).all(|i| text.contains(&format!("m\n@@NATA-CASE-{i}@@\n")));
        if !ok {
            for q in chunk {
                out.push(duckdb_csv(bin, q));
            }
            continue;
        }
        let mut rest = text.as_str();
        let mut parts: Vec<&str> = Vec::new();
        for i in 0..=chunk.len() {
            let marker = format!("m\n@@NATA-CASE-{i}@@\n");
            match rest.find(&marker) {
                Some(p) if i == 0 => rest = &rest[p + marker.len()..],
                Some(p) => {
                    parts.push(&rest[..p]);
                    rest = &rest[p + marker.len()..];
                }
                None => parts.push(rest),
            }
        }
        out.extend(parts.into_iter().map(str::to_string));
    }
    out
}

/// One case: a projection, filters, and what pruning is expected to do.
pub struct Spec {
    pub columns: Option<Vec<String>>,
    pub filters: Vec<Filter>,
    pub expect: Skip,
    pub what: String,
}

/// The rows DuckDB returns when NO statistics are used: the filter is applied above a LIMIT, which DuckDB cannot push into the Parquet reader. (DuckDB 2.0.0's
/// own row-group filtering trusts statistics that leave NaN out, so with a file from pyarrow its plain `WHERE d > 5000` skips a row group that holds NaN,
/// and NaN is greater than 5000. The barrier makes the oracle independent of that; the difference is counted and reported.)
fn oracle_sql(
    files: &[&Path],
    columns: Option<&[String]>,
    filters: &[Filter],
    count_only: bool,
    barrier: bool,
) -> String {
    let list = files
        .iter()
        .map(|f| format!("'{}'", f.display()))
        .collect::<Vec<_>>()
        .join(", ");
    let select = if count_only {
        "count(*)".to_string()
    } else {
        columns.map_or("*".to_string(), |c| {
            c.iter()
                .map(|n| format!("\"{n}\""))
                .collect::<Vec<_>>()
                .join(", ")
        })
    };
    let wh = if filters.is_empty() {
        String::new()
    } else {
        format!(
            " WHERE {}",
            filters
                .iter()
                .map(|f| f.sql.clone())
                .collect::<Vec<_>>()
                .join(" AND ")
        )
    };
    let source = if barrier {
        format!("(SELECT * FROM read_parquet([{list}]) LIMIT 9000000000000000000)")
    } else {
        format!("read_parquet([{list}])")
    };
    format!("SELECT {select} FROM {source}{wh}")
}

static PUSHDOWN_DIFFERENCES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The cases where DuckDB's own answer (with its statistics-based row-group skipping) differed from its answer without; printed at the end of a test.
pub fn pushdown_differences() -> Vec<String> {
    PUSHDOWN_DIFFERENCES
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

/// Check many cases against DuckDB: the scan (pruned and unpruned) must return DuckDB's rows IN ORDER, report invariants must hold, and the skip count must
/// be as expected. Returns the reports of the pruned scans.
pub fn check_cases(duck: &str, files: &[&Path], specs: &[Spec]) -> Vec<ScanReport> {
    let mut sqls = Vec::new();
    for s in specs {
        let cols = s.columns.as_deref();
        sqls.push(oracle_sql(files, cols, &s.filters, false, true));
        sqls.push(oracle_sql(files, cols, &s.filters, true, true));
        sqls.push(oracle_sql(files, cols, &s.filters, true, false));
    }
    let answers = duckdb_batch(duck, &sqls);
    let mut reports = Vec::new();
    for (i, s) in specs.iter().enumerate() {
        let (want, barrier_count, pushed_count) =
            (&answers[3 * i], &answers[3 * i + 1], &answers[3 * i + 2]);
        if barrier_count != pushed_count {
            let note = format!(
                "{} [{}]: DuckDB with its own row-group skipping counted {:?}, without {:?}",
                s.what,
                s.filters
                    .iter()
                    .map(|f| f.sql.as_str())
                    .collect::<Vec<_>>()
                    .join(" AND "),
                pushed_count.lines().nth(1),
                barrier_count.lines().nth(1)
            );
            if let Ok(mut g) = PUSHDOWN_DIFFERENCES.lock() {
                g.push(note);
            }
        }
        let texts: Vec<&str> = s.filters.iter().map(|f| f.scan.as_str()).collect();
        let cols: Option<Vec<&str>> = s
            .columns
            .as_ref()
            .map(|c| c.iter().map(String::as_str).collect());
        let ctx = format!("{}: {}  [scan filters {texts:?}]", s.what, sqls[3 * i]);
        let pruned = scan(files, cols.as_deref(), &texts, true, None)
            .unwrap_or_else(|e| panic!("{ctx}\nscan failed: {e}"));
        let diff = |a: &str, b: &str| {
            if mode().unordered {
                diff_unordered(a, b)
            } else {
                diff_ordered(a, b)
            }
        };
        if let Some(d) = diff(want, &pruned.csv) {
            panic!(
                "{ctx}\npruned scan differs from DuckDB: {d}\nreport: {}",
                pruned.report.line()
            );
        }
        let full = scan(files, cols.as_deref(), &texts, false, None)
            .unwrap_or_else(|e| panic!("{ctx}\nunpruned scan failed: {e}"));
        if let Some(d) = diff(want, &full.csv) {
            panic!("{ctx}\nunpruned scan differs from DuckDB: {d}");
        }
        let (r, u) = (&pruned.report, &full.report);
        let rows = records(want).len().saturating_sub(1) as u64;
        assert_eq!(r.rows_returned, rows, "{ctx}: rows_returned of the report");
        assert_eq!(
            u.rows_returned, rows,
            "{ctx}: rows_returned of the unpruned report"
        );
        assert_eq!(
            r.row_groups_skipped + r.row_groups_read,
            r.row_groups_total,
            "{ctx}: skipped + read = total ({})",
            r.line()
        );
        assert_eq!(
            u.row_groups_skipped, 0,
            "{ctx}: nothing is skipped without pruning"
        );
        assert_eq!(
            u.row_groups_read, u.row_groups_total,
            "{ctx}: every row group is read without pruning"
        );
        assert_eq!(r.row_groups_total, u.row_groups_total, "{ctx}: totals");
        assert!(
            r.bytes_read <= u.bytes_read,
            "{ctx}: pruning never reads more bytes"
        );
        match s.expect {
            Skip::Any => {}
            Skip::Zero => assert_eq!(
                r.row_groups_skipped,
                0,
                "{ctx}: designed NOT to prune, but {}",
                r.line()
            ),
            Skip::AtLeast(n) => assert!(
                r.row_groups_skipped >= n,
                "{ctx}: designed to skip at least {n}, but {}",
                r.line()
            ),
            Skip::Exactly(n) => assert_eq!(r.row_groups_skipped, n, "{ctx}: {}", r.line()),
            Skip::All => assert_eq!(
                r.row_groups_skipped,
                r.row_groups_total,
                "{ctx}: designed to prune everything, but {}",
                r.line()
            ),
        }
        reports.push(pruned.report);
    }
    reports
}

/// One case (see [`check_cases`]).
pub fn check_case(
    duck: &str,
    files: &[&Path],
    columns: Option<&[&str]>,
    filters: &[Filter],
    expect: Skip,
    what: &str,
) -> ScanReport {
    let spec = Spec {
        columns: columns.map(|c| c.iter().map(|s| s.to_string()).collect()),
        filters: filters.to_vec(),
        expect,
        what: what.to_string(),
    };
    check_cases(duck, files, &[spec]).remove(0)
}
