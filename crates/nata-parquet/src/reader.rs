//! The reader: a plan over a list of files (footers read, row groups pruned), and one independent TASK per kept row group.
//!
//! ```text
//! ScanPlan::open(files, options)   footers, schema checks, filter typing, pruning decisions: errors here name the file
//!   .tasks()                       the kept row groups, in file order and then row-group order (one task = one whole row group = one morsel, ADR 0001)
//!     RowGroupTask::run()          reads ONE row group with its own file handle, applies the exact row filter, projects, returns the batches
//! ```
//!
//! A task shares nothing mutable with the others (an `Arc` of the file's metadata and a fresh file handle). [`run_serial`] runs them one after the other
//! (the reference implementation); `crate::parallel::run_parallel` hands them to worker threads (`nata-sched`).
//!
//! # Footers are read lazily
//!
//! `ScanPlan::open` reads the footer of the FIRST file only (it fixes the columns, the filter types and the pruning terms for all the others) and
//! closes the file. The footer of file `i` is read by `ScanPlan::open_file(i)` when the scheduler reaches it: open, read the footer, close; the
//! metadata lives in the `Arc<FileScan>` that its tasks hold and is freed when the last of them is dropped. A scan of 10,000 files therefore holds
//! at most the footers of the files whose row groups are in flight, and at most one handle per worker thread.
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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use arrow_array::{BooleanArray, RecordBatch};
use arrow_schema::{Field, Schema, SchemaRef};
use arrow_select::filter::filter_record_batch;
use nata_scan::{Decision, Predicate, can_skip};
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{
    ArrowReaderMetadata, ArrowReaderOptions, ParquetRecordBatchReaderBuilder, RowSelection,
};
use parquet::errors::ParquetError;
use parquet::file::reader::{ChunkReader, Length};

use crate::adapter;
use crate::error::{ErrorKind, ScanError, redact};
use crate::filter::{self, Term};
use crate::opener::{FsOpener, OpenFile, OpenFor, Opener};
use crate::schema::{Kind, Leaf, leaves};
use crate::stats::{Counted, Reservation, ScanStats};

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
    /// Late materialization (register item P1, `docs/slices/0002-select-where-limit.md` part 5): decode the columns the pushed filter needs first, evaluate
    /// the exact row filter on them, and decode the output columns only for the rows that pass (a Parquet row selection). Same rows, same order, same
    /// errors as the default (decode every column of the row group, then filter). Off by default: the step-4b benchmark failed the keep rule. Has no effect
    /// without a filter.
    pub late_materialization: bool,
    /// Require every file to have the first file's whole schema: the same top-level columns, in the same order, with the same types. A file that does not
    /// is an error naming the file, found when the scan reaches it (register item P10: the SQL binder reads only the first footer and relies on this). Off:
    /// a later file needs only the columns the scan reads (`nata scan`).
    pub strict_schema: bool,
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
    /// Set when the scan reads late (see [`ScanOptions::late_materialization`]) and there is a filter.
    late: Option<Late>,
    terms: Arc<Vec<Term>>,
    row_groups: Vec<RowGroupInfo>,
    ctx: Arc<Ctx>,
    /// Counts this footer as held in memory until the file is dropped (the last of its tasks is gone).
    _held: Counted,
    _memory: Option<nata_sched::memory::Grant>,
}

impl FileScan {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn row_groups(&self) -> &[RowGroupInfo] {
        &self.row_groups
    }

    /// The row groups that must be read, in row-group order.
    pub fn tasks(self: &Arc<Self>) -> Vec<RowGroupTask> {
        self.row_groups
            .iter()
            .enumerate()
            .filter(|(_, rg)| rg.skipped.is_none())
            .map(|(i, _)| RowGroupTask {
                file: Arc::clone(self),
                row_group: i,
            })
            .collect()
    }
}

/// What a task needs to read a row group in two steps: the columns of the filter first, then the output columns for the rows that passed.
#[derive(Debug)]
struct Late {
    /// The columns the pushed filter needs (and nothing else).
    filter_mask: ProjectionMask,
    filter_columns: Vec<(String, arrow_schema::DataType)>,
    /// The output columns (and nothing else): a column that is both filtered and selected is decoded in both steps.
    out_mask: ProjectionMask,
    out_types: Vec<(String, arrow_schema::DataType)>,
}

/// What every file and task of one plan shares: where handles come from and where the counts go.
pub struct Ctx {
    pub opener: Arc<dyn Opener>,
    /// What each task does with every batch it produced, inside the task (the filter and the projection of `nata sql`), if anything.
    pub map: Option<BatchMap>,
    pub sink: Option<TaskSinkFactory>,
    pub stats: Arc<ScanStats>,
    pub memory: Option<Arc<nata_sched::memory::MemoryManager>>,
}

impl std::fmt::Debug for Ctx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ctx")
            .field("opener", &self.opener)
            .field("map", &self.map.as_ref().map(|_| "fn"))
            .field("stats", &self.stats)
            .finish()
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

/// A plan: the file list, the columns and filters fixed by the first file, and the first file's footer (kept until the scan reaches it).
#[derive(Debug)]
pub struct ScanPlan {
    paths: Vec<PathBuf>,
    options: ScanOptions,
    spec: Spec,
    first: Mutex<Option<Arc<FileScan>>>,
    out_schema: Vec<(String, Kind)>,
    pruning: PruningInfo,
    read_column_count: usize,
    ctx: Arc<Ctx>,
}

/// A function applied by each task to each batch it produced; see [`ScanPlan::open_with_map`].
pub type BatchMap =
    Arc<dyn Fn(RecordBatch) -> Result<Option<RecordBatch>, ErrorKind> + Send + Sync>;

