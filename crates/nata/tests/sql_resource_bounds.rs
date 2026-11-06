//! RESOURCE BOUNDS of `nata sql` (card 0002), the slice 3 tests on the real `nata sql` process:
//!   * a query over 10,000 small files under `ulimit -n 256` succeeds, with 1 and with 8 threads, prints the right rows, and holds at most one handle per
//!     worker (plus the footer reader) and a handful of footers; a LIMIT does not open the files it does not need;
//!   * peak resident memory stays under `base + K * window * M + F` (M: the decoded size of the largest row group, measured by an independent reader)
//!     with a filter and a projection above the scan and a slow consumer of stdout; with `--window 100000` (no bound) it does not.
//!
//! The data is generated under the cargo scratch directory and deleted at the end. Needs Linux (`/proc`, `ulimit` in `sh`).
//! `NATA_FILES` (default 10000) sets the number of small files, `NATA_BIG_GROUPS` (default 100) the row groups of the memory test.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "test data"
)]

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use arrow_array::{ArrayRef, Int64Array, RecordBatch, StringArray};
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::file::properties::WriterProperties;

fn scratch(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("nata-sql-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_file(path: &Path, batch: &RecordBatch, group: usize) {
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

fn field(text: &str, key: &str) -> u64 {
    text.split_whitespace()
        .find_map(|w| w.strip_prefix(&format!("{key}=")))
        .and_then(|v| v.trim_end_matches(',').parse().ok())
        .unwrap_or_else(|| panic!("no {key}= in: {text}"))
}

fn under_ulimit(args: &[&str]) -> std::process::Output {
    Command::new("sh")
        .arg("-c")
        .arg("ulimit -n 256 && exec \"$0\" \"$@\"")
        .arg(env!("CARGO_BIN_EXE_nata"))
        .args(args)
        .output()
        .expect("run")
}

#[test]
fn ten_thousand_small_files_under_ulimit_256_hold_at_most_one_handle_per_worker() {
    if !cfg!(target_os = "linux") {
        return;
    }
    let n: usize = std::env::var("NATA_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10_000);
    let dir = scratch("tenk");
    let _cleanup = Cleanup(dir.clone());
    for i in 0..n {
        let ids: ArrayRef = Arc::new(Int64Array::from_iter_values(
            (i * 3) as i64..(i * 3 + 3) as i64,
        ));
        let txt: ArrayRef = Arc::new(StringArray::from_iter_values(
            (0..3).map(|k| format!("file {i} row {k}")),
        ));
        write_file(
            &dir.join(format!("f{i:05}.parquet")),
            &RecordBatch::try_from_iter([("id", ids), ("txt", txt)]).expect("batch"),
            1000,
        );
    }
    let pattern = format!("{}/f*.parquet", dir.display());
    let mut want = String::from("next\n");
    for i in 0..n * 3 {
        want.push_str(&format!("{}\n", i + 1));
    }
    let out = Command::new("sh")
        .args(["-c", "ulimit -n 256 && ulimit -n"])
        .output()
        .expect("sh");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "256");

    // a filter above the scan (not pushable) and a projection: the rows are still the right ones
    let q = format!("SELECT id + 1 AS next FROM '{pattern}' WHERE id + 0 >= 0");
    for threads in ["1", "8"] {
        let out = under_ulimit(&["sql", "--csv", "--threads", threads, "--report", "-c", &q]);
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(0), "threads {threads}: {err}");
        assert!(
            out.stdout == want.as_bytes(),
            "threads {threads}: the rows differ from the expected"
        );
        eprintln!("nata sql, 10k files, ulimit -n 256, threads {threads}: {err}");
        let t: u64 = threads.parse().expect("threads");
        assert_eq!(field(&err, "files"), n as u64);
        assert_eq!(field(&err, "files_opened"), n as u64, "{err}");
        assert_eq!(field(&err, "rows_returned"), (n * 3) as u64);
        assert!(
            field(&err, "peak_open_files") <= t + 1,
            "handles are bounded by the threads (plus the footer reader): {err}"
        );
        assert!(
            field(&err, "peak_footers") <= t + 2,
            "footers are bounded by the window: {err}"
        );
    }
    // a limit under the same ulimit does not open the files it does not need
    let q = format!("SELECT id FROM '{pattern}' WHERE id + 0 >= 0 LIMIT 25");
    let out = under_ulimit(&["sql", "--csv", "--threads", "8", "--report", "-c", &q]);
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert_eq!(String::from_utf8_lossy(&out.stdout).lines().count(), 26);
    eprintln!("nata sql, 10k files, ulimit -n 256, limit 25: {err}");
    assert!(
        field(&err, "files_opened") < 200,
        "a limit does not read the footers of 10,000 files: {err}"
    );
}

fn slow_consumer(file: &Path, extra: &[&str]) -> String {
    let q = format!(
        "SELECT id, s, id * 2 AS twice FROM '{}' WHERE id % 3 <> 1",
        file.display()
    );
    let mut child = Command::new(env!("CARGO_BIN_EXE_nata"))
        .args(["sql", "--csv", "--report"])
        .args(extra)
        .args(["-c", &q])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut stdout = child.stdout.take().expect("stdout");
    let mut stderr = child.stderr.take().expect("stderr");
    let err_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = stdout.read(&mut buf).expect("read");
        if n == 0 {
            break;
        }
        total += n as u64;
        std::thread::sleep(Duration::from_micros(500));
    }
    let status = child.wait().expect("wait");
    let err = err_reader.join().expect("stderr thread");
    assert!(status.success(), "{err}");
    assert!(total > 0);
    err
}

