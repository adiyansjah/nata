//! Parquet source (one morsel = one row group, claimed by whichever task asks first) and a Parquet writer for the differential check.
use std::collections::VecDeque;
use std::fs::File;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

use arrow::array::RecordBatch;
use arrow::datatypes::{Field, Schema, SchemaRef};
use parquet::arrow::arrow_reader::{ArrowReaderMetadata, ParquetRecordBatchReaderBuilder};
use parquet::arrow::{ArrowWriter, ProjectionMask};

use crate::batch::Batch;
use crate::proto::api::*;
use crate::spill::ioerr;

pub struct ParquetSource {
    path: String,
    meta: ArrowReaderMetadata,
    mask_cols: Vec<String>,
    schema: SchemaRef,
    prefix: Option<(String, String)>,
    next_rg: AtomicUsize,
    rgs: usize,
}

impl ParquetSource {
    /// Read `columns` of the file. `prefix = Some(("l_", "l2_"))` renames every column that starts with the first string (a self-join needs
    /// distinct names).
    pub fn new(path: &str, columns: &[&str], prefix: Option<(&str, &str)>) -> Result<Arc<Self>> {
        let f = File::open(path).map_err(ioerr(path))?;
        let meta = ArrowReaderMetadata::load(&f, Default::default()).map_err(ioerr("parquet metadata"))?;
        let full = meta.schema().clone();
        let mut fields = vec![];
        for c in columns {
            let (_, fld) = full.fields().find(c).ok_or_else(|| QueryError(format!("no column {c} in {path}")))?;
            let name = match prefix {
                Some((a, b)) if c.starts_with(a) => format!("{b}{}", &c[a.len()..]),
                _ => c.to_string(),
            };
            fields.push(Field::new(name, fld.data_type().clone(), fld.is_nullable()));
        }
        let rgs = meta.metadata().num_row_groups();
        Ok(Arc::new(ParquetSource {
            path: path.to_string(),
            meta,
            mask_cols: columns.iter().map(|s| s.to_string()).collect(),
            schema: Arc::new(Schema::new(fields)),
            prefix: prefix.map(|(a, b)| (a.to_string(), b.to_string())),
            next_rg: AtomicUsize::new(0),
            rgs,
        }))
    }

    pub fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }
}

struct PqSource {
    sh: Arc<ParquetSource>,
    queue: VecDeque<RecordBatch>,
}

impl SourceFactory for Arc<ParquetSource> {
    fn make(&self) -> Box<dyn Source> {
        Box::new(PqSource { sh: Arc::clone(self), queue: VecDeque::new() })
    }
}

impl Source for PqSource {
    fn next(&mut self, _w: &Waker) -> Result<SourceResult> {
        loop {
            if let Some(b) = self.queue.pop_front() {
                return Ok(SourceResult::Batch(Batch::new(b)));
            }
            let rg = self.sh.next_rg.fetch_add(1, SeqCst);
            if rg >= self.sh.rgs {
                return Ok(SourceResult::Finished);
            }
            let f = File::open(&self.sh.path).map_err(ioerr("open parquet"))?;
            let b = ParquetRecordBatchReaderBuilder::new_with_metadata(f, self.sh.meta.clone());
            let mask = ProjectionMask::columns(b.parquet_schema(), self.sh.mask_cols.iter().map(|s| s.as_str()));
            let reader = b.with_projection(mask).with_row_groups(vec![rg]).with_batch_size(8192).build().map_err(ioerr("parquet reader"))?;
            for rb in reader {
                let rb = rb.map_err(ioerr("parquet read"))?;
                // the projection keeps the file's column order; reorder to the requested order and apply the renames
                let cols = self.sh.mask_cols.iter().map(|c| rb.column_by_name(c).expect("projected column").clone()).collect();
                self.queue.push_back(RecordBatch::try_new(self.sh.schema.clone(), cols).map_err(ioerr("source batch"))?);
            }
            let _ = &self.sh.prefix;
        }
    }
}

pub fn write_parquet(path: &str, schema: &SchemaRef, batches: &[RecordBatch]) {
    let f = File::create(path).expect("create output");
    let mut w = ArrowWriter::try_new(f, schema.clone(), None).expect("writer");
    for b in batches {
        w.write(b).expect("write");
    }
    w.close().expect("close");
}