/// Task-local breaker state. Dropping an unfinished task drops all its state.
pub trait TaskSink: Send {
    fn push(&mut self, batch: RecordBatch) -> Result<(), ErrorKind>;
    fn finish(self: Box<Self>) -> Result<(), ErrorKind>;
}
pub type TaskSinkFactory = Arc<dyn Fn() -> Box<dyn TaskSink> + Send + Sync>;

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
    /// The bytes of `batches` on the scan's memory gauge; released when this value is dropped (so a result waiting for the consumer counts).
    pub mem: Reservation,
}

/// How a task that did not fail ended.
#[derive(Debug)]
pub enum TaskResult {
    Done(TaskOutput),
    /// Stopped between two batches (or before starting) because the scan was cancelled; nothing is returned, the handle and the buffers are gone.
    Cancelled,
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
#[derive(Debug, Clone)]
struct Spec {
    out: Vec<(String, Kind)>,
    /// (name, kind) of every column that is read (output and filter), unique.
    read: Vec<(String, Kind)>,
    terms: Vec<Term>,
    /// For each term, its column's kind.
    term_kinds: Vec<Kind>,
    /// Every top-level column of the first file with its kind (or why it is unsupported), kept only for `strict_schema`.
    first_schema: Option<Vec<(String, Result<Kind, String>)>>,
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
        first_schema: options.strict_schema.then(|| {
            all.iter()
                .map(|l| (l.name.clone(), l.kind.clone()))
                .collect()
        }),
    })
}

fn e_text(e: &ScanError) -> String {
    match &e.kind {
        ErrorKind::Usage(m) => m.clone(),
        _ => e.to_string(),
    }
}

impl ScanPlan {
    /// Read the footer of the first file (it fixes the output columns, the filter types and the pruning terms) and close it; the other files are read
    /// by [`ScanPlan::open_file`] when the scan reaches them. Errors of the first file, and every usage or filter error, are returned here.
    pub fn open(paths: &[PathBuf], options: &ScanOptions) -> Result<ScanPlan, ScanError> {
        ScanPlan::open_with(paths, options, Arc::new(FsOpener))
    }

    /// As [`ScanPlan::open`], with the handles made by `opener` (tests count them and make them fail).
    pub fn open_with(
        paths: &[PathBuf],
        options: &ScanOptions,
        opener: Arc<dyn Opener>,
    ) -> Result<ScanPlan, ScanError> {
        ScanPlan::open_with_map(paths, options, opener, None)
    }

    /// As [`ScanPlan::open_with`], and every batch a task produces (after the pushed row filter and the projection of the scan) goes through `map` inside
    /// the task, on the worker thread: the rows of the scan are the rows `map` returns. `Ok(None)` drops the batch. An error is an error of the task
    /// (it names the file and the row group, and the lowest one wins as for any task). The decoded batch and the mapped one both count on the memory gauge.
    pub fn open_with_map(
        paths: &[PathBuf],
        options: &ScanOptions,
        opener: Arc<dyn Opener>,
        map: Option<BatchMap>,
    ) -> Result<ScanPlan, ScanError> {
        Self::open_with_operators(paths, options, opener, map, None)
    }

    /// As `open_with_map`, with a per-morsel sink finalized only after successful input.
    pub fn open_with_operators(
        paths: &[PathBuf],
        options: &ScanOptions,
        opener: Arc<dyn Opener>,
        map: Option<BatchMap>,
        sink: Option<TaskSinkFactory>,
    ) -> Result<ScanPlan, ScanError> {
        Self::open_budgeted(paths, options, opener, map, sink, None)
    }
    /// Blocking operators supply the query's shared memory service. Footer
    /// metadata is charged before Arrow/Thrift parsing and retained with it.
    pub fn open_budgeted(
        paths: &[PathBuf],
        options: &ScanOptions,
        opener: Arc<dyn Opener>,
        map: Option<BatchMap>,
        sink: Option<TaskSinkFactory>,
        memory: Option<Arc<nata_sched::memory::MemoryManager>>,
    ) -> Result<ScanPlan, ScanError> {
        let Some(first_path) = paths.first() else {
            return Err(ScanError::usage("no file given"));
        };
        let ctx = Arc::new(Ctx {
            opener,
            map,
            sink,
            stats: Arc::new(ScanStats::default()),
            memory,
        });
        let mut spec: Option<Spec> = None;
        let (first, pushed) = guarded(first_path, None, || {
            read_file(first_path, 0, options, &mut spec, &ctx)
        })?;
        let spec = spec.ok_or_else(|| ScanError::usage("no file given"))?;
        let applied = options.prune && pushed > 0;
        Ok(ScanPlan {
            paths: paths.to_vec(),
            options: options.clone(),
            out_schema: spec.out.clone(),
            read_column_count: spec.read.len(),
            pruning: PruningInfo {
                applied,
                terms: options.filters.len(),
                pushed,
            },
            spec,
            first: Mutex::new(Some(Arc::new(first))),
            ctx,
        })
    }

    /// Drop the first file's footer if the scan never reached it (a scan that ends before it, `limit` 0, an error).
    pub fn release_first_footer(&self) {
        let _ = self
            .first
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }

    /// Number of files in the plan.
    pub fn file_count(&self) -> usize {
        self.paths.len()
    }

    /// The counters of this scan (shared with every task).
    pub fn stats(&self) -> &Arc<ScanStats> {
        &self.ctx.stats
    }

    /// The output columns (name and kind), in output order.
    pub fn out_schema(&self) -> &[(String, Kind)] {
        &self.out_schema
    }

    pub fn pruning(&self) -> PruningInfo {
        self.pruning
    }

    /// Columns read from each row group (output plus filter-only columns).
    pub fn columns_read(&self) -> usize {
        self.read_column_count
    }

