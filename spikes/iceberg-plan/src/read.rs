//! Reading the rows of a data file the way a reader must: columns matched by FIELD ID (so renamed, added and dropped columns work), position deletes
//! and equality deletes applied.
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::sync::Arc;

use arrow::array::*;
use arrow::compute::{cast, concat_batches};
use arrow::datatypes::SchemaRef;
use parquet::arrow::PARQUET_FIELD_ID_META_KEY;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

use crate::meta::{Field, Schema};
use crate::plan::FileScanTask;
use crate::table::{Table, arrow_schema, arrow_type, cell};
use crate::{Res, local_path};

/// All rows of the Parquet file at `path`, as a batch of `fields` (matched by field id; absent columns are all NULL; types cast when they differ).
pub fn read_fields(path: &str, fields: &[Field]) -> Res<RecordBatch> {
    let f = File::open(local_path(path)).map_err(|e| format!("{path}: {e}"))?;
    let b = ParquetRecordBatchReaderBuilder::try_new(f).map_err(|e| format!("{path}: {e}"))?;
    let fschema: SchemaRef = b.schema().clone();
    let batches: Vec<RecordBatch> = b.build().map_err(|e| e.to_string())?.collect::<Result<_, _>>().map_err(|e| format!("{path}: {e}"))?;
    let all = if batches.is_empty() { RecordBatch::new_empty(fschema.clone()) } else { concat_batches(&fschema, &batches).map_err(|e| e.to_string())? };
    let n = all.num_rows();
    let by_id: HashMap<i32, usize> = fschema.fields().iter().enumerate().filter_map(|(i, f)| f.metadata().get(PARQUET_FIELD_ID_META_KEY).and_then(|v| v.parse().ok()).map(|id| (id, i))).collect();
    let mut cols = vec![];
    for fld in fields {
        let want = arrow_type(&fld.ty);
        let src = by_id.get(&fld.id).copied().or_else(|| if by_id.is_empty() { fschema.index_of(&fld.name).ok() } else { None });
        cols.push(match src { Some(i) => { let c = all.column(i).clone(); if c.data_type() == &want { c } else { cast(&c, &want).map_err(|e| format!("{path}: column {}: {e}", fld.name))? } } None => new_null_array(&want, n) });
    }
    let schema = Arc::new(arrow::datatypes::Schema::new(fields.iter().map(|f| arrow::datatypes::Field::new(&f.name, arrow_type(&f.ty), true).with_metadata(crate::table::fid_meta(f.id))).collect::<Vec<_>>()));
    RecordBatch::try_new_with_options(schema, cols, &RecordBatchOptions::new().with_row_count(Some(n))).map_err(|e| e.to_string())
}

/// Every column that has ever existed in the table, by id (latest definition wins): equality deletes may name a column that was dropped since.
pub fn all_fields(table: &Table) -> HashMap<i32, Field> {
    let mut m = HashMap::new();
    for s in &table.meta.schemas { for f in &s.fields { m.insert(f.id, f.clone()); } }
    m
}

fn row_key(batch: &RecordBatch, i: usize, fields: &[Field]) -> String {
    fields.iter().enumerate().map(|(c, f)| format!("{:?}", cell(batch.column(c), i, &f.ty))).collect::<Vec<_>>().join("|")
}

/// The rows of a scan task: the data file's rows minus the rows its position deletes and equality deletes remove, in the table's CURRENT schema.
pub fn read_task(table: &Table, task: &FileScanTask) -> Res<RecordBatch> {
    let schema = table.meta.current_schema();
    let af = all_fields(table);
    let data = read_fields(&task.file.path, &schema.fields)?;
    let n = data.num_rows();
    let mut keep = vec![true; n];
    // position deletes
    let pf = [Field { id: 2147483546, name: "file_path".into(), required: true, ty: crate::types::Type::String }, Field { id: 2147483545, name: "pos".into(), required: true, ty: crate::types::Type::Long }];
    for d in &task.pos_deletes {
        let b = read_fields(&d.path, &pf)?;
        let (fp, pos) = (b.column(0).as_string::<i32>(), b.column(1).as_primitive::<arrow::datatypes::Int64Type>());
        for i in 0..b.num_rows() { if local_path(fp.value(i)) == local_path(&task.file.path) && (pos.value(i) as usize) < n { keep[pos.value(i) as usize] = false; } }
    }
    // equality deletes: read the delete columns of the data file by id (even if no longer in the current schema)
    if !task.eq_deletes.is_empty() {
        let mut ids: Vec<i32> = task.eq_deletes.iter().flat_map(|d| d.equality_ids.clone()).collect();
        ids.sort(); ids.dedup();
        let dfields: Vec<Field> = ids.iter().filter_map(|i| af.get(i).cloned()).collect();
        let full = read_fields(&task.file.path, &dfields)?;
        for d in &task.eq_deletes {
            let f2: Vec<Field> = d.equality_ids.iter().filter_map(|i| af.get(i).cloned()).collect();
            let del = read_fields(&d.path, &f2)?;
            let set: HashSet<String> = (0..del.num_rows()).map(|i| row_key(&del, i, &f2)).collect();
            let idx: Vec<usize> = f2.iter().map(|f| dfields.iter().position(|x| x.id == f.id).unwrap()).collect();
            let sub = RecordBatch::try_new(Arc::new(arrow::datatypes::Schema::new(idx.iter().map(|i| full.schema().field(*i).clone()).collect::<Vec<_>>())), idx.iter().map(|i| full.column(*i).clone()).collect()).map_err(|e| e.to_string())?;
            for i in 0..n { if keep[i] && set.contains(&row_key(&sub, i, &f2)) { keep[i] = false; } }
        }
    }
    let mask = BooleanArray::from(keep);
    arrow::compute::filter_record_batch(&data, &mask).map_err(|e| e.to_string())
}

pub fn read_scan(table: &Table, tasks: &[FileScanTask]) -> Res<Vec<RecordBatch>> { tasks.iter().map(|t| read_task(table, t)).collect() }

pub fn current_arrow_schema(table: &Table) -> SchemaRef { arrow_schema(table.meta.current_schema()) }
pub fn _s(_: &Schema) {}
