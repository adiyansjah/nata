//! Aggregate scan and final partition continuations. Disk work is submitted to
//! the bounded service; the cooperative adapter releases the CPU slot on Blocked.
use super::storage::{Completion, Storage};
use super::*;
use crate::spill::{IoService, Registry, SpillFile, Submit, Ticket, Workspace};
use nata_parquet::{RowGroupCursor, RowGroupTask, ScanPlan, ScanReport};
use nata_sched::{
    cooperative::{self, Step},
    memory::Wake,
};
use std::collections::VecDeque;

fn scan_error(e: impl std::fmt::Display) -> ScanError {
    ScanError::new(ErrorKind::Internal(format!("aggregate spill: {e}")))
}
fn memory_error(shared: &Shared, minimum: usize) -> ScanError {
    let pos = shared
        .plan
        .aggregate
        .as_ref()
        .and_then(|a| {
            a.keys
                .first()
                .map(|e| e.pos)
                .or_else(|| a.aggregates.first().map(|a| a.pos))
        })
        .unwrap_or(nata_sql::Pos::NONE);
    ScanError::new(
        EvalError::at(
            pos,
            format!(
                "resource error: aggregate minimum working set requires at least {minimum} bytes, budget {} bytes",
                shared.manager.snapshot().budget,
            ),
        )
        .into_kind(),
    )
}
fn stopped(cancel: Option<&ScanCancel>) -> bool {
    cancel.is_some_and(ScanCancel::is_cancelled)
}
fn cancel_error() -> ScanError {
    ScanError::new(ErrorKind::Cancelled("aggregate cancelled".into()))
}
type Files = Vec<Arc<SpillFile>>;
pub(super) enum Partitions {
    Resident(Vec<Vec<Table>>),
    Spilled(Files),
}
/// Reusable task scratch. Every byte of capacity stays charged until the
/// task's evaluated/input allocations have been freed, including spare credit.
#[derive(Default)]
struct Scratch {
    capacity: usize,
    grants: Chunks,
}
impl Scratch {
    // The existing cache allocation floor is non-revocable while a batch is
    // evaluated. Admit it before any task can borrow memory for tables.
    const BYTES: usize = 64 << 10;
    const MINIMUM: usize = Self::BYTES + 128; // Chunks reservation metadata

    fn admitted(manager: &Arc<MemoryManager>, strings: bool) -> Result<Self, ScanError> {
        let mut scratch = Self::default();
        if strings && !scratch.reserve(manager, Self::BYTES) {
            return Err(ScanError::new(resource().into_kind()));
        }
        Ok(scratch)
    }
    fn reserve(&mut self, manager: &Arc<MemoryManager>, bytes: usize) -> bool {
        let extra = bytes.saturating_sub(self.capacity);
        if extra == 0 {
            return true;
        }
        // As with the old per-batch request, pending/conflicting requests are
        // pressure: the caller spills and retries, or reports a minimum-set
        // error when it owns no reclaimable state. No allocation on failure.
        if !matches!(
            self.grants
                .reserve(manager, 0, extra, bytes.max(Self::BYTES)),
            Ok(true)
        ) {
            return false;
        }
        self.capacity = bytes;
        true
    }
}