    /// Open file `index`: its footer is read and checked against the first file, the handle is closed, and the pruning is decided. File 0 was read by
    /// `open` and is handed out once from there (a second call reads it again). The returned file frees its metadata when the last `Arc` of it,
    /// held by the tasks, is dropped.
    pub fn open_file(&self, index: usize) -> Result<Arc<FileScan>, ScanError> {
        let Some(path) = self.paths.get(index) else {
            return Err(ScanError::new(ErrorKind::Internal(
                "a file number beyond the plan was asked for".into(),
            )));
        };
        let cached = if index == 0 {
            self.first
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        } else {
            None
        };
        let file = match cached {
            Some(f) => f,
            None => {
                // the plan's spec is read-only after `open`; `read_file` takes it by value (a few column names and literals)
                let mut spec = Some(self.spec.clone());
                guarded(path, None, || {
                    read_file(path, index, &self.options, &mut spec, &self.ctx)
                })
                .map(|(f, _)| Arc::new(f))?
            }
        };
        let st = &self.ctx.stats;
        ScanStats::add(&st.files_opened, 1);
        let skipped = file
            .row_groups
            .iter()
            .filter(|r| r.skipped.is_some())
            .count() as u64;
        ScanStats::add(&st.row_groups_total, file.row_groups.len() as u64);
        ScanStats::add(&st.row_groups_skipped, skipped);
        Ok(file)
    }

    /// The tasks of every file, in order. OPENS EVERY FILE NOW and holds all footers: for tests and small plans. The drivers do not use it.
    pub fn tasks(&self) -> Result<Vec<RowGroupTask>, ScanError> {
        let mut all = Vec::new();
        for i in 0..self.paths.len() {
            all.extend(self.open_file(i)?.tasks());
        }
        Ok(all)
    }
}

