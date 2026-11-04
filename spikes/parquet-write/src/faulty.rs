//! Known-bad writers: single-column files whose statistics are WRONG in one specific way. parquet-rs refuses to write bounds narrower than the
//! data (its `write_batch_with_statistics` only ever widens them), so each file is written correctly and then its FOOTER is rewritten with the bad
//! statistics (the data pages and the page index are untouched). The verifier must flag each; the `control-*` files carry legitimately loose or
//! absent bounds and must pass.
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use parquet::basic::Compression;
use parquet::data_type::{ByteArray, ByteArrayType, DoubleType, FixedLenByteArray, FixedLenByteArrayType, Int32Type, Int64Type};
use parquet::file::metadata::{ParquetMetaDataBuilder, ParquetMetaDataReader, ParquetMetaDataWriter};
use parquet::file::properties::{EnabledStatistics, WriterProperties};
use parquet::file::statistics::Statistics;
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::parser::parse_message_type;

fn open(path: &Path, msg: &str) -> SerializedFileWriter<File> {
    let schema = Arc::new(parse_message_type(msg).unwrap());
    let props = Arc::new(WriterProperties::builder().set_statistics_enabled(EnabledStatistics::Chunk).set_compression(Compression::UNCOMPRESSED).set_dictionary_enabled(false).build());
    SerializedFileWriter::new(File::create(path).unwrap(), schema, props).unwrap()
}

macro_rules! one_column {
    ($path:expr, $msg:expr, $ty:ty, $vals:expr, $defs:expr) => {{
        let mut w = open($path, $msg);
        let mut rg = w.next_row_group().unwrap();
        let mut c = rg.next_column().unwrap().unwrap();
        c.typed::<$ty>().write_batch(&$vals, Some(&$defs), None).unwrap();
        c.close().unwrap();
        rg.close().unwrap();
        w.close().unwrap();
    }};
}

fn ba(s: &[u8]) -> ByteArray { ByteArray::from(s.to_vec()) }

pub fn dec16(v: i128) -> FixedLenByteArray { FixedLenByteArray::from(ByteArray::from(v.to_be_bytes().to_vec())) }

/// Replace the statistics of column 0, row group 0 by `f(old)`; `None` removes them. The rest of the file is byte for byte the same.
pub fn patch_stats(path: &Path, f: impl FnOnce(Option<&Statistics>) -> Option<Statistics>) {
    let bytes = std::fs::read(path).unwrap();
    let n = bytes.len();
    let flen = u32::from_le_bytes(bytes[n - 8..n - 4].try_into().unwrap()) as usize;
    let start = n - 8 - flen;
    let md = ParquetMetaDataReader::decode_metadata(&bytes[start..n - 8]).unwrap();
    let mut rgs = md.row_groups().to_vec();
    let mut cols = rgs[0].columns().to_vec();
    let old = cols[0].statistics().cloned();
    let mut b = cols[0].clone().into_builder();
    b = match f(old.as_ref()) { Some(s) => b.set_statistics(s), None => b.clear_statistics() };
    cols[0] = b.build().unwrap();
    rgs[0] = rgs[0].clone().into_builder().set_column_metadata(cols).build().unwrap();
    let md2 = ParquetMetaDataBuilder::new(md.file_metadata().clone()).set_row_groups(rgs).build();
    let mut out = bytes[..start].to_vec();
    ParquetMetaDataWriter::new(&mut out, &md2).finish().unwrap();
    std::fs::write(path, out).unwrap();
}

/// (name, expected error kind or None for a control)
pub const FAULTS: &[(&str, Option<&str>)] = &[
    ("string-max-truncated-no-increment", Some("max-not-upper-bound")),
    ("string-min-too-large", Some("min-not-lower-bound")),
    ("float-max-nan", Some("stat-nan")),
    ("int-min-too-high", Some("min-not-lower-bound")),
    ("int-max-too-low", Some("max-not-upper-bound")),
    ("bytes-signed-compare", Some("max-not-upper-bound")),
    ("decimal-unsigned-compare", Some("max-not-upper-bound")),
    ("u64-signed-compare", Some("max-not-upper-bound")),
    ("stats-for-all-null-column", Some("stat-for-nothing")),
    ("string-invalid-utf8-bound", Some("stat-utf8")),
    ("null-count-wrong", Some("null-count")),
    ("control-truncated-sound-bounds", None),
    ("control-no-statistics", None),
];

