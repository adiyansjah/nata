//! Running a plan: the slice 3 scan (with the pushed conjuncts and the columns the operators read), then, inside each scan task on its worker thread,
//! `Filter` and `Project` on every batch, then, on the consuming thread in file order, `Offset` and `Limit`.
//!
//! * **Streaming.** A batch is evaluated by the task that decoded it and handed to the sink in the order of the files, row groups and rows; nothing
//!   waits for the whole result. Memory is the scan's bound (the window of row groups in flight) plus the evaluator's temporaries for one batch.
//! * **Project after the limit.** With a `LIMIT`, the workers only filter; the projection runs on the consuming thread on the first `offset + n` rows
//!   that the filter keeps (at most `offset + n` rows in all). So an error of a projected expression (a division by zero, an overflow) is raised only for those rows,
//!   as in DuckDB (it pushes the limit below the projection; the rows `OFFSET` drops are still projected): `SELECT 1 // x FROM t LIMIT 1` does not fail on the tenth row. Filter errors
//!   are never skipped: the filter decides which rows count. Without a `LIMIT` the projection runs in the workers, in parallel.
//! * **Limit.** The scan counts the rows that reach the consumer (after the filter) and, at `offset + n`, stops claiming row groups, stops running tasks
//!   at their next batch, releases their handles and buffers, and joins every worker before this function returns.
//! * **Cancellation and errors.** As the scan's: an external cancel (Ctrl-C), a sink error (a closed stdout) and a task error (the lowest failing
//!   row group wins, also for errors of the expressions) end the query with every thread joined and every handle released.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use arrow_array::{ArrayRef, BooleanArray, RecordBatch};
use arrow_schema::{Field, Schema};
use arrow_select::filter::filter_record_batch;
use nata_parquet::{
    BatchMap, ErrorKind, Opener, ParallelConfig, ScanCancel, ScanError, ScanPlan, ScanReport,
    run_parallel_with,
};
use nata_sql::plan::LogicalPlan;
use nata_sql::texpr::{Expr, ExprKind};
use nata_sql::types::SqlType;

use crate::error::{EvalError, R};
use crate::eval::Evaluator;

/// How to run a plan.
#[derive(Clone, Debug)]
pub struct ExecConfig {
    /// Worker threads of the scan (and so of the expression evaluation).
    pub threads: usize,
    /// Row groups in flight (the scan's window).
    pub window: usize,
    /// Deliver in the order of the files, row groups and rows (the default of `nata sql`; unordered is for measurements).
    pub ordered: bool,
    /// Where file handles come from; `None` opens real files. Tests substitute the fault opener of slice 3.
    pub opener: Option<Arc<dyn Opener>>,
}

impl ExecConfig {
    pub fn new(threads: usize) -> ExecConfig {
        let threads = threads.max(1);
        ExecConfig {
            threads,
            window: threads.saturating_mul(2),
            ordered: true,
            opener: None,
        }
    }
}

/// What the operators counted while running (summed over the worker threads; times are CPU time of the operator, not wall time).
#[derive(Debug, Default)]
struct Counters {
    batches: AtomicU64,
    scan_rows: AtomicU64,
    filter_rows: AtomicU64,
    filter_nanos: AtomicU64,
    project_nanos: AtomicU64,
    project_rows: AtomicU64,
}

/// A snapshot of the per-operator counts of one run, for `EXPLAIN ANALYZE`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperatorStats {
    /// Batches that came out of the scan (after its pushed filter) into the operators.
    pub batches: u64,
    /// Rows that the scan handed to the operators.
    pub scan_rows: u64,
    /// Rows the filter kept (equal to `scan_rows` when there is no filter).
    pub filter_rows: u64,
    pub filter_time: Duration,
    pub project_time: Duration,
    /// Rows the projection computed (with a limit: only the rows the limit lets through).
    pub project_rows: u64,
    /// Rows that reached the limit operator (the filter's rows, up to the point where the limit stopped the scan; a result computed ahead by a
    /// worker and thrown away at the limit is not counted).
    pub limit_in: u64,
    /// Rows delivered to the sink: what the query returns.
    pub rows_out: u64,
}

/// What a run did.
#[derive(Clone, Debug)]
pub struct ExecReport {
    pub scan: ScanReport,
    pub ops: OperatorStats,
    /// Wall time from the open of the scan to the join of its last worker.
    pub elapsed: Duration,
}

