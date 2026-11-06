//! FAULT TOLERANCE and SECURITY at the size of the card (rows "Fault tolerance" and "Security and privacy"): byte flips, truncations, zeroed and random
//! ranges of valid files, 3,000 cases per file shape, five shapes, each case scanned with 1 and with 4 threads. For every case:
//!   * no panic (an `Internal` error is a caught panic and fails the test), no hang (20 s per case), no handle, footer or byte left alive;
//!   * an error names the FILE, and the row group where it carries one; no error text contains a planted cell value;
//!   * 1 thread and 4 threads agree: the same error text, or the same rows (the lowest error wins, and the order is the single-thread order).
//!
//! Plain Parquet pages have no checksum by default, so a flipped value byte can decode to a different valid value: outcomes are CLASSIFIED (error /
//! same rows / different rows) and the counts are printed, not hidden. Three more shapes are files WRITTEN WITH page checksums (R4: `tests/fixtures/`, made by
//! pyarrow; parquet-rs cannot write them): with the verification compiled in (opt-in cargo feature `page-crc`) the damage that stays inside the page region
//! must never read as other rows (class 0); the damage that touches the footer is not covered by a page checksum and is counted apart. Built WITHOUT the
//! feature (`--no-default-features`) the same damage MUST give a non-zero class: that is the known-bad build, the proof that this check can fail. `NATA_FUZZ_CASES` (default 3000 per shape), `NATA_FUZZ_SEED` (default
//! 20261106); a failure prints `shape, case, seed` and the mutation, and `NATA_FUZZ_ONLY=<shape>:<case>` replays it.

mod common;
use common::faults::FaultOpener;
use common::*;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use arrow_array::{
    ArrayRef, BooleanArray, Date32Array, Decimal128Array, Float64Array, Int32Array, Int64Array,
    RecordBatch, StringArray,
};
use nata_parquet::{ErrorKind, ParallelConfig, ScanOptions, ScanPlan, csv, run_parallel};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, Encoding};
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};
use parquet::schema::types::ColumnPath;

const MARKER: &str = "ZQXJ-PLANTED-7731";
const NUMBER: i64 = 987_654_321;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn write_batches(path: &Path, batches: &[RecordBatch], props: WriterProperties) {
    let mut w = ArrowWriter::try_new(
        File::create(path).expect("create"),
        batches[0].schema(),
        Some(props),
    )
    .expect("writer");
    for b in batches {
        w.write(b).expect("write");
        w.flush().expect("flush"); // one row group per batch
    }
    w.close().expect("close");
}

fn lineitem_like(path: &Path) {
    let mut batches = vec![];
    for g in 0..4i64 {
        let n = 1500usize;
        let base = g * n as i64;
        let cols: Vec<(&str, ArrayRef)> = vec![
            (
                "l_orderkey",
                Arc::new(Int64Array::from_iter_values(base..base + n as i64)),
            ),
            (
                "l_partkey",
                Arc::new(Int32Array::from_iter_values((0..n as i32).map(|i| i % 211))),
            ),
            (
                "l_quantity",
                Arc::new(
                    Decimal128Array::from_iter_values((0..n as i128).map(|i| 100 * (1 + i % 50)))
                        .with_precision_and_scale(15, 2)
                        .expect("decimal"),
                ),
            ),
            (
                "l_extendedprice",
                Arc::new(
                    Decimal128Array::from_iter_values((0..n as i128).map(|i| 90_000 + i * 37))
                        .with_precision_and_scale(15, 2)
                        .expect("decimal"),
                ),
            ),
            (
                "l_returnflag",
                Arc::new(StringArray::from_iter_values(
                    (0..n).map(|i| ["A", "N", "R"][i % 3]),
                )),
            ),
            (
                "l_shipdate",
                Arc::new(Date32Array::from_iter_values(
                    (0..n as i32).map(|i| 8000 + i % 2500),
                )),
            ),
            (
                "l_comment",
                Arc::new(StringArray::from_iter_values(
                    (0..n).map(|i| format!("{MARKER} c{}", i % 97)),
                )),
            ),
            (
                "l_secret",
                Arc::new(Int64Array::from_iter_values((0..n).map(|_| NUMBER))),
            ),
        ];
        batches.push(RecordBatch::try_from_iter(cols).expect("batch"));
    }
    write_batches(path, &batches, WriterProperties::builder().build());
}

