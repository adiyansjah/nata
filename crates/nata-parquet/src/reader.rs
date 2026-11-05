//! The reader: a plan over a list of files (footers read, row groups pruned), and one independent TASK per kept row group.
//!
//! ```text
//! ScanPlan::open(files, options)   footers, schema checks, filter typing, pruning decisions: errors here name the file
//!   .tasks()                       the kept row groups, in file order and then row-group order (one task = one whole row group = one morsel, ADR 0001)
//!     RowGroupTask::run()          reads ONE row group with its own file handle, applies the exact row filter, projects, returns the batches
//! ```
//!
//! A task shares nothing mutable with the others (an `Arc` of the file's metadata and a fresh file handle), so a later step can hand the tasks of
//! `tasks()` to worker threads unchanged. This step runs them one after the other in [`run_serial`].
//!
//! # What a task returns
//!
//! Batches of the PROJECTED columns, in the requested order, with the filter already applied: a filtered batch is COMPACTED (arrow's
//! `filter_record_batch`: the selected rows are copied; dictionary columns keep their dictionary and only the keys are copied), a batch in which every row
//! matches is passed through as it is, and a batch in which no row matches is dropped. No selection vector is returned (the scan is the last stage in this
//! step; ADR 0003's selection vector comes with the pipeline operators). Columns that only the filter needs are read, used and dropped.

use std::fs::File;
use std::io::Read;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arrow_array::{BooleanArray, RecordBatch};
use arrow_schema::{Field, Schema, SchemaRef};
use arrow_select::filter::filter_record_batch;
use nata_scan::{Decision, Predicate, can_skip};
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{
    ArrowReaderMetadata, ArrowReaderOptions, ParquetRecordBatchReaderBuilder,
};
use parquet::errors::ParquetError;
use parquet::file::reader::{ChunkReader, Length};

use crate::adapter;
use crate::error::{ErrorKind, ScanError, redact};
use crate::filter::{self, Term};
use crate::schema::{Kind, Leaf, leaves};

/// Rows per batch handed out by the Arrow reader.
pub const BATCH_ROWS: usize = 8192;

/// What to scan.
#[derive(Clone, Debug, Default)]
pub struct ScanOptions {
    /// Output columns in output order; `None` = every column of the first file, in file order.
    pub columns: Option<Vec<String>>,
    /// Filter texts, `col op literal` (see [`crate::filter`]); a row must satisfy all of them.
    pub filters: Vec<String>,
    /// Use the footer statistics to skip row groups. `false` reads every row group (the filter still applies); used by the tests and by `--no-prune`.
    pub prune: bool,
}

/// One row group of one file, and what pruning decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowGroupInfo {
    pub rows: u64,
    /// `Some(reason)` if the statistics proved that no row matches (the row group is not read).
    pub skipped: Option<String>,
}

/// A file that has been opened: its footer is in memory, its pruning is decided.
#[derive(Debug)]
pub struct FileScan {
    path: PathBuf,
    meta: ArrowReaderMetadata,
    mask: ProjectionMask,
    /// The names of the columns that the filter needs and the output wants, in the order of this file's schema, with their expected Arrow types.
    read_columns: Vec<(String, arrow_schema::DataType)>,
    out_columns: Vec<String>,
    terms: Arc<Vec<Term>>,
    row_groups: Vec<RowGroupInfo>,
}

impl FileScan {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn row_groups(&self) -> &[RowGroupInfo] {
        &self.row_groups
    }
}

/// How pruning was set up, for the report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PruningInfo {
    /// Pruning was asked for AND there was at least one term it could use.
    pub applied: bool,
    /// Filter terms in all.
    pub terms: usize,
    /// Terms that were handed to the pruning decision (a literal that has no pruning form, such as an integer beyond 64 bits, is only row-filtered).
    pub pushed: usize,
}

/// A plan: the files, opened.
#[derive(Debug)]
pub struct ScanPlan {
    files: Vec<Arc<FileScan>>,
    out_schema: Vec<(String, Kind)>,
    pruning: PruningInfo,
    read_column_count: usize,
}