struct ScanState {
    partial: Partial,
    cursor: Option<RowGroupCursor>,
    evaluated: Option<Evaluated>,
    input: Option<RecordBatch>,
    ticket: Option<Ticket<Completion>>,
    finishing: bool,
    pressure_since: Option<Instant>,
    source: Option<(usize, std::path::PathBuf, usize)>,
    // Reader and evaluation allocations above are freed before the grant.
    strings: Scratch,
    _decode: Grant,
}
fn empty_tables(fanout: usize) -> Vec<Table> {
    (0..fanout).map(|_| Table::new()).collect()
}
fn files(registry: &Arc<Registry>, fanout: usize, depth: u8) -> Result<Files, ScanError> {
    (0..fanout)
        .map(|_| registry.create_at_level(depth).map_err(scan_error))
        .collect()
}
fn submit_write(
    state: &mut ScanState,
    storage: &Arc<Storage>,
    files: &Files,
    io: &IoService<Completion>,
    wake: Arc<dyn Wake>,
) -> Result<(), ScanError> {
    let tables = std::mem::replace(&mut state.partial.tables, empty_tables(PARTITIONS));
    let storage = Arc::clone(storage);
    let files = files.clone();
    let shared = Arc::clone(&state.partial.shared);
    let job = Box::new(move || {
        let _stats =
            crate::query_stats::Timer::new(&shared.stats, crate::query_stats::Stage::SpillWrite);
        storage.write(
            tables,
            &files,
            shared
                .plan
                .aggregate
                .as_ref()
                .ok_or_else(|| std::io::Error::other("missing aggregate"))?,
            0,
        )?;
        Ok(Completion::Written)
    });
    match io.submit_observed(state.partial.shared.stats.as_ref(), "io_write_end", 0, job) {
        Submit::Queued(ticket) => {
            wake.wake();
            state.ticket = Some(ticket);
            Ok(())
        }
        Submit::Full(_) | Submit::Closed(_) => Err(scan_error("spill queue admission exhausted")),
    }
}
pub(super) fn accumulate(
    shared: &Arc<Shared>,
    cfg: &ExecConfig,
    cancel: Option<&ScanCancel>,
    observed: &mut Observed,
) -> Result<(ScanReport, Partitions, Arc<Storage>, IoService<Completion>), ScanError> {
    // The initial runtime/metadata/output workspace is reserved before opening
    // a scan or constructing queues and continuation vectors.
    observed.stats = Some(Arc::new(nata_parquet::ScanStats::default()));
    let runtime = Arc::new(
        grant(&shared.manager, 2 << 20)
            .map_err(|_| memory_error(shared, (2 << 20) + shared.manager.snapshot().progress))?,
    );
    let mut options = shared.plan.scan.scan_options();
    options.late_materialization = cfg.late_materialization;
    if shared.plan.scan.columns.is_empty() {
        options.columns = Some(vec![crate::exec::first_readable_column(
            &shared.plan,
            cfg.remote.as_deref(),
        )?]);
    }
    let scan = Arc::new(ScanPlan::open_remote(
        &shared.plan.scan.files,
        &options,
        cfg.opener
            .clone()
            .unwrap_or_else(|| Arc::new(nata_parquet::FsOpener)),
        None,
        None,
        Some(Arc::clone(&shared.manager)),
        cfg.remote.as_ref(),
    )?);
    if let Some(stats) = &cfg.query_stats {
        let _ = scan.stats().query_stats.set(Arc::clone(stats));
    }
    observed.stats = Some(Arc::clone(scan.stats()));
    // a cancel also wakes a worker that is waiting for a store (a no-op for local files)
    let _remote_cancel = scan.cancel_hook(cancel);
    let mut tasks = VecDeque::new();
    let failure: Mutex<Option<(usize, ScanError)>> = Mutex::new(None);
    let mut decode_bytes = 64 << 10;
    for f in 0..scan.file_count() {
        let file = match scan.open_file(f) {
            Ok(file) => file,
            Err(e) => {
                *failure
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((tasks.len(), e));
                break;
            }
        };
        for task in file.tasks() {
            let expression_bytes = usize::try_from(task.rows())
                .map_err(|_| scan_error("scan rows"))?
                .min(256)
                .checked_mul(super::expression_slots(&shared.plan))
                .and_then(|n| n.checked_mul(128))
                .and_then(|n| n.checked_add(256 * 8 + CANCEL_ROWS * 16))
                .ok_or_else(|| scan_error("expression minimum working set overflow"))?;
            decode_bytes = decode_bytes.max(
                task.cursor_bytes(256)
                    .and_then(|n| n.checked_add(expression_bytes))
                    .ok_or_else(|| scan_error("scan minimum working set overflow"))?,
            );
            tasks.push_back((tasks.len(), task));
        }
    }
    scan.release_first_footer();
    let budget = shared.manager.snapshot().budget;
    let base = cfg
        .aggregate_spill_directory
        .clone()
        .unwrap_or_else(std::env::temp_dir);
    let available = crate::spill::available_bytes(&base).map_err(scan_error)?;
    let quota = cfg
        .aggregate_spill_disk_bytes
        .unwrap_or((budget as u64).saturating_mul(4).min(available / 2));
    let allowance = crate::spill::descriptor_allowance(cfg.threads.min(32)).map_err(scan_error)?;
    let registry =
        Registry::new(&base, quota, allowance, Arc::clone(&shared.manager)).map_err(scan_error)?;
    if let Some(after) = cfg.aggregate_spill_fail_after {
        registry.inject_write_failure(after);
    }
    let workspace = Workspace::new(&shared.manager).map_err(scan_error)?;
    let io = IoService::new(32, &shared.manager).map_err(scan_error)?;
    let expected_rows = tasks.iter().try_fold(0usize, |rows, (_, task)| {
        rows.checked_add(
            usize::try_from(task.rows()).map_err(|_| scan_error("spill estimator overflow"))?,
        )
        .ok_or_else(|| scan_error("spill estimator overflow"))
    })?;
    let a = shared
        .plan
        .aggregate
        .as_ref()
        .ok_or_else(|| scan_error("aggregate missing"))?;
    let resident_estimate = estimate_resident(expected_rows, a.keys.len(), a.aggregates.len())?;
    let snapshot = shared.manager.snapshot();
    let merge_budget = snapshot
        .budget
        .saturating_sub(snapshot.progress)
        .saturating_sub(snapshot.held);
    let initial_fanout = initial_fanout(resident_estimate, merge_budget);
    let storage = Arc::new(Storage {
        registry,
        runtime,
        initial_fanout,
        resident_estimate,
        cancel: cancel.cloned(),
        hash: shared.hash,
        partial_rows: Arc::default(),
        largest_partial: Arc::default(),
        rehashes: Arc::default(),
        workspace: Mutex::new(workspace),
    });
    let targets = files(&storage.registry, storage.initial_fanout, 0)?;
    let before_decode = shared.manager.snapshot();
    let free = before_decode
        .budget
        .saturating_sub(before_decode.progress)
        .saturating_sub(before_decode.held);
    let has_strings = shared
        .plan
        .scan
        .columns
        .iter()
        .any(|c| c.ty == SqlType::Varchar);
    let task_bytes = decode_bytes
        .checked_add(if has_strings { Scratch::MINIMUM } else { 0 })
        .ok_or_else(|| scan_error("scan minimum working set overflow"))?;
    // Preserve the existing 1 MiB table-progress allowance in addition to
    // every admitted cursor and its non-revocable string scratch floor.
    let minimum = task_bytes.saturating_add(1 << 20);
    if minimum > free {
        return Err(memory_error(
            shared,
            budget.saturating_sub(free).saturating_add(minimum),
        ));
    }
    let n = scan_concurrency(cfg.threads, tasks.len(), free, task_bytes, budget);
    if let Some(stats) = &shared.stats {
        stats.admission(n, free, decode_bytes, task_bytes);
    }
    let tasks = Mutex::new(tasks);
    let mut states = Vec::new();
    for _ in 0..n {
        let decode =
            grant(&shared.manager, decode_bytes).map_err(|e| ScanError::new(e.into_kind()))?;
        states.push(ScanState {
            partial: Partial {
                shared: Arc::clone(shared),
                tables: empty_tables(PARTITIONS),
                program: None,
                rows: Vec::new(),
                hashes: Vec::new(),
            },
            cursor: None,
            evaluated: None,
            input: None,
            ticket: None,
            finishing: false,
            pressure_since: None,
            source: None,
            strings: Scratch::admitted(&shared.manager, has_strings)?,
            _decode: decode,
        });
    }
    let results: Mutex<Vec<Vec<Table>>> = Mutex::new(Vec::new());
    cooperative::run_observed(
        shared.stats.as_ref(),
        states,
        n,
        |state, wake| {
            if stopped(cancel) {
                io.cancel();
                return Step::Done(Err(cancel_error()));
            }
            let start = Instant::now();
            let step = (|| -> Result<Step<Vec<Table>, ScanError>, ScanError> {
                loop {
                    if stopped(cancel) {
                        return Err(cancel_error());
                    }
                    if let Some(ticket) = &state.ticket {
                        let Some(result) = ticket.poll(wake.clone()) else {
                            return Ok(Step::Blocked);
                        };
                        match result.map_err(scan_error)? {
                            Completion::Written => (),
                            _ => return Err(scan_error("unexpected write completion")),
                        }
                        state.ticket = None;
                        // All tasks belong to this one spillable operator. Discard the
                        // coalesced request after release; each row retries reservation.
                        shared
                            .manager
                            .finish(0)
                            .map_err(|_| scan_error("spill pressure cleanup"))?;
                    }
                    let failed_at = failure
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .as_ref()
                        .map(|(i, _)| *i);
                    if failed_at.is_some_and(|failed| {
                        state.source.as_ref().is_some_and(|(i, _, _)| *i > failed)
                    }) {
                        return Ok(Step::Done(Ok(empty_tables(PARTITIONS))));
                    }
                    if state.finishing {
                        return Ok(Step::Done(Ok(std::mem::replace(
                            &mut state.partial.tables,
                            empty_tables(PARTITIONS),
                        ))));
                    }
                    if let Some(evaluated) = &mut state.evaluated {
                        let finished = state
                            .partial
                            .resume(evaluated, &|| stopped(cancel))
                            .map_err(|e| ScanError::new(e.into_kind()))?;
                        if !finished {
                            if state.partial.tables.iter().all(|t| t.groups.is_empty()) {
                                // Finished local states remain revocable until the
                                // entire scan phase ends. Reclaim one before parking.
                                let victim = results
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .pop();
                                if let Some(tables) = victim {
                                    state.partial.tables = tables;
                                    submit_write(state, &storage, &targets, &io, wake.clone())?;
                                    return Ok(Step::Yield);
                                }
                                // A peer may still own its table while its write is
                                // queued. Park without retaining a CPU slot; its grant
                                // release wakes the operator, and the row retries.
                                if state
                                    .pressure_since
                                    .get_or_insert_with(Instant::now)
                                    .elapsed()
                                    > Duration::from_secs(5)
                                {
                                    return Err(scan_error(
                                        "aggregate minimum record cannot fit memory budget",
                                    ));
                                }
                                state.partial.tables = empty_tables(PARTITIONS);
                                shared
                                    .manager
                                    .finish(0)
                                    .map_err(|_| scan_error("memory pressure cleanup"))?;
                                if let Some(stats) = &shared.stats {
                                    stats.event(
                                        "admission_wait_start",
                                        0,
                                        state.source.as_ref().map_or(0, |s| s.0 as u64),
                                        0,
                                        0,
                                    );
                                }
                                shared
                                    .manager
                                    .park(0, wake.clone())
                                    .map_err(|_| scan_error("memory pressure park"))?;
                                return Ok(Step::Blocked);
                            }
                            submit_write(state, &storage, &targets, &io, wake.clone())?;
                            return Ok(Step::Yield);
                        }
                        if let Some(start) = state.pressure_since.take()
                            && let Some(stats) = &shared.stats
                        {
                            stats.add(
                                crate::query_stats::Stage::AdmissionWait,
                                u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX),
                            );
                            stats.event(
                                "admission_wait_end",
                                0,
                                state.source.as_ref().map_or(0, |s| s.0 as u64),
                                0,
                                0,
                            );
                        }
                        if let Some(evaluated) = state.evaluated.take() {
                            state.partial.recycle(evaluated);
                        }
                        if cfg.aggregate_force_spill || shared.manager.revoked(0).unwrap_or(false) {
                            submit_write(state, &storage, &targets, &io, wake.clone())?;
                            return Ok(Step::Yield);
                        }
                    }
                    if let Some(batch) = &state.input {
                        // A dictionary can expand to much more text than its codes.
                        // Stream a prefix through admitted scratch rather than ask
                        // for a larger multi-row working set under table pressure.
                        let mut rows = batch.num_rows();
                        let mut input = batch.clone();
                        let mut bytes = string_scratch(&input, &shared.plan)
                            .map_err(|e| ScanError::new(e.into_kind()))?;
                        while bytes > state.strings.capacity && rows > 1 {
                            rows = rows.div_ceil(2);
                            input = batch.slice(0, rows);
                            bytes = string_scratch(&input, &shared.plan)
                                .map_err(|e| ScanError::new(e.into_kind()))?;
                        }
                        if !state.strings.reserve(&shared.manager, bytes) {
                            if state.partial.tables.iter().any(|t| !t.groups.is_empty()) {
                                submit_write(state, &storage, &targets, &io, wake.clone())?;
                                return Ok(Step::Yield);
                            }
                            return Err(memory_error(
                                shared,
                                bytes.saturating_add(
                                    shared.manager.snapshot().progress
                                        + state._decode.bytes()
                                        + (2 << 20),
                                ),
                            ));
                        }
                        let evaluated = state
                            .partial
                            .evaluate(&input)
                            .map_err(|e| ScanError::new(e.into_kind()))?;
                        state.input = (rows < batch.num_rows())
                            .then(|| batch.slice(rows, batch.num_rows() - rows));
                        state.evaluated = Some(evaluated);
                        continue;
                    }
                    if state.cursor.is_none() {
                        let task: Option<(usize, RowGroupTask)> = tasks
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .pop_front();
                        let Some((index, task)) = task else {
                            state.finishing = true;
                            return Ok(Step::Yield);
                        };
                        if failure
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .as_ref()
                            .is_some_and(|(failed, _)| index >= *failed)
                        {
                            return Ok(Step::Done(Ok(empty_tables(PARTITIONS))));
                        }
                        if let Some(stats) = &shared.stats {
                            stats.event(
                                "row_group_start",
                                0,
                                index as u64,
                                0,
                                task.row_group() as u64,
                            );
                        }
                        state.source = Some((index, task.path().to_path_buf(), task.row_group()));
                        let _stats = crate::query_stats::Timer::new(
                            &shared.stats,
                            crate::query_stats::Stage::ScanDecode,
                        );
                        state.cursor = Some(task.cursor(256)?);
                    }
                    if let Some(cursor) = &mut state.cursor {
                        let _stats = crate::query_stats::Timer::new(
                            &shared.stats,
                            crate::query_stats::Stage::ScanDecode,
                        );
                        if let Some(batch) = cursor.next_batch()? {
                            state.input = Some(batch);
                        } else {
                            if let (Some(stats), Some((index, _, _))) =
                                (&shared.stats, &state.source)
                            {
                                stats.event("row_group_end", 0, *index as u64, 0, 0);
                            }
                            state.cursor = None;
                            // A morsel is the scheduling quantum. Decode, evaluate
                            // and accumulate its batches on this worker; only spill
                            // or memory pressure parks the continuation mid-morsel.
                            return Ok(Step::Yield);
                        }
                    }
                }
            })();
            shared
                .metrics
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .partial_nanos += u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
            match step {
                Ok(step) => step,
                Err(mut e) => {
                    let index = state.source.as_ref().map_or(0, |(i, _, _)| *i);
                    if let Some((_, path, rg)) = &state.source {
                        e = e.in_file(path).in_row_group(*rg);
                    }
                    let mut error = failure
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if error.as_ref().is_none_or(|(i, _)| index < *i) {
                        *error = Some((index, e));
                    }
                    Step::Done(Ok(empty_tables(PARTITIONS)))
                }
            }
        },
        |state| {
            results
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(state);
            Ok(())
        },
    )?;
    if let Some((_, error)) = failure
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
    {
        return Err(error);
    }
    let partials = results
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let a = shared
        .plan
        .aggregate
        .as_ref()
        .ok_or_else(|| scan_error("aggregate missing"))?;
    let mut partitions: Vec<Vec<Table>> = (0..PARTITIONS).map(|_| Vec::new()).collect();
    for tables in partials {
        let mut m = shared
            .metrics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        m.partial_groups += tables.iter().map(|t| t.groups.len()).sum::<usize>();
        m.largest_partial = m
            .largest_partial
            .max(tables.iter().map(|t| t.groups.len()).sum());
        m.rehashes += tables.iter().map(|t| t.groups.rehashes).sum::<usize>();
        drop(m);
        for (p, t) in tables.into_iter().enumerate() {
            if !t.groups.is_empty() {
                partitions[p].push(t);
            }
        }
    }
    if a.keys.is_empty()
        && partitions.iter().all(Vec::is_empty)
        && storage.registry.snapshot().bytes_written == 0
    {
        let mut table = Table::new();
        table
            .reserve(&shared.manager, &[], a)
            .map_err(|e| ScanError::new(e.into_kind()))?;
        table
            .groups
            .insert(vec![], a.aggregates.iter().map(State::new).collect());
        partitions[usize::try_from((shared.hash)(&[], 0) & 15)
            .map_err(|_| scan_error("global partition"))?]
        .push(table);
    }
    let partitions = if cfg.aggregate_force_spill
        || storage.registry.snapshot().bytes_written > 0
        || resident_merge_needs_spill(
            &partitions,
            shared.manager.snapshot().budget,
            fixed_width_output(&shared.plan, a),
        ) {
        // Flush remaining resident contributions through the same continuation
        // seam. No partial is published before all writes have succeeded.
        let states = partitions
            .into_iter()
            .enumerate()
            .flat_map(|(p, ts)| ts.into_iter().map(move |t| (p, t)))
            .collect::<Vec<_>>();
        for (p, t) in states {
            let mut tables = empty_tables(PARTITIONS);
            tables[p] = t;
            let storage = Arc::clone(&storage);
            let files = targets.clone();
            let stats = shared.stats.clone();
            let shared = Arc::clone(shared);
            let ticket = match io.submit_observed(
                stats.as_ref(),
                "io_write_end",
                0,
                Box::new(move || {
                    storage.write(
                        tables,
                        &files,
                        shared.plan.aggregate.as_ref().expect("aggregate"),
                        0,
                    )?;
                    Ok(Completion::Written)
                }),
            ) {
                Submit::Queued(t) => t,
                _ => return Err(scan_error("final spill queue admission")),
            };
            cooperative::run_observed(
                stats.as_ref(),
                vec![ticket],
                1,
                |ticket, wake| {
                    if stopped(cancel) {
                        io.cancel();
                        return Step::Done(Err(cancel_error()));
                    }
                    match ticket.poll(wake) {
                        Some(r) => Step::Done(r.map(|_| ()).map_err(scan_error)),
                        None => Step::Blocked,
                    }
                },
                |_| Ok(()),
            )?;
        }
        Partitions::Spilled(targets)
    } else {
        drop(targets);
        Partitions::Resident(partitions)
    };
    let report = ScanReport::from_stats(&scan, 0, n, n, false);
    Ok((report, partitions, storage, io))
}

