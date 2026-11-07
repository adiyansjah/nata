//! R4, page checksums (register item R4, `docs/slices/0002-select-where-limit.md` part 5): a Parquet page may carry a CRC-32 of its (compressed) payload; the reader
//! verifies it when it has one, and a mismatch is an error that names the file and the row group. parquet-rs 60.1.0 can do this only when it is compiled with its
//! `crc` feature (our cargo feature `page-crc`, off by default); it cannot WRITE checksums, so the files here come from pyarrow (`tests/fixtures/make_checksummed.py`,
//! committed).
//!
//! The tests state both builds: with `page-crc` a damaged page of a checksummed file is an error; WITHOUT it (`cargo test -p nata-parquet --no-default-features`) the
//! same damage reads silently as other values. That second half is the evidence that the check can fail: it is the known-bad build.

mod common;
use common::*;

use std::path::{Path, PathBuf};

#[cfg(feature = "page-crc")]
use nata_parquet::ErrorKind;

const MARKER: &str = "ZQXJ-PLANTED-7731";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("{name}.parquet"))
}

/// Offsets of the planted marker text of the VALUES of `l_comment` in the file bytes: plain-encoded, uncompressed, each preceded by its 4-byte little-endian
/// length (the marker is 17 bytes, then " c" and one or two digits). The same text also appears in the statistics of the page headers and of the footer
/// (minimum and maximum), which are not values and are skipped.
fn marker_offsets(bytes: &[u8]) -> Vec<usize> {
    let m = MARKER.as_bytes();
    (4..bytes.len().saturating_sub(m.len()))
        .filter(|i| &bytes[*i..*i + m.len()] == m)
        .filter(|i| {
            let len = u32::from_le_bytes(bytes[*i - 4..*i].try_into().expect("4 bytes"));
            len == 20 || len == 21
        })
        .collect()
}

fn scan_all(path: &Path) -> Result<Scan, nata_parquet::ScanError> {
    scan(&[path], None, &[], true, None)
}