pub fn write_fault(path: &Path, name: &str) {
    let nc = |s: Option<&Statistics>| s.and_then(|s| s.null_count_opt());
    match name {
        "string-max-truncated-no-increment" | "control-truncated-sound-bounds" | "string-min-too-large" | "string-invalid-utf8-bound" | "null-count-wrong" => {
            let vals = vec![ba(b"apple"), ba(b"applesauce-and-more"), ba(b"zebra-aaaaaaaaaaaaaaaaaaaa"), ba(b"mango")];
            one_column!(path, "message m { optional binary s (UTF8); }", ByteArrayType, vals, vec![1i16; 4]);
            patch_stats(path, |old| {
                let (min, max, nulls) = match name {
                    "string-max-truncated-no-increment" => (ba(b"apple"), ba(b"zebra-a"), nc(old)),                // a prefix of the max is SMALLER than the max
                    "control-truncated-sound-bounds" => (ba(b"app"), ba(b"zebra-b"), nc(old)),                      // min truncated; max truncated AND incremented
                    "string-min-too-large" => (ba(b"b"), ba(b"zebra-aaaaaaaaaaaaaaaaaaaa"), nc(old)),
                    "null-count-wrong" => (ba(b"apple"), ba(b"zebra-aaaaaaaaaaaaaaaaaaaa"), Some(3)),
                    _ => (ba(b"apple"), ByteArray::from(vec![b'z', 0xC3]), nc(old)),                                // a bound cut in the middle of a character
                };
                let loose = name == "control-truncated-sound-bounds";     // truncated bounds are not exact, and say so
                let st = Statistics::byte_array(Some(min), Some(max), None, nulls, false);
                Some(match st { Statistics::ByteArray(v) if loose => Statistics::ByteArray(v.with_min_is_exact(false).with_max_is_exact(false)), s => s })
            });
        }
        "float-max-nan" => {
            one_column!(path, "message m { optional double f; }", DoubleType, vec![1.0f64, 2.0, 3.0], vec![1i16; 3]);
            patch_stats(path, |old| Some(Statistics::double(Some(1.0), Some(f64::NAN), None, nc(old), false)));
        }
        "int-min-too-high" | "int-max-too-low" | "control-no-statistics" => {
            one_column!(path, "message m { optional int64 i; }", Int64Type, (1..=100).collect::<Vec<i64>>(), vec![1i16; 100]);
            patch_stats(path, |old| match name {
                "int-min-too-high" => Some(Statistics::int64(Some(2), Some(100), None, nc(old), false)),
                "int-max-too-low" => Some(Statistics::int64(Some(1), Some(99), None, nc(old), false)),
                _ => None,
            });
        }
        "bytes-signed-compare" => {
            one_column!(path, "message m { optional binary b; }", ByteArrayType, vec![ba(&[0x01]), ba(&[0x7F]), ba(&[0xFF, 0x00]), ba(&[0x80])], vec![1i16; 4]);
            // a signed byte-wise order puts 0xFF and 0x80 BELOW 0x01: its "max" is [0x7F] and its "min" [0x80]
            patch_stats(path, |old| Some(Statistics::byte_array(Some(ba(&[0x80])), Some(ba(&[0x7F])), None, nc(old), false)));
        }
        "decimal-unsigned-compare" => {
            let vals: Vec<FixedLenByteArray> = [-5i128, 3, 100, -200].iter().map(|v| dec16(*v)).collect();
            one_column!(path, "message m { optional fixed_len_byte_array(16) d (DECIMAL(38,10)); }", FixedLenByteArrayType, vals, vec![1i16; 4]);
            // an unsigned comparison of the big-endian bytes makes the negatives look huge: min = 3, max = -5
            patch_stats(path, |old| Some(Statistics::fixed_len_byte_array(Some(dec16(3)), Some(dec16(-5)), None, nc(old), false)));
        }
        "u64-signed-compare" => {
            let vals: Vec<i64> = [1u64, 5, (1 << 63) + 5, u64::MAX].iter().map(|v| *v as i64).collect();
            one_column!(path, "message m { optional int64 u (INTEGER(64,false)); }", Int64Type, vals, vec![1i16; 4]);
            patch_stats(path, |old| Some(Statistics::int64(Some(u64::MAX as i64), Some(5), None, nc(old), false)));   // signed min and max of the same bits
        }
        "stats-for-all-null-column" => {
            one_column!(path, "message m { optional int32 i; }", Int32Type, Vec::<i32>::new(), vec![0i16; 5]);
            patch_stats(path, |old| Some(Statistics::int32(Some(0), Some(0), None, nc(old), false)));
        }
        other => panic!("unknown fault {other}"),
    }
}