/// Leave room for each local partition's first table chunk, as well as the
/// existing shared 1 MiB progress allowance. Admitting only cursor workspace
/// can strand almost all of B in non-revocable readers: every small partial
/// then spills and fans out into tiny IPC frames. This is admission headroom,
/// not a reservation or a per-task table cap; tables still borrow through the
/// query manager. A single task can still run when full chunks do not fit
/// (Chunks borrows only available bytes), subject to the minimum check above.
fn scan_concurrency(
    threads: usize,
    tasks: usize,
    free: usize,
    task_bytes: usize,
    budget: usize,
) -> usize {
    let table_bytes = PARTITIONS * Table::chunk_size(budget);
    let working_set = task_bytes.saturating_add(table_bytes);
    threads
        .max(1)
        .min(tasks.max(1))
        .min(32)
        .min((free.saturating_sub(1 << 20) / working_set.max(1)).max(1))
}

/// The existing output grant covers bounded fixed-width batches/evaluator
/// slots, not arbitrary string payload copies. Inspect intermediate expressions
/// too: a fixed-width result can still contain a variable-width cast/CASE arm.
fn fixed_width_output(plan: &LogicalPlan, a: &Aggregate) -> bool {
    if a.keys.iter().any(|e| e.ty == SqlType::Varchar)
        || a.aggregates.iter().any(|e| e.ty == SqlType::Varchar)
    {
        return false;
    }
    let mut pending: Vec<_> = plan.project.exprs.iter().map(|p| &p.expr).collect();
    pending.extend(a.having.as_ref());
    while let Some(expr) = pending.pop() {
        if expr.ty == SqlType::Varchar {
            return false;
        }
        pending.extend(expr.children());
    }
    true
}

