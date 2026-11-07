//! In-memory breaker: one partial sink per morsel, disjoint hash partitions,
//! then independent final tasks. State grants move with their allocations.
use crate::error::{EvalError, R};
use crate::eval::Evaluator;
use crate::exec::{ExecConfig, ExecReport, Observed, OperatorStats};
use crate::value::{Col, Data, StrData};
use arrow_array::RecordBatch;
use hashbrown::HashTable;
use nata_kernels::aggregate::ExactSum;
use nata_parquet::{
    ErrorKind, ParallelConfig, ScanCancel, ScanError, ScanPlan, TaskSink, TaskSinkFactory,
    run_parallel_with,
};
use nata_sched::memory::{Grant, MemoryManager, Reservation};
use nata_sql::bind::{Aggregate, AggregateExpr};
use nata_sql::plan::LogicalPlan;
use nata_sql::types::SqlType;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PARTITIONS: usize = 16;
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Atom {
    Null,
    Bool(bool),
    Int(i64),
    Dec(i128),
    Float(u64),
    Str(String),
    Date(i32),
}
impl Atom {
    fn get(c: &Col, i: usize) -> Self {
        if !c.valid[i] {
            return Self::Null;
        }
        match &c.data {
            Data::Bool(v) => Self::Bool(v[i]),
            Data::Int(v) => Self::Int(v[i]),
            Data::Dec(v) => Self::Dec(v[i]),
            Data::Date(v) => Self::Date(v[i]),
            Data::Str(v) => Self::Str(v.get(i).into()),
            Data::F32(v) => Self::float(f64::from(v[i])),
            Data::F64(v) => Self::float(v[i]),
        }
    }
    fn float(v: f64) -> Self {
        Self::Float(if v.is_nan() {
            f64::NAN.to_bits()
        } else if v == 0.0 {
            0
        } else {
            v.to_bits()
        })
    }
    fn compare(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Self::Bool(a), Self::Bool(b)) => a.cmp(b),
            (Self::Int(a), Self::Int(b)) => a.cmp(b),
            (Self::Dec(a), Self::Dec(b)) => a.cmp(b),
            (Self::Date(a), Self::Date(b)) => a.cmp(b),
            (Self::Str(a), Self::Str(b)) => a.cmp(b),
            (Self::Float(a), Self::Float(b)) => f64::from_bits(*a).total_cmp(&f64::from_bits(*b)),
            _ => std::cmp::Ordering::Equal,
        }
    }
    #[allow(
        clippy::cast_possible_truncation,
        reason = "REAL states were widened exactly from f32; canonical zero/NaN also round trip"
    )]
    fn write(&self, c: &mut Col, i: usize) {
        if matches!(self, Self::Null) {
            return;
        }
        c.valid[i] = true;
        match (self, &mut c.data) {
            (Self::Bool(v), Data::Bool(d)) => d[i] = *v,
            (Self::Int(v), Data::Int(d)) => d[i] = *v,
            (Self::Dec(v), Data::Dec(d)) => d[i] = *v,
            (Self::Date(v), Data::Date(d)) => d[i] = *v,
            (Self::Str(v), Data::Str(StrData::Owned(d))) => d[i].clone_from(v),
            (Self::Float(v), Data::F64(d)) => d[i] = f64::from_bits(*v),
            (Self::Float(v), Data::F32(d)) => d[i] = f64::from_bits(*v) as f32,
            _ => {}
        }
    }
}
#[derive(Debug, Clone)]
enum State {
    Count(u64),
    Sum(ExactSum),
    Extreme(Atom),
    Final(Atom),
}
fn overflow(a: &AggregateExpr) -> EvalError {
    EvalError::at(a.pos, format!("{} overflow", a.name))
}
impl State {
    fn new(a: &AggregateExpr) -> Self {
        match a.name.as_str() {
            "count" => Self::Count(0),
            "sum" | "avg" => Self::Sum(ExactSum::default()),
            _ => Self::Extreme(Atom::Null),
        }
    }
    fn add(&mut self, value: Atom, a: &AggregateExpr) -> R<()> {
        match self {
            Self::Count(n) if a.arg.is_none() || value != Atom::Null => {
                *n = n
                    .checked_add(1)
                    .filter(|n| *n <= i64::MAX as u64)
                    .ok_or_else(|| overflow(a))?;
            }
            Self::Sum(sum) => match value {
                Atom::Int(v) => sum.add(i128::from(v)).map_err(|_| overflow(a))?,
                Atom::Dec(v) => sum.add(v).map_err(|_| overflow(a))?,
                _ => {}
            },
            Self::Extreme(v) if value != Atom::Null => {
                let cmp = value.compare(v);
                if *v == Atom::Null
                    || (a.name == "min" && cmp.is_lt())
                    || (a.name == "max" && cmp.is_gt())
                {
                    *v = value;
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn merge(&mut self, other: State, a: &AggregateExpr) -> R<()> {
        match (self, other) {
            (Self::Count(n), Self::Count(m)) => {
                *n = n
                    .checked_add(m)
                    .filter(|n| *n <= i64::MAX as u64)
                    .ok_or_else(|| overflow(a))?
            }
            (Self::Sum(n), Self::Sum(m)) => n.merge(m).map_err(|_| overflow(a))?,
            (s @ Self::Extreme(_), Self::Extreme(v)) => s.add(v, a)?,
            _ => return Err(EvalError::internal("incompatible aggregate states")),
        }
        Ok(())
    }
    fn finish(&self, a: &AggregateExpr) -> R<Atom> {
        Ok(match self {
            Self::Count(n) => Atom::Int(i64::try_from(*n).map_err(|_| overflow(a))?),
            Self::Extreme(v) | Self::Final(v) => v.clone(),
            Self::Sum(s) if a.name == "avg" => {
                let (p, scale) = a
                    .arg
                    .as_ref()
                    .and_then(|a| a.ty.as_decimal())
                    .ok_or_else(|| overflow(a))?;
                s.average(p, scale)
                    .map_err(|_| overflow(a))?
                    .map_or(Atom::Null, Atom::Dec)
            }
            Self::Sum(s) if a.ty == SqlType::BigInt => s
                .integer_sum()
                .map_err(|_| overflow(a))?
                .map_or(Atom::Null, Atom::Int),
            Self::Sum(s) => s
                .decimal_sum(38)
                .map_err(|_| overflow(a))?
                .map_or(Atom::Null, Atom::Dec),
        })
    }
}
fn resource() -> EvalError {
    EvalError::at(
        nata_sql::Pos::NONE,
        "resource error: aggregate memory budget exceeded; in-memory step B has no spill (step C)",
    )
}
fn grant(manager: &Arc<MemoryManager>, bytes: usize) -> R<Grant> {
    match MemoryManager::request(manager, 0, bytes.max(1)) {
        Ok(Reservation::Granted(g)) => Ok(g),
        _ => {
            manager.cancel();
            Err(resource())
        }
    }
}
fn key_hash(key: &[Atom]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    h.finish()
}
/// Reused probing table of group IDs, with owned typed key/state arenas.
struct Groups {
    hash: fn(&[Atom]) -> u64,
    index: HashTable<usize>,
    entries: Vec<(Vec<Atom>, Vec<State>)>,
}
impl Groups {
    fn new() -> Self {
        Self {
            hash: key_hash,
            index: HashTable::new(),
            entries: Vec::new(),
        }
    }
    fn len(&self) -> usize {
        self.entries.len()
    }
    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    fn capacity(&self) -> usize {
        self.index.capacity().min(self.entries.capacity())
    }
    fn contains_key(&self, key: &[Atom]) -> bool {
        self.index
            .find((self.hash)(key), |&i| self.entries[i].0 == key)
            .is_some()
    }
    fn get_mut(&mut self, key: &[Atom]) -> Option<&mut Vec<State>> {
        let i = *self
            .index
            .find((self.hash)(key), |&i| self.entries[i].0 == key)?;
        Some(&mut self.entries[i].1)
    }
    fn insert(&mut self, key: Vec<Atom>, states: Vec<State>) {
        self.index
            .insert_unique((self.hash)(&key), self.entries.len(), |&i| {
                (self.hash)(&self.entries[i].0)
            });
        self.entries.push((key, states));
    }
    fn try_reserve(&mut self, n: usize) -> Result<(), ()> {
        self.index
            .try_reserve(n, |&i| (self.hash)(&self.entries[i].0))
            .map_err(|_| ())?;
        self.entries.try_reserve(n).map_err(|_| ())
    }
    fn drain(&mut self) -> impl Iterator<Item = (Vec<Atom>, Vec<State>)> + '_ {
        self.entries.drain(..)
    }
    fn values_mut(&mut self) -> impl Iterator<Item = &mut Vec<State>> {
        self.entries.iter_mut().map(|(_, s)| s)
    }
}
impl IntoIterator for Groups {
    type Item = (Vec<Atom>, Vec<State>);
    type IntoIter = std::vec::IntoIter<Self::Item>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}
struct Table {
    // Fields drop in order: allocations before their grants.
    groups: Groups,
    grants: Vec<Grant>,
}
impl Table {
    fn new() -> Self {
        Self {
            groups: Groups::new(),
            grants: Vec::new(),
        }
    }
    fn reserve(&mut self, manager: &Arc<MemoryManager>, key: &[Atom], a: &Aggregate) -> R<()> {
        let bytes = 256usize
            .saturating_add(key.len().saturating_mul(64))
            .saturating_add(a.aggregates.len().saturating_mul(128))
            .saturating_add(
                key.iter()
                    .map(|v| if let Atom::Str(s) = v { s.len() } else { 0 })
                    .sum::<usize>(),
            );
        // Charge table growth (including rehash overlap), per-entry allocations,
        // and reservation metadata before allocating them. Conservative retained
        // charges are intentional until step C's reusable allocation ledger.
        let g = grant(manager, bytes)?;
        if self.groups.len() == self.groups.capacity() {
            let capacity = self.groups.capacity().max(4);
            let growth = grant(manager, capacity.saturating_mul(128))?;
            self.groups.try_reserve(capacity).map_err(|_| resource())?;
            self.grants.push(growth);
        }
        self.grants.push(g);
        Ok(())
    }
}
struct Shared {
    plan: Arc<LogicalPlan>,
    manager: Arc<MemoryManager>,
    partials: Mutex<Vec<Vec<Table>>>,
}
struct Partial {
    shared: Arc<Shared>,
    tables: Vec<Table>,
}
impl TaskSink for Partial {
    fn push(&mut self, batch: RecordBatch) -> Result<(), ErrorKind> {
        self.batch(batch).map_err(EvalError::into_kind)
    }
    fn finish(self: Box<Self>) -> Result<(), ErrorKind> {
        let tables = self.tables;
        self.shared
            .partials
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(tables);
        Ok(())
    }
}
impl Partial {
    fn batch(&mut self, batch: RecordBatch) -> R<()> {
        let depth = crate::exec::max_depth(&self.shared.plan);
        if depth <= 64 {
            return self.batch_here(batch);
        }
        std::thread::scope(|scope| {
            std::thread::Builder::new()
                .name("nata-aggregate-eval".into())
                .stack_size(depth.saturating_mul(32 * 1024).saturating_add(8 << 20))
                .spawn_scoped(scope, || self.batch_here(batch))
                .map_err(|_| resource())?
                .join()
                .unwrap_or_else(|_| Err(EvalError::internal("aggregate evaluation panic")))
        })
    }
    fn batch_here(&mut self, batch: RecordBatch) -> R<()> {
        let plan = &self.shared.plan;
        let a = plan
            .aggregate
            .as_ref()
            .ok_or_else(|| EvalError::internal("aggregate missing"))?;
        // Evaluator scratch is granted before evaluation; scan decode remains
        // covered by the existing scan window until step C.
        let scratch_bytes =
            batch
                .get_array_memory_size()
                .saturating_mul(4)
                .saturating_add(batch.num_rows().saturating_mul(128).saturating_mul(
                    a.keys.len() + a.aggregates.len() + plan.scan.columns.len() + 1,
                ));
        let _scratch = grant(&self.shared.manager, scratch_bytes)?;
        let types: Vec<_> = plan.scan.columns.iter().map(|c| c.ty).collect();
        let ev = Evaluator::new(&batch, &types);
        let rows = if let Some(f) = &plan.filter {
            ev.keep_mask(&f.predicate)?
                .into_iter()
                .enumerate()
                .filter(|(_, k)| *k)
                .map(|(i, _)| u32::try_from(i).map_err(|_| resource()))
                .collect::<R<Vec<_>>>()?
        } else {
            ev.all_rows()
        };
        let keys = a
            .keys
            .iter()
            .map(|e| ev.eval(e, &rows))
            .collect::<R<Vec<_>>>()?;
        let args = a
            .aggregates
            .iter()
            .map(|a| a.arg.as_ref().map(|e| ev.eval(e, &rows)).transpose())
            .collect::<R<Vec<_>>>()?;
        for r in rows {
            let i = r as usize;
            let key: Vec<_> = keys.iter().map(|c| Atom::get(c, i)).collect();
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            key.hash(&mut hash);
            let p = usize::try_from(hash.finish() & 15).map_err(|_| resource())?;
            let table = &mut self.tables[p];
            if !table.groups.contains_key(&key) {
                table.reserve(&self.shared.manager, &key, a)?;
                table
                    .groups
                    .insert(key.clone(), a.aggregates.iter().map(State::new).collect());
            }
            if let Some(states) = table.groups.get_mut(&key) {
                for ((state, agg), arg) in states.iter_mut().zip(&a.aggregates).zip(&args) {
                    let value = arg.as_ref().map_or(Atom::Null, |c| Atom::get(c, i));
                    if let (State::Extreme(old), Atom::Str(new)) = (&*state, &value) {
                        let previous = if let Atom::Str(s) = old { s.len() } else { 0 };
                        if new.len() > previous {
                            table
                                .grants
                                .push(grant(&self.shared.manager, new.len() - previous + 64)?);
                        }
                    }
                    state.add(value, agg)?;
                }
            }
        }
        Ok(())
    }
}
fn merge(
    mut tables: Vec<Table>,
    a: &Aggregate,
    manager: &Arc<MemoryManager>,
    cancelled: &dyn Fn() -> bool,
) -> R<Table> {
    tables.sort_by_key(|t| t.groups.len());
    let mut out = tables.pop().unwrap_or_else(Table::new);
    for mut table in tables {
        for (key, states) in table.groups.drain() {
            if cancelled() {
                return Err(EvalError::at(nata_sql::Pos::NONE, "cancelled"));
            }
            if let Some(dest) = out.groups.get_mut(&key) {
                for ((dest, source), a) in dest.iter_mut().zip(states).zip(&a.aggregates) {
                    dest.merge(source, a)?;
                }
            } else {
                out.reserve(manager, &key, a)?;
                out.groups.insert(key, states);
            }
        }
        out.grants.append(&mut table.grants);
    }
    for states in out.groups.values_mut() {
        if cancelled() {
            return Err(EvalError::at(nata_sql::Pos::NONE, "cancelled"));
        }
        for (s, a) in states.iter_mut().zip(&a.aggregates) {
            *s = State::Final(s.finish(a)?);
        }
    }
    Ok(out)
}
fn batch(rows: &[(Vec<Atom>, Vec<State>)], a: &Aggregate) -> R<RecordBatch> {
    let types = a
        .keys
        .iter()
        .map(|k| k.ty)
        .chain(a.aggregates.iter().map(|a| a.ty));
    let mut columns = Vec::new();
    for (i, ty) in types.enumerate() {
        let mut c = Col::empty(ty, rows.len());
        for (r, (key, states)) in rows.iter().enumerate() {
            if i < key.len() {
                key[i].write(&mut c, r);
            } else if let State::Final(v) = &states[i - key.len()] {
                v.write(&mut c, r);
            }
        }
        columns.push((format!("slot{i}"), c.to_arrow()?));
    }
    if columns.is_empty() {
        return RecordBatch::try_new_with_options(
            Arc::new(arrow_schema::Schema::empty()),
            vec![],
            &arrow_array::RecordBatchOptions::new().with_row_count(Some(rows.len())),
        )
        .map_err(|_| EvalError::internal("aggregate output"));
    }
    RecordBatch::try_from_iter(columns).map_err(|_| EvalError::internal("aggregate output"))
}
fn default_budget() -> usize {
    let physical = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|n| n.parse::<usize>().ok())
        })
        .map(|n| n.saturating_mul(1024));
    let mut limits = Vec::new();
    if let Ok(membership) = std::fs::read_to_string("/proc/self/cgroup") {
        for line in membership.lines() {
            let fields: Vec<_> = line.splitn(3, ':').collect();
            if fields.len() != 3 {
                continue;
            }
            let (root, file) = if fields[0] == "0" && fields[1].is_empty() {
                (std::path::PathBuf::from("/sys/fs/cgroup"), "memory.max")
            } else if fields[1].split(',').any(|c| c == "memory") {
                (
                    std::path::PathBuf::from("/sys/fs/cgroup/memory"),
                    "memory.limit_in_bytes",
                )
            } else {
                continue;
            };
            let mut path = root.join(fields[2].trim_start_matches('/'));
            while path.starts_with(&root) {
                if let Some(n) = std::fs::read_to_string(path.join(file))
                    .ok()
                    .and_then(|s| s.trim().parse::<usize>().ok())
                {
                    limits.push(n);
                }
                if path == root || !path.pop() {
                    break;
                }
            }
        }
    }
    let cgroup = limits.into_iter().min();
    physical
        .into_iter()
        .chain(cgroup)
        .min()
        .unwrap_or(128 << 20)
        / 2
}
fn eval_on_stack<T: Send>(depth: usize, f: impl FnOnce() -> R<T> + Send) -> R<T> {
    if depth <= 64 {
        return f();
    }
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("nata-aggregate-output".into())
            .stack_size(depth.saturating_mul(32 * 1024).saturating_add(8 << 20))
            .spawn_scoped(scope, f)
            .map_err(|_| resource())?
            .join()
            .unwrap_or_else(|_| Err(EvalError::internal("aggregate output evaluation panic")))
    })
}
pub(crate) fn execute(
    plan: Arc<LogicalPlan>,
    cfg: &ExecConfig,
    cancel: Option<&ScanCancel>,
    observed: &mut Observed,
    mut sink: impl FnMut(&RecordBatch) -> Result<(), ScanError>,
) -> Result<ExecReport, ScanError> {
    let started = Instant::now();
    let manager = MemoryManager::new(
        cfg.aggregate_memory_bytes.unwrap_or_else(default_budget),
        4096,
        1,
    )
    .map_err(|_| ScanError::new(resource().into_kind()))?;
    observed.aggregate_memory = Some(Arc::clone(&manager));
    let shared = Arc::new(Shared {
        plan: Arc::clone(&plan),
        manager: Arc::clone(&manager),
        partials: Mutex::new(Vec::new()),
    });
    let factory: TaskSinkFactory = {
        let shared = Arc::clone(&shared);
        Arc::new(move || {
            Box::new(Partial {
                shared: Arc::clone(&shared),
                tables: shared
                    .partials
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .pop()
                    .unwrap_or_else(|| (0..PARTITIONS).map(|_| Table::new()).collect()),
            })
        })
    };
    let mut options = plan.scan.scan_options();
    options.late_materialization = cfg.late_materialization;
    if plan.scan.columns.is_empty() {
        options.columns = Some(vec![crate::exec::first_readable_column(&plan)?]);
    }
    let scan = Arc::new(ScanPlan::open_with_operators(
        &plan.scan.files,
        &options,
        cfg.opener
            .clone()
            .unwrap_or_else(|| Arc::new(nata_parquet::FsOpener)),
        None,
        Some(factory),
    )?);
    observed.stats = Some(Arc::clone(scan.stats()));
    let report = run_parallel_with(
        scan,
        ParallelConfig {
            threads: cfg.threads,
            window: cfg.window,
            ordered: cfg.ordered,
            footers_ahead: cfg.footers_ahead,
        },
        None,
        cancel,
        |_| Ok(()),
    )?;
    let a = plan
        .aggregate
        .as_ref()
        .ok_or_else(|| ScanError::new(ErrorKind::Internal("aggregate missing".into())))?;
    let partials = std::mem::take(
        &mut *shared
            .partials
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    let mut partitions: Vec<Vec<Table>> = (0..PARTITIONS).map(|_| Vec::new()).collect();
    for tables in partials {
        for (i, t) in tables.into_iter().enumerate() {
            if !t.groups.is_empty() {
                partitions[i].push(t);
            }
        }
    }
    if a.keys.is_empty() && partitions.iter().all(Vec::is_empty) {
        let mut table = Table::new();
        table
            .reserve(&manager, &[], a)
            .map_err(|e| ScanError::new(e.into_kind()))?;
        table
            .groups
            .insert(vec![], a.aggregates.iter().map(State::new).collect());
        partitions[0].push(table);
    }
    let types: Vec<_> = a
        .keys
        .iter()
        .map(|k| k.ty)
        .chain(a.aggregates.iter().map(|a| a.ty))
        .collect();
    let mut seen = 0u64;
    let mut output = 0u64;
    let limit = plan.limit.map_or(u64::MAX, |l| {
        if l.n == 0 {
            0
        } else {
            l.offset.saturating_add(l.n)
        }
    });
    nata_sched::unordered::run(
        partitions,
        cfg.threads,
        || cancel.is_some_and(ScanCancel::is_cancelled),
        |tables, cancelled| {
            merge(tables, a, &manager, cancelled).map_err(|e| {
                if cancel.is_some_and(ScanCancel::is_cancelled) {
                    ScanError::new(ErrorKind::Cancelled("aggregate cancelled".into()))
                } else {
                    ScanError::new(e.into_kind())
                }
            })
        },
        |table| {
            let mut iter = table.groups.into_iter();
            loop {
                let rows: Vec<_> = iter.by_ref().take(1024).collect();
                if rows.is_empty() {
                    break;
                }
                if cancel.is_some_and(ScanCancel::is_cancelled) {
                    return Err(ScanError::new(ErrorKind::Cancelled(
                        "aggregate cancelled".into(),
                    )));
                }
                let b = batch(&rows, a).map_err(|e| ScanError::new(e.into_kind()))?;
                let evaluated = eval_on_stack(crate::exec::max_depth(&plan), || {
                    let ev = Evaluator::new(&b, &types);
                    let keep = if let Some(h) = &a.having {
                        ev.keep_mask(h)?
                    } else {
                        vec![true; b.num_rows()]
                    };
                    let admitted: Vec<_> = keep
                        .iter()
                        .enumerate()
                        .filter_map(|(i, k)| {
                            if !k {
                                return None;
                            }
                            let n = seen;
                            seen = seen.saturating_add(1);
                            (n < limit).then(|| u32::try_from(i).map_err(|_| resource()))
                        })
                        .collect::<R<Vec<_>>>()?;
                    if admitted.is_empty() {
                        return Ok(None);
                    }
                    let arrays = plan
                        .project
                        .exprs
                        .iter()
                        .map(|p| {
                            ev.eval(&p.expr, &admitted)
                                .and_then(|c| c.to_arrow())
                                .map(|a| (p.name.clone(), a))
                        })
                        .collect::<R<Vec<_>>>()?;
                    let projected = RecordBatch::try_from_iter(arrays)
                        .map_err(|_| EvalError::internal("aggregate projection"))?;
                    let offset = plan.limit.map_or(0, |l| l.offset);
                    let before = seen.saturating_sub(keep.iter().filter(|k| **k).count() as u64);
                    let select: Vec<_> = admitted
                        .iter()
                        .enumerate()
                        .filter_map(|(j, _)| {
                            (before + j as u64 >= offset).then_some(*admitted.get(j)? as usize)
                        })
                        .collect();
                    Ok(Some((projected, select)))
                })
                .map_err(|e| ScanError::new(e.into_kind()))?;
                let Some((projected, select)) = evaluated else {
                    continue;
                };
                for i in select {
                    sink(&projected.slice(i, 1))?;
                    output += 1;
                }
            }
            drop(iter);
            drop(table.grants);
            Ok(())
        },
    )?;
    if cancel.is_some_and(ScanCancel::is_cancelled) {
        return Err(ScanError::new(ErrorKind::Cancelled(
            "aggregate cancelled".into(),
        )));
    }
    Ok(ExecReport {
        ops: OperatorStats {
            batches: report.row_groups_read,
            scan_rows: report.rows_read,
            filter_rows: report.rows_read,
            filter_time: Duration::ZERO,
            project_time: Duration::ZERO,
            project_rows: seen.min(limit),
            limit_in: seen,
            rows_out: output,
        },
        scan: report,
        elapsed: started.elapsed(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collision_equality_and_canonical_float_keys() {
        let mut groups = Groups::new();
        groups.hash = |_| 0;
        for i in 0..100 {
            groups.insert(vec![Atom::Int(i)], vec![State::Count(i.unsigned_abs())]);
        }
        assert!(groups.contains_key(&[Atom::Int(42)]));
        assert!(!groups.contains_key(&[Atom::Null]));
        assert!(matches!(
            groups.get_mut(&[Atom::Int(42)]).expect("key")[0],
            State::Count(42)
        ));
        assert_eq!(Atom::float(-0.0), Atom::float(0.0));
        assert_eq!(Atom::float(-f64::NAN), Atom::float(f64::NAN));
        assert_ne!(
            key_hash(&[Atom::Null]),
            key_hash(&[Atom::Str(String::new())])
        );
    }
    #[test]
    fn final_partition_merges_every_contribution_exactly_once() {
        let manager = MemoryManager::new(1 << 20, 4096, 1).expect("memory");
        let a = Aggregate {
            keys: vec![],
            aggregates: vec![AggregateExpr {
                name: "count".into(),
                arg: None,
                ty: SqlType::BigInt,
                pos: nata_sql::Pos::NONE,
            }],
            having: None,
        };
        let mut tables = Vec::new();
        for n in [2, 3, 5] {
            let mut t = Table::new();
            t.reserve(&manager, &[], &a).expect("grant");
            t.groups.insert(vec![], vec![State::Count(n)]);
            tables.push(t);
        }
        let mut out = merge(tables, &a, &manager, &|| false).expect("merge");
        assert!(matches!(
            out.groups.get_mut(&[]).expect("global")[0],
            State::Final(Atom::Int(10))
        ));
        drop(out);
        assert_eq!(manager.snapshot().held, 0);
    }
}
