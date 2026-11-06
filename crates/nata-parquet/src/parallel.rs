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
//! * **Memory.** Peak decoded bytes in flight is at most `window` times the largest task, see `ScanReport` and the record for the formula.

use std::sync::Arc;

use arrow_array::RecordBatch;
use nata_sched::{CancelToken, Config, Flow, Run, Scheduler, Work};

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
}

impl ParallelConfig {
    pub fn new(threads: usize) -> ParallelConfig {
        let threads = threads.max(1);
        ParallelConfig {
            threads,
            window: threads,
            ordered: true,
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

/// Run the plan on `cfg.threads` worker threads and hand the batches to `sink` (on this thread). See the module doc for order, limit and errors.
pub fn run_parallel(
    plan: Arc<ScanPlan>,
    cfg: ParallelConfig,
    limit: Option<u64>,
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
            },
        );
        let outcome = sched.run(|_seq, out| {
            for batch in &out.batches {
                if lim.feed(batch, &mut sink)? {
                    return Ok(Flow::Stop);
                }
            }
            Ok(Flow::Continue)
        });
        plan.release_first_footer();
        outcome.result?;
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