fn nanos(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

/// The deepest nesting of the plan's expressions, counted with a loop.
pub fn max_depth(plan: &LogicalPlan) -> usize {
    fn depth(root: &Expr) -> usize {
        let mut best = 0;
        let mut stack = vec![(root, 1usize)];
        while let Some((e, d)) = stack.pop() {
            best = best.max(d);
            for c in e.children() {
                stack.push((c, d + 1));
            }
        }
        best
    }
    let mut best = 0;
    if let Some(f) = &plan.filter {
        best = best.max(depth(&f.predicate));
    }
    for p in &plan.project.exprs {
        best = best.max(depth(&p.expr));
    }
    best
}

/// Expressions nested deeper than this are evaluated on a thread with a stack sized for them (a worker's default stack is 2 MB).
const DEEP: usize = 64;

struct Run {
    plan: Arc<LogicalPlan>,
    types: Vec<SqlType>,
    counters: Counters,
    /// The stack size for the evaluation of a deep expression, `None` for an ordinary one.
    big_stack: Option<usize>,
    /// The projection runs after `OFFSET` and `LIMIT`, on the consuming thread.
    project_late: bool,
}

impl Run {
    /// Filter then project one batch; `Ok(None)` when no row survives the filter.
    fn batch(&self, batch: RecordBatch) -> R<Option<RecordBatch>> {
        let c = &self.counters;
        let rows = batch.num_rows();
        c.batches.fetch_add(1, Ordering::Relaxed);
        c.scan_rows
            .fetch_add(u64::try_from(rows).unwrap_or(u64::MAX), Ordering::Relaxed);
        let mut batch = batch;
        if let Some(f) = &self.plan.filter {
            let t = Instant::now();
            let keep = Evaluator::new(&batch, &self.types).keep_mask(&f.predicate)?;
            let kept = keep.iter().filter(|k| **k).count();
            c.filter_rows
                .fetch_add(u64::try_from(kept).unwrap_or(u64::MAX), Ordering::Relaxed);
            if kept == 0 {
                c.filter_nanos
                    .fetch_add(nanos(t.elapsed()), Ordering::Relaxed);
                return Ok(None);
            }
            if kept < rows {
                batch = filter_record_batch(&batch, &BooleanArray::from(keep))
                    .map_err(|e| EvalError::internal(format!("filter: {e}")))?;
            }
            c.filter_nanos
                .fetch_add(nanos(t.elapsed()), Ordering::Relaxed);
        } else {
            c.filter_rows
                .fetch_add(u64::try_from(rows).unwrap_or(u64::MAX), Ordering::Relaxed);
        }
        if self.project_late {
            return Ok(Some(batch));
        }
        Ok(Some(self.timed_project(&batch)?))
    }

    fn timed_project(&self, batch: &RecordBatch) -> R<RecordBatch> {
        let t = Instant::now();
        let out = self.project(batch)?;
        self.counters
            .project_nanos
            .fetch_add(nanos(t.elapsed()), Ordering::Relaxed);
        self.counters.project_rows.fetch_add(
            u64::try_from(out.num_rows()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        Ok(out)
    }

    fn project(&self, batch: &RecordBatch) -> R<RecordBatch> {
        let ev = Evaluator::new(batch, &self.types);
        let all = ev.all_rows();
        let mut arrays: Vec<ArrayRef> = Vec::with_capacity(self.plan.project.exprs.len());
        for p in &self.plan.project.exprs {
            match &p.expr.kind {
                // a bare column passes through as the scan made it (a dictionary of strings stays a dictionary)
                ExprKind::Column(i) => {
                    arrays.push(Arc::clone(batch.columns().get(*i).ok_or_else(|| {
                        EvalError::internal("a projected column is missing from the batch")
                    })?))
                }
                _ => arrays.push(ev.eval(&p.expr, &all)?.to_arrow()?),
            }
        }
        let fields: Vec<Field> = self
            .plan
            .project
            .exprs
            .iter()
            .zip(&arrays)
            .map(|(p, a)| Field::new(p.name.clone(), a.data_type().clone(), true))
            .collect();
        RecordBatch::try_new(Arc::new(Schema::new(fields)), arrays)
            .map_err(|e| EvalError::internal(format!("project: {e}")))
    }

    fn project_on_big_stack(&self, batch: &RecordBatch) -> R<RecordBatch> {
        match self.big_stack {
            None => self.timed_project(batch),
            Some(stack) => std::thread::scope(|s| {
                match std::thread::Builder::new()
                    .name("nata-eval".into())
                    .stack_size(stack)
                    .spawn_scoped(s, || self.timed_project(batch))
                {
                    Ok(h) => h
                        .join()
                        .unwrap_or_else(|_| Err(EvalError::internal("a panic while evaluating"))),
                    Err(_) => Err(EvalError::internal(
                        "cannot start a thread with a stack for this expression",
                    )),
                }
            }),
        }
    }

    fn map(self: &Arc<Self>, batch: RecordBatch) -> Result<Option<RecordBatch>, ErrorKind> {
        let r = match self.big_stack {
            None => self.batch(batch),
            Some(stack) => {
                let me = Arc::clone(self);
                std::thread::scope(|s| {
                    match std::thread::Builder::new()
                        .name("nata-eval".into())
                        .stack_size(stack)
                        .spawn_scoped(s, move || me.batch(batch))
                    {
                        Ok(h) => h.join().unwrap_or_else(|_| {
                            Err(EvalError::internal("a panic while evaluating"))
                        }),
                        Err(_) => Err(EvalError::internal(
                            "cannot start a thread with a stack for this expression",
                        )),
                    }
                })
            }
        };
        r.map_err(EvalError::into_kind)
    }
}

/// What a run exposes besides its result, also when it failed: the scan's counters (handles, footers and bytes alive), which the leak checks of the
/// reliability tests read after the run.
#[derive(Debug, Default)]
pub struct Observed {
    pub stats: Option<Arc<nata_parquet::ScanStats>>,
}

/// Run the plan, handing every result batch (after `Filter`, `Project`, `Offset` and `Limit`) to `sink` in order.
pub fn execute(
    plan: Arc<LogicalPlan>,
    cfg: &ExecConfig,
    cancel: Option<&ScanCancel>,
    sink: impl FnMut(&RecordBatch) -> Result<(), ScanError>,
) -> Result<ExecReport, ScanError> {
    execute_observed(plan, cfg, cancel, &mut Observed::default(), sink)
}

/// As [`execute`], and `observed` receives the scan's counters as soon as the scan is open.
pub fn execute_observed(
    plan: Arc<LogicalPlan>,
    cfg: &ExecConfig,
    cancel: Option<&ScanCancel>,
    observed: &mut Observed,
    mut sink: impl FnMut(&RecordBatch) -> Result<(), ScanError>,
) -> Result<ExecReport, ScanError> {
    let started = Instant::now();
    let mut options = plan.scan.scan_options();
    if plan.scan.columns.is_empty() {
        // `SELECT 1 FROM 'f'`: no column is needed, but the scan delivers rows with at least one: the first one the scan supports
        options.columns = Some(vec![first_readable_column(&plan)?]);
    }
    let depth = max_depth(&plan);
    let big_stack = (depth > DEEP).then(|| depth.saturating_mul(32 * 1024).saturating_add(8 << 20));
    let run = Arc::new(Run {
        types: plan.scan.columns.iter().map(|c| c.ty).collect(),
        plan: Arc::clone(&plan),
        counters: Counters::default(),
        big_stack,
        project_late: plan.limit.is_some(),
    });
    let map: BatchMap = {
        let run = Arc::clone(&run);
        Arc::new(move |b| run.map(b))
    };
    let opener: Arc<dyn Opener> = match &cfg.opener {
        Some(o) => Arc::clone(o),
        None => Arc::new(nata_parquet::FsOpener),
    };
    let scan = Arc::new(ScanPlan::open_with_map(
        &plan.scan.files,
        &options,
        opener,
        Some(map),
    )?);
    observed.stats = Some(Arc::clone(scan.stats()));
    let (offset, limit) = match plan.limit {
        // the scan counts rows delivered: skip `offset` of them here and stop at offset + n
        Some(l) if l.n == 0 => (0, Some(0)),
        Some(l) => (l.offset, Some(l.offset.saturating_add(l.n))),
        None => (0, None),
    };
    let mut seen: u64 = 0;
    let mut rows_out: u64 = 0;
    let pcfg = ParallelConfig {
        threads: cfg.threads.max(1),
        window: cfg.window.max(1),
        ordered: cfg.ordered,
    };
    let scan_report = run_parallel_with(scan, pcfg, limit, cancel, |batch| {
        let n = u64::try_from(batch.num_rows()).unwrap_or(u64::MAX);
        let before = seen;
        seen = seen.saturating_add(n);
        // with a limit the projection runs here, on every row up to `offset + n` (the scan stops there), including the rows OFFSET then drops: as DuckDB,
        // an error of a projected expression in a row before the offset is still an error
        let projected;
        let batch = if run.project_late {
            projected = run
                .project_on_big_stack(batch)
                .map_err(|e| ScanError::new(e.into_kind()))?;
            &projected
        } else {
            batch
        };
        if seen <= offset {
            return Ok(());
        }
        let skip = usize::try_from(offset.saturating_sub(before)).unwrap_or(usize::MAX);
        let part = if skip > 0 {
            batch.slice(skip, batch.num_rows().saturating_sub(skip))
        } else {
            batch.clone()
        };
        rows_out = rows_out.saturating_add(u64::try_from(part.num_rows()).unwrap_or(u64::MAX));
        sink(&part)
    })?;
    let c = &run.counters;
    let ops = OperatorStats {
        batches: c.batches.load(Ordering::SeqCst),
        scan_rows: c.scan_rows.load(Ordering::SeqCst),
        filter_rows: c.filter_rows.load(Ordering::SeqCst),
        filter_time: Duration::from_nanos(c.filter_nanos.load(Ordering::SeqCst)),
        project_time: Duration::from_nanos(c.project_nanos.load(Ordering::SeqCst)),
        project_rows: c.project_rows.load(Ordering::SeqCst),
        limit_in: scan_report.rows_returned,
        rows_out,
    };
    Ok(ExecReport {
        scan: scan_report,
        ops,
        elapsed: started.elapsed(),
    })
}

/// The name of the first column of the first file that the scan can read.
fn first_readable_column(plan: &LogicalPlan) -> Result<String, ScanError> {
    let first = plan
        .scan
        .files
        .first()
        .ok_or_else(|| ScanError::usage("no file given"))?;
    let leaves = nata_parquet::read_leaves(first)?;
    leaves
        .into_iter()
        .find(|l| l.kind.is_ok())
        .map(|l| l.name)
        .ok_or_else(|| {
            ScanError::not_supported("a file none of whose columns has a supported type")
                .in_file(first)
        })
}
