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
    _decode: Grant,
}
fn empty_tables() -> Vec<Table> {
    (0..PARTITIONS).map(|_| Table::new()).collect()
}
fn files(registry: &Arc<Registry>) -> Result<Files, ScanError> {
    (0..PARTITIONS)
        .map(|_| registry.create().map_err(scan_error))
        .collect()
}
fn submit_write(
    state: &mut ScanState,
    storage: &Arc<Storage>,
    files: &Files,
    io: &IoService<Completion>,
    wake: Arc<dyn Wake>,
) -> Result<(), ScanError> {
    let tables = std::mem::replace(&mut state.partial.tables, empty_tables());
    let storage = Arc::clone(storage);
    let files = files.clone();
    let shared = Arc::clone(&state.partial.shared);
    let job = Box::new(move || {
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
    match io.submit(job) {
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
    let runtime =
        Arc::new(grant(&shared.manager, 2 << 20).map_err(|e| ScanError::new(e.into_kind()))?);
    let mut options = shared.plan.scan.scan_options();
    options.late_materialization = cfg.late_materialization;
    if shared.plan.scan.columns.is_empty() {
        options.columns = Some(vec![crate::exec::first_readable_column(&shared.plan)?]);
    }
    let scan = Arc::new(ScanPlan::open_budgeted(
        &shared.plan.scan.files,
        &options,
        cfg.opener
            .clone()
            .unwrap_or_else(|| Arc::new(nata_parquet::FsOpener)),
        None,
        None,
        Some(Arc::clone(&shared.manager)),
    )?);
    observed.stats = Some(Arc::clone(scan.stats()));
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
                .and_then(|n| n.checked_add(256 * 8))
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
    let storage = Arc::new(Storage {
        registry,
        runtime,
        cancel: cancel.cloned(),
        hash: shared.hash,
        partial_rows: Arc::default(),
        largest_partial: Arc::default(),
        rehashes: Arc::default(),
        workspace: Mutex::new(Workspace::new(&shared.manager).map_err(scan_error)?),
    });
    let before_decode = shared.manager.snapshot();
    let free = before_decode
        .budget
        .saturating_sub(before_decode.progress)
        .saturating_sub(before_decode.held);
    let n = cfg
        .threads
        .max(1)
        .min(tasks.len().max(1))
        .min(32)
        .min((free.saturating_sub(1 << 20) / decode_bytes.max(1)).max(1))
        .max(1);
    let io = IoService::new(32, &shared.manager).map_err(scan_error)?;
    let targets = files(&storage.registry)?;
    let tasks = Mutex::new(tasks);
    let mut states = Vec::new();
    for _ in 0..n {
        let decode =
            grant(&shared.manager, decode_bytes).map_err(|e| ScanError::new(e.into_kind()))?;
        states.push(ScanState {
            partial: Partial {
                shared: Arc::clone(shared),
                tables: empty_tables(),
            },
            cursor: None,
            evaluated: None,
            input: None,
            ticket: None,
            finishing: false,
            pressure_since: None,
            source: None,
            _decode: decode,
        });
    }
    let results: Mutex<Vec<Vec<Table>>> = Mutex::new(Vec::new());
    cooperative::run(
        states,
        n,
        |state, wake| {
            if stopped(cancel) {
                io.cancel();
                return Step::Done(Err(cancel_error()));
            }
            let start = Instant::now();
            let step = (|| -> Result<Step<Vec<Table>, ScanError>, ScanError> {
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
                    return Ok(Step::Done(Ok(empty_tables())));
                }
                if state.finishing {
                    return Ok(Step::Done(Ok(std::mem::replace(
                        &mut state.partial.tables,
                        empty_tables(),
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
                            state.partial.tables = empty_tables();
                            shared
                                .manager
                                .finish(0)
                                .map_err(|_| scan_error("memory pressure cleanup"))?;
                            shared
                                .manager
                                .park(0, wake.clone())
                                .map_err(|_| scan_error("memory pressure park"))?;
                            return Ok(Step::Blocked);
                        }
                        submit_write(state, &storage, &targets, &io, wake.clone())?;
                        return Ok(Step::Yield);
                    }
                    state.pressure_since = None;
                    state.evaluated = None;
                    if cfg.aggregate_force_spill || shared.manager.revoked(0).unwrap_or(false) {
                        submit_write(state, &storage, &targets, &io, wake.clone())?;
                        return Ok(Step::Yield);
                    }
                }
                if let Some(batch) = &state.input {
                    let bytes = string_scratch(batch, &shared.plan)
                        .map_err(|e| ScanError::new(e.into_kind()))?;
                    let strings = if bytes == 0 {
                        None
                    } else {
                        match MemoryManager::request(&shared.manager, 0, bytes) {
                            Ok(Reservation::Granted(g)) => Some(g),
                            _ if state.partial.tables.iter().any(|t| !t.groups.is_empty()) => {
                                submit_write(state, &storage, &targets, &io, wake.clone())?;
                                return Ok(Step::Yield);
                            }
                            _ => {
                                return Err(scan_error(
                                    "resource error: aggregate string expression minimum working set exceeds budget",
                                ));
                            }
                        }
                    };
                    let mut evaluated = state
                        .partial
                        .evaluate(batch)
                        .map_err(|e| ScanError::new(e.into_kind()))?;
                    evaluated._strings = strings;
                    state.evaluated = Some(evaluated);
                    state.input = None;
                    return Ok(Step::Yield);
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
                        return Ok(Step::Done(Ok(empty_tables())));
                    }
                    state.source = Some((index, task.path().to_path_buf(), task.row_group()));
                    state.cursor = Some(task.cursor(256)?);
                }
                if let Some(cursor) = &mut state.cursor {
                    if let Some(batch) = cursor.next_batch()? {
                        state.input = Some(batch);
                    } else {
                        state.cursor = None;
                    }
                }
                Ok(Step::Yield)
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
                    Step::Done(Ok(empty_tables()))
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
    let resident_bytes: usize = partitions
        .iter()
        .flatten()
        .map(|t| t.grants.reserved())
        .sum();
    let partitions = if cfg.aggregate_force_spill
        || storage.registry.snapshot().bytes_written > 0
        || resident_bytes.saturating_mul(3)
            > shared.manager.snapshot().budget.saturating_sub(3 << 20)
    {
        // Flush remaining resident contributions through the same continuation
        // seam. No partial is published before all writes have succeeded.
        let states = partitions
            .into_iter()
            .enumerate()
            .flat_map(|(p, ts)| ts.into_iter().map(move |t| (p, t)))
            .collect::<Vec<_>>();
        for (p, t) in states {
            let mut tables = empty_tables();
            tables[p] = t;
            let storage = Arc::clone(&storage);
            let files = targets.clone();
            let shared = Arc::clone(shared);
            let ticket = match io.submit(Box::new(move || {
                storage.write(
                    tables,
                    &files,
                    shared.plan.aggregate.as_ref().expect("aggregate"),
                    0,
                )?;
                Ok(Completion::Written)
            })) {
                Submit::Queued(t) => t,
                _ => return Err(scan_error("final spill queue admission")),
            };
            cooperative::run(
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
    let concurrency = cfg
        .threads
        .clamp(1, 16)
        .min((shared.manager.snapshot().budget / (4 << 20)).max(1));
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
                    partial_rows: Arc::clone(&storage.partial_rows),
                    largest_partial: Arc::clone(&storage.largest_partial),
                    rehashes: Arc::clone(&storage.rehashes),
                    workspace: Mutex::new(workspace),
                }),
                limit: cfg.aggregate_merge_partition_bytes.unwrap_or(
                    shared.manager.snapshot().budget.saturating_sub(3 << 20) / concurrency / 2,
                ),
            });
        }
        cooperative::run(
            tasks,
            concurrency,
            |task, wake| {
                if stopped(cancel) {
                    io.cancel();
                    return Step::Done(Err(cancel_error()));
                }
                let start = Instant::now();
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
                    for states in task.table.groups.values_mut() {
                        if task
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
        while task.row < frame.batch().num_rows() {
            let (key, state) =
                storage::record(frame, task.row, a, task.partition, task.depth, shared.hash)
                    .map_err(scan_error)?;
            let previous = task.table.groups.get(&key);
            let string_bytes = state
                .iter()
                .enumerate()
                .map(|(slot, s)| {
                    if let State::Extreme(Atom::Str(s)) = s {
                        let old = previous.and_then(|p| p.get(slot)).map_or(0, |s| {
                            if let State::Extreme(Atom::Str(s)) = s {
                                s.len()
                            } else {
                                0
                            }
                        });
                        s.len().saturating_sub(old)
                    } else {
                        0
                    }
                })
                .sum();
            if task
                .table
                .reserve_bytes(&shared.manager, string_bytes)
                .is_err()
            {
                return Ok(Step::Done(Ok(Merged::Repartition {
                    file: Arc::clone(&task.file),
                    partition: task.partition,
                    depth: task.depth,
                })));
            }
            if let Some(dest) = task.table.groups.get_mut(&key) {
                for ((dest, source), a) in dest.iter_mut().zip(state).zip(&a.aggregates) {
                    dest.merge(source, a)
                        .map_err(|e| ScanError::new(e.into_kind()))?;
                }
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
                task.table.groups.insert(key, state);
            }
            task.row += 1;
        }
        task.frame = None;
    }
    let file = Arc::clone(&task.file);
    let mut handle = task.handle.take();
    let storage = Arc::clone(&task.storage);
    let partition = u32::try_from(task.partition).map_err(|_| scan_error("spill partition"))?;
    let depth = task.depth;
    match io.submit(Box::new(move || {
        let mut h = match handle.take() {
            Some(h) => h,
            None => file.open()?,
        };
        let frame = storage
            .workspace
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .read(&mut h, partition, depth)?;
        Ok(Completion::Read { frame, handle: h })
    })) {
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
    let children = files(&storage.registry)?;
    let storage = Arc::clone(storage);
    let child_files = children.clone();
    let shared = Arc::clone(shared);
    let ticket = match io.submit(Box::new(move || {
        let a = shared
            .plan
            .aggregate
            .as_ref()
            .ok_or_else(|| std::io::Error::other("missing aggregate"))?;
        storage.repartition(parent, &child_files, a, partition, depth, output)?;
        Ok(Completion::Written)
    })) {
        Submit::Queued(t) => t,
        _ => return Err(scan_error("repartition queue admission")),
    };
    cooperative::run(
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