/// A sole table is adopted by `merge`: no destination inserts or rehashes,
/// and states finalize in place (including moved string extrema). Its grants
/// remain live through queued results and output. Runtime/output workspace was
/// already granted before scanning. Only fixed-width output uses this path:
/// string payload copies during output need a separate capacity bound.
/// Multiple inputs still retain the conservative overlap guard: sources,
/// destination inserts/rehash and buffered results may all coexist.
fn resident_merge_needs_spill(
    partitions: &[Vec<Table>],
    budget: usize,
    fixed_output: bool,
) -> bool {
    if fixed_output && partitions.iter().all(|tables| tables.len() <= 1) {
        return false;
    }
    let resident_bytes: usize = partitions
        .iter()
        .flatten()
        .map(|t| t.grants.reserved())
        .sum();
    resident_bytes.saturating_mul(3) > budget.saturating_sub(3 << 20)
}

/// Resident charges, not serialized bytes. Each input row may produce a partial.
/// The per-row charge mirrors reserve_entry plus geometric table capacity/rehash
/// overlap (256), and allows 64 estimated text bytes per key/state. Actual text
/// and skew still go through grants and bounded recursive repartitioning.
fn estimate_resident(rows: usize, keys: usize, states: usize) -> Result<usize, ScanError> {
    keys.checked_mul(128)
        .and_then(|k| states.checked_mul(192).and_then(|s| k.checked_add(s)))
        .and_then(|n| n.checked_add(512))
        .and_then(|n| n.checked_mul(rows))
        .ok_or_else(|| scan_error("resource error: spill estimator overflow"))
}
fn initial_fanout(estimate: usize, merge_budget: usize) -> usize {
    let mut fanout = PARTITIONS;
    while fanout < 256 && estimate.div_ceil(fanout) > merge_budget / 2 {
        fanout *= 2;
    }
    fanout
}
fn merge_concurrency(threads: usize, budget: usize, partition_estimate: usize) -> usize {
    // Half each share is table space, the rest covers read/output workspaces,
    // capacity rounding and overlap. At least one task attempts the bounded
    // fallback if even the 256-way estimate exceeds the single-task grant.
    threads
        .clamp(1, 16)
        .min((budget / (4 << 20)).max(1))
        .min((budget / 2 / partition_estimate.max(1)).max(1))
}

