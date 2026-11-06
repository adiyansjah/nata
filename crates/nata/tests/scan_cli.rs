//! `nata scan`: the minimal developer command of slice 3. stdout is the CSV, stderr the report; errors name the file and exit non-zero with nothing on stdout.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use arrow_array::{ArrayRef, Int64Array, RecordBatch, StringArray};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

fn nata(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_nata"))
        .args(args)
        .output()
        .expect("failed to launch nata")
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("nata-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

/// ids first..first+rows, text `t<id>`, in row groups of 10 rows.
fn write(path: &Path, first: i64, rows: i64) {
    let ids: ArrayRef = Arc::new(Int64Array::from_iter_values(first..first + rows));
    let txt: ArrayRef = Arc::new(StringArray::from_iter_values(
        (first..first + rows).map(|i| format!("t{i}")),
    ));
    let batch = RecordBatch::try_from_iter([("id", ids), ("txt", txt)]).expect("batch");
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(10))
        .build();
    let mut w = ArrowWriter::try_new(
        File::create(path).expect("create"),
        batch.schema(),
        Some(props),
    )
    .expect("writer");
    w.write(&batch).expect("write");
    w.close().expect("close");
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn scan_prints_csv_on_stdout_and_the_report_on_stderr() {
    let d = dir("ok");
    let (a, b) = (d.join("a.parquet"), d.join("b.parquet"));
    write(&a, 0, 30);
    write(&b, 100, 5);
    let out = nata(&[
        "scan",
        a.to_str().expect("p"),
        b.to_str().expect("p"),
        "--columns",
        "txt,id",
        "--filter",
        "id >= 25",
        "--csv",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout),
        "txt,id\nt25,25\nt26,26\nt27,27\nt28,28\nt29,29\nt100,100\nt101,101\nt102,102\nt103,103\nt104,104\n"
    );
    let err = text(&out.stderr);
    // 3 + 1 row groups: the first two row groups of a.parquet and nothing of b are skipped by the statistics
    assert!(err.starts_with("scan: files=2 row_groups=4 skipped=2 read=2 not_read=0 rows_read=15 rows_returned=10 bytes_read="), "{err}");
    assert!(
        err.contains("columns_read=2 morsel=row-group pruning=applied(1 of 1 terms)"),
        "{err}"
    );
    // --no-prune reads everything and prints the same rows
    let u = nata(&[
        "scan",
        a.to_str().expect("p"),
        b.to_str().expect("p"),
        "--columns",
        "txt,id",
        "--filter",
        "id >= 25",
        "--csv",
        "--no-prune",
    ]);
    assert_eq!(text(&u.stdout), text(&out.stdout));
    assert!(
        text(&u.stderr).contains("skipped=0 read=4")
            && text(&u.stderr).contains("pruning=off(--no-prune)"),
        "{}",
        text(&u.stderr)
    );
}

#[test]
fn limit_columns_and_default_columns() {
    let d = dir("limit");
    let a = d.join("a.parquet");
    write(&a, 0, 30);
    // one thread: the window is one row group, so exactly one is read (with more threads some are read ahead and dropped)
    let out = nata(&[
        "scan",
        a.to_str().expect("p"),
        "--limit",
        "3",
        "--threads",
        "1",
        "--csv",
    ]);
    assert_eq!(text(&out.stdout), "id,txt\n0,t0\n1,t1\n2,t2\n");
    assert!(
        text(&out.stderr).contains("read=1 not_read=2"),
        "{}",
        text(&out.stderr)
    );
    let out = nata(&[
        "scan",
        a.to_str().expect("p"),
        "--filter",
        "txt = 't7'",
        "--filter",
        "id IS NOT NULL",
        "--csv",
    ]);
    assert_eq!(text(&out.stdout), "id,txt\n7,t7\n");
    assert!(
        text(&out.stderr).contains("pruning=applied(2 of 2 terms)"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn errors_name_the_file_print_nothing_on_stdout_for_the_first_file_and_exit_non_zero() {
    let d = dir("err");
    let a = d.join("a.parquet");
    write(&a, 0, 30);
    let bad = d.join("bad.parquet");
    std::fs::write(&bad, b"not parquet at all, just text").expect("write");
    let missing = d.join("missing.parquet");
    for (args, code, want) in [
        (
            vec!["scan", bad.to_str().expect("p"), "--csv"],
            1,
            "bad.parquet",
        ),
        (
            vec!["scan", missing.to_str().expect("p"), "--csv"],
            1,
            "missing.parquet",
        ),
        (
            vec![
                "scan",
                a.to_str().expect("p"),
                "--columns",
                "nosuch",
                "--csv",
            ],
            2,
            "no column named",
        ),
        (
            vec![
                "scan",
                a.to_str().expect("p"),
                "--filter",
                "id >= 1.5",
                "--csv",
            ],
            2,
            "never rounded",
        ),
        (
            vec![
                "scan",
                a.to_str().expect("p"),
                "--filter",
                "id ?? 1",
                "--csv",
            ],
            2,
            "expected one of",
        ),
        (
            vec!["scan", a.to_str().expect("p")],
            2,
            "not supported yet: output other than --csv",
        ),
    ] {
        let out = nata(&args);
        assert_eq!(
            out.status.code(),
            Some(code),
            "{args:?}: {}",
            text(&out.stderr)
        );
        assert!(
            out.stdout.is_empty(),
            "{args:?}: nothing on stdout, got {:?}",
            text(&out.stdout)
        );
        let err = text(&out.stderr);
        assert!(
            err.starts_with("error: ") && err.contains(want),
            "{args:?}: {err}"
        );
    }
    // Footers are read when the scan reaches the file (step 2b): a bad file AFTER a good one is found after the good file's rows were printed, for
    // every thread count; the order is the one of the file list, so the output is the same each time.
    for threads in ["1", "4"] {
        let out = nata(&[
            "scan",
            a.to_str().expect("p"),
            bad.to_str().expect("p"),
            "--threads",
            threads,
            "--csv",
        ]);
        assert_eq!(out.status.code(), Some(1));
        let want_rows: String = std::iter::once("id,txt".to_string())
            .chain((0..30).map(|i| format!("{i},t{i}")))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        assert_eq!(text(&out.stdout), want_rows, "threads {threads}");
        let err = text(&out.stderr);
        assert!(
            err.starts_with("error: ") && err.contains("bad.parquet"),
            "{err}"
        );
    }
    // no file at all is a usage error of clap
    let out = nata(&["scan", "--csv"]);
    assert_eq!(out.status.code(), Some(2));
}

/// `--count` (the benchmark's output) prints exactly the number of rows `--csv` prints, for every thread count, with a filter that prunes,
/// with a limit and with nothing matching; `--count` with `--csv` is a usage error.
#[test]
fn count_equals_the_number_of_csv_rows() {
    let d = dir("count");
    let (a, b) = (d.join("a.parquet"), d.join("b.parquet"));
    write(&a, 0, 95);
    write(&b, 1000, 37);
    let (pa, pb) = (a.to_str().expect("p"), b.to_str().expect("p"));
    let cases: [&[&str]; 4] = [
        &[],
        &["--filter", "id >= 40", "--filter", "id < 1010"],
        &["--limit", "23"],
        &["--filter", "id > 5000"],
    ];
    for extra in cases {
        for t in ["1", "3", "8"] {
            let mut csv_args = vec!["scan", pa, pb, "--csv", "--threads", t];
            csv_args.extend_from_slice(extra);
            let mut count_args = vec!["scan", pa, pb, "--count", "--threads", t];
            count_args.extend_from_slice(extra);
            let csv = nata(&csv_args);
            let cnt = nata(&count_args);
            assert!(
                csv.status.success() && cnt.status.success(),
                "{extra:?} threads {t}: {}",
                text(&cnt.stderr)
            );
            let csv_rows = text(&csv.stdout).lines().count().saturating_sub(1);
            assert_eq!(
                text(&cnt.stdout).trim(),
                csv_rows.to_string(),
                "{extra:?} threads {t}"
            );
        }
    }
    let both = nata(&["scan", pa, "--csv", "--count"]);
    assert_eq!(
        both.status.code(),
        Some(2),
        "--csv with --count must be refused"
    );
    let _ = std::fs::remove_dir_all(&d);
}