fn nulls_and_nans(path: &Path) {
    let mut batches = vec![];
    for (g, n) in [1000usize, 1, 500, 7].into_iter().enumerate() {
        let all_null = g == 2;
        let f: Float64Array = (0..n)
            .map(|i| {
                if all_null || i % 11 == 0 {
                    None
                } else {
                    Some(match i % 5 {
                        0 => f64::NAN,
                        1 => -0.0,
                        2 => f64::INFINITY,
                        _ => i as f64 * 0.25,
                    })
                }
            })
            .collect();
        let k: Int32Array = (0..n)
            .map(|i| {
                if all_null || i % 3 == 0 {
                    None
                } else {
                    Some(i as i32)
                }
            })
            .collect();
        let s: StringArray = (0..n)
            .map(|i| {
                if all_null || i % 4 == 0 {
                    None
                } else {
                    Some(format!("{MARKER}-{}", i % 13))
                }
            })
            .collect();
        let b: BooleanArray = (0..n)
            .map(|i| {
                if all_null || i % 5 == 0 {
                    None
                } else {
                    Some(i % 2 == 0)
                }
            })
            .collect();
        let secret: Int64Array = (0..n).map(|_| Some(NUMBER)).collect();
        batches.push(
            RecordBatch::try_from_iter([
                ("f", Arc::new(f) as ArrayRef),
                ("k", Arc::new(k)),
                ("s", Arc::new(s)),
                ("b", Arc::new(b)),
                ("secret", Arc::new(secret)),
            ])
            .expect("batch"),
        );
    }
    write_batches(path, &batches, WriterProperties::builder().build());
}

fn many_small_pages(path: &Path) {
    let mut batches = vec![];
    for g in 0..3i64 {
        let n = 3000usize;
        batches.push(
            RecordBatch::try_from_iter([
                (
                    "id",
                    Arc::new(Int64Array::from_iter_values(g * 3000..g * 3000 + n as i64))
                        as ArrayRef,
                ),
                (
                    "s",
                    Arc::new(StringArray::from_iter_values(
                        (0..n).map(|i| format!("{MARKER}-{}", i % 1000)),
                    )),
                ),
                (
                    "secret",
                    Arc::new(Int64Array::from_iter_values((0..n).map(|_| NUMBER))),
                ),
            ])
            .expect("batch"),
        );
    }
    let props = WriterProperties::builder()
        .set_data_page_size_limit(200)
        .set_write_batch_size(16)
        .set_dictionary_page_size_limit(600)
        .set_statistics_enabled(EnabledStatistics::Page)
        .build();
    write_batches(path, &batches, props);
}

fn v2_zstd_delta(path: &Path) {
    let mut batches = vec![];
    for g in 0..3i64 {
        let n = 2000usize;
        batches.push(
            RecordBatch::try_from_iter([
                (
                    "id",
                    Arc::new(Int64Array::from_iter_values(g * 2000..g * 2000 + n as i64))
                        as ArrayRef,
                ),
                (
                    "s",
                    Arc::new(StringArray::from_iter_values(
                        (0..n).map(|i| format!("{MARKER}-{i}")),
                    )),
                ),
                (
                    "secret",
                    Arc::new(Int64Array::from_iter_values((0..n).map(|_| NUMBER))),
                ),
            ])
            .expect("batch"),
        );
    }
    let props = WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .set_compression(Compression::ZSTD(Default::default()))
        .set_dictionary_enabled(false)
        .set_column_encoding(ColumnPath::from("id"), Encoding::DELTA_BINARY_PACKED)
        .set_column_encoding(ColumnPath::from("s"), Encoding::DELTA_BYTE_ARRAY)
        .set_column_encoding(ColumnPath::from("secret"), Encoding::DELTA_BINARY_PACKED)
        .build();
    write_batches(path, &batches, props);
}

