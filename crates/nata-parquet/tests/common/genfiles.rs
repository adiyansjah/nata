//! Small generated files for the parallel, fault and resource tests (parquet-rs as the writer; no DuckDB needed).
#![allow(dead_code, reason = "each test file uses a different part")]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::{ArrayRef, Float64Array, Int64Array, RecordBatch, StringArray};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

/// `rows` rows with `id = first..first+rows`, a low-cardinality text column `cat` (dictionary encoded), and a double, in row groups of `group` rows.
pub fn write_ids(path: &Path, first: i64, rows: i64, group: usize) {
    let ids: ArrayRef = Arc::new(Int64Array::from_iter_values(first..first + rows));
    let cat: ArrayRef = Arc::new(StringArray::from_iter_values(
        (first..first + rows).map(|i| format!("cat{}", i % 7)),
    ));
    let x: ArrayRef = Arc::new(Float64Array::from_iter_values(
        (first..first + rows).map(|i| (i as f64) * 0.5),
    ));
    let batch = RecordBatch::try_from_iter([("id", ids), ("cat", cat), ("x", x)]).expect("batch");
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(group))
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

/// `files` files in `dir`, `a000.parquet`, ... with consecutive ids; file `i` has `base + (i * 13) % spread` rows, row groups of `group` rows. Returns the paths in order.
pub fn write_many(dir: &Path, files: usize, base: i64, spread: i64, group: usize) -> Vec<PathBuf> {
    let mut next = 0i64;
    let mut out = Vec::new();
    for i in 0..files {
        let rows = base + (i as i64 * 13) % spread;
        let p = dir.join(format!("a{i:05}.parquet"));
        write_ids(&p, next, rows, group);
        next += rows;
        out.push(p);
    }
    out
}
