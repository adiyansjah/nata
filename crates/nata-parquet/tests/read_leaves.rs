//! `read_leaves`: the schema of a file from its footer alone, for the SQL binder (slice 4). It lists unsupported columns with their reason, and its errors
//! name the file.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::{
    ArrayRef, Decimal128Array, Int32Array, RecordBatch, StringArray, TimestampMicrosecondArray,
};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use nata_parquet::schema::Kind;
use nata_parquet::{ErrorKind, read_leaves};
use parquet::arrow::ArrowWriter;

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("nata-leaves-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).expect("scratch dir");
    d
}

fn write(path: &Path) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int32, false),
        Field::new("Name", DataType::Utf8, true),
        Field::new("price", DataType::Decimal128(15, 2), true),
        Field::new(
            "ts",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            true,
        ),
    ]));
    let cols: Vec<ArrayRef> = vec![
        Arc::new(Int32Array::from(vec![1, 2])),
        Arc::new(StringArray::from(vec![Some("a"), None])),
        Arc::new(
            Decimal128Array::from(vec![Some(150), None])
                .with_precision_and_scale(15, 2)
                .expect("decimal"),
        ),
        Arc::new(TimestampMicrosecondArray::from(vec![Some(1), None]).with_timezone("UTC")),
    ];
    let batch = RecordBatch::try_new(schema.clone(), cols).expect("batch");
    let mut w = ArrowWriter::try_new(std::fs::File::create(path).expect("create"), schema, None)
        .expect("writer");
    w.write(&batch).expect("write");
    w.close().expect("close");
}

#[test]
fn the_footer_gives_names_in_file_order_with_kinds_and_reasons() {
    let d = scratch("ok");
    let f = d.join("t.parquet");
    write(&f);
    let leaves = read_leaves(&f).expect("schema");
    let names: Vec<&str> = leaves.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(
        names,
        ["id", "Name", "price", "ts"],
        "file order, case kept"
    );
    assert_eq!(
        leaves[0].kind,
        Ok(Kind::Int {
            bits: 32,
            signed: true
        })
    );
    assert_eq!(leaves[1].kind, Ok(Kind::Utf8));
    assert_eq!(
        leaves[2].kind,
        Ok(Kind::Decimal {
            precision: 15,
            scale: 2
        })
    );
    let why = leaves[3]
        .kind
        .clone()
        .expect_err("a timestamp with a time zone is not supported");
    assert!(why.contains("TIMESTAMP"), "{why}");
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn a_missing_or_damaged_file_is_an_error_that_names_the_file() {
    let d = scratch("bad");
    let missing = d.join("nope.parquet");
    let e = read_leaves(&missing).expect_err("missing");
    assert!(matches!(e.kind, ErrorKind::Io(_)), "{e}");
    assert_eq!(e.file.as_deref(), Some(missing.as_path()));
    let junk = d.join("junk.parquet");
    std::fs::write(&junk, b"this is not a parquet file at all, not even close").expect("write");
    let e = read_leaves(&junk).expect_err("junk");
    assert!(matches!(e.kind, ErrorKind::Corrupt(_)), "{e}");
    assert_eq!(e.file.as_deref(), Some(junk.as_path()));
    std::fs::remove_dir_all(&d).ok();
}