fn tiny_and_empty_groups(path: &Path) {
    // a one-row group, then an empty one (zero rows are flushed as a group only if the writer makes it), then another
    let b1 = RecordBatch::try_from_iter([
        ("id", Arc::new(Int64Array::from(vec![NUMBER])) as ArrayRef),
        ("s", Arc::new(StringArray::from(vec![MARKER]))),
    ])
    .expect("batch");
    let b0 = RecordBatch::try_from_iter([
        (
            "id",
            Arc::new(Int64Array::from(Vec::<i64>::new())) as ArrayRef,
        ),
        ("s", Arc::new(StringArray::from(Vec::<&str>::new()))),
    ])
    .expect("batch");
    write_batches(
        path,
        &[b1.clone(), b0, b1],
        WriterProperties::builder().build(),
    );
}

/// A file written with page checksums (a committed fixture, see `tests/fixtures/make_checksummed.py`), copied to `path`.
fn copy_fixture(name: &str, path: &Path) {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{name}.parquet"));
    std::fs::copy(&src, path).unwrap_or_else(|e| panic!("{}: {e}", src.display()));
}

fn checksummed_lineitem_like(path: &Path) {
    copy_fixture("checksummed_lineitem_like", path);
}

fn checksummed_v2_zstd(path: &Path) {
    copy_fixture("checksummed_v2_zstd", path);
}

fn checksummed_nulls_dict(path: &Path) {
    copy_fixture("checksummed_nulls_dict", path);
}

