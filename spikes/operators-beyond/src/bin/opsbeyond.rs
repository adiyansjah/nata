//! `opsbeyond <query> --data <tpch dir> --out <result.parquet> [--mem-mb N] [--threads N] [--tasks N] [--fanin N] [--grace N] [--spill-dir D]`
//! Queries: q18 q21 q13 sort topn window-peers window-unique join-inner join-left join-right join-full join-semi join-anti.
//! `--grace N` runs the join queries (q13, join-*) as the partitioned spilling join with N partitions. Prints one `key=value` line of statistics.
use std::sync::atomic::Ordering::SeqCst;
use std::time::{Duration, Instant};

use arrow::array::RecordBatch;
use spike_operators_beyond::join::JoinKind;
use spike_operators_beyond::queries::*;
use spike_operators_beyond::spill::Budget;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let q = a.get(1).cloned().unwrap_or_else(|| usage());
    let get = |k: &str, d: &str| a.iter().position(|x| x == k).map_or(d.to_string(), |i| a[i + 1].clone());
    let data = get("--data", "data/tpch-sf1");
    let out = get("--out", "");
    let mem_mb: usize = get("--mem-mb", "0").parse().unwrap();
    let threads: usize = get("--threads", &std::thread::available_parallelism().map_or(4, |n| n.get()).to_string()).parse().unwrap();
    let tasks: usize = get("--tasks", &threads.to_string()).parse().unwrap();
    let fanin: usize = get("--fanin", "8").parse().unwrap();
    let grace: usize = get("--grace", "0").parse().unwrap();
    let dir = get("--spill-dir", &format!("{}/opsbeyond-spill-{}", std::env::temp_dir().display(), std::process::id()));
    let budget = Budget::new(if mem_mb == 0 { usize::MAX } else { mem_mb << 20 }, std::path::Path::new(&dir));
    let d = Data { dir: data };
    let mode = if grace > 0 { Mode::Grace(grace) } else { Mode::Mem };
    let (ps, sink, schema) = match q.as_str() {
        "q18" => q18(&d, &budget, tasks),
        "q21" => q21(&d, &budget, tasks),
        "q13" => q13_nolike(&d, mode, &budget, fanin, tasks),
        "sort" => lineitem_sort(&d, &budget, fanin, tasks),
        "topn" => lineitem_topn(&d, tasks),
        "window-peers" => window_orders(&d, true, &budget, fanin, tasks),
        "window-unique" => window_orders(&d, false, &budget, fanin, tasks),
        j if j.starts_with("join-") => {
            let kind = match &j[5..] {
                "inner" => JoinKind::Inner,
                "left" => JoinKind::Left,
                "right" => JoinKind::Right,
                "full" => JoinKind::Full,
                "semi" => JoinKind::Semi,
                "anti" => JoinKind::Anti,
                _ => usage(),
            };
            join_counts(&d, kind, mode, &budget, tasks)
        }
        _ => usage(),
    };
    let t0 = Instant::now();
    let res = run(ps, threads, Duration::from_secs(3600));
    let secs = t0.elapsed().as_secs_f64();
    if let Err(e) = res {
        println!("query={q} status=FAILED error={e:?} secs={secs:.2}");
        std::process::exit(1);
    }
    let batches: Vec<RecordBatch> = sink.result();
    let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    if !out.is_empty() {
        spike_operators_beyond::io::write_parquet(&out, &schema, &batches);
    }
    println!(
        "query={q} status=ok rows={rows} secs={secs:.2} threads={threads} tasks={tasks} mem_mb={mem_mb} peak_reserved_mb={} spilled_mb={} spill_files={} max_open_files={} leftover_spill_files={}",
        budget.peak.load(SeqCst) >> 20,
        budget.spilled_bytes.load(SeqCst) >> 20,
        budget.files_created.load(SeqCst),
        budget.max_open.load(SeqCst),
        budget.leftover_files()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn usage() -> ! {
    eprintln!("usage: opsbeyond <q18|q21|q13|sort|topn|window-peers|window-unique|join-inner|join-left|join-right|join-full|join-semi|join-anti> [--data D] [--out F] [--mem-mb N] [--threads N] [--tasks N] [--fanin N] [--grace N] [--spill-dir D]");
    std::process::exit(2)
}
