//! The scan planner: manifest list -> (partition summaries) -> manifests -> (partition tuple, column metrics) -> data files, then the delete files that
//! apply to each (by the spec's sequence-number and partition rules). Parallel over manifests.
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use rayon::prelude::*;

use crate::expr::*;
use crate::manifest::*;
use crate::meta::Schema;
use crate::table::Table;
use crate::types::*;
use crate::{Res, err};

#[derive(Clone, Debug)]
pub struct FileScanTask { pub file: DataFile, pub spec_id: i32, pub seq: i64, pub pos_deletes: Vec<DataFile>, pub eq_deletes: Vec<DataFile> }

#[derive(Default, Debug)]
pub struct PlanStats {
    pub manifests: usize, pub manifests_pruned: usize, pub entries: usize, pub pruned_partition: usize, pub pruned_metrics: usize, pub data_files: usize, pub delete_files: usize,
}

#[derive(Clone)]
pub struct PlanOpts { pub threads: usize, pub use_summaries: bool }
impl Default for PlanOpts { fn default() -> Self { PlanOpts { threads: 4, use_summaries: true } } }

/// Bounds of column `f` as described by a data file's metrics.
pub fn file_bounds(df: &DataFile, f: &crate::meta::Field) -> Bounds {
    let is_float = matches!(f.ty, Type::Float | Type::Double);
    let cnt = |m: &std::collections::BTreeMap<i32, i64>| match m.get(&f.id) { Some(0) => Count::Zero, Some(_) => Count::Some_, None => Count::Unknown };
    let (vals, nulls, nans) = (df.value_counts.get(&f.id).copied(), df.null_counts.get(&f.id).copied(), df.nan_counts.get(&f.id).copied());
    let non_null = vals.zip(nulls).map(|(v, n)| v - n);
    Bounds {
        lower: df.lower.get(&f.id).and_then(|b| deserialize(&f.ty, b).ok()), upper: df.upper.get(&f.id).and_then(|b| deserialize(&f.ty, b).ok()),
        nulls: cnt(&df.null_counts), nans: if is_float { cnt(&df.nan_counts) } else { Count::Zero },
        nulls_only: non_null == Some(0) && vals.is_some_and(|v| v > 0),
        nans_only: is_float && non_null.zip(nans).is_some_and(|(nn, nc)| nn > 0 && nn == nc), is_float,
    }
}

fn summary_might_match(p: &PProj, sums: &[FieldSummary], types: &[Type]) -> bool {
    match p {
        PProj::True => true,
        PProj::False => false,
        PProj::And(a, b) => summary_might_match(a, sums, types) && summary_might_match(b, sums, types),
        PProj::Or(a, b) => summary_might_match(a, sums, types) || summary_might_match(b, sums, types),
        PProj::Leaf(i, pp, _) => {
            let (Some(s), Some(t)) = (sums.get(*i), types.get(*i)) else { return true };
            let is_float = matches!(t, Type::Float | Type::Double);
            let (lo, hi) = (s.lower.as_ref().and_then(|b| deserialize(t, b).ok()), s.upper.as_ref().and_then(|b| deserialize(t, b).ok()));
            let nan_known_absent = !is_float || s.contains_nan == Some(false);
            let b = Bounds { nulls: if s.contains_null { Count::Some_ } else { Count::Zero }, nans: if !is_float { Count::Zero } else { match s.contains_nan { Some(false) => Count::Zero, Some(true) => Count::Some_, None => Count::Unknown } },
                             nulls_only: lo.is_none() && hi.is_none() && s.contains_null && nan_known_absent, nans_only: is_float && lo.is_none() && hi.is_none() && s.contains_nan == Some(true),
                             lower: lo, upper: hi, is_float };
            might_match_bounds(pp, &b)
        }
    }
}

fn check_columns(e: &Expr, schema: &Schema) -> Res<()> {
    match e {
        Expr::And(a, b) | Expr::Or(a, b) => { check_columns(a, schema)?; check_columns(b, schema) }
        Expr::Not(a) => check_columns(a, schema),
        Expr::Pred(c, _) => if schema.by_name(c).is_some() { Ok(()) } else { err(format!("unknown column {c} in the filter")) },
        _ => Ok(()),
    }
}