#[test]
fn peak_memory_is_bounded_by_the_window_times_the_largest_row_group_with_operators_above_the_scan()
{
    if !cfg!(target_os = "linux") {
        return;
    }
    let groups: usize = std::env::var("NATA_BIG_GROUPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    let rows_per_group = 20_000usize;
    let dir = scratch("mem");
    let _cleanup = Cleanup(dir.clone());
    let file = dir.join("big.parquet");
    {
        let props = WriterProperties::builder()
            .set_max_row_group_row_count(Some(rows_per_group))
            .build();
        let schema = {
            let ids: ArrayRef = Arc::new(Int64Array::from(vec![0i64]));
            let s: ArrayRef = Arc::new(StringArray::from(vec!["x"]));
            RecordBatch::try_from_iter([("id", ids), ("s", s)])
                .expect("batch")
                .schema()
        };
        let mut w = ArrowWriter::try_new(File::create(&file).expect("create"), schema, Some(props))
            .expect("writer");
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        for g in 0..groups {
            let base = (g * rows_per_group) as i64;
            let ids: ArrayRef = Arc::new(Int64Array::from_iter_values(
                base..base + rows_per_group as i64,
            ));
            let strings: Vec<String> = (0..rows_per_group)
                .map(|_| {
                    let mut s = String::with_capacity(100);
                    while s.len() < 100 {
                        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
                        let mut z = state;
                        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                        s.push_str(&format!("{:016x}", z ^ (z >> 31)));
                    }
                    s.truncate(100);
                    s
                })
                .collect();
            let s: ArrayRef = Arc::new(StringArray::from(strings));
            w.write(&RecordBatch::try_from_iter([("id", ids), ("s", s)]).expect("batch"))
                .expect("write");
        }
        w.close().expect("close");
    }
    let m: u64 = {
        let b = ParquetRecordBatchReaderBuilder::try_new(File::open(&file).expect("open"))
            .expect("builder");
        let n_groups = b.metadata().num_row_groups();
        assert_eq!(n_groups, groups);
        let mut largest = 0u64;
        for g in [0, n_groups / 2, n_groups - 1] {
            let r = ParquetRecordBatchReaderBuilder::try_new(File::open(&file).expect("open"))
                .expect("builder")
                .with_row_groups(vec![g])
                .build()
                .expect("reader");
            let size: usize = r.map(|b| b.expect("batch").get_array_memory_size()).sum();
            largest = largest.max(size as u64);
        }
        largest
    };
    let base = {
        let q = format!("SELECT id FROM '{}' LIMIT 0", file.display());
        let out = Command::new(env!("CARGO_BIN_EXE_nata"))
            .args(["sql", "--csv", "--report", "--threads", "1", "-c", &q])
            .output()
            .expect("run");
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    let base_kib = field(&base, "peak_rss_kib");
    // K x window x M: the decoded batch, the filtered copy and the projected columns of a row group in flight (the evaluator's own temporaries are
    // one batch of 8,192 rows); F: CSV buffers, thread stacks and the allocator
    const K: f64 = 3.0;
    let fixed_kib = 32 * 1024u64;
    let bound_kib = |window: u64| base_kib + (K * (window * m) as f64 / 1024.0) as u64 + fixed_kib;
    for (threads, extra) in [
        (4u64, vec!["--threads", "4"]),
        (4, vec!["--threads", "4", "--unordered"]),
        (1, vec!["--threads", "1"]),
    ] {
        let err = slow_consumer(&file, &extra);
        let peak = field(&err, "peak_rss_kib");
        let window = field(&err, "window");
        assert!(window >= threads, "{err}");
        let bound = bound_kib(window);
        assert!(
            peak <= bound,
            "{extra:?}: peak RSS {peak} KiB is above the bound {bound} KiB (base {base_kib}, M = {m} bytes, window {window}): {err}"
        );
        let inflight = field(&err, "peak_inflight_bytes");
        let largest = field(&err, "largest_task_bytes");
        assert!(inflight <= window * largest, "{err}");
        assert!(
            (largest as f64) <= K * m as f64,
            "one row group accounted at {largest} against an independent {m}"
        );
        eprintln!(
            "nata sql {extra:?}: peak RSS {peak} KiB <= bound {bound} KiB; accounted peak {inflight} B, largest task {largest} B, independent M {m} B, base {base_kib} KiB"
        );
    }
    let err = slow_consumer(&file, &["--threads", "4", "--window", "100000"]);
    let peak = field(&err, "peak_rss_kib");
    let bound = bound_kib(4);
    eprintln!(
        "nata sql, unbounded window: peak RSS {peak} KiB, the bound for window 4 is {bound} KiB"
    );
    assert!(
        peak > bound,
        "the unbounded window must break the bound (peak {peak} KiB, bound {bound} KiB), or the data is too small to show it"
    );
}
