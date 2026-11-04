//! Q1 and Q6 on the protocol scheduler over the real Parquet file: equal to the generic-ops engine, independent of
//! threads and tasks, equal to the scalar reference with NULLs. (The DuckDB diff is scripts/duckdb_diff.sh.)
mod common;

use std::sync::atomic::Ordering::SeqCst;

use common::*;
use spike_arrow_pipeline::genops::engine::{Output, q1, q6, run as genops_run};
use spike_arrow_pipeline::genops::nulls::{self, parquet_visit};
use spike_arrow_pipeline::genops::source::Source as ParquetFile;
use spike_arrow_pipeline::queries::{columns, q1_pipeline, q6_pipeline, run_query, run_watched};
use spike_arrow_pipeline::sources::{Chaos, ParquetScan};
use std::time::Duration;

fn baseline(name: &str) -> Output {
    let plan = if name == "q1" { q1() } else { q6() };
    let src = ParquetFile::open(&lineitem(), plan.columns);
    genops_run(src.row_groups, &parquet_visit(&src, 0), &plan, 4).expect("generic-ops")
}

#[test]
fn q1_and_q6_equal_the_generic_ops_engine_at_every_thread_and_task_count() {
    for name in ["q1", "q6"] {
        let want = baseline(name);
        for threads in [1usize, 2, 4, 8] {
            for tasks in [1usize, threads, 2 * threads, 3] {
                let (got, claimed, total) = run_query(name, &lineitem(), threads, tasks, 0);
                assert_eq!(claimed, total, "a full scan reads every morsel");
                assert_eq!(got, want, "{name} threads={threads} tasks={tasks}");
            }
        }
    }
}

#[test]
fn q1_and_q6_with_synthetic_nulls_equal_the_scalar_reference() {
    for name in ["q1", "q6"] {
        let plan = if name == "q1" { q1() } else { q6() };
        let src = ParquetFile::open(&lineitem(), plan.columns);
        for pct in [10u32, 50, 100] {
            let want = if name == "q1" { nulls::reference_q1(&src, pct) } else { nulls::reference_q6(&src, pct) };
            for threads in [1usize, 4, 8] {
                let (got, _, _) = run_query(name, &lineitem(), threads, threads, pct);
                assert_eq!(got, want, "{name} {pct}% NULLs {threads} threads");
            }
        }
    }
}

#[test]
fn q1_and_q6_survive_blocked_sources_and_slow_stages() {
    // The same queries with the source answering Blocked now and then (a helper thread wakes the task) and
    // sleeping now and then: parking, waking and the combine step all happen, the result must not change.
    for name in ["q1", "q6"] {
        let want = baseline(name);
        for seed in 0..6u64 {
            let chaos = Chaos { seed, delay_per_mille: 100, max_delay_us: 200, block_per_mille: 150, max_block_us: 300, stuck_from_task: None };
            let scan = ParquetScan::with_chaos(&lineitem(), columns(name), 0, chaos);
            let (p, sink) = if name == "q1" { q1_pipeline(scan.clone(), 6) } else { q6_pipeline(scan.clone(), 6) };
            run_watched(vec![p], 3, Duration::from_secs(120)).expect("query");
            assert_eq!(sink.result().expect("result"), want, "{name} seed {seed}");
            assert!(scan.stats.blocked.load(SeqCst) > 0, "the chaos never blocked: the test is vacuous");
            assert_eq!(scan.stats.finished.load(SeqCst), 6);
        }
    }
}

#[test]
fn limit_over_the_parquet_scan_stops_reading_row_groups() {
    use spike_arrow_pipeline::batch::Ref;
    use spike_arrow_pipeline::ops::{FilterDec, Limit};
    use spike_arrow_pipeline::proto::api::{OperatorFactory, Pipeline};
    use spike_arrow_pipeline::sinks::CollectSink;
    let _ = Ref::Col("x");
    for &(k, threads) in &[(1usize, 1usize), (100, 1), (100, 4), (1000, 8), (50_000, 4)] {
        let scan = ParquetScan::new(&lineitem(), &["l_quantity", "l_discount"], 0);
        let total = scan.morsels();
        let sink = CollectSink::new();
        let ops: Vec<std::sync::Arc<dyn OperatorFactory>> = vec![std::sync::Arc::new(FilterDec { col: "l_discount", lo: 5, hi: 7 }), std::sync::Arc::new(Limit::new(k))];
        let p = Pipeline { name: "limit".into(), source: std::sync::Arc::new(scan.clone()), ops, sink: std::sync::Arc::new(sink.clone()), deps: vec![], tasks: threads };
        run_watched(vec![p], threads, Duration::from_secs(60)).expect("query");
        let rows: usize = sink.batches().iter().map(|b| b.len()).sum();
        let claimed = scan.stats.claimed.load(SeqCst);
        println!("LIMIT {k} threads={threads}: {rows} rows, morsels read {claimed} of {total}");
        assert_eq!(rows, k, "exact row count");
        // Every row group of lineitem SF1 has ~120k rows and ~20% match: k <= 50,000 needs at most 2 row groups.
        assert!(claimed <= 2 + 2 * threads, "read {claimed} of {total} row groups");
        assert!(claimed < total);
        assert_eq!(scan.stats.finished.load(SeqCst), threads);
    }
}
