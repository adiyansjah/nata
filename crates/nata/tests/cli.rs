//! What is not there yet must fail loudly: exit code 2 and a `not supported yet` message, never a wrong or empty result.

use std::process::Command;

fn nata(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_nata"))
        .args(args)
        .output()
        .expect("failed to launch nata")
}

#[test]
fn sql_outside_the_subset_is_not_supported_yet() {
    let out = nata(&["sql", "--csv", "-c", "SELECT 1"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty(), "must not print a result");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("not supported yet"), "stderr was: {err}");
}

#[test]
fn run_is_not_supported_yet() {
    let out = nata(&["run"]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("not supported yet"), "stderr was: {err}");
}
