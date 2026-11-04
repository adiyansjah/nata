//! arrow-pipeline: modes `csv`, `check`, `nullcheck`, `bench` (end to end), `membench` (engine only).
use std::sync::Arc;
use std::sync::atomic::Ordering::SeqCst;
use std::time::{Duration, Instant};

use spike_arrow_pipeline::genops::engine::{Output, Plan, Visit, q1, q6, run as genops_run};
use spike_arrow_pipeline::genops::nulls::{self, mem_visit, parquet_visit, preload};
use spike_arrow_pipeline::genops::source::Source as ParquetFile;
use spike_arrow_pipeline::queries::{columns, q1_pipeline, q6_pipeline, run_query, run_watched};
use spike_arrow_pipeline::sources::{Chaos, MemScan, ParquetScan};

fn quantile(v: &mut [f64], q: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * q).round() as usize]
}

fn plan_of(name: &str) -> Plan {
    if name == "q1" { q1() } else { q6() }
}

fn load_avg() -> String {
    std::fs::read_to_string("/proc/loadavg").map_or("?".into(), |s| s.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
}

/// The protocol pipeline over a pre-opened file or in-memory batches. Returns the result.
fn protocol_run(name: &str, scan_file: &Arc<ParquetFile>, mem: Option<&Arc<Vec<Vec<arrow::array::RecordBatch>>>>, threads: usize) -> Output {
    let tasks = threads;
    let build = |p: Arc<ParquetScan>| if name == "q1" { q1_pipeline(p, tasks) } else { q6_pipeline(p, tasks) };
    let (pipeline, sink) = match mem {
        None => build(ParquetScan::from_file(Arc::clone(scan_file), 0, Chaos::default())),
        Some(m) => {
            // Same pipeline, in-memory source (each batch is one morsel, like the generic-ops `membench`).
            let scan = MemScan::new(Arc::clone(m), Chaos::default());
            let (mut p, sink) = build(ParquetScan::from_file(Arc::clone(scan_file), 0, Chaos::default()));
            p.source = Arc::new(scan);
            (p, sink)
        }
    };
    run_watched(vec![pipeline], threads, Duration::from_secs(300)).unwrap_or_else(|e| panic!("{name}: {e:?}"));
    sink.result().expect("finalized")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map_or("", String::as_str);
    let path = args.get(2).expect("usage: arrow-pipeline <mode> <lineitem.parquet> ...").as_str();
    match mode {
        // csv <q1|q6> <parquet> <threads> [tasks]: the result as DuckDB prints it with -csv (no header).
        "csv" => {
            let name = args.get(3).expect("q1|q6");
            let threads: usize = args.get(4).expect("threads").parse().expect("number");
            let tasks: usize = args.get(5).map_or(threads, |s| s.parse().expect("number"));
            let (out, claimed, total) = run_query(name, path, threads, tasks, 0);
            eprintln!("{name}: threads={threads} tasks={tasks} morsels read {claimed} of {total}");
            println!("{}", out.csv());
        }
        "check" => {
            for name in ["q1", "q6"] {
                let plan = plan_of(name);
                let src = ParquetFile::open(path, plan.columns);
                let v = parquet_visit(&src, 0);
                let baseline = genops_run(src.row_groups, &v, &plan, 4).expect("generic-ops");
                let mut first: Option<Output> = None;
                for threads in [1usize, 4, 8] {
                    let mut task_counts = vec![1usize, threads, 2 * threads];
                    task_counts.dedup();
                    for tasks in task_counts {
                        let (out, claimed, total) = run_query(name, path, threads, tasks, 0);
                        assert_eq!(claimed, total, "a full scan reads every morsel");
                        assert_eq!(out, baseline, "{name} threads={threads} tasks={tasks}: protocol result differs from the generic-ops engine");
                        if let Some(f) = &first {
                            assert_eq!(&out, f, "{name}: result depends on threads/tasks");
                        }
                        first = Some(out);
                        println!("ok   {name} threads={threads} tasks={tasks} equals the generic-ops engine and every other configuration");
                    }
                }
                println!("== {name}\n{}", baseline.csv());
            }
        }
        "nullcheck" => {
            for name in ["q1", "q6"] {
                let plan = plan_of(name);
                let src = ParquetFile::open(path, plan.columns);
                for pct in [10u32, 50, 100] {
                    let want = if name == "q1" { nulls::reference_q1(&src, pct) } else { nulls::reference_q6(&src, pct) };
                    for threads in [1usize, 4, 8] {
                        let (got, _, _) = run_query(name, path, threads, threads, pct);
                        assert_eq!(got, want, "{name} with {pct}% NULLs, {threads} threads: protocol result disagrees with the scalar reference");
                    }
                    let v = parquet_visit(&src, pct);
                    let g = genops_run(src.row_groups, &v, &plan, 4).expect("generic-ops");
                    assert_eq!(g, want, "{name} {pct}%: generic-ops engine disagrees with the reference");
                    println!("ok   {name} with {pct:>3}% NULLs: protocol at 1, 4, 8 threads == scalar reference == generic-ops engine ({})", want.csv().lines().next().unwrap_or("").chars().take(60).collect::<String>());
                }
            }
        }
        "bench" | "membench" => {
            let in_memory = mode == "membench";
            let rounds: usize = args.get(3).map_or(15, |s| s.parse().expect("number"));
            let threads_list: Vec<usize> = args.get(4).map_or(vec![1, 2, 4, 8], |s| s.split(',').map(|x| x.parse().expect("number")).collect());
            println!(
                "{} -- sanity only unless run on a quiet machine. load average at start: {}\nrounds: {rounds} (+1 warm-up); arms interleaved in rotating order: protocol, generic-ops, protocol-again (control); min / median ms; ratios are medians of paired per-round ratios [Q1, Q3]\n",
                if in_memory { "Engine only (batches already decoded, in memory)" } else { "End to end (Parquet decode included)" },
                load_avg()
            );
            println!("| query | threads | protocol ms | generic-ops ms | protocol / generic-ops [Q1, Q3] | control (protocol again / protocol) [Q1, Q3] |\n|---|---:|---|---|---|---|");
            for name in ["q1", "q6"] {
                let plan = plan_of(name);
                let src = ParquetFile::open(path, plan.columns);
                let file = Arc::new(ParquetFile::open(path, columns(name)));
                let batches = if in_memory { preload(&src, 0) } else { Vec::new() };
                let mem: Option<Arc<Vec<Vec<arrow::array::RecordBatch>>>> = in_memory.then(|| Arc::new(batches.iter().map(|b| vec![b.clone()]).collect()));
                let (pq, mv);
                let (morsels, visit): (usize, Visit) = if in_memory {
                    mv = mem_visit(&batches);
                    (batches.len(), &mv)
                } else {
                    pq = parquet_visit(&src, 0);
                    (src.row_groups, &pq)
                };
                let reference = genops_run(morsels, visit, &plan, 4).expect("generic-ops");
                for &threads in &threads_list {
                    let mut t: [Vec<f64>; 3] = Default::default();
                    for round in 0..=rounds {
                        let mut ms = [0.0f64; 3];
                        for k in 0..3 {
                            let arm = (round + k) % 3;
                            let start = Instant::now();
                            let out = if arm == 1 { genops_run(morsels, visit, &plan, threads).expect("generic-ops") } else { protocol_run(name, &file, mem.as_ref(), threads) };
                            ms[arm] = start.elapsed().as_secs_f64() * 1000.0;
                            assert_eq!(out, reference, "{name} arm {arm} disagrees with the reference result");
                        }
                        if round > 0 {
                            (0..3).for_each(|a| t[a].push(ms[a]));
                        }
                    }
                    let mut pg: Vec<f64> = t[0].iter().zip(&t[1]).map(|(p, g)| p / g).collect();
                    let mut cp: Vec<f64> = t[2].iter().zip(&t[0]).map(|(c, p)| c / p).collect();
                    let cell = |v: &mut Vec<f64>| format!("{:.2} [{:.2}, {:.2}]", quantile(v, 0.5), quantile(v, 0.25), quantile(v, 0.75));
                    let mm = |v: &mut Vec<f64>| format!("{:.0} / {:.0}", quantile(v, 0.0), quantile(v, 0.5));
                    println!("| {name} | {threads} | {} | {} | {} | {} |", mm(&mut t[0]), mm(&mut t[1]), cell(&mut pg), cell(&mut cp));
                }
            }
            println!("\nload average at end: {}", load_avg());
        }
        _ => eprintln!("modes: csv | check | nullcheck | bench | membench"),
    }
    let _ = SeqCst;
}