/// The top-level columns of the Parquet file at `path`, in file order, from its FOOTER alone (no data page is read): the schema a front end binds names
/// against. A column of a kind the scan does not support is listed with its reason (`Leaf::kind` is an `Err`); it is an error only if a query touches it.
/// Errors name the file, as everywhere in the scan.
pub fn read_leaves(path: &Path) -> Result<Vec<Leaf>, ScanError> {
    guarded(path, None, || {
        let OpenFile { mut file, .. } = FsOpener
            .open(path, OpenFor::Footer(0))
            .map_err(|e| io_error(&e, path))?;
        check_magic(&mut file, path)?;
        let meta = ArrowReaderMetadata::load(&file, ArrowReaderOptions::new())
            .map_err(|e| parquet_error(&e, path, None))?;
        leaves(meta.metadata().file_metadata().schema_descr())
            .map_err(|m| ScanError::not_supported(m).in_file(path))
    })
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

/// `strict_schema`: the file must have the first file's top-level columns in the same order with the same kinds. The error names the first column that
/// differs (never a value).
fn strict_schema_check(
    first: &[(String, Result<Kind, String>)],
    all: &[Leaf],
    path: &Path,
) -> Result<(), ScanError> {
    let describe = |name: &str, kind: &Result<Kind, String>| match kind {
        Ok(k) => format!("{name} {}", k.sql_name()),
        Err(_) => format!("{name} (unsupported type)"),
    };
    let differs = |i: usize| match (all.get(i), first.get(i)) {
        (Some(a), Some((bn, bk))) => a.name != *bn || a.kind != *bk,
        _ => true,
    };
    if all.len() == first.len() && !(0..all.len()).any(differs) {
        return Ok(());
    }
    let why = (0..all.len().max(first.len()))
        .find(|i| differs(*i))
        .map(|i| match (all.get(i), first.get(i)) {
            (Some(a), Some((bn, bk))) => format!(
                "column {} here, {} in the first file",
                describe(&a.name, &a.kind),
                describe(bn, bk)
            ),
            _ => format!(
                "{} columns here, {} in the first file",
                all.len(),
                first.len()
            ),
        })
        .unwrap_or_default();
    Err(ScanError::new(ErrorKind::Schema(format!(
        "the files of a pattern must have the same schema ({why})"
    )))
    .in_file(path))
}

/// Open one file, read its footer, CLOSE it, and decide its pruning.
fn read_file(
    path: &Path,
    index: usize,
    options: &ScanOptions,
    spec: &mut Option<Spec>,
    ctx: &Arc<Ctx>,
) -> Result<(FileScan, usize), ScanError> {
    let mut metadata_grant = None;
    let plain = {
        let OpenFile {
            mut file, _guard, ..
        } = ctx
            .opener
            .open(path, OpenFor::Footer(index))
            .map_err(|e| io_error(&e, path))?;
        let _count = Counted::new(&ctx.stats.handles);
        check_magic(&mut file, path)?;
        if let Some(memory) = &ctx.memory {
            use std::io::{Seek, SeekFrom};
            let mut tail = [0u8; 8];
            file.seek(SeekFrom::End(-8))
                .and_then(|_| file.read_exact(&mut tail))
                .map_err(|e| io_error(&e, path))?;
            let encoded = u64::from(u32::from_le_bytes(
                tail[..4]
                    .try_into()
                    .map_err(|_| ScanError::usage("footer length"))?,
            ));
            let file_size = file.metadata().map_err(|e| io_error(&e, path))?.len();
            if encoded > file_size.saturating_sub(12) {
                return Err(ScanError::new(ErrorKind::Corrupt(
                    "footer length exceeds file".into(),
                ))
                .in_file(path));
            }
            // Capacity bound includes parser structures, Arrow schema/metadata,
            // pruning vectors and the row-group continuation descriptors.
            let bytes = usize::try_from(encoded)
                .ok()
                .and_then(|n| n.checked_mul(128))
                .and_then(|n| n.checked_add(4096))
                .ok_or_else(|| ScanError::usage("footer memory bound overflow"))?;
            metadata_grant =
                Some(
                    match nata_sched::memory::MemoryManager::request(memory, 0, bytes) {
                        Ok(nata_sched::memory::Reservation::Granted(g)) => g,
                        _ => return Err(ScanError::new(ErrorKind::Eval {
                            message:
                                "resource error: aggregate footer metadata exceeds memory budget"
                                    .into(),
                            line: 0,
                            column: 0,
                        })
                        .in_file(path)),
                    },
                );
        }
        ArrowReaderMetadata::load(&file, ArrowReaderOptions::new())
            .map_err(|e| parquet_error(&e, path, None))?
        // the handle is closed here
    };
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
    if let Some(first) = &spec_ref.first_schema {
        strict_schema_check(first, &all, path)?;
    }
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
    let late = (options.late_materialization && !spec_ref.terms.is_empty()).then(|| {
        let leaf_index = |name: &str| all.iter().find(|l| l.name == name).map(|l| l.index);
        let mut filter_names: Vec<&str> = Vec::new();
        for t in &spec_ref.terms {
            if !filter_names.contains(&t.column.as_str()) {
                filter_names.push(t.column.as_str());
            }
        }
        let typed = |names: &dyn Fn(&str) -> bool| -> Vec<(String, arrow_schema::DataType)> {
            read_columns
                .iter()
                .filter(|(n, _)| names(n))
                .cloned()
                .collect()
        };
        Late {
            filter_mask: ProjectionMask::leaves(
                descr,
                filter_names.iter().filter_map(|n| leaf_index(n)),
            ),
            filter_columns: typed(&|n| filter_names.contains(&n)),
            out_mask: ProjectionMask::leaves(
                descr,
                spec_ref.out.iter().filter_map(|(n, _)| leaf_index(n)),
            ),
            out_types: typed(&|n| spec_ref.out.iter().any(|(o, _)| o == n)),
        }
    });

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
            late,
            terms: Arc::new(spec_ref.terms.clone()),
            row_groups,
            ctx: Arc::clone(ctx),
            _held: Counted::new(&ctx.stats.footers),
            _memory: metadata_grant,
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

/// A file handle that counts the bytes requested through it (the reader asks for column chunks; the footer was read before), and can be made to fail.
struct CountingFile {
    file: File,
    bytes: Arc<AtomicU64>,
    fail_after: Option<u64>,
    delay: Option<std::time::Duration>,
}

struct CountingRead<R> {
    inner: R,
    bytes: Arc<AtomicU64>,
    fail_after: Option<u64>,
    delay: Option<std::time::Duration>,
    /// The reader has returned end of file once. parquet-rs 60.1.0's streaming Thrift reader (`ThriftReadInputProtocol::skip_bytes` and
    /// `read_bytes_owned`) does not check that `io::copy` copied the length a damaged page header claims, and its list skip loops over a count
    /// read from the file: a corrupt header then reads past the end billions of times (a hang no cancel can reach, found by the nightly
    /// damaged-file fuzz, seed 20261107, shape tiny_and_empty_groups, case 10072). A correct decoder never reads again after `Ok(0)`, so a
    /// second read at end of file is an `UnexpectedEof` error, which the scan reports as a corrupt file naming the file and row group.
    at_eof: bool,
}

fn injected_error() -> std::io::Error {
    std::io::Error::other("injected I/O error")
}

impl<R: Read> Read for CountingRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(limit) = self.fail_after
            && self.bytes.load(Ordering::Relaxed) >= limit
        {
            return Err(injected_error());
        }
        if let Some(d) = self.delay {
            std::thread::sleep(d);
        }
        if self.at_eof && !buf.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "read past the end of the file (a damaged page header claims more bytes than the file holds)",
            ));
        }
        let n = self.inner.read(buf)?;
        if n == 0 && !buf.is_empty() {
            self.at_eof = true;
        }
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
            fail_after: self.fail_after,
            delay: self.delay,
            at_eof: false,
        })
    }

    fn get_bytes(&self, start: u64, length: usize) -> parquet::errors::Result<bytes::Bytes> {
        if let Some(limit) = self.fail_after
            && self.bytes.load(Ordering::Relaxed) >= limit
        {
            return Err(ParquetError::External(Box::new(injected_error())));
        }
        if let Some(d) = self.delay {
            std::thread::sleep(d);
        }
        let b = self.file.get_bytes(start, length)?;
        self.bytes.fetch_add(b.len() as u64, Ordering::Relaxed);
        Ok(b)
    }
}

/// One counting file handle shared by the two readers of a late-materializing task (and used by the one reader of the default task).
#[derive(Clone)]
struct SharedFile(Arc<CountingFile>);

impl Length for SharedFile {
    fn len(&self) -> u64 {
        self.0.len()
    }
}

impl ChunkReader for SharedFile {
    type T = <CountingFile as ChunkReader>::T;

    fn get_read(&self, start: u64) -> parquet::errors::Result<Self::T> {
        self.0.get_read(start)
    }

    fn get_bytes(&self, start: u64, length: usize) -> parquet::errors::Result<bytes::Bytes> {
        self.0.get_bytes(start, length)
    }
}

/// Adds what a task did to the scan's counters when it ends, however it ends (done, cancelled, failed, panicked).
struct Tally<'a> {
    stats: &'a ScanStats,
    rows: u64,
    bytes: Arc<AtomicU64>,
}