/// One row group to read: the unit of work (the morsel of ADR 0001).
#[derive(Clone, Debug)]
pub struct RowGroupTask {
    file: Arc<FileScan>,
    row_group: usize,
}

/// What a task produced.
#[derive(Debug)]
pub struct TaskOutput {
    pub batches: Vec<RecordBatch>,
    /// Rows decoded (before the filter).
    pub rows_read: u64,
    /// Rows that satisfied the filter.
    pub rows_returned: u64,
    /// Bytes requested from the file for the data of the row group (the column chunks read; the footer is not counted).
    pub bytes_read: u64,
}

// ------------------------------------------------------------------------------------------------------------------------------ opening

fn io_error(e: &std::io::Error, path: &Path) -> ScanError {
    // the OS error text names the reason (no such file, permission denied), never file contents
    ScanError::new(ErrorKind::Io(redact(&e.to_string()))).in_file(path)
}

fn parquet_error(e: &ParquetError, path: &Path, row_group: Option<usize>) -> ScanError {
    let kind = match e {
        ParquetError::NYI(what) => ErrorKind::NotSupported(redact(what)),
        ParquetError::IndexOutOfBound(..) => ErrorKind::Corrupt("an index is out of bounds".into()),
        other => ErrorKind::Corrupt(redact(&other.to_string())),
    };
    let mut err = ScanError::new(kind).in_file(path);
    if let Some(rg) = row_group {
        err = err.in_row_group(rg);
    }
    err
}

/// Run `f`, turning a panic into an error that does not carry the panic message (which could hold a value).
fn guarded<T>(
    path: &Path,
    row_group: Option<usize>,
    f: impl FnOnce() -> Result<T, ScanError>,
) -> Result<T, ScanError> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(_) => {
            let mut e =
                ScanError::new(ErrorKind::Internal("a panic while reading".into())).in_file(path);
            if let Some(rg) = row_group {
                e = e.in_row_group(rg);
            }
            Err(e)
        }
    }
}

/// What the first file fixes for all the others.
struct Spec {
    out: Vec<(String, Kind)>,
    /// (name, kind) of every column that is read (output and filter), unique.
    read: Vec<(String, Kind)>,
    terms: Vec<Term>,
    /// For each term, its column's kind.
    term_kinds: Vec<Kind>,
}

fn kind_of(leaf: &Leaf, path: &Path) -> Result<Kind, ScanError> {
    leaf.kind.clone().map_err(|why| {
        ScanError::not_supported(format!("column {:?}: {why}", leaf.name)).in_file(path)
    })
}

fn find<'a>(all: &'a [Leaf], name: &str, path: &Path) -> Result<&'a Leaf, ScanError> {
    all.iter().find(|l| l.name == name).ok_or_else(|| {
        let names: Vec<&str> = all.iter().map(|l| l.name.as_str()).collect();
        ScanError::usage(format!(
            "no column named {name:?} (the columns are: {})",
            names.join(", ")
        ))
        .in_file(path)
    })
}

fn first_spec(all: &[Leaf], options: &ScanOptions, path: &Path) -> Result<Spec, ScanError> {
    let names: Vec<String> = match &options.columns {
        Some(c) if c.is_empty() => {
            return Err(ScanError::usage("--columns needs at least one column name"));
        }
        Some(c) => c.clone(),
        None => all.iter().map(|l| l.name.clone()).collect(),
    };
    for (i, n) in names.iter().enumerate() {
        if names[..i].contains(n) {
            return Err(ScanError::usage(format!("column {n:?} is listed twice")));
        }
    }
    let mut out = Vec::new();
    for n in &names {
        let leaf = find(all, n, path)?;
        out.push((n.clone(), kind_of(leaf, path)?));
    }
    let mut read = out.clone();
    let mut terms = Vec::new();
    let mut term_kinds = Vec::new();
    for text in &options.filters {
        let filter_error =
            |msg: String| ScanError::new(ErrorKind::Filter(format!("filter {text:?}: {msg}")));
        let (column, raw) = filter::parse_syntax(text).map_err(filter_error)?;
        let leaf = find(all, &column, path)
            .map_err(|e| filter_error(format!("column {column:?}: {}", e_text(&e))))?;
        let kind = kind_of(leaf, path)?;
        let test = match raw {
            filter::RawTest::IsNull => filter::Test::IsNull,
            filter::RawTest::IsNotNull => filter::Test::IsNotNull,
            filter::RawTest::Cmp(op, lit) => filter::Test::Cmp(
                op,
                filter::type_literal(kind, &lit).map_err(|m| {
                    filter_error(format!("column {column:?} ({}): {m}", kind.sql_name()))
                })?,
            ),
        };
        if !read.iter().any(|(n, _)| *n == column) {
            read.push((column.clone(), kind));
        }
        terms.push(Term { column, test });
        term_kinds.push(kind);
    }
    Ok(Spec {
        out,
        read,
        terms,
        term_kinds,
    })
}

