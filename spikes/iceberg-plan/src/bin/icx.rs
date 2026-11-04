//! icx gen <dir> <seed> [--nan-free]                 write a random Iceberg table (the same builder as the soundness test)
//! icx scan <table> <out.parquet>                    read the whole table with OUR reader (planner + deletes) into one Parquet file
//! icx filters <table> <seed> <n> <out.json>         n random filters with OUR plan for each (files, matching rows), as SQL and as a JSON AST
//! icx stats <table>                                 manifests, entries and delete files of the current snapshot
#[path = "../../tests/common/mod.rs"]
mod common;

use std::collections::BTreeMap;
use std::path::Path;

use arrow::array::RecordBatch;
use common::*;
use iceplan::expr::*;
use iceplan::meta::Schema;
use iceplan::plan::*;
use iceplan::read::*;
use iceplan::table::*;
use iceplan::types::*;
use serde_json::json;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("gen") => {
            if a.contains(&"--nan-free".to_string()) { NAN_FREE.with(|c| c.set(true)); }
            let dest = Path::new(&a[2]);
            let _ = std::fs::remove_dir_all(dest);
            let (t, log) = random_table_in(a[3].parse().unwrap(), dest);
            println!("{}", t.metadata_path().display());
            eprintln!("{log:?}");
        }
        Some("overflow") => {
            // truncate[10] on an int column with values at the extremes: what do other engines do with them?
            let schema = iceplan::table::simple_schema(&[("id", Type::Long), ("k", Type::Int)]);
            let spec = iceplan::meta::Spec { id: 0, fields: vec![iceplan::meta::PartField { source_id: 2, field_id: 1000, name: "k_trunc".into(), transform: iceplan::transform::Transform::Truncate(10) }] };
            let dest = Path::new(&a[2]);
            let _ = std::fs::remove_dir_all(dest);
            let mut t = Table::create(dest, schema.clone(), spec, 1);
            let ks = [i32::MIN, i32::MIN + 1, i32::MIN + 9, i32::MIN + 10, -5, 3, 100, i32::MAX - 9, i32::MAX];
            let b = RecordBatch::try_new(arrow_schema(&schema), vec![build_array(&Type::Long, &(0..ks.len() as i64).map(|i| Some(Value::Long(i))).collect::<Vec<_>>()), build_array(&Type::Int, &ks.iter().map(|k| Some(Value::Int(*k))).collect::<Vec<_>>())]).unwrap();
            t.append(&b);
            println!("{}", t.metadata_path().display());
        }
        Some("scan") => {
            let t = Table::open(Path::new(&a[2])).unwrap();
            let (tasks, _) = plan(&t, &Expr::True, &PlanOpts::default()).unwrap();
            let batches = read_scan(&t, &tasks).unwrap();
            let schema = current_arrow_schema(&t);
            let f = std::fs::File::create(&a[3]).unwrap();
            let mut w = parquet::arrow::ArrowWriter::try_new(f, schema.clone(), None).unwrap();
            for b in &batches { let b2 = RecordBatch::try_new(schema.clone(), b.columns().to_vec()).unwrap(); w.write(&b2).unwrap(); }
            w.close().unwrap();
            println!("{} files, {} rows", tasks.len(), batches.iter().map(|b| b.num_rows()).sum::<usize>());
        }
        Some("filters") => {
            let t = Table::open(Path::new(&a[2])).unwrap();
            let (seed, n): (u64, usize) = (a[3].parse().unwrap(), a[4].parse().unwrap());
            let schema = t.meta.current_schema().clone();
            let portable = Schema { id: schema.id, fields: schema.fields.iter().filter(|f| !matches!(f.ty, Type::Binary | Type::Float)).cloned().collect() };
            let samples = sample_values(&t);
            let naive = naive_tasks(&t);
            let all: Vec<RecordBatch> = naive.iter().map(|task| read_task(&t, task).unwrap()).collect();
            let mut r = Rng(seed.wrapping_mul(2654435761) | 1);
            let mut out = vec![];
            for i in 0..n {
                let f = random_filter(&portable, &samples, 2, &mut r);
                let (planned, st) = plan(&t, &f, &PlanOpts::default()).unwrap();
                let matched: usize = all.iter().map(|b| matching_rows_sem(&t, b, &f, Some(FloatSem::Sql)).len()).sum();
                out.push(json!({"id": i, "sql": to_sql(&f, &schema), "ast": to_json(&f, &schema), "planned_files": planned.iter().map(|p| p.file.path.clone()).collect::<Vec<_>>(),
                                "live_files": naive.len(), "matched_rows": matched, "pruned_partition": st.pruned_partition, "pruned_metrics": st.pruned_metrics}));
            }
            std::fs::write(&a[5], serde_json::to_vec_pretty(&out).unwrap()).unwrap();
            println!("{n} filters");
        }
        Some("stats") => {
            let t = Table::open(Path::new(&a[2])).unwrap();
            let entries = t.live_entries().unwrap();
            let mut by: BTreeMap<i32, usize> = BTreeMap::new();
            for (_, e) in &entries { *by.entry(e.file.content).or_default() += 1; }
            println!("{} specs, {} schemas, live files by content {:?}", t.meta.specs.len(), t.meta.schemas.len(), by);
        }
        _ => { eprintln!("usage: icx gen|scan|filters|stats ..."); std::process::exit(2) }
    }
}

