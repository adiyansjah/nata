//! parquet-rs as a WRITER: re-encode a file (written by DuckDB) with chosen encodings, codecs, page versions, statistics truncation and row-group size.
#![allow(dead_code, reason = "each test file uses a different part")]

use std::fs::File;
use std::path::Path;

use arrow_array::RecordBatch;
use arrow_select::concat::concat_batches;
use parquet::arrow::ArrowWriter;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::{Compression, Encoding, GzipLevel, ZstdLevel};
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};
use parquet::schema::types::ColumnPath;

#[derive(Clone, Debug)]
pub struct Style {
    pub name: &'static str,
    pub compression: Compression,
    pub version: WriterVersion,
    pub dictionary: bool,
    /// (column name or `*` for every column of that kind of type, encoding); applies to columns whose type accepts the encoding.
    pub int_encoding: Option<Encoding>,
    pub float_encoding: Option<Encoding>,
    pub text_encoding: Option<Encoding>,
    pub row_group_rows: usize,
    pub truncate: Option<usize>,
    pub page_rows: Option<usize>,
}

impl Style {
    pub fn base(name: &'static str) -> Style {
        Style {
            name,
            compression: Compression::UNCOMPRESSED,
            version: WriterVersion::PARQUET_1_0,
            dictionary: true,
            int_encoding: None,
            float_encoding: None,
            text_encoding: None,
            row_group_rows: 2048,
            truncate: None,
            page_rows: None,
        }
    }
}

/// The matrix of styles of the writer test: every codec, both page versions, dictionary and the non-dictionary encodings.
pub fn styles() -> Vec<Style> {
    let b = Style::base;
    let mut v = vec![
        b("plain-dict-v1-none"),
        Style {
            compression: Compression::SNAPPY,
            ..b("snappy-dict-v1")
        },
        Style {
            compression: Compression::ZSTD(ZstdLevel::default()),
            ..b("zstd-dict-v1")
        },
        Style {
            compression: Compression::GZIP(GzipLevel::default()),
            ..b("gzip-dict-v1")
        },
        Style {
            compression: Compression::LZ4_RAW,
            ..b("lz4raw-dict-v1")
        },
        Style {
            compression: Compression::SNAPPY,
            version: WriterVersion::PARQUET_2_0,
            ..b("snappy-dict-v2")
        },
        Style {
            compression: Compression::ZSTD(ZstdLevel::default()),
            version: WriterVersion::PARQUET_2_0,
            ..b("zstd-dict-v2")
        },
        Style {
            dictionary: false,
            ..b("plain-v1")
        },
        Style {
            dictionary: false,
            version: WriterVersion::PARQUET_2_0,
            compression: Compression::SNAPPY,
            ..b("plain-v2-snappy")
        },
        Style {
            dictionary: false,
            version: WriterVersion::PARQUET_2_0,
            int_encoding: Some(Encoding::DELTA_BINARY_PACKED),
            text_encoding: Some(Encoding::DELTA_LENGTH_BYTE_ARRAY),
            ..b("delta-v2")
        },
        Style {
            dictionary: false,
            version: WriterVersion::PARQUET_2_0,
            int_encoding: Some(Encoding::DELTA_BINARY_PACKED),
            text_encoding: Some(Encoding::DELTA_BYTE_ARRAY),
            compression: Compression::ZSTD(ZstdLevel::default()),
            ..b("delta-bytearray-v2-zstd")
        },
        Style {
            dictionary: false,
            version: WriterVersion::PARQUET_2_0,
            float_encoding: Some(Encoding::BYTE_STREAM_SPLIT),
            int_encoding: Some(Encoding::BYTE_STREAM_SPLIT),
            ..b("bss-v2")
        },
        Style {
            page_rows: Some(500),
            row_group_rows: 3000,
            ..b("small-pages")
        },
        Style {
            truncate: Some(4),
            ..b("truncate4")
        },
    ];
    v.sort_by_key(|s| s.name);
    v
}

pub fn read_all(path: &Path) -> RecordBatch {
    let file = File::open(path).expect("open");
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).expect("builder");
    let schema = builder.schema().clone();
    let batches: Vec<RecordBatch> = builder
        .build()
        .expect("reader")
        .map(|b| b.expect("batch"))
        .collect();
    concat_batches(&schema, &batches).expect("concat")
}

/// Write `batch` as one Parquet file in this style.
pub fn write(batch: &RecordBatch, path: &Path, style: &Style) {
    let mut props = WriterProperties::builder()
        .set_compression(style.compression)
        .set_writer_version(style.version)
        .set_dictionary_enabled(style.dictionary)
        .set_max_row_group_row_count(Some(style.row_group_rows))
        .set_statistics_enabled(EnabledStatistics::Chunk)
        .set_statistics_truncate_length(style.truncate);
    if let Some(p) = style.page_rows {
        props = props.set_data_page_row_count_limit(p);
    }
    if !style.dictionary {
        for f in batch.schema().fields() {
            use arrow_schema::DataType as D;
            let enc = match f.data_type() {
                D::Int8
                | D::Int16
                | D::Int32
                | D::Int64
                | D::UInt8
                | D::UInt16
                | D::UInt32
                | D::UInt64
                | D::Date32
                | D::Timestamp(..) => style.int_encoding,
                D::Float32 | D::Float64 => style.float_encoding,
                D::Utf8 | D::Binary | D::LargeUtf8 => style.text_encoding,
                _ => None,
            };
            if let Some(e) = enc {
                props = props.set_column_encoding(ColumnPath::from(f.name().as_str()), e);
            }
        }
    }
    let file = File::create(path).expect("create");
    let mut w = ArrowWriter::try_new(file, batch.schema(), Some(props.build())).expect("writer");
    w.write(batch).expect("write");
    w.close().expect("close");
}
