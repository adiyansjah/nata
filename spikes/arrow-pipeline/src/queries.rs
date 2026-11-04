//! Q1 and Q6 as pipelines on the protocol scheduler.
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use crate::batch::Ref;
use crate::genops::engine::{Output, Q1_CUTOFF};
use crate::ops::{FilterDate, FilterDec, ProjectOpFactory, StepSpec};
use crate::proto::api::*;
use crate::proto::sched::Query;
use crate::sinks::{Agg, AggSink};
use crate::sources::ParquetScan;

pub const Q1_COLUMNS: &[&str] = &["l_returnflag", "l_linestatus", "l_quantity", "l_extendedprice", "l_discount", "l_shipdate"];
pub const Q6_COLUMNS: &[&str] = &["l_quantity", "l_extendedprice", "l_discount", "l_shipdate"];

pub fn columns(q: &str) -> &'static [&'static str] {
    if q == "q1" { Q1_COLUMNS } else { Q6_COLUMNS }
}

/// SELECT l_returnflag, l_linestatus, sum(l_quantity), sum(l_extendedprice),
///        sum(l_extendedprice * (1 - l_discount)), count(*)
/// FROM lineitem WHERE l_shipdate <= DATE '1998-09-02' GROUP BY 1, 2
pub fn q1_pipeline(scan: Arc<ParquetScan>, tasks: usize) -> (Pipeline, Arc<AggSink>) {
    let sink = AggSink::new(Agg::Q1 { vals: [Ref::Col("l_quantity"), Ref::Col("l_extendedprice"), Ref::Tmp(1)] });
    let ops: Vec<Arc<dyn OperatorFactory>> = vec![
        Arc::new(FilterDate { col: "l_shipdate", lo: i32::MIN, hi: Q1_CUTOFF }),
        Arc::new(ProjectOpFactory(vec![
            StepSpec::SubConstLeft { c: 100, a: Ref::Col("l_discount") },
            StepSpec::Mul { a: Ref::Col("l_extendedprice"), b: Ref::Tmp(0) },
        ])),
    ];
    (Pipeline { name: "q1".into(), source: Arc::new(scan), ops, sink: Arc::new(Arc::clone(&sink)), deps: vec![], tasks }, sink)
}

/// SELECT sum(l_extendedprice * l_discount) FROM lineitem
/// WHERE l_shipdate >= DATE '1994-01-01' AND l_shipdate < DATE '1995-01-01'
///   AND l_discount BETWEEN 0.05 AND 0.07 AND l_quantity < 24
pub fn q6_pipeline(scan: Arc<ParquetScan>, tasks: usize) -> (Pipeline, Arc<AggSink>) {
    let sink = AggSink::new(Agg::Q6 { val: Ref::Tmp(0) });
    let ops: Vec<Arc<dyn OperatorFactory>> = vec![
        Arc::new(FilterDate { col: "l_shipdate", lo: 8766, hi: 9130 }),
        Arc::new(FilterDec { col: "l_discount", lo: 5, hi: 7 }),
        Arc::new(FilterDec { col: "l_quantity", lo: i64::MIN, hi: 2399 }),
        Arc::new(ProjectOpFactory(vec![StepSpec::Mul { a: Ref::Col("l_extendedprice"), b: Ref::Col("l_discount") }])),
    ];
    (Pipeline { name: "q6".into(), source: Arc::new(scan), ops, sink: Arc::new(Arc::clone(&sink)), deps: vec![], tasks }, sink)
}

/// Run a pipeline set; a hang is reported as a failure (deadlock or lost wake-up) after `watchdog`.
/// `Err(None)` = timeout, `Err(Some(e))` = query error.
pub fn run_watched(pipelines: Vec<Pipeline>, threads: usize, watchdog: Duration) -> std::result::Result<(), Option<QueryError>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(Query::new(pipelines).run(threads));
    });
    match rx.recv_timeout(watchdog) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(Some(e)),
        Err(_) => Err(None),
    }
}

/// Run Q1 or Q6 over a Parquet file on the protocol scheduler. Returns the result and the morsels claimed.
pub fn run_query(name: &str, path: &str, threads: usize, tasks: usize, null_pct: u32) -> (Output, usize, usize) {
    let scan = ParquetScan::new(path, columns(name), null_pct);
    let total = scan.morsels();
    let (p, sink) = if name == "q1" { q1_pipeline(Arc::clone(&scan), tasks) } else { q6_pipeline(Arc::clone(&scan), tasks) };
    run_watched(vec![p], threads, Duration::from_secs(120)).unwrap_or_else(|e| panic!("{name}: {e:?}"));
    assert_eq!(sink.combined.load(std::sync::atomic::Ordering::SeqCst), tasks, "every task must combine its partial state");
    (sink.result().expect("finalized"), scan.stats.claimed.load(std::sync::atomic::Ordering::SeqCst), total)
}
