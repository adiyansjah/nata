//! Writes the same four columns three ways: Arrow IPC file, Arrow IPC stream, Parquet.
//!   d64   Decimal64(15,2)  with NULLs
//!   d128  Decimal128(15,2) with NULLs
//!   d64_wide Decimal64(18,0), values near the 18-digit limit
//! Values (unscaled): 12345 (123.45), NULL, -5 (-0.05), 999999999999999 (9999999999999.99, the 15-digit max), 0
use std::fs::File;
use std::sync::Arc;

use arrow::array::{Array, ArrayRef, Decimal128Array, Decimal64Array, Int32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::ipc::writer::{FileWriter, StreamWriter};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args().nth(1).expect("usage: writer <out-dir>");
    std::fs::create_dir_all(&dir)?;
    let u: Vec<Option<i64>> = vec![Some(12345), None, Some(-5), Some(999_999_999_999_999), Some(0)];
    let d64 = Decimal64Array::from(u.clone()).with_precision_and_scale(15, 2)?;
    let d128 = Decimal128Array::from(u.iter().map(|x| x.map(i128::from)).collect::<Vec<_>>())
        .with_precision_and_scale(15, 2)?;
    let wide = Decimal64Array::from(vec![Some(999_999_999_999_999_999i64), None, Some(-999_999_999_999_999_999), Some(1), Some(0)])
        .with_precision_and_scale(18, 0)?;
    let id = Int32Array::from(vec![1, 2, 3, 4, 5]);
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int32, false),
        Field::new("d64", d64.data_type().clone(), true),
        Field::new("d128", d128.data_type().clone(), true),
        Field::new("d64_wide", wide.data_type().clone(), true),
    ]));
    let cols: Vec<ArrayRef> = vec![Arc::new(id), Arc::new(d64), Arc::new(d128), Arc::new(wide)];
    let batch = RecordBatch::try_new(schema.clone(), cols)?;
    println!("schema: {:?}", schema);

    let mut fw = FileWriter::try_new(File::create(format!("{dir}/d64.arrow"))?, &schema)?;
    fw.write(&batch)?;
    fw.finish()?;
    let mut sw = StreamWriter::try_new(File::create(format!("{dir}/d64.arrows"))?, &schema)?;
    sw.write(&batch)?;
    sw.finish()?;

    // Parquet: what does the arrow-rs writer do with Decimal64?
    let mut pw = ArrowWriter::try_new(File::create(format!("{dir}/d64.parquet"))?, schema.clone(), None)?;
    pw.write(&batch)?;
    let meta = pw.close()?;
    for c in meta.file_metadata().schema_descr().columns() {
        println!("parquet column {}: physical={:?} logical={:?} converted={:?}", c.name(), c.physical_type(), c.logical_type_ref(), c.converted_type());
    }
    // Read it back with arrow-rs to see what type comes back
    let rb = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(File::open(format!("{dir}/d64.parquet"))?)?;
    println!("parquet read-back arrow schema: {:?}", rb.schema());
    Ok(())
}
