# Deep check of the Parquet scan: items A and B of card 0004

**Verdicts (2026-11-07).**
- **A: the rule is met on the pinned file.** The scan of the fixture ends in 0.00 s with `Corrupt`, naming the file and the row group; without the fix it is still running at the 20 s deadline. `read_bytes_owned`'s `Vec::with_capacity(len)` is **not reachable** on our scan path (source and a runtime probe), and is **an upstream-only risk**, recorded as R6d. The nightly-size damaged-file fuzz for A is **not run here**: it is to be run by the parent on GCP (commands below).
- **B: the damage was outside what a page CRC covers; the test's claim was too wide, and is narrowed.** Case 16900 of shape `checksummed_v2_zstd` flipped one bit in a data page v2 **page header** (`definition_levels_byte_length` 3 to 7). R4's limit is stated in `docs/improvements.md`. No CRC-covered byte was found changed with the check passing in the part of the run that was done (see "What was not run"). The known-bad build (no `page-crc`) is still caught.
- Mutants: 4 of 4 CAUGHT. Checks: all pass (timings below). Nothing was pushed; no GCP.

Code identity: branch `t3/parquet-eof-hang-regression`; the fix is the parent's `nightly-hang-fix.patch` (crates part), whose fixture was not inside the patch ("Binary files differ" without data) and was copied from the parent's worktree (1,189 bytes, `sha256 220a90d5d61b83b99b165d06f5a18201a63aa740a8c9f5075efef2fe41914e19`, identical in two worktrees).

## A: a damaged Parquet file must end the scan with an error

**Rule as written (card 0004, row A).** "A read after end of file is an error (`CountingRead::at_eof`). The scan of the pinned file ends in under 1 s with `Corrupt` naming the file and row group. Also find whether `read_bytes_owned`'s `Vec::with_capacity(len)` can be reached with a huge length (an abort no catch stops). If it can, bound it in our wrapper or record it as an upstream-only risk." Check: "Pinned fixture test: passes with the fix, hangs past its 20 s deadline without it. The damaged-file fuzz at nightly size (30,000 per shape) passes."

**Numbers.**
- Case: fuzz seed 20261107, shape `tiny_and_empty_groups`, case 10072, "random bytes 120..169"; fixture `crates/nata-parquet/tests/fixtures/thrift_skip_past_eof.parquet`, 1,189 bytes.
- Structure (generic Thrift-compact walk of the fixture): the dictionary page header is at 99..113 and its 8-byte payload at 113..121; the next page header starts at 121, so the random bytes 120..169 are the last payload byte and the first 48 bytes of the next page header. Its first field is `0x0a`, a set with a long-form field id, followed by garbage that the skip code reads as element counts and types (the walk meets a type-11 map at byte 166). The exact skip that looped on this file was not traced; the standalone example below reproduces the same behaviour with a list count.
- With the fix: `damaged_regressions::a_page_header_that_claims_bytes_past_the_end_is_an_error_not_a_hang` passes in 0.00 s (release). Without the `at_eof` check (edited in place, then restored): `FAILED ... the scan of a damaged page header must end (it hung before the fix): Timeout`, finished in 20.00 s (wall 24.4 s with the build).
- `read_bytes_owned` (parquet 60.1.0 source, `parquet_thrift.rs:641`, `ThriftReadInputProtocol`): the length comes from the file and goes to `Vec::with_capacity(len)`. Its callers are `String::read_thrift` (no page-header field is a string) and the `binary` arm of `thrift_struct!` (`parquet_macros.rs:558`); in a page header the only binary fields are the four of `PageStatistics` (`max`, `min`, `max_value`, `min_value`). Those are decoded only by `PageHeader::read_thrift`, chosen when `SerializedPageReaderContext::read_stats` is true, which is `ReaderProperties::read_page_stats` (default `false`, `properties.rs:2078`). Our reader goes through the Arrow reader, which builds its page readers with `SerializedPageReader::new` (`arrow_reader/mod.rs:1541`), whose properties are the defaults: so it calls `read_thrift_without_stats`, and the statistics field is skipped by `skip_binary` then `skip_bytes` (`io::copy` into a sink: nothing reserved). The other user, `read_column_dictionary` (`dictionary.rs:87`, `read_stats: true`), is a public function our code does not call. The column chunk's byte range does **not** bound the page-header read (`get_read(start)` of a `File` runs to the end of the file), so a bound by range would not have helped; it is not needed.
- Runtime probe: `damaged_regressions::a_page_header_whose_statistics_claim_a_terabyte_is_an_error_not_an_allocation` writes a file with page-header statistics (`set_write_page_header_statistics(true)`), rewrites the length of the first statistics field to 2^40 and scans it: an error (`Corrupt`, naming the file), the process lives, and the test passes under `ulimit -v 6000000`. The known-bad arm is the standalone example below: with `set_read_page_statistics(true)` the same file aborts with `memory allocation of 1099511627776 bytes failed` (exit 134).

