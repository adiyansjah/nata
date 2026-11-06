//! Fixtures: Parquet files written with parquet-rs, and a way to run a query to CSV text through the whole front end and the executor.
#![allow(dead_code, reason = "each test file uses a different part")]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "test data: small loop counters become small column values"
)]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::{
    ArrayRef, BooleanArray, Date32Array, Decimal128Array, Float64Array, Int8Array, Int32Array,
    Int64Array, RecordBatch, StringArray,
};
use nata_exec::output::{Format, ResultWriter, render_scan_error, render_sql_error};
use nata_exec::{ExecConfig, ExecReport, execute};
use nata_parquet::ScanError;
use nata_sql::{FsSource, plan_sql};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

pub fn dir(name: &str) -> PathBuf {
    let base = std::env::var("NATA_TEST_TMP").map_or_else(|_| std::env::temp_dir(), PathBuf::from);
    let d = base.join(format!("nata-exec-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("mkdir");
    d
}

pub fn write_batch(path: &Path, batch: &RecordBatch, row_group: usize) {
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(row_group))
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

/// 20 rows, row groups of 5. id 0..19; a = id % 5 - 2 (NULL when id % 7 == 3); ti = id as i8 (NULL at id 6); b = id * 1_000_000_000_000;
/// price DECIMAL(10,2) = id * 1.25; d DOUBLE: NaN at 7, -0.0 at 8, 0.0 at 9, inf at 10, else id / 2; s text; dt date; f boolean.
pub fn standard(path: &Path) {
    let n: i32 = 20;
    let id: ArrayRef = Arc::new(Int32Array::from_iter_values(0..n));
    let a: ArrayRef = Arc::new(Int32Array::from_iter(
        (0..n).map(|i| (i % 7 != 3).then_some(i % 5 - 2)),
    ));
    let ti: ArrayRef = Arc::new(Int8Array::from_iter(
        (0..n).map(|i| (i != 6).then_some(i as i8)),
    ));
    let b: ArrayRef = Arc::new(Int64Array::from_iter_values(
        (0..i64::from(n)).map(|i| i * 1_000_000_000_000),
    ));
    let price: ArrayRef = Arc::new(
        Decimal128Array::from_iter_values((0..i128::from(n)).map(|i| i * 125))
            .with_precision_and_scale(10, 2)
            .expect("decimal"),
    );
    let d: ArrayRef = Arc::new(Float64Array::from_iter_values((0..n).map(|i| match i {
        7 => f64::NAN,
        8 => -0.0,
        9 => 0.0,
        10 => f64::INFINITY,
        _ => f64::from(i) / 2.0,
    })));
    let s: ArrayRef = Arc::new(StringArray::from_iter((0..n).map(|i| match i % 4 {
        0 => Some("alpha".to_string()),
        1 => Some("Beta".to_string()),
        2 => None,
        _ => Some(format!("g,{i}")),
    })));
    let dt: ArrayRef = Arc::new(Date32Array::from_iter_values(
        (0..n).map(|i| 8766 + i * 100),
    ));
    let f: ArrayRef = Arc::new(BooleanArray::from_iter(
        (0..n).map(|i| (i % 3 != 2).then_some(i % 2 == 0)),
    ));
    let batch = RecordBatch::try_from_iter([
        ("id", id),
        ("a", a),
        ("ti", ti),
        ("b", b),
        ("price", price),
        ("d", d),
        ("s", s),
        ("dt", dt),
        ("f", f),
    ])
    .expect("batch");
    write_batch(path, &batch, 5);
}

pub struct Outcome {
    pub csv: Result<String, String>,
    pub report: Option<ExecReport>,
    pub rendered_error: Option<String>,
}

/// The query's result as CSV text (header included), or the rendered error.
/// `NATA_LATE_MATERIALIZATION=1` runs every query of these tests with late materialization in the scan (P1): the same rows are required.
pub fn late_from_env() -> bool {
    std::env::var("NATA_LATE_MATERIALIZATION").is_ok_and(|v| v == "1")
}

pub fn run_with(sql: &str, cfg: &ExecConfig) -> Outcome {
    let mut cfg = cfg.clone();
    cfg.late_materialization |= late_from_env();
    let cfg = &cfg;
    let planned = match plan_sql(sql, &FsSource) {
        Ok(p) => p,
        Err(e) => {
            let text = render_sql_error(sql, &e);
            return Outcome {
                csv: Err(text.clone()),
                report: None,
                rendered_error: Some(text),
            };
        }
    };
    let names: Vec<String> = planned.output.iter().map(|c| c.name.clone()).collect();
    let types: Vec<_> = planned.output.iter().map(|c| c.ty).collect();
    let mut buf: Vec<u8> = Vec::new();
    let result = {
        let mut w = ResultWriter::new(&mut buf, Format::Csv, names, &types);
        let r = execute(Arc::new(planned.plan), cfg, None, |b| w.write_batch(b));
        match r {
            Ok(rep) => w.finish().map(|()| rep),
            Err(e) => Err(e),
        }
    };
    match result {
        Ok(rep) => Outcome {
            csv: Ok(String::from_utf8(buf).expect("utf8")),
            report: Some(rep),
            rendered_error: None,
        },
        Err(e) => {
            let text = render_scan_error(sql, &e, 0);
            Outcome {
                csv: Err(text.clone()),
                report: None,
                rendered_error: Some(text),
            }
        }
    }
}

pub fn run(sql: &str) -> Result<String, String> {
    run_with(sql, &ExecConfig::new(4)).csv
}

/// The rows (without the header) as lines.
pub fn rows(sql: &str) -> Vec<String> {
    let csv = run(sql).unwrap_or_else(|e| panic!("{sql}\n{e}"));
    csv.lines().skip(1).map(str::to_string).collect()
}

pub fn err(sql: &str) -> String {
    match run(sql) {
        Ok(csv) => panic!("{sql}: expected an error, got\n{csv}"),
        Err(e) => e,
    }
}

pub fn scan_error_text(e: &ScanError) -> String {
    e.to_string()
}