fn e_text(e: &ScanError) -> String {
    match &e.kind {
        ErrorKind::Usage(m) => m.clone(),
        _ => e.to_string(),
    }
}

impl ScanPlan {
    /// Open every file (footer only), check that they agree on the columns that are used, type the filters, decide the pruning.
    pub fn open(paths: &[PathBuf], options: &ScanOptions) -> Result<ScanPlan, ScanError> {
        if paths.is_empty() {
            return Err(ScanError::usage("no file given"));
        }
        let mut spec: Option<Spec> = None;
        let mut files = Vec::with_capacity(paths.len());
        let mut pushed = 0usize;
        for path in paths {
            let (file, p) = guarded(path, None, || open_file(path, options, &mut spec))?;
            pushed = p;
            files.push(Arc::new(file));
        }
        let spec = spec.ok_or_else(|| ScanError::usage("no file given"))?;
        let applied = options.prune && pushed > 0;
        Ok(ScanPlan {
            files,
            out_schema: spec.out,
            pruning: PruningInfo {
                applied,
                terms: options.filters.len(),
                pushed,
            },
            read_column_count: spec.read.len(),
        })
    }

    /// The output columns (name and kind), in output order.
    pub fn out_schema(&self) -> &[(String, Kind)] {
        &self.out_schema
    }

    pub fn files(&self) -> &[Arc<FileScan>] {
        &self.files
    }

    pub fn pruning(&self) -> PruningInfo {
        self.pruning
    }

    /// Columns read from each row group (output plus filter-only columns).
    pub fn columns_read(&self) -> usize {
        self.read_column_count
    }

    /// Row groups in all, and how many the statistics skipped.
    pub fn row_group_counts(&self) -> (u64, u64) {
        let mut total = 0u64;
        let mut skipped = 0u64;
        for f in &self.files {
            for rg in &f.row_groups {
                total += 1;
                if rg.skipped.is_some() {
                    skipped += 1;
                }
            }
        }
        (total, skipped)
    }

    /// The row groups that must be read, in the order of the file list and then of the row groups of each file.
    pub fn tasks(&self) -> impl Iterator<Item = RowGroupTask> + '_ {
        self.files.iter().flat_map(|f| {
            f.row_groups
                .iter()
                .enumerate()
                .filter(|(_, rg)| rg.skipped.is_none())
                .map(move |(i, _)| RowGroupTask {
                    file: Arc::clone(f),
                    row_group: i,
                })
        })
    }
}

