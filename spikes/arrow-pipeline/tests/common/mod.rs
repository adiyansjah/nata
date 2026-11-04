//! Shared helpers for the LIMIT / early-finish tests: build a pipeline over synthetic data, run it under a
//! watchdog, and check it against the scalar reference.
#![allow(dead_code)]
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::mpsc;
use std::time::Duration;

use spike_arrow_pipeline::batch::Batch;
use spike_arrow_pipeline::ops::Limit;
use spike_arrow_pipeline::proto::api::*;
use spike_arrow_pipeline::proto::sched::Query;
use spike_arrow_pipeline::sinks::{CollectSink, SinkChaos};
use spike_arrow_pipeline::sources::{Chaos, MemScan, ScanStats};
use spike_arrow_pipeline::synth::*;

pub fn watchdog() -> Duration {
    Duration::from_secs(std::env::var("WATCHDOG_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(30))
}

pub fn lineitem() -> String {
    std::env::var("LINEITEM").unwrap_or_else(|_| "../../data/tpch-sf1/lineitem.parquet".into())
}

#[derive(Clone, Debug)]
pub struct Case {
    pub data: SynthCfg,
    pub filters: Vec<F>,
    pub project: bool,
    pub k: usize,
    pub threads: usize,
    pub tasks: usize,
    /// `true`: the LIMIT is the sink (`SinkResult::Finished`); `false`: a `LIMIT` operator (`OpResult::Finished`).
    pub sink_limit: bool,
    pub chaos: Chaos,
    pub sink_chaos: Option<SinkChaos>,
}

pub struct Run {
    pub result: std::result::Result<(), Option<QueryError>>,
    pub rows: Vec<OutRow>,
    pub stats: Arc<ScanStats>,
    pub total_morsels: usize,
    pub all: Vec<Row>,
}

pub fn build(case: &Case) -> (Pipeline, Arc<MemScan>, Arc<CollectSink>, Vec<Row>) {
    let (morsels, all) = generate(&case.data);
    let scan = MemScan::new(morsels, case.chaos.clone());
    let mut ops: Vec<Arc<dyn OperatorFactory>> = case.filters.iter().map(F::op).collect();
    if case.project {
        ops.push(project_op());
    }
    let sink = if case.sink_limit {
        CollectSink::with_limit(case.k)
    } else {
        ops.push(Arc::new(Limit::new(case.k)));
        CollectSink::new()
    };
    let sink = match &case.sink_chaos {
        Some(c) => sink.blocking(c.clone()),
        None => sink,
    };
    let p = Pipeline { name: "limit".into(), source: Arc::new(Arc::clone(&scan)), ops, sink: Arc::new(Arc::clone(&sink)), deps: vec![], tasks: case.tasks };
    (p, scan, sink, all)
}

pub fn run_watched(p: Pipeline, threads: usize, dog: Duration) -> std::result::Result<(), Option<QueryError>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(Query::new(vec![p]).run(threads));
    });
    match rx.recv_timeout(dog) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(Some(e)),
        Err(_) => Err(None),
    }
}

pub fn run_case(case: &Case, dog: Duration) -> Run {
    let (p, scan, sink, all) = build(case);
    let result = run_watched(p, case.threads, dog);
    let batches: Vec<Batch> = sink.batches();
    Run { result, rows: read_rows(&batches), stats: Arc::clone(&scan.stats), total_morsels: scan.morsel_count(), all }
}