type Shape = (&'static str, fn(&Path));

const SHAPES: [Shape; 8] = [
    ("lineitem_like", lineitem_like),
    ("nulls_nans_allnull_group", nulls_and_nans),
    ("many_small_pages", many_small_pages),
    ("v2_zstd_delta", v2_zstd_delta),
    ("tiny_and_empty_groups", tiny_and_empty_groups),
    ("checksummed_lineitem_like", checksummed_lineitem_like),
    ("checksummed_v2_zstd", checksummed_v2_zstd),
    ("checksummed_nulls_dict", checksummed_nulls_dict),
];

/// Apply a random damage to `clean`; returns the damaged bytes and a description.
fn damage(clean: &[u8], rng: &mut Rng) -> (Vec<u8>, String) {
    let len = clean.len();
    let footer_len = if len >= 8 {
        u32::from_le_bytes([
            clean[len - 8],
            clean[len - 7],
            clean[len - 6],
            clean[len - 5],
        ]) as usize
    } else {
        0
    };
    let footer_start = len.saturating_sub(8 + footer_len).max(4);
    let mut b = clean.to_vec();
    let kind = rng.below(9);
    let desc = match kind {
        0 => {
            let (i, bit) = (rng.below(len), rng.below(8));
            b[i] ^= 1 << bit;
            format!("flip bit {bit} of byte {i}")
        }
        1 => {
            let n = 1 + rng.below(8);
            let mut at = vec![];
            for _ in 0..n {
                let i = 4 + rng.below(footer_start.saturating_sub(4).max(1));
                let i = i.min(len - 1);
                b[i] ^= 1 << rng.below(8);
                at.push(i);
            }
            format!("flip {n} bits in the page region at {at:?}")
        }
        2 => {
            let n = 1 + rng.below(6);
            let mut at = vec![];
            for _ in 0..n {
                let i = footer_start + rng.below(len - footer_start);
                b[i] ^= 1 << rng.below(8);
                at.push(i);
            }
            format!("flip {n} bits in the footer region at {at:?}")
        }
        3 => {
            let cut = rng.below(len);
            b.truncate(cut);
            format!("truncate to {cut} bytes")
        }
        4 => {
            let (s, l) = (rng.below(len), 1 + rng.below(64));
            let e = (s + l).min(len);
            b[s..e].fill(0);
            format!("zero bytes {s}..{e}")
        }
        5 => {
            let (s, l) = (rng.below(len), 1 + rng.below(64));
            let e = (s + l).min(len);
            for x in &mut b[s..e] {
                *x = rng.next() as u8;
            }
            format!("random bytes {s}..{e}")
        }
        6 => {
            let cut = len / 2 + rng.below(len / 2);
            b.truncate(cut);
            let i = rng.below(cut.max(1));
            if cut > 0 {
                b[i] ^= 1 << rng.below(8);
            }
            format!("truncate to {cut} and flip a bit of byte {i}")
        }
        7 => {
            let (s, d, l) = (rng.below(len), rng.below(len), 1 + rng.below(128));
            let chunk: Vec<u8> = b[s..(s + l).min(len)].to_vec();
            for (k, x) in chunk.iter().enumerate() {
                if d + k < len {
                    b[d + k] = *x;
                }
            }
            format!("copy {} bytes from {s} over {d}", chunk.len())
        }
        _ => {
            // the footer length field and the magic numbers
            let i = len - 1 - rng.below(8.min(len));
            b[i] = rng.next() as u8;
            format!("random byte at {i} (footer length or magic)")
        }
    };
    (b, desc)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Result_ {
    Rows(String),
    Error { text: String, kind_internal: bool },
}

fn scan_once(path: &Path, threads: usize, opener: Arc<FaultOpener>) -> (Result_, Arc<ScanPlan>) {
    let opts = ScanOptions {
        columns: None,
        filters: vec![],
        prune: true,
        late_materialization: late_default(),
        strict_schema: false,
    };
    let paths = [path.to_path_buf()];
    let plan = match ScanPlan::open_with(&paths, &opts, opener) {
        Ok(p) => Arc::new(p),
        Err(e) => {
            // no plan: build an empty one for the gauges' sake is impossible; report the error with a placeholder plan from a valid file
            return (
                Result_::Error {
                    text: e.to_string(),
                    kind_internal: matches!(e.kind, ErrorKind::Internal(_)),
                },
                placeholder(),
            );
        }
    };
    let mut out: Vec<u8> = Vec::new();
    let names: Vec<String> = plan.out_schema().iter().map(|(n, _)| n.clone()).collect();
    csv::write_header(&mut out, &names).expect("header");
    let r = run_parallel(Arc::clone(&plan), ParallelConfig::new(threads), None, |b| {
        csv::write_batch(&mut out, b)
    });
    let res = match r {
        Ok(_) => Result_::Rows(String::from_utf8_lossy(&out).into_owned()),
        Err(e) => Result_::Error {
            text: e.to_string(),
            kind_internal: matches!(e.kind, ErrorKind::Internal(_)),
        },
    };
    (res, plan)
}

fn placeholder() -> Arc<ScanPlan> {
    static P: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let p = P.get_or_init(|| {
        let d = scratch("fuzz-placeholder");
        let f = d.join("p.parquet");
        tiny_and_empty_groups(&f);
        f
    });
    let plan = ScanPlan::open(
        std::slice::from_ref(p),
        &ScanOptions {
            columns: None,
            filters: vec![],
            prune: true,
            late_materialization: late_default(),
            strict_schema: false,
        },
    )
    .expect("plan");
    // a scan that reads nothing releases the footer the plan keeps for its first file
    nata_parquet::run_serial(&plan, Some(0), |_| Ok(())).expect("empty scan");
    Arc::new(plan)
}

fn thread_count() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Threads:"))
                .and_then(|l| l.split_whitespace().nth(1).and_then(|n| n.parse().ok()))
        })
        .unwrap_or(0)
}

