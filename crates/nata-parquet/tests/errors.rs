//! FAULT TOLERANCE and SECURITY of the reader: a corrupt, truncated or unreadable file is an error that names the FILE (and the row group where it is
//! known), never a panic, never a hang, never a cell value in the message; a type outside the card answers `not supported yet: <what>`, and only when the
//! scan touches that column.
//!
//! The fuzz of the card (3,000 byte flips and truncations per file shape, with timeouts) is a later step; this file is the first, deterministic sample
//! (about 700 mutated files, `NATA_ERROR_CASES` for more), with the property the card states: no panic, no hang, the error names the file.

mod common;
use common::*;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use arrow_array::builder::{Int32Builder, ListBuilder};
use arrow_array::types::TimestampMicrosecondType;
use arrow_array::{
    ArrayRef, Decimal128Array, Float64Array, Int64Array, RecordBatch, StringArray, StructArray,
    TimestampNanosecondArray,
};
use arrow_array::{Decimal256Array, PrimitiveArray, Time64MicrosecondArray};
use arrow_buffer::i256;
use arrow_schema::{DataType, Field};
use nata_parquet::{ErrorKind, ScanError, ScanOptions, ScanPlan, run_serial};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::{EnabledStatistics, WriterProperties};

const MARKER: &str = "ZZMARKER";

/// 3 row groups of 1,000 rows: an id, a text column whose every value contains the marker, a decimal, a double. Uncompressed, so the markers are in the file.
fn write_victim(path: &Path) {
    let n = 3000i64;
    let ids: ArrayRef = Arc::new(Int64Array::from_iter_values(0..n));
    let secret: ArrayRef = Arc::new(StringArray::from_iter_values(
        (0..n).map(|i| format!("{MARKER}-{i:05}-QQ")),
    ));
    let dec: ArrayRef = Arc::new(
        Decimal128Array::from_iter_values((0..n).map(|i| i128::from(i) * 7919))
            .with_precision_and_scale(12, 2)
            .expect("decimal"),
    );
    let dbl: ArrayRef = Arc::new(Float64Array::from_iter_values(
        (0..n).map(|i| i as f64 * 0.5),
    ));
    let batch =
        RecordBatch::try_from_iter([("id", ids), ("secret", secret), ("dec", dec), ("dbl", dbl)])
            .expect("batch");
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(1000))
        .set_statistics_enabled(EnabledStatistics::Chunk)
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

/// Open and scan everything in a worker thread with a deadline. `Err(None)` = it did not finish (a hang); the inner result is the scan's.
fn scan_with_deadline(
    path: &Path,
    filters: &[&str],
    secs: u64,
) -> Result<Result<u64, (bool, ScanError)>, ()> {
    let path = path.to_path_buf();
    let filters: Vec<String> = filters.iter().map(|s| s.to_string()).collect();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let options = ScanOptions {
            columns: None,
            filters,
            prune: true,
        };
        let r = match ScanPlan::open(&[path], &options) {
            Err(e) => Err((false, e)),
            Ok(plan) => run_serial(&plan, None, |_| Ok(()))
                .map(|r| r.rows_returned)
                .map_err(|e| (true, e)),
        };
        let _ = tx.send(r);
    });
    rx.recv_timeout(Duration::from_secs(secs)).map_err(|_| ())
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// What every error of a damaged file must satisfy.
fn assert_good_error(e: &ScanError, in_run_stage: bool, path: &Path, case: &str) {
    let text = e.to_string();
    assert!(
        !matches!(e.kind, ErrorKind::Internal(_)),
        "{case}: a panic was caught (an internal error): {text}"
    );
    assert_eq!(
        e.file.as_deref(),
        Some(path),
        "{case}: the error names the file: {text}"
    );
    assert!(
        text.contains(&path.display().to_string()),
        "{case}: the message names the file: {text}"
    );
    assert!(
        !text.contains(MARKER),
        "{case}: the message carries a cell value: {text}"
    );
    assert!(
        !text.contains("QQ"),
        "{case}: the message carries a cell value: {text}"
    );
    if in_run_stage {
        assert!(
            e.row_group.is_some() && text.contains("row group"),
            "{case}: an error while reading names the row group: {text}"
        );
    }
}

