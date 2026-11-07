//! The parallel driver: the row-group tasks of a plan on `nata-sched`'s workers, results to the sink on the calling thread.
//!
//! # Semantics (decided in step 2b; the record `bench/results/20261106-scan-parallel.md` has the reasons)
//!
//! * **Order.** Default ORDERED: the sink sees exactly the rows of the single-thread scan, in the order (file, row group, row), for every thread count
//!   and window. A bounded reorder window makes that cheap: at most `window` results are in flight, finished and waiting, or in the sink's hands.
//!   `ordered = false` hands results over as they finish: the same multiset of rows, in no promised order.
//! * **Limit.** `limit = N` counts rows in the delivered order. When the N-th row is handed over, no further row group is claimed, running tasks stop
//!   at their next batch boundary (and drop reader, handle and buffers), finished results nobody took are dropped, and every worker is joined before
//!   the function returns. Ordered mode returns exactly the first N rows. Unordered mode returns N rows (which ones depends on timing).
//! * **Errors.** A task's error stops further claims and cancels the tasks above it; tasks below it finish. The error returned is the one of the LOWEST
//!   (file, row group) that failed, naming file and row group and never a cell value. Ordered mode also delivers every row before it. An error
//!   found after the limit was satisfied is never reported. A file whose footer cannot be read has a place in the order like any row group.
//! * **Sink errors.** A sink error (a closed stdout) cancels everything exactly like a limit and is returned (it wins over a task error: the output
//!   is gone).
//! * **Footers ahead.** `footers_ahead = K` lets up to K files be pending at once (see `ParallelConfig`); the rows, their order and the errors are those
//!   of `K = 1`. A file that cannot be opened (or whose schema differs) has its place in the order; files after it may have been opened ahead and are
//!   dropped.
//! * **Memory.** Peak decoded bytes in flight is at most `window` times the largest task, see `ScanReport` and the record for the formula.

use std::sync::{Arc, Mutex};

use arrow_array::RecordBatch;
use nata_sched::{CancelToken, Config, Finish, Flow, Run, Scheduler, Work};

use crate::error::{ErrorKind, ScanError};
use crate::reader::{Limiter, RowGroupTask, ScanPlan, ScanReport, TaskOutput, TaskResult};

/// How to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParallelConfig {
    /// Worker threads; the calling thread consumes. At least 1.
    pub threads: usize,
    /// Results in flight (see the module doc); the default is `threads`, which makes the memory bound `threads` times the largest task.
    pub window: usize,
    /// Deliver in the order of the single-thread scan.
    pub ordered: bool,
    /// Footers read ahead (register item P2): how many files may be pending at once (their footer being read, read and waiting for an earlier file, or
    /// with row groups not yet started). `1` is slice 3's behaviour: one file at a time, and the next only when no row group is queued. With `K` the
    /// workers read the next files' footers while earlier row groups run, and the footers in memory stay bounded by `K` plus the files whose row groups
    /// are running. Default 8: the slice-4 benchmark cleared 1.3x on both CPU types; set 1 to disable.
    pub footers_ahead: usize,
}

impl ParallelConfig {
    pub fn new(threads: usize) -> ParallelConfig {
        let threads = threads.max(1);
        ParallelConfig {
            threads,
            window: threads,
            ordered: true,
            footers_ahead: 8,
        }
    }
}

struct ScanWork {
    plan: Arc<ScanPlan>,
}

impl Work for ScanWork {
    type Task = RowGroupTask;
    type Out = TaskOutput;
    type Err = ScanError;

    fn files(&self) -> usize {
        self.plan.file_count()
    }

    fn open(&self, file: usize) -> Result<Vec<RowGroupTask>, ScanError> {
        // the Arc<FileScan> is dropped here; the tasks keep it alive until the last of them is gone
        Ok(self.plan.open_file(file)?.tasks())
    }

    fn run(&self, task: &RowGroupTask, cancel: &CancelToken) -> Run<TaskOutput, ScanError> {
        match task.run_cancellable(&|| cancel.is_cancelled()) {
            Ok(TaskResult::Done(o)) => Run::Done(o),
            Ok(TaskResult::Cancelled) => Run::Cancelled,
            Err(e) => Run::Failed(e),
        }
    }

    fn internal_error(&self, what: &str) -> ScanError {
        ScanError::new(ErrorKind::Internal(what.to_string()))
    }
}

#[derive(Default)]
struct CancelState {
    cancelled: bool,
    hook: Option<Box<dyn Fn() + Send + Sync>>,
}

/// Cancels a running scan from another thread (a signal handler, a supervisor, a test). A scan that is cancelled before it finished returns
/// `ErrorKind::Cancelled`: what the sink has seen is a prefix of the result in ordered mode, and the caller is told that it is not all of it.
/// Cancelling before the scan starts, or after it ended, is allowed (the first makes the scan stop at once, the second does nothing).
#[derive(Clone, Default)]
pub struct ScanCancel(Arc<Mutex<CancelState>>);

impl ScanCancel {
    pub fn new() -> ScanCancel {
        ScanCancel::default()
    }

    pub fn is_cancelled(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .cancelled
    }

    pub fn cancel(&self) {
        let mut g = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        g.cancelled = true;
        if let Some(h) = &g.hook {
            h();
        }
    }

    fn register(&self, hook: Box<dyn Fn() + Send + Sync>) {
        let mut g = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if g.cancelled {
            hook();
        }
        g.hook = Some(hook);
    }

    fn clear(&self) {
        let mut g = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        g.hook = None;
    }
}

/// Run the plan on `cfg.threads` worker threads and hand the batches to `sink` (on this thread). See the module doc for order, limit and errors.
pub fn run_parallel(
    plan: Arc<ScanPlan>,
    cfg: ParallelConfig,
    limit: Option<u64>,
    sink: impl FnMut(&RecordBatch) -> Result<(), ScanError>,
) -> Result<ScanReport, ScanError> {
    run_parallel_with(plan, cfg, limit, None, sink)
}

/// As [`run_parallel`], and `cancel` can stop it from another thread.
pub fn run_parallel_with(
    plan: Arc<ScanPlan>,
    cfg: ParallelConfig,
    limit: Option<u64>,
    cancel: Option<&ScanCancel>,
    mut sink: impl FnMut(&RecordBatch) -> Result<(), ScanError>,
) -> Result<ScanReport, ScanError> {
    let (threads, window) = (cfg.threads.max(1), cfg.window.max(1));
    let mut lim = Limiter {
        remaining: limit,
        returned: 0,
    };
    if limit != Some(0) {
        let work = Arc::new(ScanWork {
            plan: Arc::clone(&plan),
        });
        let sched = Scheduler::new(
            work,
            Config {
                threads,
                window,
                ordered: cfg.ordered,
                open_ahead: cfg.footers_ahead.max(1),
            },
        );
        if let Some(c) = cancel {
            let handle = sched.cancel_handle();
            c.register(Box::new(move || handle.cancel()));
        }
        let outcome = sched.run(|_seq, out| {
            for batch in &out.batches {
                if lim.feed(batch, &mut sink)? {
                    return Ok(Flow::Stop);
                }
            }
            Ok(Flow::Continue)
        });
        if let Some(c) = cancel {
            c.clear();
        }
        plan.release_first_footer();
        if outcome.result? == Finish::Cancelled {
            return Err(ScanError::new(ErrorKind::Cancelled(
                "the scan was cancelled before it finished".into(),
            )));
        }
    }
    plan.release_first_footer();
    Ok(ScanReport::from_stats(
        &plan,
        lim.returned,
        threads,
        window,
        cfg.ordered,
    ))
}