**Verdict by the rule.** The scan of the pinned file ends in under 1 s with `Corrupt` naming the file and row group: **met**. The pinned test hangs past its 20 s deadline without the fix: **shown**. `read_bytes_owned`'s reservation cannot be reached from our scan path: **upstream-only risk** (R6d), pinned by the probe test so that a future parquet-rs that turns page statistics on for the Arrow reader makes the test binary abort instead of pass. The fuzz at nightly size: **to be run by the parent on GCP**.

**Reproduce.** `cargo test --release -p nata-parquet --test damaged_regressions` (both tests); `python3 crates/nata-parquet/mutate.py damaged-eof-check-removed damaged-eof-never-recorded`.

## B: a checksummed file read as other rows with `page-crc` on

**Rule as written (card 0004, row B).** "Find the case. If the damage is outside the bytes a page CRC covers (a page header), the test's claim is too wide: narrow it to the CRC-covered payload and state R4's limit in `docs/improvements.md`. If CRC-covered bytes changed and the check passed, it is a bug: fix it." Check: "Replayed case, before and after. A mutant that skips verification is still caught."

**Numbers.**
- Case: seed 20261107, shape `checksummed_v2_zstd`, case **16900**, "flip bit 3 of byte 826", file of 11,588 bytes (the parent's report said `fuzz_files.rs:657`; the same assertion is at line 664 here after this change added lines).
- Structure (a throwaway Python Thrift-compact walk of the footer and every page header, not kept in the repo; it also checks each page's CRC): bytes 800..878 are the header of page 1 (a data page v2) of column 0 of row group 0; its payload is 878..1450 (572 bytes), whose CRC `98139411` matches in the clean and in the damaged file. Byte 826 is inside field 5 of the `DataPageHeaderV2`, `definition_levels_byte_length`: **3 became 7**. Every other header field and every payload byte is unchanged. The footer is unchanged (the damage is before it).
- Before (old test, fix A applied): `shape checksummed_v2_zstd, case 16900, seed 20261107: flip bit 3 of byte 826: damage inside the page region of a checksummed file read as other rows`, test FAILED in 0.24 s on replay.
- After (narrowed test): the same replay passes in 0.30 s and classifies the case as `different rows 1 (damage inside the pages: 1; of those, payload bytes only 0, touching a page header 1)`.
- New change: `crc_covered_bytes` in `tests/fuzz_files.rs` computes the payload ranges of the clean file from the footer's column chunk ranges and a walk over the page headers (a small Thrift compact reader, 60 lines). Damage whose changed bytes all fall in payloads is asserted to be an error or the same rows with the check on, and to give other rows in some case without it. Damage that touches a header (or the bytes between pages) is counted apart in the table. `NATA_FUZZ_ONLY=<shape>` now runs one whole shape (for example a checksummed shape alone) as well as `<shape>:<case>`.
- Pinned limit: `tests/checksums.rs::a_page_header_is_outside_the_page_checksum_but_its_payload_is_inside` (passes in both builds): the header flip reads as other rows, and a flipped bit in the same page's payload is a `Corrupt` error naming the CRC (with the check on).
- Default-size fuzz (3,000 cases per shape, seed 20261106, 8 shapes, 1 and 4 threads), class table of the three checksummed shapes:

| Build | Shape | error | same rows | different rows | of those: payload bytes only | touching a page header |
|---|---|---:|---:|---:|---:|---:|
| `page-crc` | `checksummed_lineitem_like` | 2,911 | 89 | 0 | 0 | 0 |
| `page-crc` | `checksummed_v2_zstd` | 2,855 | 145 | 0 | 0 | 0 |
| `page-crc` | `checksummed_nulls_dict` | 2,874 (+1 caught panic) | 125 | 0 | 0 | 0 |
| none (known-bad) | `checksummed_lineitem_like` | 1,839 | 89 | 1,072 | 1,066 | 6 |
| none (known-bad) | `checksummed_v2_zstd` | 2,077 | 154 | 769 | 734 | 35 |
| none (known-bad) | `checksummed_nulls_dict` | 2,007 (+2) | 132 | 859 | 791 | 68 |

Without verification, 2,591 payload-damage cases read as other rows; with it, none. That is the check that can fail.

**Verdict by the rule.** The changed byte is in a page header, outside the bytes a page CRC covers: the rule says the claim is too wide, narrow it and state R4's limit. **Done** (`tests/fuzz_files.rs`, `docs/improvements.md` R4, `crates/nata-parquet/README.md`). It is not a bug in the verification: no CRC-covered byte changed in this case. The known-bad build stays caught (table above).

**Reproduce.** The case, before: check out `dbd1cbf` with the A fix and run `NATA_FUZZ_SEED=20261107 NATA_FUZZ_CASES=30000 NATA_FUZZ_ONLY=checksummed_v2_zstd:16900 cargo test --release -p nata-parquet --features page-crc --test fuzz_files -- --nocapture` (fails); after: the same command on this branch (passes). The table: `cargo test --release -p nata-parquet [--features page-crc] --test fuzz_files -- --nocapture` (default sizes).

## What was run at nightly size, and what was not

The brief changed during the work: heavy runs go to GCP. Before that change, two single shapes were run at nightly size to find B's case (diagnosis only, one process at a time, seed 20261107, 30,000 cases, release, `--features page-crc`, old test code with A's fix): `checksummed_lineitem_like` passed (310 s); `checksummed_v2_zstd` failed at case 16900 (208 s, on a VM at load average 16 to 25). A third, `checksummed_nulls_dict`, was stopped when the instruction arrived and has no result. **No full run, and no run of the narrowed test at 30,000 cases, has been done.** So it is **unknown** whether the rest of `checksummed_v2_zstd` or `checksummed_nulls_dict` holds a CRC-covered case that read as other rows; the narrowed test asserts exactly that, and the run below decides it.

**To be run by the parent on GCP (generic `heavy` job), results to be added here:**

```
NATA_FUZZ_SEED=20261107 NATA_FUZZ_CASES=30000 cargo test --release -p nata-parquet --features page-crc --test fuzz_files -- --nocapture
NATA_FUZZ_SEED=20261108 NATA_FUZZ_CASES=30000 cargo test --release -p nata-parquet --features page-crc --test fuzz_files -- --nocapture
```

| Seed | Result | Wall time | Machine |
|---|---|---|---|
| 20261107 | **PASS**, exit 0: 240,000 cases, 480,000 scans, no hang; checksummed shapes: 4 / 2 / 1 different rows, all touching a page header or the footer; payload bytes only: **0**; 580 dependency panics caught at the task boundary | 836 s | GCP Spot c3d-highcpu-16, run `card0004-heavy3`, 2026-11-07, main `21c7e37` (code tree `d2131a21`) |
| 20261108 | **PASS**, exit 0: 240,000 cases, 480,000 scans, no hang; checksummed shapes: different rows only where a page header or the footer changed; payload bytes only: **0**; 643 dependency panics caught at the task boundary | 845 s | GCP Spot c3d-highcpu-16, run `card0004-heavy3`, 2026-11-07, main `21c7e37` (code tree `d2131a21`) |

If a case fails with CRC-covered bytes changed, it is a bug in the verification (the card's rule): replay it with `NATA_FUZZ_ONLY=<shape>:<case>`. Neither run had one. Raw outputs: `raw/20261107-card0004-heavy/` (the four runs ran in parallel on one VM; $0.039 for the VM, orchestrator estimate).

## Mutants

Run with `python3 crates/nata-parquet/mutate.py --jobs 2 damaged-eof-check-removed damaged-eof-never-recorded crc-on-by-default-in-nata-parquet crc-on-by-default-in-the-nata-binary` (baseline 456 s, total 881 s, VM at high load): **4 CAUGHT, 0 SURVIVED, 0 INVALID**.

| Mutant | Edit | Result | Caught by |
|---|---|---|---|
| `damaged-eof-check-removed` | `if self.at_eof && ...` becomes `if false && self.at_eof && ...` | CAUGHT | `damaged_regressions::a_page_header_that_claims_bytes_past_the_end_is_an_error_not_a_hang`, through its 20 s deadline |
| `damaged-eof-never-recorded` | the `self.at_eof = true;` on `Ok(0)` removed | CAUGHT | the same test |
| `crc-on-by-default-in-nata-parquet` (existing) | `default = ["page-crc"]` | CAUGHT | `checksums::page_checksums_are_opt_in` |
| `crc-on-by-default-in-the-nata-binary` (existing) | the same in `nata` | CAUGHT | `scan_cli::page_checksums_of_the_binary_are_opt_in` |

The mutant that skips verification is the build without the feature: the fuzz class table above (payload damage read as other rows: 1,066, 734 and 791 cases) and `checksums::every_flipped_byte_of_the_pages_...` (which asserts `different > 0` without the feature) are its evidence, and both still hold. `mutate.py` gains a `damaged-*` prefix whose group list runs `damaged_regressions` first.

## Checks (this VM, load average 16 to 25 from other work, so times are upper bounds)

| Check | Result | Time |
|---|---|---|
| `cargo fmt --check` | pass | 3 s |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass | 2 s incremental (2 m 52 s from cold earlier) |
| `cargo test --workspace` (debug, default sizes) | pass | 697 s |
| `cargo test --release -p nata-parquet` (no feature) | pass | 342 s |
| `NATA_CRC_BUILD=1 cargo test --release -p nata-parquet --features page-crc` | pass | 263 s |
| `scripts/check-leaks.sh --staged` | clean | under 1 s |
| `python3 -m unittest test_mutate` | 4 tests pass | under 1 s |

The checks were run once on the combined tree (the sum of the four commits), not once per commit.

## Standalone example for a possible upstream report (not in the repo, owner decides)

A cargo project at `~/parquet-eof-loop` against `parquet = "=60.1.0"` from crates.io (the newest release on the index on 2026-11-07; not run against `main`, which item H needs). Two programs. `parquet-eof-loop` shows the looping read; `read_bytes_owned` shows the allocation.

Output of `cargo run --release -- out` (rates depend on the load of the machine: 0.56 and 1.29 million empty reads per second in two runs, so "between half an hour and an hour"; Nata's own fuzz saw about 10,000 per second through its fault-injecting reader):

```
parquet 60.1.0, file of 940 bytes
valid file:                 Ok(100 rows) in 7.145322ms
damaged: first page header = type, sizes, then list<double> of 2147483647 elements (13 bytes written over the header)
damaged, as it is:          STILL RUNNING (no result) after 5.000158808s; read calls 6429153, of them returning 0 bytes 6429023
   rate 1285764 empty reads/s; the list has 2147483647 elements, so about 0.5 hours at this rate
damaged, second read at EOF refused: Err(External: Parquet argument error: External: read after end of file) after 505.008µs; read calls 131, returning 0 bytes 1
```

Output of `read_bytes_owned default out` and `read_bytes_owned page-stats out`, each under `ulimit -v 4194304`:

```
== default (ulimit -v 4 GiB)
claimed length 2^40 = 1099511627776 bytes, patched at byte 25; mode default
reading the first page ...
Err(External: failed to fill whole buffer)
process still alive
exit status 0
== page-stats (ulimit -v 4 GiB)
claimed length 2^40 = 1099511627776 bytes, patched at byte 25; mode page-stats
reading the first page ...
memory allocation of 1099511627776 bytes failed
/bin/bash: line 1: 1292057 Aborted                 ./target/release/read_bytes_owned $m out
exit status 134
```

`Cargo.toml`:

```toml
[package]
name = "parquet-eof-loop"
version = "0.1.0"
edition = "2021"

[dependencies]
# exactly the release under report; default features off (no compression codecs, no async)
parquet = { version = "=60.1.0", default-features = false, features = ["arrow"] }
arrow-array = "=60.1.0"
bytes = "1"

[profile.release]
debug = false
```

`src/main.rs`:

```rust
//! parquet 60.1.0: a damaged page header makes the streaming Thrift reader loop on end of file.
//!
//! `ThriftReadInputProtocol::skip_bytes` does `io::copy(&mut reader.take(n), &mut sink())` and ignores how many bytes came, and the list branch of
//! `skip_till_depth` loops `for _ in 0..size` over a size read from the file. With a list of 2^31-1 doubles in an unknown page-header field, every
//! element after the end of the file is one more empty read that succeeds with `Ok(0)`: the reader does not fail, and does not finish for hours.
//!
//! The program writes a valid one-column file, overwrites the first bytes of its first page header with a header that has such a list, and reads it
//! with the public API (`ParquetRecordBatchReaderBuilder`), once as it is and once behind a `ChunkReader` that refuses a second read after `Ok(0)`.
use std::fs::File;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arrow_array::{ArrayRef, Int32Array, RecordBatch};
use bytes::Bytes;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::file::reader::{ChunkReader, Length};

/// Counts `read` calls, and the ones that return 0 bytes. With `refuse_after_eof` a read after the first `Ok(0)` is an error.
struct Probe {
    file: File,
    reads: Arc<AtomicU64>,
    empty_reads: Arc<AtomicU64>,
    refuse_after_eof: bool,
}
struct ProbeRead {
    inner: <File as ChunkReader>::T,
    reads: Arc<AtomicU64>,
    empty_reads: Arc<AtomicU64>,
    refuse_after_eof: bool,
    at_eof: bool,
}
impl Read for ProbeRead {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.at_eof && !buf.is_empty() {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "read after end of file"));
        }
        self.reads.fetch_add(1, Ordering::Relaxed);
        let n = self.inner.read(buf)?;
        if n == 0 && !buf.is_empty() {
            self.empty_reads.fetch_add(1, Ordering::Relaxed);
            self.at_eof = self.refuse_after_eof;
        }
        Ok(n)
    }
}
impl Length for Probe {
    fn len(&self) -> u64 {
        self.file.len()
    }
}
impl ChunkReader for Probe {
    type T = ProbeRead;
    fn get_read(&self, start: u64) -> parquet::errors::Result<ProbeRead> {
        Ok(ProbeRead {
            inner: self.file.get_read(start)?,
            reads: self.reads.clone(),
            empty_reads: self.empty_reads.clone(),
            refuse_after_eof: self.refuse_after_eof,
            at_eof: false,
        })
    }
    fn get_bytes(&self, start: u64, length: usize) -> parquet::errors::Result<Bytes> {
        self.file.get_bytes(start, length)
    }
}

fn write_valid(path: &str) {
    let col: ArrayRef = Arc::new(Int32Array::from_iter_values(0..100));
    let batch = RecordBatch::try_from_iter([("a", col)]).unwrap();
    let mut w = ArrowWriter::try_new(File::create(path).unwrap(), batch.schema(), None).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
}

/// The compact-protocol bytes of a page header: type = DATA_PAGE, sizes 1 and 1, then field 10 = list<double> with `size` elements (a field a reader
/// must skip). Nothing after it is needed: the list never ends before the end of the file.
fn bad_header(size: u32) -> Vec<u8> {
    let mut h = vec![0x15, 0x00, 0x15, 0x02, 0x15, 0x02];
    h.push(0x79); // field id +7 (10), type list (9)
    h.push(0xF7); // list header: size follows as a varint, element type double (7)
    let mut v = size;
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            h.push(b);
            break;
        }
        h.push(b | 0x80);
    }
    h
}

fn read_all(path: &str, refuse_after_eof: bool, wait: Duration) -> (String, u64, u64, Duration) {
    let reads = Arc::new(AtomicU64::new(0));
    let empty = Arc::new(AtomicU64::new(0));
    let probe = Probe { file: File::open(path).unwrap(), reads: reads.clone(), empty_reads: empty.clone(), refuse_after_eof };
    let (tx, rx) = mpsc::channel();
    let t0 = Instant::now();
    std::thread::spawn(move || {
        let r = ParquetRecordBatchReaderBuilder::try_new(probe)
            .and_then(|b| Ok(b.build()?))
            .and_then(|r| {
                let mut rows = 0;
                for b in r {
                    rows += b?.num_rows();
                }
                Ok(rows)
            });
        let _ = tx.send(match r {
            Ok(rows) => format!("Ok({rows} rows)"),
            Err(e) => format!("Err({e})"),
        });
    });
    let outcome = rx.recv_timeout(wait).unwrap_or_else(|_| "STILL RUNNING (no result)".to_string());
    (outcome, reads.load(Ordering::Relaxed), empty.load(Ordering::Relaxed), t0.elapsed())
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let good = format!("{dir}/good.parquet");
    let bad = format!("{dir}/bad.parquet");
    write_valid(&good);
    println!("parquet 60.1.0, file of {} bytes", std::fs::metadata(&good).unwrap().len());
    let (o, _, _, t) = read_all(&good, false, Duration::from_secs(10));
    println!("valid file:                 {o} in {t:?}");

    let mut bytes = std::fs::read(&good).unwrap();
    let h = bad_header(i32::MAX as u32);
    assert_eq!(&bytes[..4], b"PAR1");
    bytes[4..4 + h.len()].copy_from_slice(&h); // the first page header starts right after the magic number
    File::create(&bad).unwrap().write_all(&bytes).unwrap();
    println!("damaged: first page header = type, sizes, then list<double> of {} elements ({} bytes written over the header)", i32::MAX, h.len());

    let (o, r, e, t) = read_all(&bad, false, Duration::from_secs(5));
    println!("damaged, as it is:          {o} after {t:?}; read calls {r}, of them returning 0 bytes {e}");
    println!("   rate {:.0} empty reads/s; the list has {} elements, so about {:.1} hours at this rate", e as f64 / t.as_secs_f64(), i32::MAX, i32::MAX as f64 / (e as f64 / t.as_secs_f64()) / 3600.0);

    let (o, r, e, t) = read_all(&bad, true, Duration::from_secs(5));
    println!("damaged, second read at EOF refused: {o} after {t:?}; read calls {r}, returning 0 bytes {e}");
    std::process::exit(0); // the first reader thread is still looping
}
```

`src/bin/read_bytes_owned.rs`:

```rust
//! parquet 60.1.0: `ThriftReadInputProtocol::read_bytes_owned` does `Vec::with_capacity(len)` with `len` from the file. Which page-header fields reach it?
//! Only the binary fields of the page statistics, and only when `ReaderProperties::read_page_stats` is on (default: off; the Arrow reader never turns it on).
//! The program writes a file whose page headers carry statistics, makes the first statistics field claim 2^40 bytes, and reads the first page twice through the
//! public file reader: with the default properties, and with `set_read_page_statistics(true)`. Run it under `ulimit -v` to keep the second read bounded.
use std::fs::File;
use std::io::Write;
use std::sync::Arc;

use arrow_array::{ArrayRef, Int64Array, RecordBatch};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::{EnabledStatistics, ReaderProperties, WriterProperties};
use parquet::file::reader::FileReader;
use parquet::file::serialized_reader::{ReadOptionsBuilder, SerializedFileReader};

fn main() {
    let mode = std::env::args().nth(1).expect("usage: read_bytes_owned default|page-stats DIR");
    let dir = std::env::args().nth(2).unwrap_or_else(|| ".".into());
    let path = format!("{dir}/stats.parquet");
    let col: ArrayRef = Arc::new(Int64Array::from_iter_values(0..1000));
    let batch = RecordBatch::try_from_iter([("a", col)]).unwrap();
    let props = WriterProperties::builder()
        .set_dictionary_enabled(false)
        .set_statistics_enabled(EnabledStatistics::Page)
        .set_write_page_header_statistics(true)
        .build();
    let mut w = ArrowWriter::try_new(File::create(&path).unwrap(), batch.schema(), Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    // first binary field of the first page header's statistics: field header with type 8 in the low nibble, then the length 8 of an INT64 value
    let at = (4..80).find(|&i| bytes[i] & 0x0f == 8 && bytes[i] >> 4 != 0 && bytes[i + 1] == 8).unwrap();
    bytes.splice(at + 1..at + 2, [0x80u8, 0x80, 0x80, 0x80, 0x80, 0x20]); // length 1 << 40
    File::create(&path).unwrap().write_all(&bytes).unwrap();
    println!("claimed length 2^40 = {} bytes, patched at byte {at}; mode {mode}", 1u64 << 40);

    let reader_props = match mode.as_str() {
        "default" => ReaderProperties::builder().build(),
        "page-stats" => ReaderProperties::builder().set_read_page_statistics(true).build(),
        m => panic!("unknown mode {m}"),
    };
    // the metadata is read from the footer (not damaged); the page header is read when the first page is asked for
    let file = SerializedFileReader::new_with_options(
        File::open(&path).unwrap(),
        ReadOptionsBuilder::new().with_reader_properties(reader_props).build(),
    )
    .unwrap();
    let rg = file.get_row_group(0).unwrap();
    let mut pages = rg.get_column_page_reader(0).unwrap();
    println!("reading the first page ...");
    match pages.get_next_page() {
        Ok(p) => println!("Ok({:?})", p.map(|p| p.page_type())),
        Err(e) => println!("Err({e})"),
    }
    println!("process still alive");
}
```