#[test]
fn every_truncation_of_a_valid_file_is_an_error_that_names_the_file() {
    let dir = scratch("trunc");
    let good = dir.join("good.parquet");
    write_victim(&good);
    let bytes = std::fs::read(&good).expect("read");
    // sanity: the valid file scans
    assert_eq!(scan_with_deadline(&good, &[], 30), Ok(Ok(3000)));
    let marker_in_file = bytes.windows(MARKER.len()).any(|w| w == MARKER.as_bytes());
    assert!(
        marker_in_file,
        "the test data really holds the marker in clear"
    );
    let mut cuts: Vec<usize> = vec![
        0,
        1,
        3,
        4,
        5,
        7,
        8,
        9,
        11,
        12,
        100,
        bytes.len() / 4,
        bytes.len() / 2,
        bytes.len() - 100,
        bytes.len() - 13,
        bytes.len() - 12,
        bytes.len() - 9,
        bytes.len() - 8,
        bytes.len() - 5,
        bytes.len() - 4,
        bytes.len() - 1,
    ];
    let mut rng = Rng(0x1234_5678_9abc_def1);
    for _ in 0..60 {
        cuts.push(rng.below(bytes.len()));
    }
    for cut in cuts {
        let f = dir.join(format!("cut-{cut}.parquet"));
        std::fs::write(&f, &bytes[..cut]).expect("write");
        let case = format!("truncated to {cut} of {} bytes", bytes.len());
        match scan_with_deadline(&f, &[], 30) {
            Err(()) => panic!("{case}: HANG (no answer in 30 s)"),
            Ok(Ok(rows)) => {
                panic!("{case}: a truncated file returned {rows} rows instead of an error")
            }
            Ok(Err((run, e))) => assert_good_error(&e, run, &f, &case),
        }
    }
}

#[test]
fn garbage_footers_and_garbage_files() {
    let dir = scratch("garbage");
    let good = dir.join("good.parquet");
    write_victim(&good);
    let bytes = std::fs::read(&good).expect("read");
    let n = bytes.len();
    let footer_len = u32::from_le_bytes(bytes[n - 8..n - 4].try_into().expect("4 bytes")) as usize;
    let footer_start = n - 8 - footer_len;
    let mut cases: Vec<(String, Vec<u8>)> = Vec::new();
    let with = |f: &dyn Fn(&mut Vec<u8>)| {
        let mut b = bytes.clone();
        f(&mut b);
        b
    };
    cases.push((
        "footer filled with 0xFF".into(),
        with(&|b| b[footer_start..n - 8].fill(0xFF)),
    ));
    cases.push((
        "footer filled with zeros".into(),
        with(&|b| b[footer_start..n - 8].fill(0)),
    ));
    cases.push((
        "footer length field is huge".into(),
        with(&|b| b[n - 8..n - 4].copy_from_slice(&u32::MAX.to_le_bytes())),
    ));
    cases.push((
        "footer length field is zero".into(),
        with(&|b| b[n - 8..n - 4].copy_from_slice(&0u32.to_le_bytes())),
    ));
    cases.push((
        "footer length field is one too big".into(),
        with(&|b| b[n - 8..n - 4].copy_from_slice(&((footer_len + 1) as u32).to_le_bytes())),
    ));
    cases.push((
        "trailing magic damaged".into(),
        with(&|b| b[n - 4..].copy_from_slice(b"PAR2")),
    ));
    cases.push((
        "leading magic damaged".into(),
        with(&|b| b[..4].copy_from_slice(b"XXXX")),
    ));
    cases.push(("an empty file".into(), Vec::new()));
    cases.push((
        "trailing PARE (an encrypted footer)".into(),
        with(&|b| b[n - 4..].copy_from_slice(b"PARE")),
    ));
    cases.push(("only the magic".into(), b"PAR1".to_vec()));
    cases.push(("magic twice".into(), b"PAR1PAR1".to_vec()));
    cases.push((
        "plain text".into(),
        b"this is not a parquet file, but a sentence\n".to_vec(),
    ));
    cases.push((
        "a CSV file".into(),
        b"id,secret\n1,ZZMARKER-00001-QQ\n".to_vec(),
    ));
    cases.push((
        "footer bytes shuffled".into(),
        with(&|b| b[footer_start..n - 8].reverse()),
    ));
    for (name, content) in cases {
        let f = dir.join(format!("{}.parquet", name.replace([' ', '(', ')'], "_")));
        std::fs::write(&f, content).expect("write");
        match scan_with_deadline(&f, &[], 30) {
            Err(()) => panic!("{name}: HANG"),
            Ok(Ok(rows)) => panic!("{name}: returned {rows} rows instead of an error"),
            Ok(Err((run, e))) => {
                assert_good_error(&e, run, &f, &name);
                if name.contains("encrypted") {
                    assert!(
                        e.is_not_supported()
                            && e.to_string()
                                .contains("not supported yet: Parquet encryption"),
                        "{e}"
                    );
                }
            }
        }
    }
}