/// The magic numbers at both ends of the file. `parquet-rs` reads the footer only, so a file whose first four bytes are wrong would still decode: it is
/// refused here. `PARE` is an encrypted file (not supported).
fn check_magic(file: &mut File, path: &Path) -> Result<(), ScanError> {
    use std::io::{Seek, SeekFrom};
    let len = file.metadata().map_err(|e| io_error(&e, path))?.len();
    let corrupt = |what: &str| ScanError::new(ErrorKind::Corrupt(what.to_string())).in_file(path);
    if len < 12 {
        return Err(corrupt("the file is too short to be a Parquet file"));
    }
    let mut head = [0u8; 4];
    let mut tail = [0u8; 4];
    file.seek(SeekFrom::Start(0))
        .and_then(|_| file.read_exact(&mut head))
        .map_err(|e| io_error(&e, path))?;
    file.seek(SeekFrom::End(-4))
        .and_then(|_| file.read_exact(&mut tail))
        .map_err(|e| io_error(&e, path))?;
    if &head == b"PARE" || &tail == b"PARE" {
        return Err(ScanError::not_supported("Parquet encryption").in_file(path));
    }
    if &head != b"PAR1" {
        return Err(corrupt(
            "the file does not start with the Parquet magic number PAR1",
        ));
    }
    if &tail != b"PAR1" {
        return Err(corrupt(
            "the file does not end with the Parquet magic number PAR1",
        ));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|e| io_error(&e, path))?;
    Ok(())
}

fn open_file(
    path: &Path,
    options: &ScanOptions,
    spec: &mut Option<Spec>,
) -> Result<(FileScan, usize), ScanError> {
    let mut file = File::open(path).map_err(|e| io_error(&e, path))?;
    check_magic(&mut file, path)?;
    let plain = ArrowReaderMetadata::load(&file, ArrowReaderOptions::new())
        .map_err(|e| parquet_error(&e, path, None))?;
    let metadata = Arc::clone(plain.metadata());
    let descr = metadata.file_metadata().schema_descr();
    let all = leaves(descr).map_err(|m| ScanError::not_supported(m).in_file(path))?;

    // The first file fixes the output columns, the filter terms and the kinds; later files must agree.
    if spec.is_none() {
        *spec = Some(first_spec(&all, options, path)?);
    }
    let Some(spec_ref) = spec.as_ref() else {
        return Err(ScanError::new(ErrorKind::Internal("no spec".into())).in_file(path));
    };
    for (name, kind) in &spec_ref.read {
        let leaf = find(&all, name, path)
            .map_err(|e| ScanError::new(ErrorKind::Schema(e_text(&e))).in_file(path))?;
        let have = kind_of(leaf, path)?;
        if have != *kind {
            return Err(ScanError::new(ErrorKind::Schema(format!(
                "column {name:?} is {} here and {} in the first file",
                have.sql_name(),
                kind.sql_name()
            )))
            .in_file(path));
        }
    }

    // the Arrow types we ask for: dictionary strings, Decimal64 (ADR 0003); every other column as the reader would give it
    let mut read_columns: Vec<(String, arrow_schema::DataType)> = Vec::new();
    let fields: Vec<Arc<Field>> = plain
        .schema()
        .fields()
        .iter()
        .map(
            |f| match spec_ref.read.iter().find(|(n, _)| n == f.name()) {
                Some((n, kind)) => {
                    let t = kind.arrow_type();
                    read_columns.push((n.clone(), t.clone()));
                    Arc::new(Field::new(f.name(), t, f.is_nullable()))
                }
                None => Arc::clone(f),
            },
        )
        .collect();
    let schema: SchemaRef = Arc::new(Schema::new(fields));
    let meta = ArrowReaderMetadata::try_new(
        Arc::clone(&metadata),
        ArrowReaderOptions::new().with_schema(schema),
    )
    .map_err(|e| parquet_error(&e, path, None))?;
    let indices: Vec<usize> = spec_ref
        .read
        .iter()
        .filter_map(|(n, _)| all.iter().find(|l| l.name == *n).map(|l| l.index))
        .collect();
    let mask = ProjectionMask::leaves(descr, indices);

    // pruning
    let predicate = Predicate::new(
        spec_ref
            .terms
            .iter()
            .zip(&spec_ref.term_kinds)
            .filter_map(|(t, k)| t.scan_term(*k))
            .collect(),
    );
    let pushed = predicate.terms.len();
    let filter_leaves: Vec<(&str, &Leaf)> = {
        let mut v: Vec<(&str, &Leaf)> = Vec::new();
        for t in &spec_ref.terms {
            if !v.iter().any(|(n, _)| *n == t.column)
                && let Some(l) = all.iter().find(|l| l.name == t.column)
            {
                v.push((t.column.as_str(), l));
            }
        }
        v
    };
    let mut row_groups = Vec::new();
    for (i, rg) in metadata.row_groups().iter().enumerate() {
        let rows = u64::try_from(rg.num_rows()).map_err(|_| {
            ScanError::new(ErrorKind::Corrupt(
                "the footer gives a negative row count".into(),
            ))
            .in_file(path)
            .in_row_group(i)
        })?;
        let skipped = if options.prune && pushed > 0 {
            match adapter::row_group_stats(rg, metadata.file_metadata(), &filter_leaves) {
                Some(stats) => match can_skip(&stats, &predicate) {
                    Decision::Skip(reason) => Some(format!("{reason:?}")),
                    Decision::Keep(_) => None,
                },
                None => None,
            }
        } else {
            None
        };
        row_groups.push(RowGroupInfo { rows, skipped });
    }

    Ok((
        FileScan {
            path: path.to_path_buf(),
            meta,
            mask,
            read_columns,
            out_columns: spec_ref.out.iter().map(|(n, _)| n.clone()).collect(),
            terms: Arc::new(spec_ref.terms.clone()),
            row_groups,
        },
        pushed,
    ))
}

