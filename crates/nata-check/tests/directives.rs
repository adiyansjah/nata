//! End-to-end tests of the per-query directives, with fake engines: shell
//! scripts that behave according to marker words in the SQL they are given.
//!
//! Reference: `ref -csv -c <sql>` (the SQL is `$3`). Candidate: `cand sql --csv -c <sql>` (`$4`).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const REFERENCE: &str = r#"#!/bin/sh
case "$3" in
  *REFFAIL*) echo "reference boom" >&2; exit 1;;
  *REFVAL*) printf 'v\n0.33333333333\n';;
  *) printf 'v\n1\n';;
esac
"#;

const CANDIDATE: &str = r#"#!/bin/sh
case "$4" in
  *CANDUNSUPPORTED*) echo "error: not supported yet: thing" >&2; exit 2;;
  *CANDFAIL*) echo "error: Division by zero in a / b" >&2; exit 1;;
  *CANDOTHER*) echo "error: something else" >&2; exit 1;;
  *CANDVAL*) printf 'v\n0.33333333\n';;
  *) printf 'v\n1\n';;
esac
"#;

fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, body).expect("write script");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

struct Run {
    stdout: String,
    stderr: String,
    code: Option<i32>,
}

fn run_with(tag: &str, queries: &[(&str, &str)]) -> Run {
    let dir = std::env::temp_dir().join(format!("nata-check-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("q")).expect("mkdir");
    fs::create_dir_all(dir.join("data")).expect("mkdir");
    let reference = script(&dir, "ref", REFERENCE);
    let candidate = script(&dir, "cand", CANDIDATE);
    for (name, sql) in queries {
        fs::write(dir.join("q").join(format!("{name}.sql")), sql).expect("write query");
    }
    let out = Command::new(env!("CARGO_BIN_EXE_nata-check"))
        .args(["--data", dir.join("data").to_str().expect("path")])
        .args(["--queries", dir.join("q").to_str().expect("path")])
        .args(["--reference", reference.to_str().expect("path")])
        .args(["--candidate", candidate.to_str().expect("path")])
        .output()
        .expect("run nata-check");
    let _ = fs::remove_dir_all(&dir);
    Run {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        code: out.status.code(),
    }
}

/// The label printed for a query, e.g. `PASS` or `MISMATCH`.
fn label(stdout: &str, name: &str) -> String {
    stdout
        .lines()
        .find_map(|l| {
            let mut parts = l.split_whitespace();
            let label = parts.next()?;
            (parts.next() == Some(name)).then(|| label.to_string())
        })
        .unwrap_or_else(|| panic!("no result line for {name} in:\n{stdout}"))
}

#[test]
fn directives_decide_what_passes() {
    let run = run_with(
        "all",
        &[
            ("plain_match", "SELECT 1"),
            // expect-error: both engines must fail.
            ("both_fail", "-- expect-error\nSELECT REFFAIL CANDFAIL"),
            ("only_reference_fails", "-- expect-error\nSELECT REFFAIL"),
            ("reference_succeeds", "-- expect-error\nSELECT CANDFAIL"),
            (
                "both_but_unsupported",
                "-- expect-error\nSELECT REFFAIL CANDUNSUPPORTED",
            ),
            // expect-error-nata: the reference is not consulted, even though it fails here.
            (
                "nata_error_ok",
                "-- expect-error-nata: division by zero\nSELECT CANDFAIL REFFAIL",
            ),
            (
                "nata_returns_rows",
                "-- expect-error-nata: division by zero\nSELECT 1",
            ),
            (
                "nata_wrong_message",
                "-- expect-error-nata: division by zero\nSELECT CANDOTHER",
            ),
            (
                "nata_unsupported",
                "-- expect-error-nata: division by zero\nSELECT CANDUNSUPPORTED",
            ),
            // tolerance.
            ("tol_loose", "-- tolerance: 1e-7\nSELECT REFVAL CANDVAL"),
            ("tol_tight", "-- tolerance: 1e-9\nSELECT REFVAL CANDVAL"),
            ("tol_none", "SELECT REFVAL CANDVAL"),
        ],
    );
    let l = |n: &str| label(&run.stdout, n);
    assert_eq!(l("plain_match"), "PASS");
    assert_eq!(l("both_fail"), "PASS");
    assert_eq!(l("only_reference_fails"), "MISMATCH");
    assert_eq!(l("reference_succeeds"), "REFERENCE-ERROR");
    assert_eq!(l("both_but_unsupported"), "UNSUPPORTED");
    assert_eq!(l("nata_error_ok"), "PASS");
    assert_eq!(l("nata_returns_rows"), "MISMATCH");
    assert_eq!(l("nata_wrong_message"), "MISMATCH");
    assert_eq!(l("nata_unsupported"), "UNSUPPORTED");
    assert_eq!(l("tol_loose"), "PASS");
    assert_eq!(l("tol_tight"), "MISMATCH");
    assert_eq!(l("tol_none"), "MISMATCH");
    assert_eq!(
        run.code,
        Some(1),
        "not everything passed, so the exit code is non-zero"
    );
}

#[test]
fn a_reference_failure_without_a_directive_is_still_suspect() {
    let run = run_with("noreffail", &[("typo", "SELECT REFFAIL CANDFAIL")]);
    assert_eq!(label(&run.stdout, "typo"), "REFERENCE-ERROR");
}

#[test]
fn a_malformed_directive_stops_the_run() {
    let run = run_with("malformed", &[("bad", "-- tolerance: abc\nSELECT 1")]);
    assert_eq!(run.code, Some(2));
    assert!(
        run.stderr.contains("bad.sql") && run.stderr.contains("tolerance"),
        "stderr: {}",
        run.stderr
    );
}