#[test]
fn a_column_chunk_that_is_cut_names_the_file_and_the_row_group() {
    let dir = scratch("cutchunk");
    let good = dir.join("good.parquet");
    write_victim(&good);
    let bytes = std::fs::read(&good).expect("read");
    let n = bytes.len();
    let footer_len = u32::from_le_bytes(bytes[n - 8..n - 4].try_into().expect("4 bytes")) as usize;
    let footer_start = n - 8 - footer_len;
    // the byte range of every column chunk of every row group
    let md = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(
        File::open(&good).expect("open"),
    )
    .expect("builder");
    let mut found_in_rg = std::collections::BTreeSet::new();
    for rg in 0..md.metadata().num_row_groups() {
        for c in md.metadata().row_group(rg).columns() {
            let (start, len) = c.byte_range();
            let (start, len) = (start as usize, len as usize);
            // cut the second half of the chunk out of the file and keep the footer, whose offsets now point at the wrong bytes
            let mut b = bytes[..start + len / 2].to_vec();
            b.extend_from_slice(&bytes[footer_start..]);
            let f = dir.join(format!("cut-rg{rg}-{}.parquet", c.column_path().string()));
            std::fs::write(&f, b).expect("write");
            let case = format!("chunk of {} in row group {rg} cut", c.column_path());
            match scan_with_deadline(&f, &[], 30) {
                Err(()) => panic!("{case}: HANG"),
                Ok(Ok(rows)) => panic!("{case}: returned {rows} rows instead of an error"),
                Ok(Err((run, e))) => {
                    assert_good_error(&e, run, &f, &case);
                    if run {
                        // the damaged row group is the first one that fails, and it is the one named
                        assert_eq!(e.row_group, Some(rg), "{case}: {e}");
                        found_in_rg.insert(rg);
                    }
                }
            }
        }
    }
    assert!(
        found_in_rg.len() >= 2,
        "errors surfaced in several row groups (not only the first): {found_in_rg:?}"
    );
}

