//! RESOURCE BOUNDS (card row "Resource bounds"), on the real `nata scan` process:
//!   * a scan of 10,000 small files under `ulimit -n 256` succeeds, with 1 and with 8 threads, prints the right rows, and holds at most one handle per
//!     worker (plus the footer reader) and a handful of footers;
//!   * peak resident memory stays under `base + K * window * M + F`, where M is the decoded size of the largest row group measured by an independent
//!     reader, even when the consumer of stdout is slow; with `--window 1000` (no bound) it does not.
//!
//! The data is generated under the cargo scratch directory and deleted at the end. Needs Linux (`/proc`, `ulimit` in `sh`).
//! `NATA_FILES` (default 10000) sets the number of small files, `NATA_BIG_GROUPS` (default 100) the row groups of the memory test.

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
    let d =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("nata-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

/// Deletes the directory when dropped, also when the test fails.
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

fn report_field(stderr: &str, key: &str) -> u64 {
    stderr
        .split_whitespace()
        .find_map(|w| w.strip_prefix(&format!("{key}=")))
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("no {key}= in the report: {stderr}"))
}

fn nata_cmd() -> Command {
    Command::new(
        std::env::var_os("NATA_RSS_BINARY").unwrap_or_else(|| env!("CARGO_BIN_EXE_nata").into()),
    )
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
    let mut files = Vec::with_capacity(n);
    for i in 0..n {
        let ids: ArrayRef = Arc::new(Int64Array::from_iter_values(
            (i * 3) as i64..(i * 3 + 3) as i64,
        ));
        let txt: ArrayRef = Arc::new(StringArray::from_iter_values(
            (0..3).map(|k| format!("file {i} row {k}")),
        ));
        let p = dir.join(format!("f{i:05}.parquet"));
        write_file(
            &p,
            &RecordBatch::try_from_iter([("id", ids), ("txt", txt)]).expect("batch"),
            1000,
        );
        files.push(p);
    }
    let size = std::fs::metadata(&files[0]).expect("meta").len();
    assert!(size < 4096, "a few KB each: {size}");
    let mut want = String::from("id\n");
    for i in 0..n * 3 {
        want.push_str(&format!("{i}\n"));
    }

    // the ulimit really is 256 inside the shell that starts nata
    let out = Command::new("sh")
        .args(["-c", "ulimit -n 256 && ulimit -n"])
        .output()
        .expect("sh");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "256");

    // (threads, footers read ahead): 1 is slice 3's behaviour; 4 and 8 are register item P2: the footers of the next files are read while earlier row
    // groups run, and the handles and footers are bounded all the same
    for (threads, ahead) in [("1", "1"), ("8", "1"), ("1", "4"), ("8", "8"), ("3", "4")] {
        let out = Command::new("sh")
            .arg("-c")
            .arg("ulimit -n 256 && exec \"$0\" \"$@\"")
            .arg(
                std::env::var_os("NATA_RSS_BINARY")
                    .unwrap_or_else(|| env!("CARGO_BIN_EXE_nata").into()),
            )
            .arg("scan")
            .args(&files)
            .args([
                "--columns",
                "id",
                "--threads",
                threads,
                "--footers-ahead",
                ahead,
                "--csv",
            ])
            .output()
            .expect("run");
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert_eq!(out.status.code(), Some(0), "threads {threads}: {err}");
        assert!(
            out.stdout == want.as_bytes(),
            "threads {threads}: the rows differ from the expected 0..{}",
            n * 3
        );
        eprintln!("10k files, ulimit -n 256, threads {threads}, footers ahead {ahead}: {err}");
        let t: u64 = threads.parse().expect("threads");
        let k: u64 = ahead.parse().expect("ahead");
        assert_eq!(report_field(&err, "files"), n as u64);
        assert_eq!(report_field(&err, "files_opened"), n as u64, "{err}");
        assert_eq!(report_field(&err, "rows_returned"), (n * 3) as u64);
        assert_eq!(report_field(&err, "row_groups"), n as u64);
        assert!(
            report_field(&err, "peak_open_files") <= t + 1,
            "handles are bounded by the threads (plus the footer reader): {err}"
        );
        assert!(
            report_field(&err, "peak_footers") <= t + k + 1,
            "footers are bounded by the threads and the footers ahead: {err}"
        );
    }
    // the unordered mode and a limit under the same ulimit
    let out = Command::new("sh")
        .arg("-c")
        .arg("ulimit -n 256 && exec \"$0\" \"$@\"")
        .arg(
            std::env::var_os("NATA_RSS_BINARY")
                .unwrap_or_else(|| env!("CARGO_BIN_EXE_nata").into()),
        )
        .arg("scan")
        .args(&files)
        .args([
            "--columns",
            "id",
            "--threads",
            "8",
            "--unordered",
            "--footers-ahead",
            "8",
            "--limit",
            "25",
            "--csv",
        ])
        .output()
        .expect("run");
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert_eq!(String::from_utf8_lossy(&out.stdout).lines().count(), 26);
    eprintln!("10k files, ulimit -n 256, unordered, limit 25: {err}");
    assert!(
        report_field(&err, "files_opened") < 200,
        "a limit does not read the footers of 10,000 files, also with footers read ahead: {err}"
    );
}

