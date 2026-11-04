//! Parquet scan: one row group per morsel, columns read as the types the engine wants
//! (dictionary strings with Int32 keys, Decimal64(15,2) for the decimal columns).
use std::fs::File;

use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{
    ArrowReaderMetadata, ArrowReaderOptions, ParquetRecordBatchReader, ParquetRecordBatchReaderBuilder,
};

pub const BATCH_ROWS: usize = 8192;

fn wanted_type(name: &str) -> Option<DataType> {
    match name {
        "l_returnflag" | "l_linestatus" => Some(DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))),
        "l_quantity" | "l_extendedprice" | "l_discount" | "l_tax" => Some(DataType::Decimal64(15, 2)),
        _ => None,
    }
}

pub struct Source {
    path: String,
    meta: ArrowReaderMetadata,
    mask: ProjectionMask,
    pub row_groups: usize,
    batch_rows: usize,
}

impl Source {
    pub fn open(path: &str, columns: &[&str]) -> Self {
        let file = File::open(path).expect("open parquet file");
        let plain = ArrowReaderMetadata::load(&file, ArrowReaderOptions::new()).expect("metadata");
        let fields: Vec<_> = plain
            .schema()
            .fields()
            .iter()
            .map(|f| match wanted_type(f.name()) {
                Some(t) => std::sync::Arc::new(Field::new(f.name(), t, f.is_nullable())),
                None => f.clone(),
            })
            .collect();
        let schema: SchemaRef = std::sync::Arc::new(Schema::new(fields));
        let meta = ArrowReaderMetadata::load(&file, ArrowReaderOptions::new().with_schema(schema)).expect("metadata");
        let descr = meta.metadata().file_metadata().schema_descr_ptr();
        let mask = ProjectionMask::columns(&descr, columns.iter().copied());
        let row_groups = meta.metadata().num_row_groups();
        Source { path: path.to_string(), meta, mask, row_groups, batch_rows: BATCH_ROWS }
    }

    /// Rows per batch (default 8,192); `nullcheck` also uses sizes that are not a multiple of 64.
    pub fn with_batch_rows(mut self, n: usize) -> Self {
        self.batch_rows = n;
        self
    }

    /// A fresh file handle per reader: cloned handles share the seek offset and corrupt concurrent reads.
    pub fn reader(&self, row_group: usize) -> ParquetRecordBatchReader {
        ParquetRecordBatchReaderBuilder::new_with_metadata(File::open(&self.path).expect("open"), self.meta.clone())
            .with_projection(self.mask.clone())
            .with_row_groups(vec![row_group])
            .with_batch_size(self.batch_rows)
            .build()
            .expect("reader")
    }
}
