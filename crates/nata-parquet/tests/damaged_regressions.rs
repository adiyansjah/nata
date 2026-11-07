//! Damaged files that once made the scan misbehave, pinned as fixtures so the fix cannot regress.
//!
//! `thrift_skip_past_eof.parquet` (1,189 bytes, from the damaged-file fuzz: seed 20261107, shape `tiny_and_empty_groups`, case 10072,
//! random bytes 120..169; found by the first nightly run on 2026-11-07). Its damaged page header claims a list and a length longer than the
//! file. parquet-rs 60.1.0's streaming Thrift reader skips such fields with `io::copy(reader.take(n))` without checking that `n` bytes
//! came, and loops over the list count from the file, so it read past the end billions of times: a hang inside the dependency, out of reach
//! of the scan's cancel. The reader now refuses a second read at end of file (`CountingRead::at_eof`), which turns it into an error.

mod common;
use common::*;

use nata_parquet::ErrorKind;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn a_page_header_that_claims_bytes_past_the_end_is_an_error_not_a_hang() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/thrift_skip_past_eof.parquet");
    let (tx, rx) = mpsc::channel();
    let p = path.clone();
    let helper = std::thread::spawn(move || {
        let _ = tx.send(scan(&[p.as_path()], None, &[], true, None).map(|s| s.csv));
    });
    // Before the fix this took minutes to hours (about 10,000 empty reads per second); a correct scan answers in milliseconds.
    let result = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("the scan of a damaged page header must end (it hung before the fix)");
    helper.join().expect("helper thread");
    let e = result.expect_err("a damaged page header is an error, not rows");
    assert!(matches!(e.kind, ErrorKind::Corrupt(_)), "{e}");
    assert_eq!(e.file.as_deref(), Some(path.as_path()), "{e}");
    assert!(e.row_group.is_some(), "the error names the row group: {e}");
}

/// Card 0004, item A (second part): parquet-rs 60.1.0 `ThriftReadInputProtocol::read_bytes_owned` reserves `Vec::with_capacity(len)` with `len` read from
/// the file, and 1 TiB would abort the process (an allocation failure is not a panic: no catch stops it). On the scan path it is reached only by the
/// binary fields of the page STATISTICS in a page header, and only when `ReaderProperties::read_page_stats` is on; the Arrow reader builds its page
/// readers with the default properties (`SerializedPageReader::new`, `arrow_reader/mod.rs:1541`), where it is off, so the statistics are skipped with
/// `skip_bytes`, which copies into a sink and reserves nothing. This test pins that: a data page whose statistics claim 2^40 bytes is an error, and the
/// process lives. If a later parquet-rs turns page statistics on for the Arrow reader, this test aborts the test binary instead of passing.
#[test]
fn a_page_header_whose_statistics_claim_a_terabyte_is_an_error_not_an_allocation() {
    use arrow_array::{ArrayRef, Int64Array, RecordBatch};
    use parquet::arrow::ArrowWriter;
    use parquet::file::properties::{EnabledStatistics, WriterProperties};
    use std::sync::Arc;

    let dir = scratch("huge-statistics");
    let clean = dir.join("clean.parquet");
    let col: ArrayRef = Arc::new(Int64Array::from_iter_values(0..1000));
    let batch = RecordBatch::try_from_iter([("a", col)]).expect("batch");
    let props = WriterProperties::builder()
        .set_dictionary_enabled(false)
        .set_statistics_enabled(EnabledStatistics::Page)
        .set_write_page_header_statistics(true)
        .build();
    let mut w = ArrowWriter::try_new(
        std::fs::File::create(&clean).expect("create"),
        batch.schema(),
        Some(props),
    )
    .expect("writer");
    w.write(&batch).expect("write");
    w.close().expect("close");
    let mut bytes = std::fs::read(&clean).expect("read");
    scan(&[clean.as_path()], None, &[], true, None).expect("the clean file scans");

    // the first page header starts after the magic number; its statistics hold binary fields (type 8 in the low nibble of the field header) of 8 bytes
    // (the value of an INT64 minimum or maximum): replace the length 8 of the first one with the varint of 2^40
    let at = (4..80)
        .find(|&i| bytes[i] & 0x0f == 8 && bytes[i] >> 4 != 0 && bytes[i + 1] == 8)
        .expect("a binary statistics field in the first page header");
    let huge_len = [0x80u8, 0x80, 0x80, 0x80, 0x80, 0x20]; // 1 << 40
    bytes.splice(at + 1..at + 2, huge_len);
    let damaged = dir.join("huge.parquet");
    std::fs::write(&damaged, &bytes).expect("write");

    let (tx, rx) = mpsc::channel();
    let p = damaged.clone();
    let helper = std::thread::spawn(move || {
        let _ = tx.send(scan(&[p.as_path()], None, &[], true, None).map(|s| s.csv));
    });
    let result = rx
        .recv_timeout(Duration::from_secs(20))
        .expect("the scan of a page header with a huge statistics length must end");
    helper.join().expect("helper thread");
    let e = result.expect_err("a statistics field longer than the file is an error, not rows");
    assert!(matches!(e.kind, ErrorKind::Corrupt(_)), "{e}");
    assert_eq!(e.file.as_deref(), Some(damaged.as_path()), "{e}");
}
