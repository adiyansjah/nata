//! Small lineitem-shaped Parquet files made in memory (16 columns, orderkeys ascending so statistics can prune), for the tests.

use std::sync::Arc;

use arrow_array::{ArrayRef, Date32Array, Decimal128Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

use nata_objstore::util::Rng;

/// `rows` rows, row groups of `rg_rows`, from `seed`. Orderkeys ascend with gaps; every other column is pseudo-random.
pub fn lineitem_like(rows: usize, rg_rows: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut ok = Vec::with_capacity(rows);
    let mut k = 1i64;
    for i in 0..rows {
        if i % 4 == 0 {
            k += 1 + rng.below(5) as i64;
        }
        ok.push(k);
    }
    let i64s = |f: &mut dyn FnMut() -> i64| -> ArrayRef {
        Arc::new(Int64Array::from((0..rows).map(|_| f()).collect::<Vec<_>>()))
    };
    let f64s = |rng: &mut Rng| -> ArrayRef {
        Arc::new(
            Decimal128Array::from(
                (0..rows)
                    .map(|_| rng.below(10_000_000) as i128)
                    .collect::<Vec<_>>(),
            )
            .with_precision_and_scale(15, 2)
            .expect("test data"),
        )
    };
    let strs = |rng: &mut Rng, n: u64, pre: &str| -> ArrayRef {
        Arc::new(StringArray::from(
            (0..rows)
                .map(|_| format!("{pre}{}", rng.below(n)))
                .collect::<Vec<_>>(),
        ))
    };
    let dates = |rng: &mut Rng| -> ArrayRef {
        Arc::new(Date32Array::from(
            (0..rows)
                .map(|_| 8000 + rng.below(2500) as i32)
                .collect::<Vec<_>>(),
        ))
    };
    let mut r1 = Rng::new(seed ^ 1);
    let cols: Vec<(&str, DataType, ArrayRef)> = vec![
        (
            "l_orderkey",
            DataType::Int64,
            Arc::new(Int64Array::from(ok)),
        ),
        (
            "l_partkey",
            DataType::Int64,
            i64s(&mut || 1 + r1.below(200_000) as i64),
        ),
        (
            "l_suppkey",
            DataType::Int64,
            i64s(&mut || 1 + r1.below(10_000) as i64),
        ),
        (
            "l_linenumber",
            DataType::Int64,
            i64s(&mut || 1 + r1.below(7) as i64),
        ),
        ("l_quantity", DataType::Decimal128(15, 2), f64s(&mut rng)),
        (
            "l_extendedprice",
            DataType::Decimal128(15, 2),
            f64s(&mut rng),
        ),
        ("l_discount", DataType::Decimal128(15, 2), f64s(&mut rng)),
        ("l_tax", DataType::Decimal128(15, 2), f64s(&mut rng)),
        ("l_returnflag", DataType::Utf8, strs(&mut rng, 3, "R")),
        ("l_linestatus", DataType::Utf8, strs(&mut rng, 2, "S")),
        ("l_shipdate", DataType::Date32, dates(&mut rng)),
        ("l_commitdate", DataType::Date32, dates(&mut rng)),
        ("l_receiptdate", DataType::Date32, dates(&mut rng)),
        ("l_shipinstruct", DataType::Utf8, strs(&mut rng, 4, "I")),
        ("l_shipmode", DataType::Utf8, strs(&mut rng, 7, "M")),
        (
            "l_comment",
            DataType::Utf8,
            strs(&mut rng, 1_000_000, "a comment about the order "),
        ),
    ];
    let schema = Arc::new(Schema::new(
        cols.iter()
            .map(|(n, t, _)| Field::new(*n, t.clone(), true))
            .collect::<Vec<_>>(),
    ));
    let batch = RecordBatch::try_new(
        schema.clone(),
        cols.into_iter().map(|(_, _, a)| a).collect(),
    )
    .expect("test data");
    let mut out = Vec::new();
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(rg_rows))
        .build();
    let mut w = ArrowWriter::try_new(&mut out, schema, Some(props)).expect("test data");
    w.write(&batch).expect("test data");
    w.close().expect("test data");
    out
}