pub fn plan(table: &Table, filter: &Expr, opts: &PlanOpts) -> Res<(Vec<FileScanTask>, PlanStats)> {
    let meta = &table.meta;
    let schema = meta.current_schema();
    check_columns(filter, schema)?;
    let filter = rewrite_not(filter);
    let Some(snap) = meta.current_snapshot.and_then(|s| meta.snapshot(s)) else { return Ok((vec![], PlanStats::default())) };
    let manifests = read_manifest_list(&crate::read_file(&snap.manifest_list)?)?;
    let (n_pruned, n_entries, n_part, n_met) = (AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0));
    let projections: HashMap<i32, PProj> = meta.specs.iter().map(|s| (s.id, project(&filter, schema, s))).collect();
    let work = || -> Res<Vec<(i32, Entry, i64)>> {
        let per: Vec<Res<Vec<(i32, Entry, i64)>>> = manifests.par_iter().map(|m| {
            let spec = meta.spec(m.spec_id).ok_or_else(|| format!("manifest {} uses unknown spec {}", m.path, m.spec_id))?;
            let proj = &projections[&spec.id];
            if m.added_files + m.existing_files == 0 && m.deleted_files >= 0 && (m.added_rows + m.existing_rows == 0) { n_pruned.fetch_add(1, Relaxed); return Ok(vec![]); }
            let ptypes = table.part_types(spec);
            if opts.use_summaries && !summary_might_match(proj, &m.partitions, &ptypes) { n_pruned.fetch_add(1, Relaxed); return Ok(vec![]); }
            let (_, entries) = read_manifest(&crate::read_file(&m.path)?, &ptypes)?;
            let mut out = vec![];
            for mut e in entries {
                if e.status == DELETED { continue; }
                n_entries.fetch_add(1, Relaxed);
                if e.seq.is_none() { e.seq = Some(m.seq); }
                if e.file_seq.is_none() { e.file_seq = Some(m.seq); }
                if e.file.record_count == 0 { continue; }
                if !tuple_might_match(proj, &e.file.partition) { n_part.fetch_add(1, Relaxed); continue; }
                if e.file.content == DATA && !might_match(&filter, &|c| schema.by_name(c).map(|f| file_bounds(&e.file, f))) { n_met.fetch_add(1, Relaxed); continue; }
                out.push((m.spec_id, e, m.seq));
            }
            Ok(out)
        }).collect();
        let mut all = vec![];
        for r in per { all.extend(r?); }
        Ok(all)
    };
    let pool = rayon::ThreadPoolBuilder::new().num_threads(opts.threads.max(1)).build().map_err(|e| e.to_string())?;
    let entries = pool.install(work)?;
    let key = |spec: i32, t: &[Option<Value>]| format!("{spec}|{t:?}");
    let mut pos: HashMap<String, Vec<(i64, DataFile)>> = HashMap::new();
    let mut eq: HashMap<String, Vec<(i64, DataFile)>> = HashMap::new();
    let mut eq_global: Vec<(i64, DataFile)> = vec![];
    let mut data = vec![];
    for (spec_id, e, _) in entries {
        let seq = e.seq.unwrap_or(0);
        match e.file.content {
            DATA => data.push((spec_id, seq, e.file)),
            POS_DELETES => pos.entry(key(spec_id, &e.file.partition)).or_default().push((seq, e.file)),
            _ => if meta.spec(spec_id).is_some_and(|s| s.fields.is_empty()) { eq_global.push((seq, e.file)) } else { eq.entry(key(spec_id, &e.file.partition)).or_default().push((seq, e.file)) },
        }
    }
    let mut tasks: Vec<FileScanTask> = data.into_iter().map(|(spec_id, seq, f)| {
        let k = key(spec_id, &f.partition);
        let pos_deletes = pos.get(&k).map(|v| v.iter().filter(|(ds, d)| *ds >= seq && d.referenced.as_deref().is_none_or(|r| crate::local_path(r) == crate::local_path(&f.path))).map(|(_, d)| d.clone()).collect()).unwrap_or_default();
        let mut eq_deletes: Vec<DataFile> = eq.get(&k).map(|v| v.iter().filter(|(ds, _)| *ds > seq).map(|(_, d)| d.clone()).collect()).unwrap_or_default();
        eq_deletes.extend(eq_global.iter().filter(|(ds, _)| *ds > seq).map(|(_, d)| d.clone()));
        FileScanTask { file: f, spec_id, seq, pos_deletes, eq_deletes }
    }).collect();
    tasks.sort_by(|a, b| a.file.path.cmp(&b.file.path));
    let stats = PlanStats { manifests: manifests.len(), manifests_pruned: n_pruned.load(Relaxed), entries: n_entries.load(Relaxed), pruned_partition: n_part.load(Relaxed), pruned_metrics: n_met.load(Relaxed),
                            data_files: tasks.len(), delete_files: pos.values().chain(eq.values()).map(|v| v.len()).sum::<usize>() + eq_global.len() };
    Ok((tasks, stats))
}
