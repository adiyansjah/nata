//! `nata sql` as a process: output formats, stdin, exit codes, errors with a caret, EXPLAIN and EXPLAIN ANALYZE (golden, times masked), a partial result
//! followed by an error, missing / truncated / damaged files, a closed stdout.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use arrow_array::{ArrayRef, Decimal128Array, Int32Array, Int64Array, RecordBatch, StringArray};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

pub fn nata(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_nata"))
        .args(args)
        .output()
        .expect("failed to launch nata")
}

fn dir(name: &str) -> PathBuf {
    let base = std::env::var("NATA_TEST_TMP").map_or_else(|_| std::env::temp_dir(), PathBuf::from);
    let d = base.join(format!("nata-sqlcli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

pub fn write_batch(path: &Path, batch: &RecordBatch, group: usize) {
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(group))
        .build();
    let mut w = ArrowWriter::try_new(
        File::create(path).expect("create"),
        batch.schema(),
        Some(props),
    )
    .expect("writer");
    w.write(batch).expect("write");
    w.close().expect("close");
}

/// 20 rows in 4 row groups of 5: id 0..19, a = id % 5 - 2 (NULL when id % 7 == 3), price DECIMAL(10,2) = id * 1.25, s = alpha / Beta / NULL / "g,<id>".
fn standard(path: &Path) {
    let n: i32 = 20;
    let id: ArrayRef = Arc::new(Int32Array::from_iter_values(0..n));
    let a: ArrayRef = Arc::new(Int32Array::from_iter(
        (0..n).map(|i| (i % 7 != 3).then_some(i % 5 - 2)),
    ));
    let price: ArrayRef = Arc::new(
        Decimal128Array::from_iter_values((0..i128::from(n)).map(|i| i * 125))
            .with_precision_and_scale(10, 2)
            .expect("decimal"),
    );
    let s: ArrayRef = Arc::new(StringArray::from_iter((0..n).map(|i| match i % 4 {
        0 => Some("alpha".to_string()),
        1 => Some("Beta".to_string()),
        2 => None,
        _ => Some(format!("g,{i}")),
    })));
    write_batch(
        path,
        &RecordBatch::try_from_iter([("id", id), ("a", a), ("price", price), ("s", s)])
            .expect("batch"),
        5,
    );
}

fn std_file(name: &str) -> (PathBuf, String) {
    let d = dir(name);
    let f = d.join("t.parquet");
    standard(&f);
    let p = f.to_str().expect("path").to_string();
    (d, p)
}

#[test]
fn csv_and_the_aligned_table() {
    let (_d, p) = std_file("formats");
    let q = format!(
        "SELECT id, a, price AS cost, s FROM '{p}' WHERE id IN (0, 3, 7, 8) ORDER_PLACEHOLDER"
    )
    .replace(" ORDER_PLACEHOLDER", "");
    let out = nata(&["sql", "--csv", "-c", &q]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout),
        "id,a,cost,s\n0,-2,0.00,alpha\n3,NULL,3.75,\"g,3\"\n7,0,8.75,\"g,7\"\n8,1,10.00,alpha\n"
    );
    assert!(out.stderr.is_empty(), "{}", text(&out.stderr));
    let out = nata(&["sql", "-c", &q]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        text(&out.stdout),
        " id |    a |  cost | s     \n----+------+-------+-------\n  0 |   -2 |  0.00 | alpha \n  3 | NULL |  3.75 | g,3   \n  7 |    0 |  8.75 | g,7   \n  8 |    1 | 10.00 | alpha \n(4 rows)\n"
    );
    // one row, no rows
    let out = nata(&["sql", "-c", &format!("SELECT id FROM '{p}' WHERE id = 5")]);
    assert_eq!(text(&out.stdout), " id \n----\n  5 \n(1 row)\n");
    let out = nata(&["sql", "-c", &format!("SELECT id FROM '{p}' WHERE id < 0")]);
    assert_eq!(text(&out.stdout), " id \n----\n(0 rows)\n");
    let out = nata(&[
        "sql",
        "--csv",
        "-c",
        &format!("SELECT id FROM '{p}' WHERE id < 0"),
    ]);
    assert_eq!(text(&out.stdout), "id\n");
}

#[test]
fn the_query_can_come_from_stdin() {
    let (_d, p) = std_file("stdin");
    let mut child = Command::new(env!("CARGO_BIN_EXE_nata"))
        .args(["sql", "--csv"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(format!("SELECT id\nFROM '{p}'\nWHERE id > 17;\n").as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("wait");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "id\n18\n19\n");
}

#[test]
fn exit_codes_and_the_rendering_of_errors() {
    let (_d, p) = std_file("errors");
    // syntax, bind, not supported yet: 2, with class, message, line, column and a caret
    for (q, code, class) in [
        (format!("SELEC id FROM '{p}'"), 2, "syntax error"),
        (format!("SELECT idd FROM '{p}'"), 2, "bind error"),
        (format!("SELECT id + 'a' FROM '{p}'"), 2, "bind error"),
        (
            format!("SELECT id FROM '{p}' ORDER BY id"),
            2,
            "not supported yet",
        ),
        (
            format!("SELECT id FROM '{p}' GROUP BY ALL"),
            2,
            "not supported yet",
        ),
    ] {
        let out = nata(&["sql", "--csv", "-c", &q]);
        let err = text(&out.stderr);
        assert_eq!(out.status.code(), Some(code), "{q}: {err}");
        assert!(out.stdout.is_empty(), "{q}: nothing on stdout");
        assert!(
            err.contains(class) && err.contains("line 1, column") && err.contains(" | "),
            "{q}: {err}"
        );
        assert!(
            err.lines().any(|l| l.trim_end().ends_with('^')),
            "{q}: a caret under the position\n{err}"
        );
    }
    // the caret stands under the column
    let out = nata(&["sql", "-c", &format!("SELECT idd FROM '{p}'")]);
    let err = text(&out.stderr);
    let lines: Vec<&str> = err.lines().collect();
    let src = lines
        .iter()
        .position(|l| l.starts_with("1 | "))
        .expect("source line");
    assert_eq!(
        lines[src + 1].find('^'),
        Some(lines[src].find("idd").expect("idd")),
        "{err}"
    );
    assert!(err.contains("did you mean `id`?"), "{err}");
    // an error of the data: 1, the file and the row group named, the position of the expression
    let out = nata(&[
        "sql",
        "--csv",
        "-c",
        &format!("SELECT 10 // (id - id) AS x FROM '{p}'"),
    ]);
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(out.stdout.is_empty());
    assert!(
        err.contains(&p)
            && err.contains("row group 0")
            && err.contains("division by zero")
            && err.contains("line 1, column 8"),
        "{err}"
    );
    // usage
    assert_eq!(
        nata(&["sql", "--threads", "0", "-c", "SELECT 1"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(nata(&["sql", "--csv", "-c", ""]).status.code(), Some(2));
}

#[test]
fn errors_never_carry_a_cell_value() {
    let d = dir("secret");
    let f = d.join("t.parquet");
    let id: ArrayRef = Arc::new(Int64Array::from_iter_values([1, 2, 3]));
    let s: ArrayRef = Arc::new(StringArray::from(vec!["SECRET-1", "SECRET-2", "SECRET-3"]));
    write_batch(
        &f,
        &RecordBatch::try_from_iter([("id", id), ("s", s)]).expect("batch"),
        2,
    );
    let p = f.to_str().expect("path");
    for q in [
        format!("SELECT CAST(s AS INTEGER) FROM '{p}'"),
        format!("SELECT CAST(s AS DATE) FROM '{p}'"),
        format!("SELECT CAST(s AS DECIMAL(5,2)) FROM '{p}'"),
        format!("SELECT id * 9223372036854775807 FROM '{p}'"),
        format!("SELECT 1 // (id - id) FROM '{p}'"),
        format!("SELECT s + 1 FROM '{p}'"),
        format!("SELECT s FROM '{p}' WHERE s = 1"),
    ] {
        let out = nata(&["sql", "-c", &q]);
        assert_ne!(out.status.code(), Some(0), "{q}");
        assert!(
            !text(&out.stderr).contains("SECRET"),
            "{q}: {}",
            text(&out.stderr)
        );
    }
}

fn mask(s: &str) -> String {
    let mut out = String::new();
    for line in s.lines() {
        let mut l = line.to_string();
        // times
        let mut done = String::new();
        let mut rest = l.as_str();
        while let Some(i) = rest.find("time=") {
            done.push_str(&rest[..i]);
            done.push_str("time=<T>");
            let after = &rest[i + 5..];
            rest = &after[after.find("ms").map_or(after.len(), |e| e + 2)..];
        }
        done.push_str(rest);
        l = done;
        // the memory line is timing dependent
        if l.trim_start().starts_with("memory:") {
            l = format!("{}memory: <masked>", &l[..l.len() - l.trim_start().len()]);
        }
        out.push_str(&l);
        out.push('\n');
    }
    out
}

fn golden(name: &str, got: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var("NATA_UPDATE_GOLDEN").is_ok() {
        std::fs::write(&path, got).expect("write golden");
    }
    let want = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        got, want,
        "golden file {name} (NATA_UPDATE_GOLDEN=1 rewrites it)"
    );
}

#[test]
fn explain_and_explain_analyze_have_golden_output() {
    let (_d, p) = std_file("explain");
    // the golden numbers are those of the default read (the scan's bytes and footers change with the developer options), whatever the environment says
    let nata = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_nata"))
            .args(args)
            .env_remove("NATA_LATE_MATERIALIZATION")
            .env_remove("NATA_FOOTERS_AHEAD")
            .output()
            .expect("failed to launch nata")
    };
    let sub = |s: &str| s.replace(&p, "<FILE>");
    let queries = [
        "SELECT id, price * 2 AS twice FROM '{F}' WHERE id >= 5 AND id < 15 AND a > 0 LIMIT 4 OFFSET 1",
        "SELECT id FROM '{F}' WHERE a = 0 OR s = 'alpha'",
        "SELECT 1 AS one FROM '{F}'",
        "SELECT id FROM '{F}' WHERE id < 0",
        // a filter that stays in the executor and a limit that is reached inside the first row group: the filter keeps 4 rows, the projection computes 3
        "SELECT id + 1 AS n FROM '{F}' WHERE a + 0 > -5 LIMIT 3",
    ];
    let mut plans = String::new();
    let mut analyzed = String::new();
    for q in queries {
        let q = q.replace("{F}", &p);
        let shown = sub(&q);
        let out = nata(&["sql", "-c", &format!("EXPLAIN {q}")]);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        plans.push_str(&format!("{shown}\n{}\n", sub(&text(&out.stdout))));
        // EXPLAIN is the plan only: it reads no row group (the file is not scanned) and is the same whatever the thread count
        // one thread and a window of one row group make the counts of a query with a limit exact (no row group is read ahead)
        let (threads, window) = if q.contains("LIMIT 3") {
            ("1", "1")
        } else {
            ("2", "3")
        };
        let out = nata(&[
            "sql",
            "--threads",
            threads,
            "--window",
            window,
            "-c",
            &format!("EXPLAIN ANALYZE {q}"),
        ]);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        analyzed.push_str(&format!("{shown}\n{}\n", mask(&sub(&text(&out.stdout)))));
    }
    golden("explain.txt", &plans);
    golden("explain_analyze.txt", &analyzed);
}

#[test]
fn rows_printed_before_an_error_make_the_exit_code_nonzero_and_the_error_says_so() {
    let (_d, p) = std_file("partial");
    // row group 0 (ids 0..4) is fine and is printed; row group 1 holds id 7: division by zero
    let out = nata(&[
        "sql",
        "--csv",
        "-c",
        &format!("SELECT id, 10 // (id - 7) AS q FROM '{p}'"),
    ]);
    let err = text(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert_eq!(
        text(&out.stdout),
        "id,q\n0,-1\n1,-1\n2,-2\n3,-2\n4,-3\n",
        "the rows before the failing row group"
    );
    assert!(
        err.contains("division by zero") && err.contains("row group 1"),
        "{err}"
    );
    assert!(
        err.contains("5 rows had been printed") && err.contains("INCOMPLETE"),
        "{err}"
    );
    // the table format says it too, and prints what it had
    let out = nata(&[
        "sql",
        "-c",
        &format!("SELECT id, 10 // (id - 7) AS q FROM '{p}'"),
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stdout).contains("  4 | -3"),
        "{}",
        text(&out.stdout)
    );
    assert!(text(&out.stderr).contains("INCOMPLETE"));
    // an error before any row: nothing on stdout at all, not even a header
    let out = nata(&[
        "sql",
        "--csv",
        "-c",
        &format!("SELECT 10 // (id - id) AS q FROM '{p}'"),
    ]);
    assert!(out.stdout.is_empty());
    assert!(!text(&out.stderr).contains("INCOMPLETE"));
}

#[test]
fn missing_truncated_and_damaged_files_are_errors_naming_the_file() {
    let (d, p) = std_file("faults");
    let q = |f: &str| format!("SELECT id, price FROM '{f}'");
    // missing
    let missing = d.join("nope.parquet");
    let out = nata(&["sql", "--csv", "-c", &q(missing.to_str().expect("p"))]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert!(
        out.stdout.is_empty() && text(&out.stderr).contains("nope.parquet"),
        "{}",
        text(&out.stderr)
    );
    // truncated at every few bytes: never a result
    let bytes = std::fs::read(&p).expect("read");
    let good = nata(&["sql", "--csv", "-c", &q(&p)]);
    assert_eq!(good.status.code(), Some(0));
    let mut cut = bytes.len() - 1;
    let mut cuts = 0;
    while cut > 0 {
        let f = d.join("trunc.parquet");
        std::fs::write(&f, &bytes[..cut]).expect("write");
        let out = nata(&["sql", "--csv", "-c", &q(f.to_str().expect("p"))]);
        assert_eq!(
            out.status.code(),
            Some(1),
            "cut at {cut}: {}",
            text(&out.stderr)
        );
        assert!(
            text(&out.stderr).contains("trunc.parquet"),
            "cut at {cut}: {}",
            text(&out.stderr)
        );
        assert!(out.stdout.is_empty(), "cut at {cut}: no partial result");
        cuts += 1;
        cut = cut.saturating_sub(bytes.len() / 23 + 1);
    }
    assert!(cuts >= 20);
    // damaged: the magic at both ends, the footer length, and the page headers of the first row group
    for (name, pos) in [
        ("head magic", 0usize),
        ("tail magic", bytes.len() - 1),
        ("footer length", bytes.len() - 6),
        ("first page header", 4),
        ("second", 5),
        ("third", 6),
    ] {
        let mut b = bytes.clone();
        b[pos] ^= 0xff;
        let f = d.join("dmg.parquet");
        std::fs::write(&f, &b).expect("write");
        let out = nata(&["sql", "--csv", "-c", &q(f.to_str().expect("p"))]);
        let err = text(&out.stderr);
        // either the damage is found (exit 1, the file named, no partial result) or it changed nothing that was read
        match out.status.code() {
            Some(1) => assert!(
                err.contains("dmg.parquet") && !err.contains("panicked"),
                "{name}: {err}"
            ),
            Some(0) => assert_eq!(
                out.stdout, good.stdout,
                "{name}: a damaged file printed other rows without an error"
            ),
            c => panic!("{name}: exit {c:?}: {err}"),
        }
    }
}

#[test]
fn a_closed_stdout_ends_the_query_quietly_with_exit_code_zero() {
    let d = dir("closed");
    let f = d.join("big.parquet");
    let n: i64 = 400_000;
    let id: ArrayRef = Arc::new(Int64Array::from_iter_values(0..n));
    write_batch(
        &f,
        &RecordBatch::try_from_iter([("id", id)]).expect("batch"),
        20_000,
    );
    for csv in [true, false] {
        let mut args = vec!["sql"];
        if csv {
            args.push("--csv");
        }
        let q = format!("SELECT id, id + 1 AS next FROM '{}'", f.display());
        args.extend(["-c", &q]);
        let mut child = Command::new(env!("CARGO_BIN_EXE_nata"))
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn");
        let mut first = [0u8; 16];
        child
            .stdout
            .take()
            .expect("stdout")
            .read_exact(&mut first)
            .expect("read some");
        // the reader goes away (the pipe closes) while the query still has rows to print
        let out = child.wait_with_output().expect("wait");
        assert_eq!(
            out.status.code(),
            Some(0),
            "csv={csv}: {}",
            text(&out.stderr)
        );
        assert!(
            out.stderr.is_empty(),
            "csv={csv}: quiet: {}",
            text(&out.stderr)
        );
    }
}

#[test]
fn every_construct_of_the_not_supported_yet_table_of_supported_sql_answers_not_supported_yet() {
    let (_d, p) = std_file("unsupported");
    for q in [
        format!("SELECT count(DISTINCT id) FROM '{p}'"),
        format!("SELECT id FROM '{p}' GROUP BY ALL"),
        format!("SELECT id FROM '{p}' ORDER BY id"),
        "SELECT 1".to_string(),
        format!("SELECT x.id FROM '{p}' x JOIN '{p}' y ON x.id = y.id"),
        format!("SELECT abs(a) FROM '{p}'"),
        format!("SELECT id FROM '{p}' WHERE s LIKE 'a%'"),
        format!("SELECT NULLIF(a, 1) FROM '{p}'"),
        format!("SELECT TRY_CAST(a AS INTEGER) FROM '{p}'"),
        format!("SELECT CAST(a AS TINYINT) FROM '{p}'"),
        format!("SELECT id FROM '{p}' OFFSET 2"),
        format!("SELECT id FROM '{p}' LIMIT ALL"),
        format!("SELECT id FROM '{p}' LIMIT 1 + 1"),
        "SELECT id FROM 's3://bucket/x.parquet'".to_string(),
        format!("SELECT id FROM '{p}' WHERE a IN (id, 2)"),
        format!("SELECT DISTINCT id FROM '{p}'"),
        "WITH x AS (SELECT 1) SELECT * FROM x".to_string(),
        "SELECT id FROM (SELECT 1) t".to_string(),
    ] {
        let out = nata(&["sql", "--csv", "-c", &q]);
        let err = text(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{q}: {err}");
        assert!(out.stdout.is_empty(), "{q}");
        assert!(err.starts_with("error: not supported yet: "), "{q}: {err}");
        assert!(err.contains("line 1, column"), "{q}: {err}");
    }
}

#[test]
fn ctrl_c_while_the_footers_of_a_glob_are_still_being_read_stops_the_query_with_exit_code_130() {
    let d = dir("sigint_footers");
    let n = 4000i64;
    for i in 0..n {
        let id: ArrayRef = Arc::new(Int64Array::from_iter_values([i, i + n]));
        write_batch(
            &d.join(format!("f{i:05}.parquet")),
            &RecordBatch::try_from_iter([("id", id)]).expect("batch"),
            10,
        );
    }
    // no row can match, so there is no LIMIT to end the query early: every footer of the 4,000 files has to be read, at bind time or in the scan
    let q = format!("SELECT id FROM '{}/f*.parquet' WHERE id < 0", d.display());
    // the default (the binder reads one footer, the scan reads the others as it reaches them: register item P10) and the binder that reads every
    // footer before the query starts (`--bind-all-footers`: the interruptible source of the binder)
    for extra in [
        &[][..],
        &["--bind-all-footers"][..],
        &["--footers-ahead", "8"][..],
    ] {
        let (mut stopped, mut finished) = (0, 0);
        for delay_ms in [5u64, 10, 20, 40, 80, 160] {
            let child = Command::new(env!("CARGO_BIN_EXE_nata"))
                .args(["sql", "--csv"])
                .args(extra)
                .args(["-c", &q])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn");
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            let _ = Command::new("kill")
                .args(["-INT", &child.id().to_string()])
                .status();
            let out = child.wait_with_output().expect("wait");
            match out.status.code() {
                Some(130) => {
                    stopped += 1;
                    assert!(
                        text(&out.stderr).contains("interrupted"),
                        "{}",
                        text(&out.stderr)
                    );
                    assert!(
                        out.stdout.is_empty() || text(&out.stdout) == "id\n",
                        "no row was printed: the footers were still being read"
                    );
                }
                Some(0) => {
                    finished += 1;
                    assert_eq!(text(&out.stdout), "id\n");
                }
                // a SIGINT before the handler is installed (the first milliseconds of the process) ends it by the signal itself, as for any program
                None if delay_ms <= 20
                    && std::os::unix::process::ExitStatusExt::signal(&out.status) == Some(2) => {}
                c => panic!("delay {delay_ms} ms: exit {c:?}: {}", text(&out.stderr)),
            }
        }
        assert!(
            stopped >= 1,
            "{extra:?}: no interrupt came in time ({finished} runs finished first): the glob is too small for this machine"
        );
    }
    // 4,000 files: /tmp is a small RAM disk, do not leave them behind
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_later_file_with_another_schema_is_an_error_naming_the_file_found_when_the_scan_reaches_it() {
    // register item P10: `nata sql` binds on the first footer; the scan checks the schema of every other file when it reaches it
    let d = dir("schema_later");
    let ids = |lo: i64, hi: i64| -> ArrayRef { Arc::new(Int64Array::from_iter_values(lo..hi)) };
    for i in 0..3i64 {
        write_batch(
            &d.join(format!("f{i}.parquet")),
            &RecordBatch::try_from_iter([("id", ids(i * 10, i * 10 + 10))]).expect("batch"),
            5,
        );
    }
    // f3: the same column name, another type
    let wrong: ArrayRef = Arc::new(StringArray::from_iter_values(
        (0..4).map(|i| format!("s{i}")),
    ));
    write_batch(
        &d.join("f3.parquet"),
        &RecordBatch::try_from_iter([("id", wrong)]).expect("batch"),
        5,
    );
    write_batch(
        &d.join("f4.parquet"),
        &RecordBatch::try_from_iter([("id", ids(40, 50))]).expect("batch"),
        5,
    );
    let q = format!("SELECT id FROM '{}/f*.parquet'", d.display());
    for extra in [
        &[][..],
        &["--footers-ahead", "4"][..],
        &["--threads", "1"][..],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_nata"))
            .args(["sql", "--csv"])
            .args(extra)
            .args(["-c", &q])
            .output()
            .expect("run");
        let err = text(&out.stderr);
        // a failure of the data, found by the scan: exit code 1, the rows of the files before it were printed, the output is said to be incomplete
        assert_eq!(out.status.code(), Some(1), "{extra:?}: {err}");
        assert!(
            err.contains("f3.parquet") && err.contains("must have the same schema"),
            "{extra:?}: {err}"
        );
        assert!(!err.contains("f4.parquet"), "{extra:?}: {err}");
        assert_eq!(
            text(&out.stdout).lines().count(),
            31,
            "{extra:?}: the header and the 30 rows of f0..f2"
        );
        assert!(err.contains("INCOMPLETE"), "{extra:?}: {err}");
    }
    // the binder that reads every footer finds it before the query starts: a bind error (exit code 2), nothing printed
    let out = Command::new(env!("CARGO_BIN_EXE_nata"))
        .args(["sql", "--csv", "--bind-all-footers", "-c", &q])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert!(out.stdout.is_empty());
    assert!(
        text(&out.stderr).contains("f3.parquet"),
        "{}",
        text(&out.stderr)
    );
    // EXPLAIN does not run the query and does not look at the later files
    let out = Command::new(env!("CARGO_BIN_EXE_nata"))
        .args(["sql", "-c", &format!("EXPLAIN {q}")])
        .output()
        .expect("run");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_deepest_queries_the_parser_allows_run_in_the_process() {
    // R11: 10,000 terms in a chain and 1,000 levels of nesting, evaluated by the real binary (its scan workers have the default 2 MiB stacks; the executor
    // evaluates a deep expression on a thread with a stack of its own), also from stdin, with EXPLAIN and EXPLAIN ANALYZE
    let (_d, p) = std_file("deep");
    let chain = format!("id{}", " + 0".repeat(9_999));
    let nested = format!("{}id{}", "(".repeat(1000), ")".repeat(1000));
    for expr in [&chain, &nested] {
        let q = format!("SELECT {expr} AS x FROM '{p}' WHERE id IN (3, 17)");
        for extra in [&["--threads", "1"][..], &["--threads", "4"][..]] {
            let out = Command::new(env!("CARGO_BIN_EXE_nata"))
                .args(["sql", "--csv"])
                .args(extra)
                .args(["-c", &q])
                .output()
                .expect("run");
            assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
            assert_eq!(text(&out.stdout), "x\n3\n17\n");
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_nata"))
            .args(["sql", "--csv"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(q.as_bytes())
            .expect("write");
        let out = child.wait_with_output().expect("wait");
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        assert_eq!(text(&out.stdout), "x\n3\n17\n");
        for prefix in ["EXPLAIN ", "EXPLAIN ANALYZE "] {
            let out = nata(&["sql", "-c", &format!("{prefix}{q}")]);
            assert_eq!(
                out.status.code(),
                Some(0),
                "{prefix}: {}",
                text(&out.stderr)
            );
        }
    }
}