#[test]
fn files_with_checksums_scan_to_the_expected_rows_and_match_duckdb() {
    let f = fixture("checksummed_lineitem_like");
    let got = scan_all(&f).expect("a file with valid checksums scans");
    let rows: Vec<&str> = got.csv.lines().collect();
    assert_eq!(rows.len(), 2401, "header and 2400 rows");
    assert_eq!(
        rows[1],
        "0,0,1.00,9.00,A,1992-01-01,ZQXJ-PLANTED-7731 c0,987654321"
    );
    assert_eq!(got.report.row_groups_read, 4);
    // the same file through DuckDB (which does not verify checksums): same rows
    if let Some(duck) = duckdb_or_skip("files_with_checksums_scan_to_the_expected_rows") {
        let want = duckdb_csv(&duck, &format!("SELECT * FROM '{}'", f.display()));
        assert_eq!(diff_ordered(&want, &got.csv), None);
    }
    for name in ["checksummed_v2_zstd", "checksummed_nulls_dict"] {
        let g = scan_all(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(g.csv.lines().count() > 1000, "{name}");
    }
}

/// The row group that holds the `k`-th planted marker of the lineitem-like fixture (600 rows per row group).
#[cfg(feature = "page-crc")]
fn row_group_of_row(k: usize) -> usize {
    k / 600
}

#[test]
fn a_damaged_value_in_a_page_with_a_checksum_is_an_error_naming_the_file_and_the_row_group() {
    let clean_path = fixture("checksummed_lineitem_like");
    let clean_bytes = std::fs::read(&clean_path).expect("read");
    #[cfg(not(feature = "page-crc"))]
    let clean = scan_all(&clean_path).expect("clean");
    #[cfg(feature = "page-crc")]
    scan_all(&clean_path).expect("clean");
    let offsets = marker_offsets(&clean_bytes);
    assert_eq!(offsets.len(), 2400, "one marker per row, plain encoded");
    let dir = scratch("crc-damage");
    // damage one byte of the marker text of rows in each of the four row groups
    for k in [3usize, 700, 1250, 2399] {
        let mut b = clean_bytes.clone();
        b[offsets[k] + 5] ^= 0x01;
        let p = dir.join(format!("damaged-{k}.parquet"));
        std::fs::write(&p, &b).expect("write");
        let r = scan_all(&p);
        #[cfg(feature = "page-crc")]
        {
            let e = r.expect_err("the checksum of the page does not match");
            assert!(
                matches!(e.kind, ErrorKind::Corrupt(_)),
                "a damaged page is a corrupt file: {e}"
            );
            assert_eq!(e.row_group, Some(row_group_of_row(k)), "{e}");
            assert_eq!(e.file.as_deref(), Some(p.as_path()), "{e}");
            let text = e.to_string();
            assert!(text.contains("CRC"), "{text}");
            assert!(
                !text.contains("ZQXJ") && !text.contains("PLANTED"),
                "no cell value in the error: {text}"
            );
        }
        #[cfg(not(feature = "page-crc"))]
        {
            // THE KNOWN-BAD BUILD: without the verification the same damage reads as another value, silently
            let got = r.expect("without the check the damaged page reads");
            assert_ne!(got.csv, clean.csv, "damage at row {k}");
            let diff: Vec<_> = got
                .csv
                .lines()
                .zip(clean.csv.lines())
                .filter(|(a, b)| a != b)
                .collect();
            assert_eq!(diff.len(), 1, "exactly the damaged cell reads differently");
        }
    }
}

#[test]
fn the_same_damage_in_a_file_without_checksums_is_a_silent_wrong_answer_in_every_build() {
    // a file written by parquet-rs (no checksums): a flipped value byte reads as another valid value, with or without the verification
    let dir = scratch("crc-none");
    let f = dir.join("plain.parquet");
    common::genfiles::write_ids(&f, 0, 2000, 500);
    let clean_bytes = std::fs::read(&f).expect("read");
    let clean = scan_all(&f).expect("clean");
    let at = clean_bytes
        .windows(4)
        .position(|w| w == b"cat3")
        .expect("a value of the text column is in the file as plain bytes");
    let mut b = clean_bytes.clone();
    b[at + 3] = b'4';
    let p = dir.join("damaged.parquet");
    std::fs::write(&p, &b).expect("write");
    let got = scan_all(&p).expect("no checksum, no error");
    assert_ne!(got.csv, clean.csv, "another valid value");
}

#[test]
fn every_flipped_byte_of_the_pages_of_a_checksummed_file_is_an_error_or_harmless_with_the_check_on()
{
    // exhaustive over the page region of the small dictionary fixture: flip every byte (one bit) one at a time. With the check on, a case must never read
    // as DIFFERENT rows unless the flipped byte is in a page HEADER or in the footer (those are not covered by the page checksum: the spec protects the payload)
    let src = fixture("checksummed_nulls_dict");
    let clean_bytes = std::fs::read(&src).expect("read");
    let clean = scan_all(&src).expect("clean");
    let dir = scratch("crc-every-byte");
    let p = dir.join("one.parquet");
    let footer_len = u32::from_le_bytes(
        clean_bytes[clean_bytes.len() - 8..clean_bytes.len() - 4]
            .try_into()
            .expect("4 bytes"),
    ) as usize;
    let body_end = clean_bytes.len() - 8 - footer_len;
    let (mut errors, mut same, mut different) = (0usize, 0usize, 0usize);
    for i in 4..body_end {
        let mut b = clean_bytes.clone();
        b[i] ^= 0x40;
        std::fs::write(&p, &b).expect("write");
        match scan_all(&p) {
            Err(_) => errors += 1,
            Ok(g) if g.csv == clean.csv => same += 1,
            Ok(_) => different += 1,
        }
    }
    eprintln!(
        "every byte of the body of checksummed_nulls_dict ({} bytes): error {errors}, same rows {same}, different rows {different}",
        body_end - 4
    );
    #[cfg(feature = "page-crc")]
    assert_eq!(
        different, 0,
        "a flipped byte of a checksummed page must never read as other rows"
    );
    #[cfg(not(feature = "page-crc"))]
    assert!(
        different > 0,
        "without the check, damage to page payloads reads as other rows: the test can fail"
    );
}

/// R4's limit (card 0004, item B), pinned. A page CRC covers the payload of a page, not its header. In `checksummed_v2_zstd` the header of the second
/// page of the first column (bytes 800..878) holds, in field 5 of the data page v2 header, `definition_levels_byte_length` (3). One flipped bit (byte 826,
/// bit 3: 3 becomes 7) moves the split between the level bytes and the values inside a payload whose CRC still matches, and the file reads as other rows with
/// the check ON. This is the case 16900 of the damaged-file fuzz (seed 20261107, shape `checksummed_v2_zstd`). A flipped bit inside the payload of the
/// same page (byte 900) is an error with the check on.
#[test]
fn a_page_header_is_outside_the_page_checksum_but_its_payload_is_inside() {
    let src = fixture("checksummed_v2_zstd");
    let clean_bytes = std::fs::read(&src).expect("read");
    let clean = scan_all(&src).expect("clean");
    let dir = scratch("crc-header");
    // the bytes are where the test says they are: the page header of the second page of column 0 starts at 800 and its payload (572 bytes) at 878
    assert_eq!(&clean_bytes[800..804], &[0x15, 0x06, 0x15, 0xf8]);
    let mut header = clean_bytes.clone();
    header[826] ^= 0x08;
    let p = dir.join("header.parquet");
    std::fs::write(&p, &header).expect("write");
    let got = scan_all(&p)
        .expect("a damaged level length is not caught by a payload checksum, in any build");
    assert_ne!(got.csv, clean.csv, "other rows, silently: R4's limit");

    #[cfg(feature = "page-crc")]
    {
        let mut payload = clean_bytes.clone();
        payload[900] ^= 0x08;
        let q = dir.join("payload.parquet");
        std::fs::write(&q, &payload).expect("write");
        let e = scan_all(&q).expect_err("the payload checksum does not match");
        assert!(matches!(e.kind, ErrorKind::Corrupt(_)), "{e}");
        assert!(e.to_string().contains("CRC"), "{e}");
    }
}

#[test]
fn page_checksums_are_opt_in() {
    // Default builds omit verification; enabled test runs explicitly set NATA_CRC_BUILD=1.
    // This catches either accidentally enabling the default or losing the opt-in feature.
    assert_eq!(
        cfg!(feature = "page-crc"),
        std::env::var("NATA_CRC_BUILD").is_ok(),
        "set NATA_CRC_BUILD=1 alongside --features page-crc for the opt-in test run"
    );
}