impl Drop for Tally<'_> {
    fn drop(&mut self) {
        ScanStats::add(&self.stats.rows_read, self.rows);
        ScanStats::add(&self.stats.bytes_read, self.bytes.load(Ordering::Relaxed));
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
        match self.run_cancellable(&|| false)? {
            TaskResult::Done(o) => Ok(o),
            TaskResult::Cancelled => Err(ScanError::new(ErrorKind::Internal(
                "a task that cannot be cancelled was cancelled".into(),
            ))),
        }
    }

    /// As [`RowGroupTask::run`], looking at `cancel` before the handle is opened and between two batches. When it answers true the task drops its reader,
    /// its handle and the batches it holds, and returns `Cancelled`.
    pub fn run_cancellable(&self, cancel: &dyn Fn() -> bool) -> Result<TaskResult, ScanError> {
        let (path, rg) = (&self.file.path, self.row_group);
        guarded(path, Some(rg), || self.run_unguarded(cancel))
    }

    fn run_unguarded(&self, cancel: &dyn Fn() -> bool) -> Result<TaskResult, ScanError> {
        let f = &*self.file;
        let stats = &*f.ctx.stats;
        let (path, rg) = (&f.path, self.row_group);
        let abandoned = || {
            ScanStats::add(&stats.row_groups_abandoned, 1);
            Ok(TaskResult::Cancelled)
        };
        if cancel() {
            return abandoned();
        }
        let OpenFile {
            file: handle,
            fail_reads_after,
            read_delay,
            _guard,
        } = f
            .ctx
            .opener
            .open(path, OpenFor::RowGroup(rg))
            .map_err(|e| io_error(&e, path).in_row_group(rg))?;
        let _open = Counted::new(&stats.handles);
        let bytes = Arc::new(AtomicU64::new(0));
        let mut tally = Tally {
            stats,
            rows: 0,
            bytes: Arc::clone(&bytes),
        };
        let source = SharedFile(Arc::new(CountingFile {
            file: handle,
            bytes: Arc::clone(&bytes),
            fail_after: fail_reads_after,
            delay: read_delay,
        }));
        let mut run = TaskRun {
            task: self,
            sink: f.ctx.sink.as_ref().map(|factory| factory()),
            cancel,
            out: TaskOutput {
                batches: Vec::new(),
                rows_read: 0,
                rows_returned: 0,
                bytes_read: 0,
                mem: Reservation::new(&stats.memory),
            },
            task_peak: 0,
        };
        let step = match &f.late {
            Some(late) => run.read_late(late, &source, &mut tally)?,
            None => run.read_early(&source, &mut tally)?,
        };
        if step == Step::Cancelled {
            return abandoned();
        }
        if let Some(sink) = run.sink.take() {
            sink.finish().map_err(|kind| run.at(kind))?;
        }
        let TaskRun {
            mut out, task_peak, ..
        } = run;
        out.bytes_read = bytes.load(Ordering::Relaxed);
        ScanStats::add(&stats.row_groups_read, 1);
        stats.largest_task.fetch_max(task_peak, Ordering::SeqCst);
        Ok(TaskResult::Done(out))
    }
}

/// A row-group continuation for blocking sinks. The caller reserves
/// `cursor_bytes` before construction and keeps that grant until this is dropped.
pub struct RowGroupCursor {
    reader: parquet::arrow::arrow_reader::ParquetRecordBatchReader,
    task: RowGroupTask,
    rows: u64,
    bytes: Arc<AtomicU64>,
    _open: Counted,
    _guard: Option<Box<dyn Send + Sync>>,
    done: bool,
}
impl RowGroupTask {
    /// Conservative compressed/decode/evaluation allowance, checked before the
    /// reader is built. Includes whole column chunks retained by parquet-rs,
    /// decode overlap, and fixed-width selection/expression storage.
    pub fn cursor_bytes(&self, batch_rows: usize) -> Option<usize> {
        let rg = self.file.meta.metadata().row_group(self.row_group);
        let mut compressed = 0usize;
        let mut decoded = 0usize;
        for (i, col) in rg.columns().iter().enumerate() {
            if self.file.mask.leaf_included(i) {
                compressed =
                    compressed.checked_add(usize::try_from(col.compressed_size()).ok()?)?;
                decoded = decoded.checked_add(usize::try_from(col.uncompressed_size()).ok()?)?;
            }
        }
        compressed
            .checked_add(decoded.checked_mul(8)?)?
            .checked_add(
                usize::try_from(self.rows())
                    .ok()?
                    .min(batch_rows)
                    .checked_mul(
                        self.file
                            .read_columns
                            .len()
                            .checked_add(1)?
                            .checked_mul(128)?,
                    )?,
            )?
            .checked_add(64 << 10)
    }
    pub fn cursor(&self, batch_rows: usize) -> Result<RowGroupCursor, ScanError> {
        guarded(&self.file.path, Some(self.row_group), || {
            let f = &self.file;
            if f.ctx.map.is_some() || f.ctx.sink.is_some() {
                return Err(ScanError::new(ErrorKind::Internal(
                    "cursor cannot own a batch map or sink".into(),
                )));
            }
            let OpenFile {
                file,
                fail_reads_after,
                read_delay,
                _guard,
            } = f
                .ctx
                .opener
                .open(&f.path, OpenFor::RowGroup(self.row_group))
                .map_err(|e| io_error(&e, &f.path).in_row_group(self.row_group))?;
            let bytes = Arc::new(AtomicU64::new(0));
            let source = SharedFile(Arc::new(CountingFile {
                file,
                bytes: Arc::clone(&bytes),
                fail_after: fail_reads_after,
                delay: read_delay,
            }));
            let reader = ParquetRecordBatchReaderBuilder::new_with_metadata(source, f.meta.clone())
                .with_projection(f.mask.clone())
                .with_row_groups(vec![self.row_group])
                .with_batch_size(batch_rows.max(1))
                .build()
                .map_err(|e| parquet_error(&e, &f.path, Some(self.row_group)))?;
            Ok(RowGroupCursor {
                reader,
                task: self.clone(),
                rows: 0,
                bytes,
                _open: Counted::new(&f.ctx.stats.handles),
                _guard,
                done: false,
            })
        })
    }
}
impl RowGroupCursor {
    /// The caller checks cancellation between turns. Every decoding error and
    /// panic keeps its file/row-group position and excludes cell values.
    pub fn next_batch(&mut self) -> Result<Option<RecordBatch>, ScanError> {
        let task = self.task.clone();
        guarded(&task.file.path, Some(task.row_group), || {
            let f = &task.file;
            let Some(batch) = self.reader.next() else {
                if self.rows != task.rows() {
                    return Err(ScanError::new(ErrorKind::Corrupt(
                        "cursor row count differs from footer".into(),
                    ))
                    .in_file(&f.path)
                    .in_row_group(task.row_group));
                }
                if !self.done {
                    ScanStats::add(&f.ctx.stats.row_groups_read, 1);
                }
                self.done = true;
                return Ok(None);
            };
            let batch = batch.map_err(|e| {
                ScanError::new(ErrorKind::Corrupt(redact(&e.to_string())))
                    .in_file(&f.path)
                    .in_row_group(task.row_group)
            })?;
            self.rows += batch.num_rows() as u64;
            if !column_types_are_as_requested(&batch, &f.read_columns) {
                return Err(ScanError::new(ErrorKind::Internal(
                    "cursor returned an unexpected Arrow type".into(),
                )));
            }
            let batch = if f.terms.is_empty() {
                batch
            } else {
                let mask = filter::evaluate(&f.terms, &batch)
                    .map_err(|m| ScanError::new(ErrorKind::Internal(redact(&m))))?;
                filter_record_batch(&batch, &BooleanArray::from(mask)).map_err(|e| {
                    ScanError::new(ErrorKind::Corrupt(redact(&e.to_string())))
                        .in_file(&f.path)
                        .in_row_group(task.row_group)
                })?
            };
            let indices = f
                .out_columns
                .iter()
                .map(|n| batch.schema().index_of(n))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| {
                    ScanError::new(ErrorKind::Internal("cursor output column missing".into()))
                })?;
            batch
                .project(&indices)
                .map(Some)
                .map_err(|_| ScanError::new(ErrorKind::Internal("cursor projection failed".into())))
        })
    }
}
impl Drop for RowGroupCursor {
    fn drop(&mut self) {
        let stats = &self.task.file.ctx.stats;
        ScanStats::add(&stats.rows_read, self.rows);
        ScanStats::add(&stats.bytes_read, self.bytes.load(Ordering::Relaxed));
        if !self.done {
            ScanStats::add(&stats.row_groups_abandoned, 1);
        }
    }
}

