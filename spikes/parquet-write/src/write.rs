//! The writer wrapper: the options a table writer needs (row group and page sizes, statistics level and truncation, bloom filters, sorting,
//! rolling files at a target size), producing one or more files in a directory.
use std::fs::File;
use std::path::{Path, PathBuf};

use arrow::array::RecordBatch;
use arrow::compute::{SortColumn, SortOptions, concat_batches, lexsort_to_indices, take_record_batch};
use arrow::datatypes::SchemaRef;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::metadata::SortingColumn;
use parquet::file::properties::{EnabledStatistics, WriterProperties};

#[derive(Clone, Debug)]
pub struct SortKey {
    pub col: usize,
    pub desc: bool,
    pub nulls_first: bool,
}

#[derive(Clone, Debug)]
pub struct Opts {
    pub row_group_rows: usize,
    pub page_rows: usize,
    pub stats: EnabledStatistics,
    pub trunc: Option<usize>,
    pub index_trunc: Option<usize>,
    pub bloom: bool,
    pub dict: bool,
    pub sort: Vec<SortKey>,
    pub target_file_bytes: Option<usize>,
    pub compression: Compression,
    /// turn statistics off for float columns with an all-NaN row group (see `all_nan_columns`); false = the raw parquet-rs behaviour
    pub nan_mitigation: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { row_group_rows: 5000, page_rows: 700, stats: EnabledStatistics::Page, trunc: Some(16), index_trunc: Some(16), bloom: false, dict: true, sort: vec![],
               target_file_bytes: None, compression: Compression::SNAPPY, nan_mitigation: true }
    }
}

/// Float columns that have a row group made only of NaN (and NULL) values: parquet-rs 60.1 writes NaN as that chunk's min and max, which the format
/// forbids (a reader that orders NaN last would prune rows it must return). The writer cuts the row groups itself, so it can find them first and
/// turn statistics off for those columns: no statistics is sound, a wrong one is not.
pub fn all_nan_columns(schema: &SchemaRef, input: &[RecordBatch], rows_per_group: usize) -> Vec<String> {
    use arrow::array::{Array, AsArray};
    use arrow::datatypes::{DataType, Float32Type, Float64Type};
    let mut out = vec![];
    for (ci, f) in schema.fields().iter().enumerate() {
        if !matches!(f.data_type(), DataType::Float32 | DataType::Float64) { continue; }
        'col: for b in input {
            let mut at = 0;
            while at < b.num_rows() {
                let n = rows_per_group.min(b.num_rows() - at);
                let a = b.column(ci).slice(at, n);
                let (mut nan, mut num) = (0, 0);
                for i in 0..n {
                    if a.is_null(i) { continue; }
                    let is_nan = match f.data_type() { DataType::Float32 => a.as_primitive::<Float32Type>().value(i).is_nan(), _ => a.as_primitive::<Float64Type>().value(i).is_nan() };
                    if is_nan { nan += 1 } else { num += 1 }
                }
                if nan > 0 && num == 0 { out.push(f.name().clone()); break 'col; }
                at += n;
            }
        }
    }
    out
}

pub fn props(o: &Opts) -> WriterProperties {
    props_for(o, &[])
}

pub fn props_for(o: &Opts, no_stats: &[String]) -> WriterProperties {
    let mut b = WriterProperties::builder()
        .set_max_row_group_row_count(Some(o.row_group_rows))
        .set_data_page_row_count_limit(o.page_rows)
        .set_write_batch_size(o.page_rows.min(1024))
        .set_statistics_enabled(o.stats)
        .set_statistics_truncate_length(o.trunc)
        .set_column_index_truncate_length(o.index_trunc)
        .set_bloom_filter_enabled(o.bloom)
        .set_dictionary_enabled(o.dict)
        .set_compression(o.compression);
    for c in no_stats { b = b.set_column_statistics_enabled(parquet::schema::types::ColumnPath::from(c.as_str()), EnabledStatistics::None); }
    if !o.sort.is_empty() {
        b = b.set_sorting_columns(Some(o.sort.iter().map(|k| SortingColumn { column_idx: k.col as i32, descending: k.desc, nulls_first: k.nulls_first }).collect()));
    }
    b.build()
}

pub fn sort_table(schema: &SchemaRef, batches: &[RecordBatch], keys: &[SortKey]) -> RecordBatch {
    let all = concat_batches(schema, batches).expect("concat");
    let cols: Vec<SortColumn> = keys.iter().map(|k| SortColumn { values: all.column(k.col).clone(), options: Some(SortOptions { descending: k.desc, nulls_first: k.nulls_first }) }).collect();
    let idx = lexsort_to_indices(&cols, None).expect("sort");
    take_record_batch(&all, &idx).expect("take")
}

/// Writes `batches` (sorted first when `o.sort` is set) to `dir/<prefix>-<n>.parquet`, starting a new file whenever the current one reaches
/// `target_file_bytes` (checked after every row group). Returns the files in order.
pub fn write_files(dir: &Path, prefix: &str, schema: &SchemaRef, batches: &[RecordBatch], o: &Opts) -> Vec<PathBuf> {
    std::fs::create_dir_all(dir).expect("dir");
    let sorted;
    let input: &[RecordBatch] = if o.sort.is_empty() { batches } else { sorted = [sort_table(schema, batches, &o.sort)]; &sorted };
    let p = props_for(o, &if o.nan_mitigation { all_nan_columns(schema, input, o.row_group_rows) } else { vec![] });
    let mut files = vec![];
    let mut cur: Option<ArrowWriter<File>> = None;
    for b in input {
        let mut at = 0;
        while at < b.num_rows() {
            let n = o.row_group_rows.min(b.num_rows() - at);
            if cur.is_none() {
                let path = dir.join(format!("{prefix}-{:04}.parquet", files.len()));
                cur = Some(ArrowWriter::try_new(File::create(&path).expect("file"), schema.clone(), Some(p.clone())).expect("writer"));
                files.push(path);
            }
            let w = cur.as_mut().unwrap();
            w.write(&b.slice(at, n)).expect("write");
            w.flush().expect("flush row group");
            at += n;
            if let Some(t) = o.target_file_bytes {
                if w.bytes_written() >= t {
                    cur.take().unwrap().close().expect("close");
                }
            }
        }
    }
    if let Some(w) = cur { w.close().expect("close"); }
    files
}
