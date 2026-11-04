//! Parquet scan: one row group per morsel, columns read as the types the engine wants
//! (dictionary strings with Int32 keys for low-cardinality columns, Decimal64(15,2) for decimals,
//! plain Utf8 for everything else, so a "plain string" key really is a plain string column).
use std::fs::File;
use std::sync::Arc;

use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{
    ArrowReaderMetadata, ArrowReaderOptions, ParquetRecordBatchReader, ParquetRecordBatchReaderBuilder,
};

pub const BATCH_ROWS: usize = 8192;

fn wanted_type(name: &str) -> Option<DataType> {
    match name {
        "l_returnflag" | "l_linestatus" | "l_shipmode" | "l_shipinstruct" | "c_mktsegment" | "o_orderpriority" => {
            Some(DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)))
        }
        "l_quantity" | "l_extendedprice" | "l_discount" | "l_tax" | "o_totalprice" => Some(DataType::Decimal64(15, 2)),
        _ => None,
    }
}

pub struct Source {
    path: String,
    meta: ArrowReaderMetadata,
    mask: ProjectionMask,
    pub row_groups: usize,
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
                Some(t) => Arc::new(Field::new(f.name(), t, f.is_nullable())),
                None => f.clone(),
            })
            .collect();
        let schema: SchemaRef = Arc::new(Schema::new(fields));
        let meta = ArrowReaderMetadata::load(&file, ArrowReaderOptions::new().with_schema(schema)).expect("metadata");
        let descr = meta.metadata().file_metadata().schema_descr_ptr();
        let mask = ProjectionMask::columns(&descr, columns.iter().copied());
        let row_groups = meta.metadata().num_row_groups();
        Source { path: path.to_string(), meta, mask, row_groups }
    }

    /// A fresh file handle per reader: cloned handles share the seek offset and corrupt concurrent reads.
    pub fn reader(&self, row_group: usize) -> ParquetRecordBatchReader {
        ParquetRecordBatchReaderBuilder::new_with_metadata(File::open(&self.path).expect("open"), self.meta.clone())
            .with_projection(self.mask.clone())
            .with_row_groups(vec![row_group])
            .with_batch_size(BATCH_ROWS)
            .build()
            .expect("reader")
    }
}