/// How the reading half of a task ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Done,
    Cancelled,
}

/// A task while it runs: what it has produced so far, and the memory high-water mark.
struct TaskRun<'a> {
    sink: Option<Box<dyn TaskSink>>,
    task: &'a RowGroupTask,
    cancel: &'a dyn Fn() -> bool,
    out: TaskOutput,
    task_peak: u64,
}

impl TaskRun<'_> {
    fn at(&self, kind: ErrorKind) -> ScanError {
        ScanError::new(kind)
            .in_file(&self.task.file.path)
            .in_row_group(self.task.row_group)
    }

    /// An error of a batch the reader failed to decode (a damaged page).
    fn corrupt(&self, e: &impl std::fmt::Display) -> ScanError {
        self.at(ErrorKind::Corrupt(redact(&e.to_string())))
    }

    fn builder(
        &self,
        source: &SharedFile,
        mask: &ProjectionMask,
    ) -> ParquetRecordBatchReaderBuilder<SharedFile> {
        ParquetRecordBatchReaderBuilder::new_with_metadata(
            source.clone(),
            self.task.file.meta.clone(),
        )
        .with_projection(mask.clone())
        .with_row_groups(vec![self.task.row_group])
        .with_batch_size(BATCH_ROWS)
    }

    /// The default read: every column of the row group (output and filter) is decoded, then the exact filter is applied to each batch.
    fn read_early(
        &mut self,
        source: &SharedFile,
        tally: &mut Tally<'_>,
    ) -> Result<Step, ScanError> {
        let f = &*self.task.file;
        let stats = &*f.ctx.stats;
        let reader = self
            .builder(source, &f.mask)
            .build()
            .map_err(|e| parquet_error(&e, &f.path, Some(self.task.row_group)))?;
        for batch in reader {
            if (self.cancel)() {
                return Ok(Step::Cancelled);
            }
            let batch = batch.map_err(|e| self.corrupt(&e))?;
            self.out.rows_read += batch.num_rows() as u64;
            tally.rows += batch.num_rows() as u64;
            // the decoded batch is held while it is filtered: counted until the end of this turn
            let mut transient = Reservation::new(&stats.memory);
            transient.grow(batch.get_array_memory_size() as u64);
            self.task_peak = self.task_peak.max(self.out.mem.bytes() + transient.bytes());
            // the types the pipeline relies on (dictionary strings, Decimal64) must be what came out
            if !column_types_are_as_requested(&batch, &f.read_columns) {
                return Err(self.at(ErrorKind::Internal(
                    "the reader returned a column of another Arrow type than requested".into(),
                )));
            }
            let kept = if f.terms.is_empty() {
                Some(batch)
            } else {
                let mask = filter::evaluate(&f.terms, &batch)
                    .map_err(|m| self.at(ErrorKind::Internal(redact(&m))))?;
                let selected = mask.iter().filter(|b| **b).count();
                if selected == 0 {
                    None
                } else if selected == mask.len() {
                    Some(batch)
                } else {
                    Some(
                        filter_record_batch(&batch, &BooleanArray::from(mask))
                            .map_err(|e| self.at(ErrorKind::Internal(redact(&e.to_string()))))?,
                    )
                }
            };
            if let Some(b) = kept {
                self.emit(&b, transient.bytes())?;
            }
        }
        self.check_row_count()?;
        Ok(Step::Done)
    }

    /// Late materialization (P1): step 1 decodes ONLY the filter columns of the whole row group and evaluates the exact filter on them, giving a row
    /// selection; step 2 decodes ONLY the output columns, and only for the selected rows. Nothing is decoded in step 2 when no row passed, and no
    /// selection is used when every row passed. The batches of step 2 are already filtered. Both steps share the task's one file handle.
    fn read_late(
        &mut self,
        late: &Late,
        source: &SharedFile,
        tally: &mut Tally<'_>,
    ) -> Result<Step, ScanError> {
        let f = &*self.task.file;
        let stats = &*f.ctx.stats;
        let rg = self.task.row_group;
        // step 1: the filter columns
        let reader = self
            .builder(source, &late.filter_mask)
            .build()
            .map_err(|e| parquet_error(&e, &f.path, Some(rg)))?;
        let mut masks: Vec<BooleanArray> = Vec::new();
        let (mut seen, mut selected) = (0u64, 0u64);
        for batch in reader {
            if (self.cancel)() {
                return Ok(Step::Cancelled);
            }
            let batch = batch.map_err(|e| self.corrupt(&e))?;
            let n = batch.num_rows() as u64;
            seen += n;
            self.out.rows_read += n;
            tally.rows += n;
            let mut transient = Reservation::new(&stats.memory);
            transient.grow(batch.get_array_memory_size() as u64);
            self.task_peak = self.task_peak.max(self.out.mem.bytes() + transient.bytes());
            if !column_types_are_as_requested(&batch, &late.filter_columns) {
                return Err(self.at(ErrorKind::Internal(
                    "the reader returned a column of another Arrow type than requested".into(),
                )));
            }
            let mask = filter::evaluate(&f.terms, &batch)
                .map_err(|m| self.at(ErrorKind::Internal(redact(&m))))?;
            selected += mask.iter().filter(|b| **b).count() as u64;
            masks.push(BooleanArray::from(mask));
        }
        self.check_row_count()?;
        if selected == 0 {
            return Ok(Step::Done);
        }
        let selection = (selected < seen).then(|| RowSelection::from_filters(&masks));
        drop(masks);
        // step 2: the output columns, for the selected rows
        let mut builder = self.builder(source, &late.out_mask);
        if let Some(s) = selection {
            builder = builder.with_row_selection(s);
        }
        let reader = builder
            .build()
            .map_err(|e| parquet_error(&e, &f.path, Some(rg)))?;
        let mut got = 0u64;
        for batch in reader {
            if (self.cancel)() {
                return Ok(Step::Cancelled);
            }
            let batch = batch.map_err(|e| self.corrupt(&e))?;
            got += batch.num_rows() as u64;
            let mut transient = Reservation::new(&stats.memory);
            transient.grow(batch.get_array_memory_size() as u64);
            self.task_peak = self.task_peak.max(self.out.mem.bytes() + transient.bytes());
            if !column_types_are_as_requested(&batch, &late.out_types) {
                return Err(self.at(ErrorKind::Internal(
                    "the reader returned a column of another Arrow type than requested".into(),
                )));
            }
            self.emit(&batch, transient.bytes())?;
        }
        if got != selected {
            // the selection of step 1 and the rows of step 2 must be the same rows: never a wrong answer
            return Err(self.at(ErrorKind::Internal(
                "the rows decoded for the selection are not the rows the filter selected".into(),
            )));
        }
        Ok(Step::Done)
    }

    /// The rows decoded must be the rows the footer counts.
    fn check_row_count(&self) -> Result<(), ScanError> {
        if self.out.rows_read != self.task.rows() {
            return Err(self.at(ErrorKind::Corrupt(format!(
                "the row group yielded {} rows but the footer says {}",
                self.out.rows_read,
                self.task.rows()
            ))));
        }
        Ok(())
    }

    /// A batch that passed the filter: project it to the output columns, in the requested order, apply the per-batch map, and keep it. `transient` is the
    /// size of the decoded batch that is still counted on the gauge.
    fn emit(&mut self, b: &RecordBatch, transient: u64) -> Result<(), ScanError> {
        let f = &*self.task.file;
        let indices: Vec<usize> = f
            .out_columns
            .iter()
            .map(|n| {
                b.schema().index_of(n).map_err(|_| {
                    self.at(ErrorKind::Internal(
                        "an output column is missing from the batch".into(),
                    ))
                })
            })
            .collect::<Result<_, _>>()?;
        let projected = b
            .project(&indices)
            .map_err(|e| self.at(ErrorKind::Internal(redact(&e.to_string()))))?;
        let mapped = match &f.ctx.map {
            Some(map) => map(projected).map_err(|k| self.at(k))?,
            None => Some(projected),
        };
        if let Some(projected) = mapped {
            if let Some(sink) = &mut self.sink {
                sink.push(projected).map_err(|kind| self.at(kind))?;
                return Ok(());
            }
            self.out.mem.grow(projected.get_array_memory_size() as u64);
            self.task_peak = self.task_peak.max(self.out.mem.bytes() + transient);
            self.out.rows_returned += projected.num_rows() as u64;
            self.out.batches.push(projected);
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------------------------------------------ the report

/// What a scan did. Printed by `nata scan` on stderr (see [`ScanReport::line`]). Row-group counts are those of the files that were OPENED: a scan
/// that stops early (`--limit`, an error) has not read the footers of the files it never reached (`files_opened < files`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanReport {
    pub files: usize,
    pub files_opened: usize,
    pub row_groups_total: u64,
    pub row_groups_skipped: u64,
    /// Row groups decoded to the end.
    pub row_groups_read: u64,
    /// Row groups that were started or claimed and stopped by the cancellation.
    pub row_groups_abandoned: u64,
    /// Rows decoded, also by tasks that were cancelled halfway.
    pub rows_read: u64,
    /// Rows handed to the sink.
    pub rows_returned: u64,
    /// Bytes requested from the files by the row-group tasks (counted at the handle; cancelled tasks included; the footers are not counted).
    pub bytes_read: u64,
    pub columns_read: usize,
    pub pruning: PruningInfo,
    pub threads: usize,
    pub window: usize,
    pub ordered: bool,
    /// The highest number of decoded bytes held at once by running tasks and by results waiting for or held by the consumer (own accounting).
    pub peak_inflight_bytes: u64,
    /// The highest number of bytes any single task held at once.
    pub largest_task_bytes: u64,
    /// The highest number of file handles open at once.
    pub peak_open_files: u64,
    /// The highest number of footers held in memory at once.
    pub peak_footers: u64,
    /// Late materialization was asked for (it has an effect only with a filter).
    pub late_materialization: bool,
}

impl ScanReport {
    pub fn from_stats(
        plan: &ScanPlan,
        rows_returned: u64,
        threads: usize,
        window: usize,
        ordered: bool,
    ) -> ScanReport {
        let st = &plan.ctx.stats;
        ScanReport {
            files: plan.file_count(),
            files_opened: ScanStats::get(&st.files_opened) as usize,
            row_groups_total: ScanStats::get(&st.row_groups_total),
            row_groups_skipped: ScanStats::get(&st.row_groups_skipped),
            row_groups_read: ScanStats::get(&st.row_groups_read),
            row_groups_abandoned: ScanStats::get(&st.row_groups_abandoned),
            rows_read: ScanStats::get(&st.rows_read),
            rows_returned,
            bytes_read: ScanStats::get(&st.bytes_read),
            columns_read: plan.columns_read(),
            pruning: plan.pruning(),
            threads,
            window,
            ordered,
            peak_inflight_bytes: st.memory.peak(),
            largest_task_bytes: ScanStats::get(&st.largest_task),
            peak_open_files: st.handles.peak(),
            peak_footers: st.footers.peak(),
            late_materialization: plan.options.late_materialization,
        }
    }

    /// One line of `key=value` pairs. `not_read` = row groups that were neither skipped by the statistics nor decoded to the end (the scan stopped
    /// early at `--limit`, or they were abandoned).
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
            "scan: files={} row_groups={} skipped={} read={} not_read={} rows_read={} rows_returned={} bytes_read={} columns_read={} morsel=row-group pruning={} files_opened={} abandoned={} threads={} order={} window={} peak_inflight_bytes={} largest_task_bytes={} peak_open_files={} peak_footers={} late={}",
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
            pruning,
            self.files_opened,
            self.row_groups_abandoned,
            self.threads,
            if self.ordered { "ordered" } else { "unordered" },
            self.window,
            self.peak_inflight_bytes,
            self.largest_task_bytes,
            self.peak_open_files,
            self.peak_footers,
            u8::from(self.late_materialization)
        )
    }
}