// ------------------------------------------------------------------------------------------------------------------------------ reading

/// True if every requested column is in the batch with exactly the requested Arrow type (dictionary strings, `Decimal64`: ADR 0003).
fn column_types_are_as_requested(
    batch: &RecordBatch,
    want: &[(String, arrow_schema::DataType)],
) -> bool {
    want.iter().all(|(name, ty)| {
        batch
            .column_by_name(name)
            .is_some_and(|c| c.data_type() == ty)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::types::Int32Type;
    use arrow_array::{ArrayRef, DictionaryArray, Int32Array, StringArray};

    #[test]
    fn parquet_errors_name_the_file_and_the_row_group_and_keep_no_quoted_text() {
        let path = Path::new("/d/f.parquet");
        let e = parquet_error(
            &ParquetError::General("bad value 'SECRET'".into()),
            path,
            Some(3),
        );
        assert_eq!((e.file.as_deref(), e.row_group), (Some(path), Some(3)));
        assert!(
            matches!(e.kind, ErrorKind::Corrupt(_)) && !e.to_string().contains("SECRET"),
            "{e}"
        );
        let e = parquet_error(&ParquetError::General("x".into()), path, None);
        assert_eq!(e.row_group, None);
        assert!(
            parquet_error(&ParquetError::NYI("encryption".into()), path, Some(0))
                .is_not_supported()
        );
        let e = parquet_error(&ParquetError::IndexOutOfBound(7, 3), path, Some(1));
        assert!(
            !e.to_string().contains('7'),
            "no numbers of the data in the message: {e}"
        );
    }

    #[test]
    fn expanded_strings_are_not_the_requested_dictionary_type() {
        let dict_ty = Kind::Utf8.arrow_type();
        let plain: ArrayRef = Arc::new(StringArray::from(vec!["a", "b"]));
        let dict: ArrayRef = Arc::new(DictionaryArray::<Int32Type>::new(
            Int32Array::from(vec![0, 1]),
            Arc::new(StringArray::from(vec!["a", "b"])),
        ));
        let want = vec![("s".to_string(), dict_ty)];
        assert!(column_types_are_as_requested(
            &RecordBatch::try_from_iter([("s", dict)]).expect("batch"),
            &want
        ));
        assert!(
            !column_types_are_as_requested(
                &RecordBatch::try_from_iter([("s", plain)]).expect("batch"),
                &want
            ),
            "an expanded string column must be refused"
        );
        let other: ArrayRef = Arc::new(Int32Array::from(vec![1]));
        assert!(
            !column_types_are_as_requested(
                &RecordBatch::try_from_iter([("t", other)]).expect("batch"),
                &want
            ),
            "a missing column too"
        );
    }
}

/// A file handle that counts the bytes requested through it (the reader asks for column chunks; the footer was read before).
struct CountingFile {
    file: File,
    bytes: Arc<AtomicU64>,
}

struct CountingRead<R> {
    inner: R,
    bytes: Arc<AtomicU64>,
}

impl<R: Read> Read for CountingRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

impl Length for CountingFile {
    fn len(&self) -> u64 {
        self.file.len()
    }
}

impl ChunkReader for CountingFile {
    type T = CountingRead<<File as ChunkReader>::T>;

    fn get_read(&self, start: u64) -> parquet::errors::Result<Self::T> {
        Ok(CountingRead {
            inner: self.file.get_read(start)?,
            bytes: Arc::clone(&self.bytes),
        })
    }

    fn get_bytes(&self, start: u64, length: usize) -> parquet::errors::Result<bytes::Bytes> {
        let b = self.file.get_bytes(start, length)?;
        self.bytes.fetch_add(b.len() as u64, Ordering::Relaxed);
        Ok(b)
    }
}

impl RowGroupTask {
    pub fn path(&self) -> &Path {
        &self.file.path
    }

    pub fn row_group(&self) -> usize {
        self.row_group
    }

    /// Rows in the row group according to the footer.
    pub fn rows(&self) -> u64 {
        self.file
            .row_groups
            .get(self.row_group)
            .map_or(0, |r| r.rows)
    }

    /// Read this row group: decode, filter exactly, project. Any failure is an error that names the file and this row group; no rows are returned
    /// from a row group that failed.
    pub fn run(&self) -> Result<TaskOutput, ScanError> {
        let (path, rg) = (&self.file.path, self.row_group);
        guarded(path, Some(rg), || self.run_unguarded())
    }

    fn run_unguarded(&self) -> Result<TaskOutput, ScanError> {
        let f = &*self.file;
        let (path, rg) = (&f.path, self.row_group);
        let at = |kind: ErrorKind| ScanError::new(kind).in_file(path).in_row_group(rg);
        let handle = File::open(path).map_err(|e| io_error(&e, path).in_row_group(rg))?;
        let bytes = Arc::new(AtomicU64::new(0));
        let source = CountingFile {
            file: handle,
            bytes: Arc::clone(&bytes),
        };
        let reader = ParquetRecordBatchReaderBuilder::new_with_metadata(source, f.meta.clone())
            .with_projection(f.mask.clone())
            .with_row_groups(vec![rg])
            .with_batch_size(BATCH_ROWS)
            .build()
            .map_err(|e| parquet_error(&e, path, Some(rg)))?;

        let mut out = TaskOutput {
            batches: Vec::new(),
            rows_read: 0,
            rows_returned: 0,
            bytes_read: 0,
        };
        for batch in reader {
            let batch = batch.map_err(|e| {
                ScanError::new(ErrorKind::Corrupt(redact(&e.to_string())))
                    .in_file(path)
                    .in_row_group(rg)
            })?;
            out.rows_read += batch.num_rows() as u64;
            // the types the pipeline relies on (dictionary strings, Decimal64) must be what came out
            if !column_types_are_as_requested(&batch, &f.read_columns) {
                return Err(at(ErrorKind::Internal(
                    "the reader returned a column of another Arrow type than requested".into(),
                )));
            }
            let kept = if f.terms.is_empty() {
                Some(batch)
            } else {
                let mask = filter::evaluate(&f.terms, &batch)
                    .map_err(|m| at(ErrorKind::Internal(redact(&m))))?;
                let selected = mask.iter().filter(|b| **b).count();
                if selected == 0 {
                    None
                } else if selected == mask.len() {
                    Some(batch)
                } else {
                    Some(
                        filter_record_batch(&batch, &BooleanArray::from(mask))
                            .map_err(|e| at(ErrorKind::Internal(redact(&e.to_string()))))?,
                    )
                }
            };
            if let Some(b) = kept {
                let indices: Vec<usize> = f
                    .out_columns
                    .iter()
                    .map(|n| {
                        b.schema().index_of(n).map_err(|_| {
                            at(ErrorKind::Internal(
                                "an output column is missing from the batch".into(),
                            ))
                        })
                    })
                    .collect::<Result<_, _>>()?;
                let projected = b
                    .project(&indices)
                    .map_err(|e| at(ErrorKind::Internal(redact(&e.to_string()))))?;
                out.rows_returned += projected.num_rows() as u64;
                out.batches.push(projected);
            }
        }
        if out.rows_read != self.rows() {
            return Err(at(ErrorKind::Corrupt(format!(
                "the row group yielded {} rows but the footer says {}",
                out.rows_read,
                self.rows()
            ))));
        }
        out.bytes_read = bytes.load(Ordering::Relaxed);
        Ok(out)
    }
}

// ------------------------------------------------------------------------------------------------------------------------------ the serial driver

/// What a scan did. Printed by `nata scan` on stderr (see [`ScanReport::line`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanReport {
    pub files: usize,
    pub row_groups_total: u64,
    pub row_groups_skipped: u64,
    pub row_groups_read: u64,
    pub rows_read: u64,
    pub rows_returned: u64,
    pub bytes_read: u64,
    pub columns_read: usize,
    pub pruning: PruningInfo,
}

impl ScanReport {
    /// One line of `key=value` pairs. `not_read` = row groups that were neither skipped by the statistics nor read (the scan stopped early at `--limit`).
    pub fn line(&self) -> String {
        let pruning = if self.pruning.applied {
            format!(
                "applied({} of {} terms)",
                self.pruning.pushed, self.pruning.terms
            )
        } else if self.pruning.terms == 0 {
            "none(no filter)".to_string()
        } else if self.pruning.pushed == 0 {
            "none(no term can be used)".to_string()
        } else {
            "off(--no-prune)".to_string()
        };
        format!(
            "scan: files={} row_groups={} skipped={} read={} not_read={} rows_read={} rows_returned={} bytes_read={} columns_read={} morsel=row-group pruning={}",
            self.files,
            self.row_groups_total,
            self.row_groups_skipped,
            self.row_groups_read,
            self.row_groups_total
                .saturating_sub(self.row_groups_skipped)
                .saturating_sub(self.row_groups_read),
            self.rows_read,
            self.rows_returned,
            self.bytes_read,
            self.columns_read,
            pruning
        )
    }
}

/// Run every task of the plan, one after the other, in order, and hand the batches to `sink`. With `limit`, stop as soon as that many rows have been
/// handed over (the last batch is cut); `limit = Some(0)` reads nothing. (Cancelling in-flight work belongs to the parallel step.)
pub fn run_serial(
    plan: &ScanPlan,
    limit: Option<u64>,
    mut sink: impl FnMut(&RecordBatch) -> Result<(), ScanError>,
) -> Result<ScanReport, ScanError> {
    let (total, skipped) = plan.row_group_counts();
    let mut report = ScanReport {
        files: plan.files.len(),
        row_groups_total: total,
        row_groups_skipped: skipped,
        row_groups_read: 0,
        rows_read: 0,
        rows_returned: 0,
        bytes_read: 0,
        columns_read: plan.columns_read(),
        pruning: plan.pruning(),
    };
    let mut remaining = limit;
    'tasks: for task in plan.tasks() {
        if remaining == Some(0) {
            break;
        }
        let out = task.run()?;
        report.row_groups_read += 1;
        report.rows_read += out.rows_read;
        report.bytes_read += out.bytes_read;
        for batch in &out.batches {
            let take = match remaining {
                Some(r) => usize::try_from(r)
                    .unwrap_or(usize::MAX)
                    .min(batch.num_rows()),
                None => batch.num_rows(),
            };
            if take == 0 {
                break 'tasks;
            }
            if take < batch.num_rows() {
                sink(&batch.slice(0, take))?;
            } else {
                sink(batch)?;
            }
            report.rows_returned += take as u64;
            if let Some(r) = remaining.as_mut() {
                *r -= take as u64;
                if *r == 0 {
                    break 'tasks;
                }
            }
        }
    }
    Ok(report)
}