/// One scan of `file` with a slow reader of stdout; returns the report line.
fn slow_consumer_scan(file: &Path, extra: &[&str]) -> String {
    let mut child = nata_cmd()
        .arg("scan")
        .arg(file)
        .args(["--csv"])
        .args(extra)
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
        // a consumer slower than the decoder: at most about 130 MB/s
        std::thread::sleep(Duration::from_micros(500));
    }
    let status = child.wait().expect("wait");
    let err = err_reader.join().expect("stderr thread");
    assert!(status.success(), "{err}");
    assert!(total > 0);
    err
}

#[test]
fn peak_memory_is_bounded_by_the_window_times_the_largest_row_group_and_an_unbounded_window_is_not()
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
    // `groups` row groups of 20,000 rows: an id and a 100-byte text that does not compress (about 2 MB decoded per row group)
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
    // M: the decoded size of the largest row group, by an independent reader (parquet-rs, no filter, no dictionary preserved)
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
    // the process's own baseline: start, read the footer, read nothing
    let base_report = {
        let out = nata_cmd()
            .arg("scan")
            .arg(&file)
            .args(["--csv", "--limit", "0", "--threads", "1"])
            .output()
            .expect("run");
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    let base_kib = report_field(&base_report, "peak_rss_kib");
    // the bound: the baseline, K x window x M for the batch being filtered and its copy, and F for the CSV buffers, the stack of the threads and the allocator
    const K: f64 = 3.0;
    let fixed_kib = 32 * 1024u64;
    let bound_kib = |window: u64| base_kib + (K * (window * m) as f64 / 1024.0) as u64 + fixed_kib;

    for (threads, extra) in [
        (4u64, vec!["--threads", "4"]),
        (4, vec!["--threads", "4", "--unordered"]),
        (1, vec!["--threads", "1"]),
    ] {
        let err = slow_consumer_scan(&file, &extra);
        let peak = report_field(&err, "peak_rss_kib");
        // the bound is in terms of the window the scan really used (its default is 2 x threads since 2026-11-06)
        let window = report_field(&err, "window");
        assert!(
            window >= threads,
            "the window ({window}) is at least the thread count ({threads}): {err}"
        );
        let bound = bound_kib(window);
        assert!(
            peak <= bound,
            "{extra:?}: peak RSS {peak} KiB is above the bound {bound} KiB (base {base_kib}, M = {m} bytes, window {window}): {err}"
        );
        // our own accounting is within the window times the largest task, and the largest task is within K x M
        let inflight = report_field(&err, "peak_inflight_bytes");
        let largest = report_field(&err, "largest_task_bytes");
        assert!(inflight <= window * largest, "{err}");
        assert!(
            (largest as f64) <= K * m as f64,
            "the scan's own accounting of one row group ({largest}) is within {K} x the independent size {m}"
        );
        eprintln!(
            "{extra:?}: peak RSS {peak} KiB <= bound {bound} KiB; accounted peak {inflight} B, largest task {largest} B, independent M {m} B, base {base_kib} KiB"
        );
    }
    // the same scan with no bound: it must exceed the bound, or the test above shows nothing
    let err = slow_consumer_scan(&file, &["--threads", "4", "--window", "100000"]);
    let peak = report_field(&err, "peak_rss_kib");
    let bound = bound_kib(4);
    eprintln!(
        "unbounded window: peak RSS {peak} KiB, the bound for window 4 is {bound} KiB: {err}"
    );
    assert!(
        peak > bound,
        "the unbounded window must break the bound (peak {peak} KiB, bound {bound} KiB), or the data is too small to show it"
    );
}
