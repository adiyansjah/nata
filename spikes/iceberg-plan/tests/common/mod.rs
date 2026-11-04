//! Random tables (partitioned, evolved, with deletes) and random filters over them, plus a deliberately NAIVE oracle: it reads every live data file,
//! associates deletes by a plain nested loop over the spec's rules, and evaluates the filter row by row.
#![allow(dead_code)]
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use arrow::array::*;
use arrow::datatypes::{DataType, Field as AField, Schema as ASchema};
use iceplan::expr::*;
use iceplan::manifest::*;
use iceplan::meta::*;
use iceplan::plan::FileScanTask;
use iceplan::read::*;
use iceplan::table::*;
use iceplan::transform::{Op, Transform};
use iceplan::types::*;

pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 { self.0 ^= self.0 >> 12; self.0 ^= self.0 << 25; self.0 ^= self.0 >> 27; self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) }
    pub fn below(&mut self, n: u64) -> u64 { self.next() % n.max(1) }
    pub fn chance(&mut self, pct: u64) -> bool { self.below(100) < pct }
    pub fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T { &v[self.below(v.len() as u64) as usize] }
}

pub fn tmp(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("iceplan-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    p
}

const STRS: &[&str] = &["", "a", "ab", "abc", "abcdefghijklmnopqrstuvwxyz", "abcdefghijklmnop", "abcdefghijklmnopq", "abcdefghijklmnopqzzzz", "zzzzzzzzzzzzzzzzzzzzzz", "é", "éééééééééééééééééé",
                        "😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀😀", "日本語日本語日本語日本語日本語日本語", "Zebra", "apple", "apples", "\u{10FFFF}\u{10FFFF}\u{10FFFF}\u{10FFFF}\u{10FFFF}\u{10FFFF}\u{10FFFF}\u{10FFFF}\u{10FFFF}\u{10FFFF}\u{10FFFF}\u{10FFFF}\u{10FFFF}\u{10FFFF}\u{10FFFF}\u{10FFFF}\u{10FFFF}"];
const DOUBLES: &[f64] = &[f64::NAN, -0.0, 0.0, f64::INFINITY, f64::NEG_INFINITY, 1.5, -1.5, 100.25, -100.25, 1e300, -1e300, 0.1, 3.0];

thread_local! { pub static NAN_FREE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

pub fn rand_value(ty: &Type, r: &mut Rng) -> Option<Value> {
    if r.chance(12) { return None; }
    Some(match ty {
        Type::Boolean => Value::Bool(r.chance(50)),
        // NAN_FREE doubles as the "portable" switch of the interop checks: no NaN, and no integers within a width of the type's extremes (where the spec's truncate overflows)
        Type::Int => Value::Int(if r.chance(10) { *r.pick(if NAN_FREE.with(|c| c.get()) { &[0, -1, 1, 100, -100][..] } else { &[i32::MIN, i32::MAX, 0, -1, 1][..] }) } else { r.below(61) as i32 - 30 }),
        Type::Long => Value::Long(if r.chance(10) { *r.pick(if NAN_FREE.with(|c| c.get()) { &[0, -1, 1, 100, -100][..] } else { &[i64::MIN, i64::MAX, 0, -1, 1][..] }) } else { r.below(2001) as i64 - 1000 }),
        Type::Float => Value::Float(*r.pick(DOUBLES) as f32),
        Type::Double => { let nf = NAN_FREE.with(|c| c.get()); Value::Double(if r.chance(50) { let mut d = *r.pick(DOUBLES); while nf && !d.is_finite() { d = *r.pick(DOUBLES); } d } else { (r.below(2000) as f64 - 1000.0) / 8.0 }) }
        Type::Decimal(..) => Value::Decimal(r.below(40001) as i128 - 20000),
        Type::Date => Value::Date(if r.chance(10) { *r.pick(&[0, -1, 1, 19000, -25000]) } else { r.below(60000) as i32 - 30000 }),
        Type::Timestamp | Type::TimestampTz => Value::Ts(if r.chance(20) { *r.pick(&[0, -1, 1, MICROS_PER_HOUR, -MICROS_PER_HOUR, MICROS_PER_DAY, -MICROS_PER_DAY, 1_600_000_000_000_000]) } else { (r.below(4000) as i64 - 2000) * MICROS_PER_HOUR + r.below(MICROS_PER_HOUR as u64) as i64 }),
        Type::String => Value::Str(r.pick(STRS).to_string()),
        Type::Binary => { let n = r.below(24) as usize; Value::Bin((0..n).map(|_| if r.chance(30) { 0xFF } else { r.below(256) as u8 }).collect()) }
        t => panic!("no generator for {t:?}"),
    })
}

pub fn build_array(ty: &Type, vals: &[Option<Value>]) -> ArrayRef {
    macro_rules! arr { ($A:ty, $p:pat => $e:expr) => { Arc::new(<$A>::from(vals.iter().map(|v| v.as_ref().map(|v| match v { $p => $e, o => panic!("{o:?}") })).collect::<Vec<_>>())) as ArrayRef } }
    match ty {
        Type::Boolean => arr!(BooleanArray, Value::Bool(b) => *b),
        Type::Int => arr!(Int32Array, Value::Int(x) => *x), Type::Date => arr!(Date32Array, Value::Date(x) => *x),
        Type::Long => arr!(Int64Array, Value::Long(x) => *x),
        Type::Float => arr!(Float32Array, Value::Float(x) => *x), Type::Double => arr!(Float64Array, Value::Double(x) => *x),
        Type::Decimal(p, s) => Arc::new(Decimal128Array::from(vals.iter().map(|v| v.as_ref().map(|v| match v { Value::Decimal(x) => *x, o => panic!("{o:?}") })).collect::<Vec<_>>()).with_precision_and_scale(*p, *s as i8).unwrap()),
        Type::Timestamp => arr!(TimestampMicrosecondArray, Value::Ts(x) => *x),
        Type::TimestampTz => Arc::new(TimestampMicrosecondArray::from(vals.iter().map(|v| v.as_ref().map(|v| match v { Value::Ts(x) => *x, o => panic!("{o:?}") })).collect::<Vec<_>>()).with_timezone("UTC")),
        Type::String => Arc::new(StringArray::from(vals.iter().map(|v| v.as_ref().map(|v| match v { Value::Str(s) => s.clone(), o => panic!("{o:?}") })).collect::<Vec<_>>())),
        Type::Binary => Arc::new(BinaryArray::from(vals.iter().map(|v| v.as_ref().map(|v| match v { Value::Bin(b) => b.clone(), o => panic!("{o:?}") })).collect::<Vec<_>>().iter().map(|o| o.as_deref()).collect::<Vec<_>>())),
        t => panic!("{t:?}"),
    }
}

pub fn base_schema() -> Schema {
    simple_schema(&[("id", Type::Long), ("k", Type::Int), ("day", Type::Date), ("ts", Type::Timestamp), ("name", Type::String), ("score", Type::Double), ("amt", Type::Decimal(10, 2)), ("blob", Type::Binary)])
}

pub fn rand_batch(schema: &Schema, n: usize, next_id: &mut i64, r: &mut Rng) -> (RecordBatch, Vec<Vec<Option<Value>>>) {
    let mut cols = vec![]; let mut raw = vec![];
    for f in &schema.fields {
        let vals: Vec<Option<Value>> = if f.name == "id" { (0..n).map(|_| { *next_id += 1; Some(Value::Long(*next_id - 1)) }).collect() } else { (0..n).map(|_| rand_value(&f.ty, r)).collect() };
        cols.push(build_array(&f.ty, &vals)); raw.push(vals);
    }
    (RecordBatch::try_new(arrow_schema(schema), cols).unwrap(), raw)
}

pub fn rand_spec_fields(schema: &Schema, r: &mut Rng) -> Vec<(String, Transform)> {
    let has = |n: &str| schema.by_name(n).is_some();
    let mut options: Vec<Vec<(String, Transform)>> = vec![vec![]];
    let mut add = |v: Vec<(&str, Transform)>| { if v.iter().all(|(c, _)| has(c)) { options.push(v.into_iter().map(|(c, t)| (c.to_string(), t)).collect()); } };
    add(vec![("k", Transform::Identity)]); add(vec![("kk", Transform::Identity)]); add(vec![("ts", Transform::Day)]); add(vec![("day", Transform::Month), ("k", Transform::Bucket(4))]);
    add(vec![("name", Transform::Truncate(2))]); add(vec![("day", Transform::Year)]); add(vec![("ts", Transform::Hour)]); add(vec![("id", Transform::Bucket(8))]);
    add(vec![("k", Transform::Truncate(10)), ("name", Transform::Identity)]); add(vec![("amt", Transform::Truncate(500))]); add(vec![("ts", Transform::Month), ("day", Transform::Day)]);
    r.pick(&options).clone()
}

pub fn spec_from(schema: &Schema, fields: &[(String, Transform)]) -> Spec {
    Spec { id: 0, fields: fields.iter().enumerate().map(|(i, (c, tr))| PartField { source_id: schema.by_name(c).unwrap().id, field_id: 1000 + i as i32, name: format!("{c}_{}", tr.name().replace(['[', ']'], "")), transform: tr.clone() }).collect() }
}

/// Build a random table. Returns it with a log of what was done.
pub fn random_table(seed: u64, tag: &str) -> (Table, Vec<String>) { random_table_in(seed, &tmp(tag)) }

pub fn random_table_in(seed: u64, dir: &std::path::Path) -> (Table, Vec<String>) {
    let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let schema = base_schema();
    let mut log = vec![];
    let fields = rand_spec_fields(&schema, &mut r);
    let spec = spec_from(&schema, &fields);
    let mut t = Table::create(dir, schema, spec, seed);
    log.push(format!("spec {fields:?}"));
    let mut next_id = 0i64;
    let steps = 3 + r.below(5);
    for step in 0..steps {
        let n = 1 + r.below(50) as usize;
        let (b, _) = rand_batch(&t.meta.current_schema().clone(), n, &mut next_id, &mut r);
        t.append(&b);
        log.push(format!("append {n} rows"));
        if step == 1 && r.chance(60) { let f = rand_spec_fields(t.meta.current_schema(), &mut r); let names: Vec<(&str, Transform)> = f.iter().map(|(c, tr)| (c.as_str(), tr.clone())).collect(); t.evolve_spec(&names); log.push(format!("evolve spec {f:?}")); }
        if step == 1 && r.chance(50) { t.add_column("extra", Type::Int); log.push("add column extra".into()); }
        if step == 2 && r.chance(40) { t.rename_column("k", "kk"); log.push("rename k -> kk".into()); }
        if r.chance(35) {
            let entries = t.live_entries().unwrap();
            let datas: Vec<_> = entries.iter().filter(|(_, e)| e.file.content == DATA).collect();
            if !datas.is_empty() {
                let (em, e) = r.pick(&datas);
                let em_spec = em.spec_id;
                let cnt = e.file.record_count as u64;
                match r.below(3) {
                    0 => { let ps: Vec<i64> = (0..cnt as i64).filter(|_| r.chance(40)).collect(); if !ps.is_empty() { t.delete_positions(&e.file.path.clone(), &ps); log.push(format!("pos delete {} rows", ps.len())); } }
                    1 if em_spec != t.meta.default_spec_id => {}      // an equality delete takes its partition from the default spec: only pair it with a file of that spec
                    1 => { let sch = t.meta.current_schema().clone(); let col = sch.fields.iter().find(|f| f.name == "k" || f.name == "kk").unwrap().clone();
                           let vals: Vec<Option<Value>> = (0..3).map(|_| rand_value(&col.ty, &mut r)).collect(); let rows = RecordBatch::try_new(arrow_schema(&Schema { id: 0, fields: vec![col.clone()] }), vec![build_array(&col.ty, &vals)]).unwrap();
                           t.delete_equality(&[col.id], &rows, e.file.partition.clone()); log.push(format!("eq delete {vals:?}")); }
                    _ => { t.remove_files(&[e.file.path.clone()]); log.push("remove a file".into()); }
                }
            }
        }
    }
    (t, log)
}

// ------------------------------------------------------------------ filters

pub fn perturb(v: &Value, r: &mut Rng) -> Value {
    match v {
        Value::Int(x) => Value::Int(x.wrapping_add(*r.pick(&[-1, 0, 0, 1, 10]))), Value::Long(x) => Value::Long(x.wrapping_add(*r.pick(&[-1, 0, 0, 1, 10]))),
        Value::Date(x) => Value::Date(x.wrapping_add(*r.pick(&[-1, 0, 0, 1, 31, -31]))), Value::Ts(x) => Value::Ts(x.wrapping_add(*r.pick(&[-1, 0, 0, 1, MICROS_PER_HOUR, -MICROS_PER_DAY]))),
        Value::Decimal(x) => Value::Decimal(x + *r.pick(&[-1i128, 0, 0, 1, 500])),
        Value::Double(x) => Value::Double(if x.is_finite() && r.chance(30) { x + 0.5 } else { *x }),
        Value::Str(s) => match r.below(4) { 0 => Value::Str(s.chars().take(r.below(20) as usize).collect()), 1 => Value::Str(format!("{s}a")), _ => v.clone() },
        Value::Bin(b) => match r.below(3) { 0 => Value::Bin(b.iter().take(r.below(18) as usize).copied().collect()), _ => v.clone() },
        o => o.clone(),
    }
}

pub fn random_filter(schema: &Schema, samples: &BTreeMap<i32, Vec<Value>>, depth: u32, r: &mut Rng) -> Expr {
    if depth > 0 && r.chance(45) {
        let (a, b) = (random_filter(schema, samples, depth - 1, r), random_filter(schema, samples, depth - 1, r));
        return match r.below(4) { 0 => and(a, b), 1 => or(a, b), 2 => not(a), _ => and(not(b), a) };
    }
    let f = r.pick(&schema.fields).clone();
    let lit = |r: &mut Rng| -> Value { match samples.get(&f.id).filter(|v| !v.is_empty() && r.chance(85)) { Some(v) => perturb(r.pick(v), r), None => loop { if let Some(v) = rand_value(&f.ty, r) { break v; } } } };
    let lit = |r: &mut Rng| { let v = lit(r); match (&f.ty, v) { (Type::Int, Value::Long(x)) => Value::Int(x as i32), (_, v) => v } };
    let c = &f.name;
    match r.below(12) {
        0 => cmp(c, Op::Eq, lit(r)), 1 => cmp(c, Op::Lt, lit(r)), 2 => cmp(c, Op::Le, lit(r)), 3 => cmp(c, Op::Gt, lit(r)), 4 => cmp(c, Op::Ge, lit(r)),
        5 => pred(c, P::Ne(lit(r))), 6 => pred(c, P::In((0..1 + r.below(3)).map(|_| lit(r)).collect())), 7 => pred(c, P::NotIn((0..1 + r.below(3)).map(|_| lit(r)).collect())),
        8 => pred(c, P::IsNull), 9 => pred(c, P::NotNull),
        10 if matches!(f.ty, Type::Double | Type::Float) => pred(c, if r.chance(50) { P::IsNaN } else { P::NotNaN }),
        11 if matches!(f.ty, Type::String | Type::Binary) => cmp(c, Op::StartsWith, lit(r)),
        _ => cmp(c, Op::Ge, lit(r)),
    }
}

// ------------------------------------------------------------------ the oracle

pub struct Truth { pub tasks: Vec<FileScanTask>, pub rows: BTreeMap<String, Vec<String>> }   // naive tasks with their deletes; the (key -> rows) per file after deletes

/// Naive association of deletes by the spec's rules, with plain loops.
pub fn naive_tasks(t: &Table) -> Vec<FileScanTask> {
    let entries = t.live_entries().unwrap();
    let spec_of = |m: &ManifestFile| t.meta.spec(m.spec_id).unwrap().clone();
    let mut tasks = vec![];
    for (m, e) in entries.iter().filter(|(_, e)| e.file.content == DATA) {
        let (seq, mut pos, mut eq) = (e.seq.unwrap(), vec![], vec![]);
        for (dm, d) in entries.iter().filter(|(_, d)| d.file.content != DATA) {
            let (dseq, same_part) = (d.seq.unwrap(), dm.spec_id == m.spec_id && d.file.partition == e.file.partition);
            if d.file.content == POS_DELETES {
                if dseq >= seq && same_part && d.file.referenced.as_ref().is_none_or(|r| *r == e.file.path) { pos.push(d.file.clone()); }
            } else if dseq > seq && (same_part || spec_of(dm).fields.is_empty()) { eq.push(d.file.clone()); }
        }
        tasks.push(FileScanTask { file: e.file.clone(), spec_id: m.spec_id, seq, pos_deletes: pos, eq_deletes: eq });
    }
    tasks.sort_by(|a, b| a.file.path.cmp(&b.file.path));
    tasks
}

pub fn row_strings(t: &Table, b: &RecordBatch) -> Vec<String> {
    let schema = t.meta.current_schema();
    (0..b.num_rows()).map(|i| schema.fields.iter().enumerate().map(|(c, f)| format!("{:?}", cell(b.column(c), i, &f.ty))).collect::<Vec<_>>().join("|")).collect()
}

pub fn matching_rows(t: &Table, b: &RecordBatch, filter: &Expr) -> Vec<String> { matching_rows_sem(t, b, filter, None) }

/// `sem = None`: a row matches if it matches under ANY float semantics (what the planner must be sound against); `Some(s)`: exactly under `s`.
pub fn matching_rows_sem(t: &Table, b: &RecordBatch, filter: &Expr, sem: Option<FloatSem>) -> Vec<String> {
    let schema = t.meta.current_schema();
    let idx: BTreeMap<&str, (usize, &Type)> = schema.fields.iter().enumerate().map(|(i, f)| (f.name.as_str(), (i, &f.ty))).collect();
    (0..b.num_rows()).filter(|&i| { let get = |c: &str| { let (ci, ty) = idx[c]; cell(b.column(ci), i, ty) }; match sem { None => row_matches_any_sem(filter, &get), Some(s) => eval_row(filter, &get, s) == Some(true) } }).map(|i| {
        schema.fields.iter().enumerate().map(|(c, f)| format!("{:?}", cell(b.column(c), i, &f.ty))).collect::<Vec<_>>().join("|") }).collect()
}

pub fn sample_values(t: &Table) -> BTreeMap<i32, Vec<Value>> {
    let schema = t.meta.current_schema();
    let mut out: BTreeMap<i32, Vec<Value>> = BTreeMap::new();
    for task in naive_tasks(t) {
        let b = read_fields(&task.file.path, &schema.fields).unwrap();
        for (c, f) in schema.fields.iter().enumerate() { for i in 0..b.num_rows().min(12) { if let Some(v) = cell(b.column(c), i, &f.ty) { out.entry(f.id).or_default().push(v); } } }
    }
    out
}
pub fn _x(_: DataType, _: AField, _: ASchema) {}