// ------------------------------------------------------------------------------------------------------------------------------ the serial driver

/// Cut batches at a limit and count the rows that reach the sink; shared by the serial and the parallel driver.
pub(crate) struct Limiter {
    pub remaining: Option<u64>,
    pub returned: u64,
}

impl Limiter {
    /// Hand `batch` (cut if needed) to `sink`. `Ok(true)` = the limit is reached, nothing more is wanted.
    pub(crate) fn feed(
        &mut self,
        batch: &RecordBatch,
        sink: &mut impl FnMut(&RecordBatch) -> Result<(), ScanError>,
    ) -> Result<bool, ScanError> {
        let take = match self.remaining {
            Some(r) => usize::try_from(r)
                .unwrap_or(usize::MAX)
                .min(batch.num_rows()),
            None => batch.num_rows(),
        };
        if take == 0 {
            return Ok(true);
        }
        if take < batch.num_rows() {
            sink(&batch.slice(0, take))?;
        } else {
            sink(batch)?;
        }
        self.returned += take as u64;
        if let Some(r) = self.remaining.as_mut() {
            *r -= take as u64;
            return Ok(*r == 0);
        }
        Ok(false)
    }
}

/// Run every task of the plan, one after the other, in order, on this thread, and hand the batches to `sink`. The footer of each file is read when the
/// loop reaches it. With `limit`, stop as soon as that many rows have been handed over (the last batch is cut); `limit = Some(0)` reads nothing.
/// This is the reference implementation: `run_parallel` must produce the same rows in the same order.
pub fn run_serial(
    plan: &ScanPlan,
    limit: Option<u64>,
    mut sink: impl FnMut(&RecordBatch) -> Result<(), ScanError>,
) -> Result<ScanReport, ScanError> {
    let mut lim = Limiter {
        remaining: limit,
        returned: 0,
    };
    'files: for index in 0..plan.file_count() {
        if lim.remaining == Some(0) {
            break;
        }
        let file = plan.open_file(index)?;
        for task in file.tasks() {
            if lim.remaining == Some(0) {
                break 'files;
            }
            let out = task.run()?;
            for batch in &out.batches {
                if lim.feed(batch, &mut sink)? {
                    break 'files;
                }
            }
        }
    }
    plan.release_first_footer();
    Ok(ScanReport::from_stats(plan, lim.returned, 1, 1, true))
}