#[test]
fn damaged_files_never_panic_hang_leak_or_leak_a_value() {
    // the panics injected into the opener are expected: the scan must report them as errors. Any other panic is printed.
    std::panic::set_hook(Box::new(|info| {
        let injected = info
            .payload()
            .downcast_ref::<&str>()
            .is_some_and(|m| m.contains("injected panic"));
        // panics inside parquet/arrow on a damaged page are expected here: the scan catches and reports them (counted in the table)
        let in_dependency = info
            .location()
            .is_some_and(|l| l.file().contains(".cargo/registry"));
        if in_dependency
            && std::env::var("NATA_FUZZ_SHOW_PANICS").is_ok()
            && let Some(l) = info.location()
        {
            eprintln!("DEPENDENCY-PANIC {}:{}", l.file(), l.line());
        }
        if !injected && !in_dependency {
            eprintln!("{info}");
        }
    }));
    let seed: u64 = std::env::var("NATA_FUZZ_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20_261_106);
    let cases: usize = std::env::var("NATA_FUZZ_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000);
    let only: Option<(String, usize)> = std::env::var("NATA_FUZZ_ONLY").ok().and_then(|v| {
        let (s, c) = v.split_once(':')?;
        Some((s.to_string(), c.parse().ok()?))
    });
    eprintln!(
        "fuzz: seed {seed}, {cases} cases per shape, {} shapes, 1 and 4 threads (replay: NATA_FUZZ_SEED={seed} NATA_FUZZ_ONLY=<shape>:<case>)",
        SHAPES.len()
    );
    // The damaged copy is rewritten for every case (a few hundred KB, one at a time). Scratch may be on disk; unlink the previous case before
    // recreating it below, so truncation does not wait for its dirty pages to be written back. The directory is removed at the end.
    let dir = std::env::temp_dir().join(format!("nata-fuzz-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(dir.clone());
    let threads_before = thread_count();
    let mut table = vec![];
    for (name, make) in SHAPES {
        let clean_path = dir.join(format!("{name}.parquet"));
        make(&clean_path);
        let clean = std::fs::read(&clean_path).expect("read");
        let mut clean_rows = vec![];
        for t in [1usize, 4] {
            match scan_once(
                &clean_path,
                t,
                FaultOpener::new(std::slice::from_ref(&clean_path), vec![]),
            )
            .0
            {
                Result_::Rows(r) => clean_rows.push(r),
                e => panic!("the clean {name} file must scan: {e:?}"),
            }
        }
        assert_eq!(
            clean_rows[0], clean_rows[1],
            "{name}: clean scan at 1 and 4 threads"
        );
        assert!(
            clean_rows[0].contains(MARKER)
                || name == "tiny_and_empty_groups"
                || clean_rows[0].contains(&NUMBER.to_string()),
            "{name}: the planted values are in the output"
        );
        let (mut errors, mut same, mut different, mut panics) = (0usize, 0usize, 0usize, 0usize);
        // different rows when the damage stayed inside the page region (the bytes between the leading magic and the footer, the length unchanged)
        let mut different_in_pages = 0usize;
        let footer_start = {
            let len = clean.len();
            let fl =
                u32::from_le_bytes(clean[len - 8..len - 4].try_into().expect("4 bytes")) as usize;
            len.saturating_sub(8 + fl).max(4)
        };
        for case in 0..cases {
            if only.as_ref().is_some_and(|(s, c)| s != name || *c != case) {
                continue;
            }
            let mut rng = Rng(seed
                ^ (case as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93)
                ^ (name.len() as u64) << 40);
            let (bytes, how) = damage(&clean, &mut rng);
            let path = dir.join(format!("{name}-case.parquet"));
            if path.exists() {
                std::fs::remove_file(&path).expect("unlink previous case");
            }
            std::fs::write(&path, &bytes).expect("write");
            let what = format!("shape {name}, case {case}, seed {seed}: {how}");
            let mut outcomes = vec![];
            for t in [1usize, 4] {
                let (tx, rx) = mpsc::channel();
                let p = path.clone();
                let opener = FaultOpener::new(std::slice::from_ref(&path), vec![]);
                let o2 = opener.clone();
                // the case runs in a helper thread so that a hang becomes a timeout; it is JOINED after its result arrives, so that the final
                // count of live threads sees only threads of the scan (found 2026-11-06 under AddressSanitizer: an unjoined helper still
                // exiting was counted as "a thread of a scan is still alive")
                let helper = std::thread::spawn(move || {
                    let (res, plan) = scan_once(&p, t, o2);
                    let st = plan.stats();
                    let clean_state = (st.handles.now(), st.footers.now(), st.memory.now());
                    let _ = tx.send((res, clean_state));
                });
                let (res, state) = rx
                    .recv_timeout(Duration::from_secs(20))
                    .unwrap_or_else(|_| panic!("HANG ({t} threads): {what}"));
                helper.join().expect("the fuzz case's helper thread");
                assert_eq!(
                    state,
                    (0, 0, 0),
                    "{what}: handles, footers, bytes left alive ({t} threads)"
                );
                assert_eq!(
                    opener.live_handles(),
                    0,
                    "{what}: a handle was not closed ({t} threads)"
                );
                if let Result_::Error {
                    text,
                    kind_internal,
                } = &res
                {
                    if *kind_internal {
                        // A panic inside a dependency (parquet/arrow) on a corrupt page, caught at the task boundary and reported as an error that
                        // names the file and the row group. It is counted and printed as its own class; it must stay exactly this: no message of
                        // the panic in the text, the row group named, the same at 1 and 4 threads.
                        assert!(
                            text.contains("a panic while reading") && text.contains("row group "),
                            "{what}: a caught panic must name the row group and carry no panic message ({t} threads): {text}"
                        );
                    }
                    let fname = path.display().to_string();
                    assert!(
                        text.contains(&fname),
                        "{what}: the error does not name the file ({t} threads): {text}"
                    );
                    assert!(
                        !text.contains("ZQXJ")
                            && !text.contains("PLANTED")
                            && !text.contains(&NUMBER.to_string()),
                        "{what}: the error carries a cell value: {text}"
                    );
                    assert!(!text.contains("cancelled"), "{what}: {text}");
                }
                outcomes.push(res);
            }
            assert_eq!(outcomes[0], outcomes[1], "{what}: 1 and 4 threads disagree");
            match &outcomes[0] {
                Result_::Error {
                    kind_internal: true,
                    ..
                } => panics += 1,
                Result_::Error { .. } => errors += 1,
                Result_::Rows(r) if *r == clean_rows[0] => same += 1,
                Result_::Rows(_) => {
                    different += 1;
                    let changed: Vec<usize> = (0..bytes.len().min(clean.len()))
                        .filter(|i| bytes[*i] != clean[*i])
                        .collect();
                    let in_pages = bytes.len() == clean.len()
                        && !changed.is_empty()
                        && changed.iter().all(|i| (4..footer_start).contains(i));
                    if in_pages {
                        different_in_pages += 1;
                        if name.starts_with("checksummed_") && cfg!(feature = "page-crc") {
                            panic!(
                                "{what}: damage inside the page region of a checksummed file read as other rows"
                            );
                        }
                    }
                }
            }
        }
        table.push((name, errors, same, different, panics, different_in_pages));
    }
    // a joined thread leaves /proc a moment after its join returns (seen once under heavy load: 3 threads against 2): wait a little before judging
    let leave_by = std::time::Instant::now() + Duration::from_secs(5);
    while thread_count() != threads_before && std::time::Instant::now() < leave_by {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        thread_count(),
        threads_before,
        "a thread of a scan is still alive"
    );
    eprintln!(
        "fuzz outcome classes per shape (cases = error + caught dependency panic + same rows + different rows; each case ran at 1 and 4 threads with identical outcomes):"
    );
    for (n, e, s, d, p, dp) in &table {
        eprintln!(
            "  {n:<28} error {e:>5}   caught panic {p:>4}   same rows {s:>5}   different rows {d:>5} (damage inside the pages: {dp:>5})   total {}",
            e + s + d + p
        );
    }
    let total: usize = table.iter().map(|(_, e, s, d, p, _)| e + s + d + p).sum();
    let panics: usize = table.iter().map(|t| t.4).sum();
    eprintln!(
        "fuzz total: {total} cases, {} scans; {panics} cases hit a panic inside a dependency that the task boundary caught",
        total * 2
    );
    if only.is_none() {
        for (n, e, _, _, p, _) in &table {
            assert!(
                *e + *p > 0,
                "{n}: the damage never produced an error: the fuzz is not biting"
            );
        }
        // R4. With the verification: no checksummed file read as other rows after damage inside its pages (asserted case by case above). Without it
        // (the known-bad build) the same damage MUST read as other rows in some case, or this check could not fail.
        let pages_other_rows: usize = table
            .iter()
            .filter(|t| t.0.starts_with("checksummed_"))
            .map(|t| t.5)
            .sum();
        if cfg!(feature = "page-crc") {
            assert_eq!(pages_other_rows, 0);
        } else {
            assert!(
                pages_other_rows > 0,
                "without the page checksum the damage inside pages must sometimes read as other rows (it did not in {cases} cases per shape)"
            );
        }
    }
}