struct MergeTask {
    file: Arc<SpillFile>,
    partition: usize,
    depth: u8,
    table: Table,
    ticket: Option<Ticket<Completion>>,
    handle: Option<crate::spill::SpillHandle>,
    frame: Option<crate::spill::SpillBatch>,
    row: usize,
    storage: Arc<Storage>,
    limit: usize,
}
enum Merged {
    Table(Table),
    Repartition {
        file: Arc<SpillFile>,
        partition: usize,
        depth: u8,
    },
}
pub(super) fn finalize(
    partitions: Partitions,
    shared: &Arc<Shared>,
    cfg: &ExecConfig,
    cancel: Option<&ScanCancel>,
    storage: &Arc<Storage>,
    io: &IoService<Completion>,
    mut sink: impl FnMut(Table) -> Result<(), ScanError>,
) -> Result<(), ScanError> {
    let a = shared
        .plan
        .aggregate
        .as_ref()
        .ok_or_else(|| scan_error("aggregate missing"))?;
    let Partitions::Spilled(files) = partitions else {
        if let Partitions::Resident(partitions) = partitions {
            return nata_sched::unordered::run(
                partitions,
                cfg.threads,
                || stopped(cancel),
                |tables, cancelled| {
                    let start = Instant::now();
                    let _scope = crate::query_stats::Scope::new(&shared.stats);
                    let _stats = crate::query_stats::Timer::new(
                        &shared.stats,
                        crate::query_stats::Stage::Merge,
                    );
                    let result = merge(tables, a, &shared.manager, cancelled)
                        .map_err(|e| ScanError::new(e.into_kind()));
                    shared
                        .metrics
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .merge_nanos +=
                        u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
                    result
                },
                sink,
            );
        }
        unreachable!();
    };
    let mut pending: Vec<_> = files
        .into_iter()
        .enumerate()
        .map(|(partition, file)| (file, partition, 0))
        .collect();
    let snapshot = shared.manager.snapshot();
    let merge_budget = snapshot
        .budget
        .saturating_sub(snapshot.progress)
        .saturating_sub(snapshot.held);
    let concurrency = merge_concurrency(
        cfg.threads,
        merge_budget,
        storage.resident_estimate.div_ceil(storage.initial_fanout),
    );
    while !pending.is_empty() {
        let mut tasks = Vec::new();
        for _ in 0..concurrency {
            let Some((file, partition, depth)) = pending.pop() else {
                break;
            };
            let workspace = Workspace::new(&shared.manager).map_err(scan_error)?;
            tasks.push(MergeTask {
                file,
                partition,
                depth,
                table: Table::new(),
                ticket: None,
                handle: None,
                frame: None,
                row: 0,
                storage: Arc::new(Storage {
                    registry: Arc::clone(&storage.registry),
                    runtime: Arc::clone(&storage.runtime),
                    cancel: cancel.cloned(),
                    hash: shared.hash,
                    initial_fanout: storage.initial_fanout,
                    resident_estimate: storage.resident_estimate,
                    partial_rows: Arc::clone(&storage.partial_rows),
                    largest_partial: Arc::clone(&storage.largest_partial),
                    rehashes: Arc::clone(&storage.rehashes),
                    workspace: Mutex::new(workspace),
                }),
                limit: cfg
                    .aggregate_merge_partition_bytes
                    .unwrap_or(merge_budget / concurrency / 2),
            });
        }
        cooperative::run_observed(
            shared.stats.as_ref(),
            tasks,
            concurrency,
            |task, wake| {
                if stopped(cancel) {
                    io.cancel();
                    return Step::Done(Err(cancel_error()));
                }
                let start = Instant::now();
                let _stats =
                    crate::query_stats::Timer::new(&shared.stats, crate::query_stats::Stage::Merge);
                let step =
                    merge_step(task, shared, io, wake).unwrap_or_else(|e| Step::Done(Err(e)));
                let elapsed = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
                shared
                    .metrics
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .merge_nanos += elapsed;
                step
            },
            |out| match out {
                Merged::Table(table) => sink(table),
                Merged::Repartition {
                    file,
                    partition,
                    depth,
                } => {
                    if depth >= 8 {
                        return Err(scan_error(
                            "resource error: aggregate spill recursion bound (8) exceeded",
                        ));
                    }
                    let children =
                        files_for_repartition(storage, shared, io, file, partition, depth, cancel)?;
                    let mut metrics = shared
                        .metrics
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    metrics.recursion_depth = (depth + 1).max(metrics.recursion_depth);
                    drop(metrics);
                    pending.extend(
                        children
                            .into_iter()
                            .enumerate()
                            .map(|(p, f)| (f, p, depth + 1)),
                    );
                    Ok(())
                }
            },
        )?;
    }
    Ok(())
}
fn merge_step(
    task: &mut MergeTask,
    shared: &Arc<Shared>,
    io: &IoService<Completion>,
    wake: Arc<dyn Wake>,
) -> Result<Step<Merged, ScanError>, ScanError> {
    let a = shared
        .plan
        .aggregate
        .as_ref()
        .ok_or_else(|| scan_error("aggregate missing"))?;
    if let Some(ticket) = &task.ticket {
        let Some(result) = ticket.poll(wake.clone()) else {
            return Ok(Step::Blocked);
        };
        match result.map_err(scan_error)? {
            Completion::Read { frame, handle } => {
                task.handle = Some(handle);
                task.frame = frame;
                task.row = 0;
                task.ticket = None;
                if task.frame.is_none() {
                    let _stats = crate::query_stats::Timer::new(
                        &shared.stats,
                        crate::query_stats::Stage::Finalize,
                    );
                    for (row, states) in task.table.groups.values_mut().enumerate() {
                        if row.is_multiple_of(CANCEL_ROWS)
                            && task
                                .storage
                                .cancel
                                .as_ref()
                                .is_some_and(ScanCancel::is_cancelled)
                        {
                            return Err(cancel_error());
                        }
                        for (s, a) in states.iter_mut().zip(&a.aggregates) {
                            s.finalize(a).map_err(|e| ScanError::new(e.into_kind()))?;
                        }
                    }
                    return Ok(Step::Done(Ok(Merged::Table(std::mem::replace(
                        &mut task.table,
                        Table::new(),
                    )))));
                }
            }
            _ => return Err(scan_error("unexpected read completion")),
        }
    }
    if let Some(frame) = &task.frame {
        let records = storage::Records::new(frame).map_err(scan_error)?;
        let rows = frame.batch().num_rows();
        if rows > crate::spill::MAX_ROWS {
            return Err(scan_error("spill replay row bound"));
        }
        // Fixed stack scratch belongs to the admitted replay workspace. Bind
        // groups/cursors for the batch, then decode one state column at a time
        // directly into retained group slots. No per-row state Vec is built.
        let mut ids = [0; crate::spill::MAX_ROWS];
        let mut cursors: [Option<replay::States<'_>>; crate::spill::MAX_ROWS] =
            [const { None }; crate::spill::MAX_ROWS];
        for row in task.row..rows {
            let key = records
                .key(
                    row,
                    a,
                    task.partition,
                    task.depth,
                    shared.hash,
                    task.storage.initial_fanout,
                )
                .map_err(scan_error)?;
            cursors[row] = Some(records.states(row, a).map_err(scan_error)?);
            let groups = &mut task.table.groups;
            let existing = groups
                .index
                .find((groups.hash)(&key), |&id| groups.entries[id].0 == key)
                .copied();
            let id = if let Some(id) = existing {
                id
            } else {
                if (!task.table.groups.is_empty() && task.table.grants.reserved() >= task.limit)
                    || task.table.reserve(&shared.manager, &key, a).is_err()
                {
                    return Ok(Step::Done(Ok(Merged::Repartition {
                        file: Arc::clone(&task.file),
                        partition: task.partition,
                        depth: task.depth,
                    })));
                }
                // This Vec is retained group storage, allocated once per distinct
                // key. Repeated partial rows never allocate a Vec<State>.
                let id = task.table.groups.len();
                task.table
                    .groups
                    .insert(key, Vec::with_capacity(a.aggregates.len()));
                id
            };
            ids[row] = id;
        }
        for (slot, expr) in a.aggregates.iter().enumerate() {
            for row in task.row..rows {
                let states = &mut task.table.groups.entries[ids[row]].1;
                let cursor = cursors[row]
                    .as_mut()
                    .ok_or_else(|| scan_error("spill state cursor"))?;
                let source = cursor.next(expr).map_err(scan_error)?;
                let extra = match (&source, states.get(slot)) {
                    (State::Extreme(Atom::Str(text)), Some(State::Extreme(Atom::Str(old)))) => {
                        text.len().saturating_sub(old.len())
                    }
                    (State::Extreme(Atom::Str(text)), _) => text.len(),
                    _ => 0,
                };
                if extra > 0 {
                    // Reserve before retaining decoded text. A pressure return
                    // discards this table and replays the whole parent, so a
                    // partially merged record can never be counted twice.
                    let chunk = task.table.chunk_bytes;
                    if !matches!(
                        task.table.grants.reserve(&shared.manager, 0, extra, chunk),
                        Ok(true)
                    ) {
                        return Ok(Step::Done(Ok(Merged::Repartition {
                            file: Arc::clone(&task.file),
                            partition: task.partition,
                            depth: task.depth,
                        })));
                    }
                }
                if states.len() == slot {
                    states.push(source);
                } else {
                    states
                        .get_mut(slot)
                        .ok_or_else(|| scan_error("spill state slot"))?
                        .merge(source, expr)
                        .map_err(|e| ScanError::new(e.into_kind()))?;
                }
            }
        }
        for cursor in &cursors[task.row..rows] {
            cursor
                .as_ref()
                .ok_or_else(|| scan_error("spill state cursor"))?
                .end()
                .map_err(scan_error)?;
        }
        task.row = rows;
        task.frame = None;
    }
    let file = Arc::clone(&task.file);
    let mut handle = task.handle.take();
    let storage = Arc::clone(&task.storage);
    let partition = u32::try_from(task.partition).map_err(|_| scan_error("spill partition"))?;
    let depth = task.depth;
    let stats = shared.stats.clone();
    let job_stats = stats.clone();
    match io.submit_observed(
        stats.as_ref(),
        "io_read_end",
        depth,
        Box::new(move || {
            let _stats =
                crate::query_stats::Timer::new(&job_stats, crate::query_stats::Stage::SpillRead);
            let mut h = match handle.take() {
                Some(h) => h,
                None => file.open_at_level(depth)?,
            };
            let frame = storage
                .workspace
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .read(&mut h, partition, depth)?;
            Ok(Completion::Read { frame, handle: h })
        }),
    ) {
        Submit::Queued(ticket) => {
            task.ticket = Some(ticket);
            Ok(Step::Yield)
        }
        _ => Err(scan_error("spill read queue admission")),
    }
}
fn files_for_repartition(
    storage: &Arc<Storage>,
    shared: &Arc<Shared>,
    io: &IoService<Completion>,
    parent: Arc<SpillFile>,
    partition: usize,
    depth: u8,
    cancel: Option<&ScanCancel>,
) -> Result<Files, ScanError> {
    shared
        .manager
        .finish(0)
        .map_err(|_| scan_error("repartition pressure cleanup"))?;
    let output = Workspace::new(&shared.manager).map_err(scan_error)?;
    let children = files(&storage.registry, PARTITIONS, depth + 1)?;
    let storage = Arc::clone(storage);
    let child_files = children.clone();
    let stats = shared.stats.clone();
    let shared = Arc::clone(shared);
    let ticket = match io.submit_observed(
        stats.as_ref(),
        "io_repartition_end",
        depth,
        Box::new(move || {
            let a = shared
                .plan
                .aggregate
                .as_ref()
                .ok_or_else(|| std::io::Error::other("missing aggregate"))?;
            storage.repartition(parent, &child_files, a, partition, depth, output)?;
            Ok(Completion::Written)
        }),
    ) {
        Submit::Queued(t) => t,
        _ => return Err(scan_error("repartition queue admission")),
    };
    cooperative::run_observed(
        stats.as_ref(),
        vec![ticket],
        1,
        |ticket, wake| {
            if stopped(cancel) {
                io.cancel();
                return Step::Done(Err(cancel_error()));
            }
            match ticket.poll(wake) {
                Some(r) => Step::Done(r.map(|_| ()).map_err(scan_error)),
                None => Step::Blocked,
            }
        },
        |_| Ok(()),
    )?;
    Ok(children)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scan_admission_leaves_table_chunk_headroom() {
        assert_eq!(Table::chunk_size(0), 4096);
        assert_eq!(Table::chunk_size(64 << 20), 256 << 10);
        assert_eq!(Table::chunk_size(8 << 30), 1 << 20);
        // SF1 HC, 64 MiB, after the piecewise footer decoder. Cursor-only
        // admission chose three and caused hundreds of thousands of IPC opens.
        let (free, cursor, budget) = (62_887_682, 19_607_401, 64 << 20);
        assert_eq!(scan_concurrency(8, 49, free, cursor, budget), 2);
        assert_eq!(scan_concurrency(1, 49, free, cursor, budget), 1);
        assert_eq!(scan_concurrency(8, 1, free, cursor, budget), 1);
        assert_eq!(scan_concurrency(8, 49, 7 << 30, cursor, 8 << 30), 8);
        assert_eq!(scan_concurrency(64, 100, 7 << 30, cursor, 8 << 30), 32);
        // Headroom is a concurrency policy, not a new minimum-set rejection.
        assert_eq!(
            scan_concurrency(8, 49, cursor + (1 << 20), cursor, budget),
            1
        );
        assert_eq!(scan_concurrency(0, 0, 0, 0, budget), 1);
        let per_task = cursor + PARTITIONS * Table::chunk_size(budget);
        let boundary = 2 * per_task + (1 << 20);
        assert_eq!(scan_concurrency(8, 49, boundary, cursor, budget), 2);
        assert_eq!(scan_concurrency(8, 49, boundary - 1, cursor, budget), 1);
    }

    #[test]
    fn resident_output_workspace_excludes_strings_even_inside_fixed_width_expressions() {
        use nata_parquet::schema::Kind;
        use nata_sql::{plan_sql, source::MemSource};
        let source = MemSource::new().file(
            "t.parquet",
            &[
                (
                    "id",
                    Ok(Kind::Int {
                        bits: 32,
                        signed: true,
                    }),
                ),
                ("s", Ok(Kind::Utf8)),
            ],
        );
        for (sql, expected) in [
            ("SELECT id, count(*) FROM 't.parquet' GROUP BY id", true),
            ("SELECT s, count(*) FROM 't.parquet' GROUP BY s", false),
            ("SELECT count(*) FROM 't.parquet' GROUP BY s", false),
            ("SELECT id, min(s) FROM 't.parquet' GROUP BY id", false),
            (
                "SELECT CAST(id AS VARCHAR), count(*) FROM 't.parquet' GROUP BY id",
                false,
            ),
            (
                "SELECT CAST(CAST(id AS VARCHAR) AS INTEGER), count(*) FROM 't.parquet' GROUP BY id",
                false,
            ),
            (
                "SELECT id, count(*) FROM 't.parquet' GROUP BY id HAVING CAST(id AS VARCHAR) <> ''",
                false,
            ),
        ] {
            let plan = plan_sql(sql, &source).expect("plan").plan;
            assert_eq!(
                fixed_width_output(&plan, plan.aggregate.as_ref().expect("aggregate")),
                expected,
                "{sql}"
            );
        }
    }

    #[test]
    fn resident_adoption_keeps_grants_without_merge_overlap() {
        use nata_sql::{Pos, bind::AggregateExpr, types::SqlType};
        let manager = MemoryManager::new(8 << 20, 4096, 1).expect("manager");
        let mut a = Aggregate {
            keys: vec![],
            aggregates: vec![AggregateExpr {
                name: "min".into(),
                arg: None,
                ty: SqlType::Varchar,
                pos: Pos::NONE,
            }],
            having: None,
        };
        let mut table = Table::new();
        table
            .reserve_bytes(&manager, 6 << 20)
            .expect("state charge");
        let text = "x".repeat(1 << 20);
        let ptr = text.as_ptr();
        table
            .groups
            .insert(vec![], vec![State::Extreme(Atom::Str(text))]);
        let partitions = vec![vec![], vec![table]];
        assert!(resident_merge_needs_spill(&partitions, 8 << 20, false));
        let before = manager.snapshot();
        let remaining = grant(&manager, before.budget - before.progress - before.held)
            .expect("occupy all remaining workspace");
        let before = manager.snapshot();
        let tables = partitions.into_iter().nth(1).expect("partition");
        let out = merge(tables, &a, &manager, &|| false).expect("adopt");
        assert_eq!(manager.snapshot().requests, before.requests);
        assert_eq!(manager.snapshot().held, before.held);
        let State::Final(Atom::Str(value)) = &out.groups.entries[0].1[0] else {
            panic!("final string");
        };
        assert_eq!(value.as_ptr(), ptr, "extremum moved without a copy");
        assert!(
            merge(vec![out], &a, &manager, &|| true).is_err(),
            "cancel finalization"
        );
        drop(remaining);
        a.aggregates[0].name = "count".into();
        a.aggregates[0].ty = SqlType::BigInt;
        let mut overflow = Table::new();
        overflow.reserve_bytes(&manager, 4096).expect("charge");
        overflow.groups.insert(vec![], vec![State::Count(u64::MAX)]);
        assert!(
            merge(vec![overflow], &a, &manager, &|| false).is_err(),
            "final overflow"
        );
        assert_eq!(
            (manager.snapshot().held, manager.snapshot().live_grants),
            (0, 0)
        );
    }

    #[test]
    fn resident_multiple_inputs_keep_the_pressure_fallback() {
        let manager = MemoryManager::new(16 << 20, 4096, 1).expect("manager");
        let mut table = Table::new();
        table
            .reserve_bytes(&manager, 6 << 20)
            .expect("state charge");
        let partitions = vec![vec![Table::new(), table]];
        assert!(resident_merge_needs_spill(&partitions, 16 << 20, true));
        assert!(!resident_merge_needs_spill(&partitions, 32 << 20, true));
        let boundary = 3 * partitions[0][1].grants.reserved() + (3 << 20);
        assert!(!resident_merge_needs_spill(&partitions, boundary, true));
        assert!(resident_merge_needs_spill(&partitions, boundary - 1, true));
        assert!(!resident_merge_needs_spill(&[], 0, true));
        drop(partitions);
        assert_eq!(manager.snapshot().held, 0);
    }

    #[test]
    fn fanout_estimator_overflow_clamps_and_merge_grants() {
        assert_eq!(estimate_resident(1, 1, 3).expect("estimate"), 1216);
        for (rows, keys, states) in [(usize::MAX, 1, 1), (1, usize::MAX, 0), (1, 0, usize::MAX)] {
            assert!(estimate_resident(rows, keys, states).is_err());
        }
        assert_eq!(estimate_resident(0, 1, 3).expect("empty"), 0);
        assert_eq!(initial_fanout(0, 0), 16);
        assert_eq!(initial_fanout(16 * 50, 100), 16);
        assert_eq!(initial_fanout(16 * 50 + 1, 100), 32);
        assert_eq!(initial_fanout(usize::MAX, 0), 256);
        assert_eq!(initial_fanout(256 * 50, 100), 256);
        for f in [16, 32, 64, 128, 256] {
            assert_eq!(initial_fanout(f * 1024, 2048), f);
        }
        assert_eq!(merge_concurrency(8, 64 << 20, 1 << 20), 8);
        assert_eq!(merge_concurrency(8, 64 << 20, 8 << 20), 4);
        assert_eq!(merge_concurrency(8, 64 << 20, 32 << 20), 1);
        assert_eq!(merge_concurrency(0, 0, usize::MAX), 1);
        assert_eq!(merge_concurrency(32, 256 << 20, 0), 16);
        assert_eq!(merge_concurrency(8, 8 << 20, 0), 2);
    }
    #[test]
    fn dictionary_scratch_admission_precedes_revocable_tables() {
        let manager = MemoryManager::new(2 << 20, 4096, 1).expect("manager");
        let mut scratch = Scratch::admitted(&manager, true).expect("admission");
        let before = manager.snapshot();
        assert_eq!(
            before.held,
            Scratch::MINIMUM,
            "admission includes chunk metadata"
        );
        let table = grant(&manager, before.budget - before.progress - before.held)
            .expect("table borrows remainder");
        // Exactly the fresh fixture's upper bound: 256 short strings, 13
        // dictionary values, multiplied by its expression scratch allowance.
        assert!(
            scratch.reserve(&manager, 60 << 10),
            "a peer table must not starve dictionary evaluation"
        );
        assert_eq!(manager.snapshot().pending, 0);
        drop(table);
        drop(scratch);
        let after = manager.snapshot();
        assert_eq!((after.held, after.live_grants, after.pending), (0, 0, 0));
        assert!(after.peak <= after.budget);
    }
    #[test]
    fn competing_scratch_requests_remain_retriable_pressure() {
        let manager = MemoryManager::new(1024, 128, 1).expect("manager");
        let occupied = grant(&manager, 700).expect("occupied");
        assert!(matches!(
            MemoryManager::request(&manager, 0, 512).expect("queued"),
            Reservation::Pending { .. }
        ));
        let mut scratch = Scratch::default();
        // Another task of this operator has a different pending request. As
        // before caching, this is pressure: let the caller spill and retry.
        assert!(!scratch.reserve(&manager, 128));
        drop(occupied);
        manager.finish(0).expect("pressure cleanup");
        assert!(scratch.reserve(&manager, 128));
        drop(scratch);
        let snapshot = manager.snapshot();
        assert_eq!(
            (snapshot.held, snapshot.live_grants, snapshot.pending),
            (0, 0, 0)
        );
        assert!(snapshot.peak <= snapshot.budget);
    }
    #[test]
    fn reusable_scratch_charges_capacity_once_and_releases_every_chunk() {
        let manager = MemoryManager::new(1 << 20, 4096, 1).expect("manager");
        let mut scratch = Scratch::default();
        for _ in 0..100 {
            assert!(scratch.reserve(&manager, 1024));
        }
        assert_eq!(manager.snapshot().requests, 1);
        assert!(manager.snapshot().held >= 64 << 10);
        assert!(scratch.reserve(&manager, 4096));
        assert_eq!(manager.snapshot().requests, 1);
        assert!(scratch.reserve(&manager, 128 << 10));
        assert_eq!(manager.snapshot().requests, 2);
        assert!(manager.snapshot().peak <= manager.snapshot().budget);
        drop(scratch);
        let s = manager.snapshot();
        assert_eq!((s.held, s.live_grants, s.pending), (0, 0, 0));
    }
}