/// Properties (a) and the bookkeeping of (b): panics with the case on any violation.
pub fn check_run(case: &Case, run: &Run) {
    let ctx = || format!("case {case:?}");
    match &run.result {
        Ok(()) => {}
        Err(None) => panic!("TIMEOUT: query did not terminate within the watchdog (hang or lost wake-up; under heavy load this is inconclusive: re-run alone) {}", ctx()),
        Err(Some(e)) => panic!("query failed: {e} {}", ctx()),
    }
    let expected = filtered(&run.all, &case.filters);
    let by_id: HashMap<i64, &Row> = expected.iter().map(|r| (r.id, r)).collect();
    // (a) exactly min(k, n) rows
    assert_eq!(run.rows.len(), case.k.min(expected.len()), "row count: want min(k, n_filtered) {}", ctx());
    // (a) every row is in the full filtered result, none twice (ids are unique, so this is multiset containment)
    let mut seen = HashSet::new();
    for r in &run.rows {
        assert!(seen.insert(r.row.id), "row id {} emitted twice {}", r.row.id, ctx());
        let want = by_id.get(&r.row.id).unwrap_or_else(|| panic!("row id {} is not in the filtered result {}", r.row.id, ctx()));
        assert_eq!(&r.row, *want, "row content differs {}", ctx());
        if case.project {
            let want_p = ref_p(&r.row).expect("these cases never overflow");
            assert_eq!(r.p, Some(want_p), "project column of row {} {}", r.row.id, ctx());
        } else {
            assert_eq!(r.p, None);
        }
    }
    // (b) bookkeeping: every task was created, ended and told its source (Source::finish)
    let s = &run.stats;
    assert_eq!(s.made.load(SeqCst), case.tasks, "tasks created {}", ctx());
    assert_eq!(s.finished.load(SeqCst), case.tasks, "every task must end and call Source::finish {}", ctx());
    assert!(s.claimed.load(SeqCst) <= run.total_morsels, "claimed more morsels than exist {}", ctx());
}

/// A random case from a seed. `n_f` and `k` are chosen after the data exists, so the interesting `k` values
/// (0, 1, exact count, count +- 1, far above) all occur.
pub fn random_case(seed: u64) -> Case {
    let mut rng = Rng(seed);
    let morsels = [0usize, 1, 2, 5, 20, 60][rng.below(6) as usize];
    let mut data = SynthCfg::random(seed);
    data.morsels = morsels;
    data.null_pct = [0u64, 10, 50, 100][rng.below(4) as usize];
    let nf = rng.below(4) as usize;
    let filters: Vec<F> = (0..nf).map(|_| F::random(&mut rng)).collect();
    let (_, all) = generate(&data);
    let n = filtered(&all, &filters).len();
    let k = match rng.below(7) {
        0 => 0,
        1 => 1,
        2 => n,
        3 => n.saturating_sub(1),
        4 => n + 1,
        5 => n * 2 + 5,
        _ => rng.below(n as u64 + 3) as usize,
    };
    let threads = [1usize, 2, 4, 8][rng.below(4) as usize];
    let tasks = [1, threads, threads + 1, 2 * threads][rng.below(4) as usize];
    let chaos = match rng.below(3) {
        0 => Chaos::default(),
        1 => Chaos { seed, delay_per_mille: 100, max_delay_us: 50, ..Chaos::default() },
        _ => Chaos { seed, delay_per_mille: 50, max_delay_us: 30, block_per_mille: 80, max_block_us: 40, stuck_from_task: None },
    };
    let sink_chaos = (rng.below(3) == 0).then(|| SinkChaos { seed, block_per_mille: 100, max_block_us: 30 });
    Case { data, filters, project: rng.below(2) == 0, k, threads, tasks, sink_limit: rng.below(2) == 0, chaos, sink_chaos }
}

/// A case where `tasks - 1` tasks are stuck on their source forever: only early finish (or cancel) can end them.
/// Requires `1 <= k <= n_filtered`, so the one live task can satisfy the LIMIT.
pub fn stuck_case(seed: u64) -> Option<Case> {
    let mut c = random_case(seed);
    let (_, all) = generate(&c.data);
    let n = filtered(&all, &c.filters).len();
    if n == 0 {
        return None;
    }
    let mut rng = Rng(seed ^ 0x5151);
    c.k = 1 + rng.below(n as u64) as usize;
    c.tasks = 2 + rng.below(6) as usize;
    c.chaos.stuck_from_task = Some(1);
    Some(c)
}
