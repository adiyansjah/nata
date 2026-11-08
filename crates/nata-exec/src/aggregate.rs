//! Resumable partial aggregation and disjoint final partitions, with exact
//! typed state spill. State grants move with their allocations and disk jobs.
use crate::error::{EvalError, R};
use crate::eval::Evaluator;
use crate::exec::{AggregateStats, ExecConfig, ExecReport, Observed, OperatorStats};
use crate::value::{Col, Data, StrData};
use arrow_array::{RecordBatch, UInt32Array};
use arrow_select::take::take_record_batch;
use hashbrown::HashTable;
use nata_kernels::aggregate::{ExactSum, add_count};
use nata_parquet::{ErrorKind, ScanCancel, ScanError};
use nata_sched::memory::{Chunks, Grant, MemoryManager, Reservation};
mod fusion;
mod hashing;
mod pipeline;
mod replay;
mod storage;
use nata_sql::bind::{Aggregate, AggregateExpr};
use nata_sql::plan::LogicalPlan;
use nata_sql::types::SqlType;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PARTITIONS: usize = 16;
// Poll at entry to every batch/resume, and at most 1024 selected rows apart.
const CANCEL_ROWS: usize = 1024;
#[derive(Debug, Clone, PartialEq, Eq)]
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
    fn matches_column(&self, col: &Col, i: usize) -> bool {
        if !col.valid[i] {
            return matches!(self, Self::Null);
        }
        match (self, &col.data) {
            (Self::Bool(a), Data::Bool(v)) => *a == v[i],
            (Self::Int(a), Data::Int(v)) => *a == v[i],
            (Self::Dec(a), Data::Dec(v)) => *a == v[i],
            (Self::Date(a), Data::Date(v)) => *a == v[i],
            (Self::Str(a), Data::Str(v)) => a == v.get(i),
            (a @ Self::Float(_), Data::F32(v)) => *a == Self::float(f64::from(v[i])),
            (a @ Self::Float(_), Data::F64(v)) => *a == Self::float(v[i]),
            _ => false,
        }
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
                *n = add_count(*n, 1).map_err(|_| overflow(a))?;
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
            (Self::Count(n), Self::Count(m)) => *n = add_count(*n, m).map_err(|_| overflow(a))?,
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
    hashing::key(key, 0)
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
    #[cfg(test)]
    fn contains_key(&self, key: &[Atom]) -> bool {
        self.get(key).is_some()
    }
    #[cfg(test)]
    fn get(&self, key: &[Atom]) -> Option<&[State]> {
        self.get_hashed(key, (self.hash)(key))
    }
    #[cfg(test)]
    fn get_hashed(&self, key: &[Atom], hash: u64) -> Option<&[State]> {
        let i = *self.index.find(hash, |&i| self.entries[i].0 == key)?;
        Some(&self.entries[i].1)
    }
    fn get_mut(&mut self, key: &[Atom]) -> Option<&mut Vec<State>> {
        self.get_mut_hashed(key, (self.hash)(key))
    }
    fn get_mut_hashed(&mut self, key: &[Atom], hash: u64) -> Option<&mut Vec<State>> {
        let i = *self.index.find(hash, |&i| self.entries[i].0 == key)?;
        Some(&mut self.entries[i].1)
    }
    fn find_row(&self, hash: u64, columns: &[Arc<Col>], row: usize) -> Option<usize> {
        let matches = |id: usize| {
            let key = &self.entries[id].0;
            key.len() == columns.len()
                && key
                    .iter()
                    .zip(columns)
                    .all(|(v, c)| v.matches_column(c, row))
        };
        self.index.find(hash, |&id| matches(id)).copied()
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
        if bytes == 0 {
            return Ok(());
        }
        if self.chunk_bytes == 0 {
            self.chunk_bytes = (manager.snapshot().budget / 256).clamp(4096, 1 << 20);
        }
        match self.grants.reserve(manager, 0, bytes, self.chunk_bytes) {
            Ok(true) => Ok(()),
            _ => Err(resource()),
        }
    }
    fn reserve(&mut self, manager: &Arc<MemoryManager>, key: &[Atom], a: &Aggregate) -> R<()> {
        let strings = key
            .iter()
            .map(|v| if let Atom::Str(s) = v { s.len() } else { 0 })
            .sum();
        self.reserve_entry(manager, key.len(), strings, a)
    }
    fn reserve_entry(
        &mut self,
        manager: &Arc<MemoryManager>,
        keys: usize,
        strings: usize,
        a: &Aggregate,
    ) -> R<()> {
        let bytes = 256usize
            .saturating_add(keys.saturating_mul(64))
            .saturating_add(a.aggregates.len().saturating_mul(128))
            .saturating_add(strings);
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
    force_collisions: bool,
}
struct Partial {
    shared: Arc<Shared>,
    tables: Vec<Table>,
    program: Option<fusion::Program>,
    rows: Vec<u32>,
    hashes: Vec<u64>,
}
struct Evaluated {
    rows: Vec<u32>,
    hashes: Vec<u64>,
    keys: Vec<Arc<Col>>,
    args: Vec<Option<Arc<Col>>>,
    next: usize,
}
impl Partial {
    fn evaluate(&mut self, batch: &RecordBatch) -> R<Evaluated> {
        let plan = &self.shared.plan;
        let a = plan
            .aggregate
            .as_ref()
            .ok_or_else(|| EvalError::internal("aggregate missing"))?;
        eval_on_stack(crate::exec::max_depth(plan), || {
            let types: Vec<_> = plan.scan.columns.iter().map(|c| c.ty).collect();
            let ev = Evaluator::new(batch, &types);
            let mut rows = std::mem::take(&mut self.rows);
            rows.clear();
            if let Some(f) = &plan.filter {
                rows.extend(
                    ev.keep_mask(&f.predicate)?
                        .into_iter()
                        .enumerate()
                        .filter(|(_, k)| *k)
                        .map(|(i, _)| u32::try_from(i).map_err(|_| resource()))
                        .collect::<R<Vec<_>>>()?,
                );
            } else {
                rows.extend(ev.all_rows());
            }
            let program = self.program.get_or_insert_with(|| fusion::Program::new(a));
            let (keys, args) = program.evaluate(batch, &ev, a, &rows)?;
            let mut hashes = std::mem::take(&mut self.hashes);
            hashing::columns_into(&keys, &rows, &mut hashes);
            Ok(Evaluated {
                rows,
                hashes,
                keys,
                args,
                next: 0,
            })
        })
    }
    fn recycle(&mut self, evaluated: Evaluated) {
        self.rows = evaluated.rows;
        self.hashes = evaluated.hashes;
        drop((evaluated.keys, evaluated.args));
        if let Some(program) = &mut self.program {
            program.release_text();
        }
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
        let start = evaluated.next;
        while evaluated.next < evaluated.rows.len() {
            if (evaluated.next - start).is_multiple_of(CANCEL_ROWS) && cancelled() {
                return Err(EvalError::at(nata_sql::Pos::NONE, "aggregate cancelled"));
            }
            // Stack storage is bounded independently of input and charged by
            // the runtime/task reservation. IDs never survive a spill/rehash.
            let mut groups = [(0usize, 0usize); CANCEL_ROWS];
            let begin = evaluated.next;
            let stop = evaluated.rows.len().min(begin + CANCEL_ROWS);
            while evaluated.next < stop {
                let r = evaluated.rows[evaluated.next];
                let i = r as usize;
                let hash = evaluated.hashes[evaluated.next];
                let partition = if self.shared.force_collisions {
                    0
                } else {
                    hash
                };
                let p = usize::try_from(partition & 15).map_err(|_| resource())?;
                let table = &mut self.tables[p];
                let found = table.groups.find_row(hash, &evaluated.keys, i);
                let old = found.map(|id| table.groups.entries[id].1.as_slice());
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
                    break;
                }
                let id = if let Some(id) = found {
                    id
                } else {
                    let strings = evaluated
                        .keys
                        .iter()
                        .map(|c| {
                            if c.valid[i]
                                && let Data::Str(text) = &c.data
                            {
                                text.get(i).len()
                            } else {
                                0
                            }
                        })
                        .sum();
                    if table
                        .reserve_entry(&self.shared.manager, evaluated.keys.len(), strings, a)
                        .is_err()
                    {
                        break;
                    }
                    // Only a new group owns a key. Lookups borrow column values;
                    // reservation precedes key/state allocation and table growth.
                    let key = evaluated.keys.iter().map(|c| Atom::get(c, i)).collect();
                    let id = table.groups.len();
                    table
                        .groups
                        .insert(key, a.aggregates.iter().map(State::new).collect());
                    id
                };
                groups[evaluated.next - begin] = (p, id);
                evaluated.next += 1;
            }
            let rows = &evaluated.rows[begin..evaluated.next];
            let ids = &groups[..rows.len()];
            // Dispatch argument representation and aggregate once per column,
            // rather than constructing an Atom for every numeric addend.
            for (slot, (agg, arg)) in a.aggregates.iter().zip(&evaluated.args).enumerate() {
                match (agg.name.as_str(), arg.as_ref().map(|c| &c.data)) {
                    ("sum" | "avg", Some(Data::Int(values))) => {
                        let col = arg.as_ref().ok_or_else(resource)?;
                        for (&row, &(p, id)) in rows.iter().zip(ids) {
                            let i = row as usize;
                            if col.valid[i]
                                && let State::Sum(sum) =
                                    &mut self.tables[p].groups.entries[id].1[slot]
                            {
                                sum.add(i128::from(values[i])).map_err(|_| overflow(agg))?;
                            }
                        }
                    }
                    ("sum" | "avg", Some(Data::Dec(values))) => {
                        let col = arg.as_ref().ok_or_else(resource)?;
                        for (&row, &(p, id)) in rows.iter().zip(ids) {
                            let i = row as usize;
                            if col.valid[i]
                                && let State::Sum(sum) =
                                    &mut self.tables[p].groups.entries[id].1[slot]
                            {
                                sum.add(values[i]).map_err(|_| overflow(agg))?;
                            }
                        }
                    }
                    ("count", _) => {
                        for (&row, &(p, id)) in rows.iter().zip(ids) {
                            if arg.as_ref().is_none_or(|c| c.valid[row as usize])
                                && let State::Count(n) =
                                    &mut self.tables[p].groups.entries[id].1[slot]
                            {
                                *n = add_count(*n, 1).map_err(|_| overflow(agg))?;
                            }
                        }
                    }
                    _ => {
                        for (&row, &(p, id)) in rows.iter().zip(ids) {
                            self.tables[p].groups.entries[id].1[slot].add(
                                arg.as_ref()
                                    .map_or(Atom::Null, |c| Atom::get(c, row as usize)),
                                agg,
                            )?;
                        }
                    }
                }
            }
            // Commit every admitted prefix before spilling; the first row
            // denied a grant stays unconsumed for replay against new tables.
            if evaluated.next < stop {
                return Ok(false);
            }
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
    hashing::key(key, depth)
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
        for (row, (key, states)) in table.groups.drain().enumerate() {
            if row.is_multiple_of(CANCEL_ROWS) && cancelled() {
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
    for (row, states) in out.groups.values_mut().enumerate() {
        if row.is_multiple_of(CANCEL_ROWS) && cancelled() {
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
// Projection evaluates at the original batch positions: gathering beforehand
// would change selected-error behavior. Compact only after every expression
// has succeeded. A contiguous survivor range shares the original Arrow buffers.
fn gather_output(batch: &RecordBatch, selected: &[usize]) -> Result<RecordBatch, ScanError> {
    let first = selected[0];
    if selected.iter().enumerate().all(|(j, i)| *i == first + j) {
        return Ok(batch.slice(first, selected.len()));
    }
    let indices = selected
        .iter()
        .map(|i| u32::try_from(*i).map_err(|_| ScanError::new(resource().into_kind())))
        .collect::<Result<Vec<_>, _>>()?;
    take_record_batch(batch, &UInt32Array::from(indices))
        .map_err(|_| ScanError::new(ErrorKind::Internal("aggregate output selection".into())))
}
// Fixed-width gathers fit the existing 128 bytes/expression/row allowance.
// Variable-width survivor runs share buffers instead: even a very large string
// must not add an uncharged gathered copy. An isolated survivor is a whole run.
fn deliver_output(
    batch: &RecordBatch,
    selected: &[usize],
    mut sink: impl FnMut(&RecordBatch) -> Result<(), ScanError>,
) -> Result<u64, ScanError> {
    if batch.columns().iter().all(|c| {
        c.data_type().is_primitive() || matches!(c.data_type(), arrow_schema::DataType::Boolean)
    }) {
        sink(&gather_output(batch, selected)?)?;
        return Ok(1);
    }
    let mut calls = 0;
    let mut start = 0;
    while start < selected.len() {
        let mut end = start + 1;
        while end < selected.len() && selected[end] == selected[end - 1] + 1 {
            end += 1;
        }
        sink(&batch.slice(selected[start], end - start))?;
        calls += 1;
        start = end;
    }
    Ok(calls)
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
pub fn default_budget() -> usize {
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
    physical.into_iter().chain(cgroup).min().unwrap_or(0) / 2
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
        force_collisions: cfg.aggregate_spill_force_collisions,
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
    let mut sink_calls = 0u64;
    let mut output_nanos = 0u64;
    let limit = plan.limit.map_or(u64::MAX, |l| {
        if l.n == 0 {
            0
        } else {
            l.offset.saturating_add(l.n)
        }
    });
    pipeline::finalize(partitions, &shared, cfg, cancel, &storage, &io, |table| {
        // This coordinator interval includes result construction, HAVING,
        // projection, delivery and table teardown, but excludes waiting for
        // final-merge tasks. The existing charged runtime/output workspace
        // covers the unchanged <=32-row batches and fixed-width gathers.
        let output_started = Instant::now();
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
            if !select.is_empty() {
                sink_calls += deliver_output(&projected, &select, &mut sink)?;
                output += select.len() as u64;
            }
        }
        drop(iter);
        drop(table.grants);
        output_nanos = output_nanos
            .saturating_add(u64::try_from(output_started.elapsed().as_nanos()).unwrap_or(u64::MAX));
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
    metrics.sink_calls = sink_calls;
    metrics.output_nanos = output_nanos;
    metrics.spill_bytes_written = spill.bytes_written;
    metrics.spill_bytes_read = spill.bytes_read;
    metrics.spill_files_opened = spill.opened;
    metrics.spill_peak_bytes = spill.peak_bytes;
    metrics.spill_peak_open = spill.peak_open;
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
    fn output_delivery_gathers_fixed_width_and_borrows_string_runs() {
        use arrow_array::{ArrayRef, Int32Array, StringArray, cast::AsArray};
        let ints = RecordBatch::try_from_iter([(
            "v",
            Arc::new(Int32Array::from(vec![0, 1, 2, 3, 4, 5])) as ArrayRef,
        )])
        .expect("ints");
        let mut values = Vec::new();
        let calls = deliver_output(&ints, &[1, 3, 4], |b| {
            values.extend(
                b.column(0)
                    .as_primitive::<arrow_array::types::Int32Type>()
                    .values()
                    .iter()
                    .copied(),
            );
            Ok(())
        })
        .expect("gather");
        assert_eq!(values, [1, 3, 4]);
        assert_eq!(calls, 1);
        let strings = RecordBatch::try_from_iter([(
            "v",
            Arc::new(StringArray::from(vec!["0", "1", "2", "3", "4", "5"])) as ArrayRef,
        )])
        .expect("strings");
        let mut values = Vec::new();
        let mut lengths = Vec::new();
        let original = strings.column(0).as_string::<i32>().value_data().as_ptr();
        let calls = deliver_output(&strings, &[1, 3, 4], |b| {
            let array = b.column(0).as_string::<i32>();
            assert_eq!(array.value_data().as_ptr(), original, "share string data");
            values.extend(array.iter().map(|v| v.expect("value").to_owned()));
            lengths.push(b.num_rows());
            Ok(())
        })
        .expect("ranges");
        assert_eq!(values, ["1", "3", "4"]);
        assert_eq!(lengths, [1, 2]);
        assert_eq!(calls, 2);
    }
    fn counting_partial() -> Partial {
        use nata_sql::plan::{Project, Scan};
        Partial {
            shared: Arc::new(Shared {
                plan: Arc::new(LogicalPlan {
                    scan: Scan {
                        pattern: String::new(),
                        files: vec![],
                        columns: vec![],
                        pushed: vec![],
                    },
                    aggregate: Some(Aggregate {
                        keys: vec![],
                        aggregates: vec![AggregateExpr {
                            name: "count".into(),
                            arg: None,
                            ty: SqlType::BigInt,
                            pos: nata_sql::Pos::NONE,
                        }],
                        having: None,
                    }),
                    filter: None,
                    project: Project { exprs: vec![] },
                    limit: None,
                }),
                manager: MemoryManager::new(1 << 20, 4096, 1).expect("manager"),
                metrics: Mutex::new(AggregateStats::default()),
                hash: partition_hash,
                force_collisions: false,
            }),
            tables: (0..PARTITIONS).map(|_| Table::new()).collect(),
            program: None,
            rows: Vec::new(),
            hashes: Vec::new(),
        }
    }
    fn counting_batch(n: u32) -> Evaluated {
        let rows: Vec<_> = (0..n).collect();
        Evaluated {
            hashes: hashing::columns(&[], &rows),
            rows,
            keys: vec![],
            args: vec![None],
            next: 0,
        }
    }
    #[test]
    fn partial_polls_at_batch_entry_and_bounded_intervals_in_large_batches() {
        use std::cell::Cell;
        let mut partial = counting_partial();
        let calls = Cell::new(0);
        let poll = || {
            calls.set(calls.get() + 1);
            false
        };
        for _ in 0..3 {
            let mut batch = counting_batch(256);
            assert!(partial.resume(&mut batch, &poll).expect("complete"));
        }
        assert_eq!(calls.get(), 3, "one cancel lock per ordinary batch");
        let mut large = counting_batch(4097);
        assert!(partial.resume(&mut large, &poll).expect("large"));
        assert_eq!(calls.get(), 8, "bounded polling within large batches");
        let mut large = counting_batch(4097);
        // A continuation can resume after an arbitrary row, not just a multiple
        // of CANCEL_ROWS. Entry still has to observe cancellation immediately.
        large.next = 17;
        assert!(partial.resume(&mut large, &|| true).is_err());
        assert_eq!(large.next, 17);
        let calls = Cell::new(0);
        assert!(
            partial
                .resume(&mut large, &|| {
                    calls.set(calls.get() + 1);
                    calls.get() == 2
                })
                .is_err()
        );
        assert_eq!(large.next, 17 + CANCEL_ROWS, "cancel latency row bound");
        let manager = Arc::clone(&partial.shared.manager);
        drop(partial);
        assert_eq!(manager.snapshot().held, 0);
    }
    #[test]
    fn group_vector_keeps_selected_null_arguments_and_distinct_groups() {
        let mut partial = counting_partial();
        let shared = Arc::get_mut(&mut partial.shared).expect("unique");
        let plan = Arc::get_mut(&mut shared.plan).expect("unique plan");
        let agg = &mut plan.aggregate.as_mut().expect("aggregate").aggregates[0];
        agg.name = "sum".into();
        let keys = Col {
            ty: SqlType::BigInt,
            valid: vec![true; 6],
            data: Data::Int(vec![2, 0, 1, 2, 1, 0]),
        };
        // Nonzero NULL junk must neither add to SUM nor increment AVG's count.
        let args = Col {
            ty: SqlType::BigInt,
            valid: vec![true, true, false, true, true, true],
            data: Data::Int(vec![5, 999, 777, 11, 999, 13]),
        };
        let rows = vec![0, 2, 3, 5];
        let hashes = hashing::columns(std::slice::from_ref(&Arc::new(keys.clone())), &rows);
        let mut evaluated = Evaluated {
            rows,
            hashes,
            keys: vec![Arc::new(keys)],
            args: vec![Some(Arc::new(args))],
            next: 0,
        };
        assert!(partial.resume(&mut evaluated, &|| false).expect("batch"));
        let mut result = Vec::new();
        for table in &partial.tables {
            for (key, states) in &table.groups.entries {
                let State::Sum(sum) = states[0] else {
                    panic!("sum");
                };
                result.push((key.clone(), sum.integer_sum().expect("sum"), sum.count()));
            }
        }
        assert_eq!(result.len(), 3);
        for (key, sum, count) in [(0, Some(13), 1), (1, None, 0), (2, Some(16), 2)] {
            assert!(
                result.contains(&(vec![Atom::Int(key)], sum, count)),
                "{result:?}"
            );
        }
    }

    #[test]
    fn resident_finalization_also_polls_at_bounded_intervals() {
        use std::cell::Cell;
        let partial = counting_partial();
        let a = partial.shared.plan.aggregate.as_ref().expect("aggregate");
        let manager = MemoryManager::new(16 << 20, 4096, 1).expect("manager");
        let mut table = Table::new();
        for i in 0..4097 {
            let key = vec![Atom::Int(i)];
            table.reserve(&manager, &key, a).expect("reserve");
            table.groups.insert(key, vec![State::Count(1)]);
        }
        let calls = Cell::new(0);
        let out = merge(vec![table], a, &manager, &|| {
            calls.set(calls.get() + 1);
            false
        })
        .expect("merge");
        assert_eq!(calls.get(), 5, "bounded finalization polling");
        drop(out);
        assert_eq!(manager.snapshot().held, 0);
    }
    #[test]
    fn borrowed_lookup_resolves_collisions_and_never_reads_null_junk() {
        let mut text = Col::empty(SqlType::Varchar, 4);
        text.valid = vec![true, true, false, true];
        text.data = Data::Str(StrData::Dict {
            values: Arc::new(arrow_array::StringArray::from(vec!["", "雪", "other"])),
            keys: vec![1, 0, u32::MAX, 2],
        });
        let mut floats = Col::empty(SqlType::Double, 4);
        floats.valid = vec![true; 4];
        floats.data = Data::F64(vec![-0.0, -f64::NAN, 1.0, 2.0]);
        let columns = [Arc::new(text), Arc::new(floats)];
        let mut groups = Groups::new();
        groups.hash = |_| 0;
        for i in 0..3 {
            groups.insert(columns.iter().map(|c| Atom::get(c, i)).collect(), vec![]);
        }
        for i in 0..3 {
            assert_eq!(groups.find_row(0, &columns, i), Some(i));
        }
        assert_eq!(groups.find_row(0, &columns, 3), None);
        assert_eq!(groups.find_row(0, &columns[..1], 0), None);
        // Hash values do not establish equality, including in a forced-collision
        // bucket containing NULL/empty strings and canonical floating keys.
        assert!(!Atom::Str("雪".into()).matches_column(&columns[0], 3));
        assert!(Atom::Null.matches_column(&columns[0], 2));
        assert!(Atom::float(0.0).matches_column(&columns[1], 0));
        assert!(Atom::float(f64::NAN).matches_column(&columns[1], 1));
    }
    #[test]
    fn borrowed_lookup_keeps_groups_across_table_growth() {
        let col = Col {
            ty: SqlType::BigInt,
            data: Data::Int((0..20).collect()),
            valid: vec![true; 20],
        };
        let mut groups = Groups::new();
        groups.hash = |_| 0;
        for n in 0..20 {
            groups.insert(vec![Atom::get(&col, n)], vec![State::Count(1)]);
            for i in 0..20 {
                assert_eq!(
                    groups.find_row(0, std::slice::from_ref(&Arc::new(col.clone())), i),
                    (i <= n).then_some(i)
                );
            }
        }
    }
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
