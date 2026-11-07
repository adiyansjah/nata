//! Resumable partial aggregation and disjoint final partitions, with exact
//! typed state spill. State grants move with their allocations and disk jobs.
use crate::error::{EvalError, R};
use crate::eval::Evaluator;
use crate::exec::{AggregateStats, ExecConfig, ExecReport, Observed, OperatorStats};
use crate::value::{Col, Data, StrData};
use arrow_array::RecordBatch;
use hashbrown::HashTable;
use nata_kernels::aggregate::ExactSum;
use nata_parquet::{ErrorKind, ScanCancel, ScanError};
use nata_sched::memory::{Chunks, Grant, MemoryManager, Reservation};
mod pipeline;
mod replay;
mod storage;
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
    fn finalize(&mut self, a: &AggregateExpr) -> R<()> {
        // Move extrema into final storage; cloning a large string here would
        // allocate another copy while the old partial's capacity is still live.
        if matches!(self, Self::Extreme(_) | Self::Final(_)) {
            let old = std::mem::replace(self, Self::Count(0));
            if let Self::Extreme(value) | Self::Final(value) = old {
                *self = Self::Final(value);
            }
        } else {
            *self = Self::Final(self.finish(a)?);
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
        "resource error: aggregate minimum working set exceeds memory budget",
    )
}
fn grant(manager: &Arc<MemoryManager>, bytes: usize) -> R<Grant> {
    match MemoryManager::request(manager, 0, bytes.max(1)) {
        Ok(Reservation::Granted(g)) => Ok(g),
        _ => Err(EvalError::at(
            nata_sql::Pos::NONE,
            format!(
                "resource error: aggregate minimum working set exceeds memory budget (request {bytes} bytes, budget {} bytes)",
                manager.snapshot().budget
            ),
        )),
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
    rehashes: usize,
    entries: Vec<(Vec<Atom>, Vec<State>)>,
}
impl Groups {
    fn new() -> Self {
        Self {
            hash: key_hash,
            index: HashTable::new(),
            rehashes: 0,
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
    fn get(&self, key: &[Atom]) -> Option<&[State]> {
        let i = *self
            .index
            .find((self.hash)(key), |&i| self.entries[i].0 == key)?;
        Some(&self.entries[i].1)
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
        self.rehashes += 1;
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
    grants: Chunks,
    chunk_bytes: usize,
}
impl Table {
    fn new() -> Self {
        Self {
            groups: Groups::new(),
            grants: Chunks::new(),
            chunk_bytes: 0,
        }
    }
    fn reserve_bytes(&mut self, manager: &Arc<MemoryManager>, bytes: usize) -> R<()> {
        if self.chunk_bytes == 0 {
            self.chunk_bytes = (manager.snapshot().budget / 256).clamp(4096, 1 << 20);
        }
        match self.grants.reserve(manager, 0, bytes, self.chunk_bytes) {
            Ok(true) => Ok(()),
            _ => Err(resource()),
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
        // charges retain capacity credit until the owning table is freed.
        self.reserve_bytes(manager, bytes)?;
        if self.groups.len() == self.groups.capacity() {
            let capacity = self.groups.capacity().max(4);
            self.reserve_bytes(manager, capacity.saturating_mul(128))?;
            self.groups.try_reserve(capacity).map_err(|_| resource())?;
        }

        Ok(())
    }
}
struct Shared {
    plan: Arc<LogicalPlan>,
    manager: Arc<MemoryManager>,
    metrics: Mutex<AggregateStats>,
    hash: fn(&[Atom], u8) -> u64,
}
struct Partial {
    shared: Arc<Shared>,
    tables: Vec<Table>,
}
struct Evaluated {
    rows: Vec<u32>,
    keys: Vec<Col>,
    args: Vec<Option<Col>>,
    next: usize,
    _strings: Option<Grant>,
}
impl Partial {
    fn evaluate(&self, batch: &RecordBatch) -> R<Evaluated> {
        let plan = &self.shared.plan;
        let a = plan
            .aggregate
            .as_ref()
            .ok_or_else(|| EvalError::internal("aggregate missing"))?;
        eval_on_stack(crate::exec::max_depth(plan), || {
            let types: Vec<_> = plan.scan.columns.iter().map(|c| c.ty).collect();
            let ev = Evaluator::new(batch, &types);
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
                .map(|e| ev.eval(e, &rows).map(|c| (*c).clone()))
                .collect::<R<Vec<_>>>()?;
            let args = a
                .aggregates
                .iter()
                .map(|a| {
                    a.arg
                        .as_ref()
                        .map(|e| ev.eval(e, &rows).map(|c| (*c).clone()))
                        .transpose()
                })
                .collect::<R<Vec<_>>>()?;
            Ok(Evaluated {
                rows,
                keys,
                args,
                next: 0,
                _strings: None,
            })
        })
    }
    // No state changes until *all* capacity for this row has been granted.
    // Returning false leaves the row cursor in place for replay after the spill.
    fn resume(&mut self, evaluated: &mut Evaluated, cancelled: &dyn Fn() -> bool) -> R<bool> {
        let a = self
            .shared
            .plan
            .aggregate
            .as_ref()
            .ok_or_else(|| EvalError::internal("aggregate missing"))?;
        while let Some(&r) = evaluated.rows.get(evaluated.next) {
            if cancelled() {
                return Err(EvalError::at(nata_sql::Pos::NONE, "aggregate cancelled"));
            }
            let i = r as usize;
            let key: Vec<_> = evaluated.keys.iter().map(|c| Atom::get(c, i)).collect();
            let p = usize::try_from((self.shared.hash)(&key, 0) & 15).map_err(|_| resource())?;
            let table = &mut self.tables[p];
            let old = table.groups.get(&key);
            let extra = evaluated
                .args
                .iter()
                .zip(&a.aggregates)
                .enumerate()
                .map(|(slot, (arg, agg))| {
                    if (agg.name == "min" || agg.name == "max")
                        && let Some(col) = arg
                        && col.valid[i]
                        && let Data::Str(text) = &col.data
                    {
                        let previous = old.and_then(|s| s.get(slot)).map_or(0, |s| {
                            if let State::Extreme(Atom::Str(s)) = s {
                                s.len()
                            } else {
                                0
                            }
                        });
                        return text.get(i).len().saturating_sub(previous);
                    }
                    0
                })
                .sum();
            if table.reserve_bytes(&self.shared.manager, extra).is_err() {
                return Ok(false);
            }
            if !table.groups.contains_key(&key) {
                if table.reserve(&self.shared.manager, &key, a).is_err() {
                    return Ok(false);
                }
                table
                    .groups
                    .insert(key.clone(), a.aggregates.iter().map(State::new).collect());
            }
            if let Some(states) = table.groups.get_mut(&key) {
                for ((state, agg), arg) in states.iter_mut().zip(&a.aggregates).zip(&evaluated.args)
                {
                    state.add(arg.as_ref().map_or(Atom::Null, |c| Atom::get(c, i)), agg)?;
                }
            }
            evaluated.next += 1;
        }
        Ok(true)
    }
}
fn string_scratch(batch: &RecordBatch, plan: &LogicalPlan) -> R<usize> {
    let mut bytes = 0usize;
    for (column, input) in batch.columns().iter().zip(&plan.scan.columns) {
        if input.ty == SqlType::Varchar {
            // The fixed-width dictionary key copy is covered by the decode
            // allowance; text is still shared here, before any expansion.
            let col = Col::from_arrow(column, input.ty)?;
            if let Data::Str(text) = &col.data {
                for i in 0..col.len() {
                    if col.valid[i] {
                        bytes = bytes.checked_add(text.get(i).len()).ok_or_else(resource)?;
                    }
                }
            }
        }
    }
    bytes
        .checked_mul(expression_slots(plan))
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(resource)
}
fn partition_hash(key: &[Atom], depth: u8) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    depth.hash(&mut h);
    key.hash(&mut h);
    h.finish()
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
            s.finalize(a)?;
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
fn expression_slots(plan: &LogicalPlan) -> usize {
    let mut nodes = Vec::new();
    if let Some(filter) = &plan.filter {
        nodes.push(&filter.predicate);
    }
    if let Some(a) = &plan.aggregate {
        nodes.extend(&a.keys);
        nodes.extend(a.aggregates.iter().filter_map(|a| a.arg.as_ref()));
        if let Some(h) = &a.having {
            nodes.push(h);
        }
    }
    nodes.extend(plan.project.exprs.iter().map(|p| &p.expr));
    let mut count = 1usize;
    while let Some(e) = nodes.pop() {
        count = count.saturating_add(1);
        nodes.extend(e.children());
    }
    count
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
struct FinishMemory(Arc<MemoryManager>, Option<ScanCancel>);
impl Drop for FinishMemory {
    fn drop(&mut self) {
        if self.1.as_ref().is_some_and(ScanCancel::is_cancelled) {
            self.0.cancel();
        } else {
            let _ = self.0.finish(0);
        }
    }
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
    let _finished = FinishMemory(Arc::clone(&manager), cancel.cloned());
    let shared = Arc::new(Shared {
        plan: Arc::clone(&plan),
        manager: Arc::clone(&manager),
        metrics: Mutex::new(AggregateStats::default()),
        hash: if cfg.aggregate_spill_force_collisions {
            |_, _| 0
        } else {
            partition_hash
        },
    });
    let (report, partitions, storage, io) = pipeline::accumulate(&shared, cfg, cancel, observed)?;
    let a = plan
        .aggregate
        .as_ref()
        .ok_or_else(|| ScanError::new(ErrorKind::Internal("aggregate missing".into())))?;
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
    pipeline::finalize(partitions, &shared, cfg, cancel, &storage, &io, |table| {
        let mut iter = table.groups.into_iter();
        loop {
            let rows: Vec<_> = iter
                .by_ref()
                .take(
                    32.min((1 << 20) / expression_slots(&plan).saturating_mul(128))
                        .max(1),
                )
                .collect();
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
    })?;
    if cancel.is_some_and(ScanCancel::is_cancelled) {
        return Err(ScanError::new(ErrorKind::Cancelled(
            "aggregate cancelled".into(),
        )));
    }
    let spill = storage.registry.snapshot();
    if spill.bytes_written > 0 {
        let mut m = shared
            .metrics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        m.partial_groups = storage
            .partial_rows
            .load(std::sync::atomic::Ordering::SeqCst);
        m.largest_partial = storage
            .largest_partial
            .load(std::sync::atomic::Ordering::SeqCst);
        m.rehashes = storage.rehashes.load(std::sync::atomic::Ordering::SeqCst);
    }
    drop(io);
    drop(storage);
    let mut metrics = shared
        .metrics
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    metrics.spill_bytes_written = spill.bytes_written;
    metrics.spill_bytes_read = spill.bytes_read;
    metrics.spill_files_opened = spill.opened;
    metrics.partitions_spilled = spill.partitions;
    let memory = manager.snapshot();
    metrics.reservation_calls = memory.requests;
    metrics.reservation_releases = memory.releases;
    metrics.memory_lock_wait_nanos = memory.lock_wait_nanos;
    metrics.peak_reservation = memory.peak;
    Ok(ExecReport {
        aggregate: Some(metrics),
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