#[test]
fn flipped_bytes_zeroed_ranges_and_cuts_never_panic_hang_or_leak_a_value() {
    let dir = scratch("fuzz");
    let good = dir.join("good.parquet");
    write_victim(&good);
    let bytes = std::fs::read(&good).expect("read");
    let cases: usize = std::env::var("NATA_ERROR_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(600);
    let seed: u64 = std::env::var("NATA_ERROR_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0xC0FFEE);
    let mut rng = Rng(seed | 1);
    let (mut errors, mut oks) = (0usize, 0usize);
    for i in 0..cases {
        let mut b = bytes.clone();
        let what = match rng.below(4) {
            0 => {
                let p = rng.below(b.len());
                b[p] ^= 1 << rng.below(8);
                format!("flip a bit at {p}")
            }
            1 => {
                let p = rng.below(b.len());
                b[p] = (rng.next() & 0xFF) as u8;
                format!("random byte at {p}")
            }
            2 => {
                let p = rng.below(b.len());
                let l = (1 + rng.below(200)).min(b.len() - p);
                b[p..p + l].fill(0);
                format!("zero {l} bytes at {p}")
            }
            _ => {
                // aimed at the footer: the last 300 bytes
                let p = b.len() - 1 - rng.below(300.min(b.len() - 1));
                b[p] ^= 0xFF;
                format!("invert the byte at {p} (footer region)")
            }
        };
        let f = dir.join(format!("m{i}.parquet"));
        std::fs::write(&f, &b).expect("write");
        let case = format!("case {i} (seed {seed}): {what}");
        // alternate: no filter, and a filter that uses the statistics
        let filters: &[&str] = if i % 2 == 0 { &[] } else { &["id >= 1500"] };
        match scan_with_deadline(&f, filters, 30) {
            Err(()) => panic!("{case}: HANG; replay with NATA_ERROR_SEED={seed}"),
            // a flipped value byte can decode to another valid value (plain Parquet pages have no checksum): not an error, not a failure
            Ok(Ok(_)) => oks += 1,
            Ok(Err((run, e))) => {
                errors += 1;
                assert_good_error(&e, run, &f, &case);
            }
        }
        let _ = std::fs::remove_file(&f);
    }
    eprintln!(
        "ERRORS fuzz sample: {cases} mutated files, {errors} errors, {oks} scanned without error (a value changed or the byte did not matter)"
    );
    assert!(
        errors > cases / 10,
        "the mutations must be able to produce errors ({errors} of {cases})"
    );
}

#[test]
fn unreadable_files_name_the_file() {
    let dir = scratch("unreadable");
    let missing = dir.join("does-not-exist.parquet");
    let e = scan(&[&missing], None, &[], true, None).expect_err("missing");
    assert!(
        matches!(e.kind, ErrorKind::Io(_)) && e.to_string().contains("does-not-exist.parquet"),
        "{e}"
    );
    // a directory
    let e = scan(&[&dir], None, &[], true, None).expect_err("directory");
    assert!(e.to_string().contains(&dir.display().to_string()), "{e}");
    // no permission
    let f = dir.join("locked.parquet");
    write_victim(&f);
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    let r = scan(&[&f], None, &[], true, None);
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).expect("chmod back");
    // (as root the file would still be readable)
    if let Err(e) = r {
        assert!(
            matches!(e.kind, ErrorKind::Io(_)) && e.to_string().contains("locked.parquet"),
            "{e}"
        );
    }
}

#[test]
fn a_later_file_that_is_corrupt_fails_the_scan_before_any_row_is_returned() {
    let dir = scratch("second-bad");
    let (a, b) = (dir.join("a.parquet"), dir.join("b.parquet"));
    write_victim(&a);
    std::fs::write(&b, b"PAR1 this is not a footer").expect("write");
    let e = scan(&[&a, &b], None, &[], true, None).expect_err("second file is corrupt");
    assert!(
        e.to_string().contains("b.parquet") && !e.to_string().contains("a.parquet"),
        "{e}"
    );
}

/// A row group whose data is damaged after the footer was read: rows of the row groups before it have already been handed to the sink; the error is
/// returned and no later row group is read.
#[test]
fn a_damaged_row_group_is_an_error_and_later_row_groups_are_not_read() {
    let dir = scratch("damaged-rg");
    let good = dir.join("good.parquet");
    write_victim(&good);
    let mut bytes = std::fs::read(&good).expect("read");
    let md = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(
        File::open(&good).expect("open"),
    )
    .expect("builder");
    let col = md
        .metadata()
        .row_group(1)
        .columns()
        .iter()
        .find(|c| c.column_path().string() == "dbl")
        .expect("dbl")
        .byte_range();
    // the page header of the chunk is destroyed
    bytes[col.0 as usize..col.0 as usize + 16].fill(0xFF);
    let f = dir.join("damaged.parquet");
    std::fs::write(&f, &bytes).expect("write");
    let options = ScanOptions {
        columns: Some(vec!["id".into(), "dbl".into()]),
        filters: vec![],
        prune: true,
    };
    let plan = ScanPlan::open(&[PathBuf::from(&f)], &options).expect("the footer is fine");
    let mut seen = 0u64;
    let e = run_serial(&plan, None, |b| {
        seen += b.num_rows() as u64;
        Ok(())
    })
    .expect_err("row group 1 is damaged");
    assert_eq!(e.row_group, Some(1), "{e}");
    assert!(
        e.to_string().contains("damaged.parquet") && e.to_string().contains("row group 1"),
        "{e}"
    );
    assert_eq!(seen, 1000, "only row group 0 reached the sink");
}

// ------------------------------------------------------------------------------------------------------------------------------ not supported yet

fn write_batch(path: &Path, batch: &RecordBatch) {
    let mut w = ArrowWriter::try_new(File::create(path).expect("create"), batch.schema(), None)
        .expect("writer");
    w.write(batch).expect("write");
    w.close().expect("close");
}

#[test]
fn types_outside_the_card_are_not_supported_yet_and_only_when_touched() {
    let dir = scratch("unsupported");
    let n = 4;
    let ids: ArrayRef = Arc::new(Int64Array::from_iter_values(0..n));
    let mut lb = ListBuilder::new(Int32Builder::new());
    for i in 0..n {
        lb.values().append_value(i as i32);
        lb.append(true);
    }
    let list: ArrayRef = Arc::new(lb.finish());
    let strukt: ArrayRef = Arc::new(StructArray::from(vec![(
        Arc::new(Field::new("a", DataType::Int64, true)),
        Arc::new(Int64Array::from_iter_values(0..n)) as ArrayRef,
    )]));
    let ns: ArrayRef = Arc::new(TimestampNanosecondArray::from_iter_values(0..n));
    let tz: ArrayRef = Arc::new(
        PrimitiveArray::<TimestampMicrosecondType>::from_iter_values(0..n).with_timezone("UTC"),
    );
    let time: ArrayRef = Arc::new(Time64MicrosecondArray::from_iter_values(0..n));
    let wide: ArrayRef = Arc::new(
        Decimal256Array::from_iter_values((0..n).map(|i| i256::from_i128(i128::from(i))))
            .with_precision_and_scale(50, 2)
            .expect("decimal256"),
    );
    for (name, col, what) in [
        ("l", list, "nested"),
        ("s", strukt, "nested"),
        ("ns", ns, "nanosecond"),
        ("tz", tz, "TIME ZONE"),
        ("tm", time, "TIME"),
        ("wide", wide, "DECIMAL(50,2)"),
    ] {
        let f = dir.join(format!("{name}.parquet"));
        write_batch(
            &f,
            &RecordBatch::try_from_iter([("id", ids.clone()), (name, col)]).expect("batch"),
        );
        // the other column is fine
        let ok = scan(&[&f], Some(&["id"]), &[], true, None)
            .unwrap_or_else(|e| panic!("{name}: projecting only id must work: {e}"));
        assert_eq!(records(&ok.csv).len(), 5);
        assert_eq!(
            ok.report.columns_read, 1,
            "{name}: the unsupported column is not even read"
        );
        // touching it: projected, filtered, or by default (all columns)
        for (cols, filters) in [
            (Some(vec![name]), vec![]),
            (Some(vec!["id"]), vec![format!("{name} IS NULL")]),
            (None, vec![]),
        ] {
            let c: Option<Vec<&str>> = cols;
            let fl: Vec<&str> = filters.iter().map(String::as_str).collect();
            let e = scan(&[&f], c.as_deref(), &fl, true, None).expect_err(name);
            let text = e.to_string();
            assert!(e.is_not_supported(), "{name}: {text}");
            assert!(
                text.contains("not supported yet:")
                    && text.contains(&format!("column \"{name}\""))
                    && text.contains(what),
                "{name}: {text}"
            );
            assert!(
                text.contains(&f.display().to_string()),
                "{name}: names the file: {text}"
            );
        }
    }
}
