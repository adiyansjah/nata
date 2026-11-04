//! Small deterministic lineitem-like Parquet files for the unit tests (no DuckDB needed), and the reference result
//! computed straight from the generated rows with plain loops (independent of the scan path).
//! The columns and types are the ones DuckDB writes for TPC-H lineitem, so the same queries run on both.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use arrow::array::{ArrayRef, Date32Array, Decimal128Array, Int64Array, StringArray};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;

use crate::exec::{Groups, QuerySpec, render_groups};
use crate::layouts::file_name;
use crate::util::Rng;

pub struct Rows {
    pub orderkey: Vec<i64>,
    pub partkey: Vec<i64>,
    pub quantity: Vec<i64>,
    pub price: Vec<i64>,
    pub flag: Vec<u8>,
    pub status: Vec<u8>,
    pub shipdate: Vec<i32>,
}

impl Rows {
    pub fn len(&self) -> usize {
        self.orderkey.len()
    }
    pub fn is_empty(&self) -> bool {
        self.orderkey.is_empty()
    }
}

pub fn gen_rows(n: usize, seed: u64) -> Rows {
    let mut rng = Rng::new(seed);
    let mut r = Rows {
        orderkey: Vec::with_capacity(n),
        partkey: Vec::with_capacity(n),
        quantity: Vec::with_capacity(n),
        price: Vec::with_capacity(n),
        flag: Vec::with_capacity(n),
        status: Vec::with_capacity(n),
        shipdate: Vec::with_capacity(n),
    };
    let mut ok = 0i64;
    for i in 0..n {
        if i % 4 == 0 {
            ok += 1 + (rng.next_u64() % 3) as i64;
        }
        r.orderkey.push(ok);
        r.partkey.push((rng.next_u64() % 200_000) as i64 + 1);
        r.quantity.push(100 * (1 + (rng.next_u64() % 50) as i64));
        r.price.push(90_000 + (rng.next_u64() % 10_000_000) as i64);
        r.flag.push(b"RAN"[(rng.next_u64() % 3) as usize]);
        r.status.push(b"OF"[(rng.next_u64() % 2) as usize]);
        // 1992-01-02 .. about 1998-12-01: some rows fall after the filter date 1998-09-02 (10471)
        r.shipdate.push(8036 + (rng.next_u64() % 2555) as i32);
    }
    r
}

pub fn schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("l_orderkey", DataType::Int64, false),
        Field::new("l_partkey", DataType::Int64, false),
        Field::new("l_quantity", DataType::Decimal128(15, 2), false),
        Field::new("l_extendedprice", DataType::Decimal128(15, 2), false),
        Field::new("l_returnflag", DataType::Utf8, false),
        Field::new("l_linestatus", DataType::Utf8, false),
        Field::new("l_shipdate", DataType::Date32, false),
    ]))
}

fn batch(rows: &Rows, a: usize, b: usize) -> RecordBatch {
    let dec = |v: &[i64]| -> ArrayRef {
        Arc::new(
            Decimal128Array::from(v.iter().map(|&x| i128::from(x)).collect::<Vec<_>>())
                .with_precision_and_scale(15, 2)
                .expect("precision"),
        )
    };
    let st = |v: &[u8]| -> ArrayRef {
        Arc::new(StringArray::from(
            v.iter().map(|&c| String::from_utf8_lossy(&[c]).into_owned()).collect::<Vec<_>>(),
        ))
    };
    RecordBatch::try_new(
        schema(),
        vec![
            Arc::new(Int64Array::from(rows.orderkey[a..b].to_vec())),
            Arc::new(Int64Array::from(rows.partkey[a..b].to_vec())),
            dec(&rows.quantity[a..b]),
            dec(&rows.price[a..b]),
            st(&rows.flag[a..b]),
            st(&rows.status[a..b]),
            Arc::new(Date32Array::from(rows.shipdate[a..b].to_vec())),
        ],
    )
    .expect("batch")
}

/// Write rows `a..b` to `path`, one row group per `rows_per_rg` rows (0 = a single row group).
pub fn write_file(path: &Path, rows: &Rows, a: usize, b: usize, rows_per_rg: usize) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let rg = if rows_per_rg == 0 { (b - a).max(1) } else { rows_per_rg };
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_max_row_group_row_count(Some(rg))
        .build();
    let f = std::fs::File::create(path)?;
    let mut w = ArrowWriter::try_new(f, schema(), Some(props)).map_err(std::io::Error::other)?;
    let mut s = a;
    while s < b {
        let e = (s + rg).min(b);
        w.write(&batch(rows, s, e)).map_err(std::io::Error::other)?;
        w.flush().map_err(std::io::Error::other)?;
        s = e;
    }
    w.close().map_err(std::io::Error::other)?;
    Ok(())
}

/// `root/<layout>/part-NNNN.parquet` for `files` files holding all rows in order.
pub fn write_layout(root: &Path, layout: &str, rows: &Rows, files: usize, rows_per_rg: usize) -> std::io::Result<()> {
    let per = rows.len().div_ceil(files.max(1));
    for i in 0..files {
        let a = (i * per).min(rows.len());
        let b = ((i + 1) * per).min(rows.len());
        write_file(&root.join(layout).join(file_name(i)), rows, a, b, rows_per_rg)?;
    }
    Ok(())
}

fn column(rows: &Rows, name: &str) -> Vec<i64> {
    match name {
        "l_orderkey" => rows.orderkey.clone(),
        "l_partkey" => rows.partkey.clone(),
        "l_quantity" => rows.quantity.clone(),
        "l_extendedprice" => rows.price.clone(),
        "l_returnflag" => rows.flag.iter().map(|&c| i64::from(c)).collect(),
        "l_linestatus" => rows.status.iter().map(|&c| i64::from(c)).collect(),
        "l_shipdate" => rows.shipdate.iter().map(|&d| i64::from(d)).collect(),
        _ => panic!("unknown column {name}"),
    }
}

/// The expected result text of `spec` over `rows`.
pub fn reference(spec: &QuerySpec, rows: &Rows) -> String {
    if spec.limit.is_some() {
        return format!("{} row(s)\n", spec.limit.unwrap_or(0).min(rows.len()));
    }
    let cols: Vec<Vec<i64>> = spec.columns.iter().map(|c| column(rows, c)).collect();
    let mut g: Groups = HashMap::new();
    for r in 0..rows.len() {
        if let Some((c, v)) = spec.filter_le {
            if cols[c][r] > v {
                continue;
            }
        }
        let key: Vec<i64> = spec.group.iter().map(|&c| cols[c][r]).collect();
        let e = g.entry(key).or_insert_with(|| (0, vec![0i128; spec.sums.len()]));
        e.0 += 1;
        for (acc, &c) in e.1.iter_mut().zip(spec.sums) {
            *acc += i128::from(cols[c][r]);
        }
    }
    if spec.group.is_empty() && g.is_empty() {
        g.insert(Vec::new(), (0, vec![0i128; spec.sums.len()]));
    }
    render_groups(spec, &g)
}
