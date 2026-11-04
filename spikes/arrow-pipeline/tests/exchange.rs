//! Early finish ACROSS an exchange, with Arrow batches: scan -> filter -> [compact] exchange -> LIMIT -> collect.
//! The consumer's LIMIT finishes; the consumer pipeline's tasks end; `Source::finish` closes the channel; the
//! producer sink answers `Finished`; the producer pipeline (and its scan) stops. Both pipelines run at once.
mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering::SeqCst;

use common::*;
use spike_arrow_pipeline::batch::Batch;
use spike_arrow_pipeline::exchange::exchange;
use spike_arrow_pipeline::ops::Limit;
use spike_arrow_pipeline::proto::api::*;
use spike_arrow_pipeline::sinks::CollectSink;
use spike_arrow_pipeline::sources::{Chaos, MemScan};
use spike_arrow_pipeline::synth::*;

struct Out {
    result: std::result::Result<(), Option<QueryError>>,
    rows: Vec<OutRow>,
    claimed: usize,
    pushed: usize,
    uncompacted: usize,
    finished: usize,
    prod_tasks: usize,
    all: Vec<Row>,
    filters: Vec<F>,
}

fn run_exchange(seed: u64, morsels: usize, k: usize, threads: usize, cap: usize, prod_tasks: usize, cons_tasks: usize, stripe: Option<(usize, usize)>) -> Out {
    let mut rng = Rng(seed);
    let mut data = SynthCfg::random(seed);
    data.morsels = morsels;
    data.null_pct = [0u64, 20][rng.below(2) as usize];
    data.stripe = stripe;
    let filters = if stripe.is_some() { vec![F::A(1, 1)] } else { (0..rng.below(3)).map(|_| F::random(&mut rng)).collect() };
    let (m, all) = generate(&data);
    let scan = MemScan::new(m, Chaos { seed, delay_per_mille: 100, max_delay_us: 30, ..Chaos::default() });
    let (xsink, xsrc, chan) = exchange(cap, prod_tasks, cons_tasks);
    let producer = Pipeline { name: "produce".into(), source: Arc::new(Arc::clone(&scan)), ops: filters.iter().map(F::op).collect(), sink: xsink, deps: vec![], tasks: prod_tasks };
    let sink = CollectSink::new();
    let consumer = Pipeline { name: "consume".into(), source: xsrc, ops: vec![Arc::new(Limit::new(k))], sink: Arc::new(Arc::clone(&sink)), deps: vec![], tasks: cons_tasks };
    let result = spike_arrow_pipeline::queries::run_watched(vec![producer, consumer], threads, watchdog());
    let batches: Vec<Batch> = sink.batches();
    Out { result, rows: read_rows(&batches), claimed: scan.stats.claimed.load(SeqCst), pushed: chan.pushed.load(SeqCst), uncompacted: chan.uncompacted.load(SeqCst), finished: scan.stats.finished.load(SeqCst), prod_tasks, all, filters }
}

#[test]
fn limit_above_an_exchange_is_exact_and_correct_on_random_cases() {
    for seed in 0..500u64 {
        let mut rng = Rng(seed ^ 0x77);
        let morsels = [0usize, 1, 3, 20, 60][rng.below(5) as usize];
        let threads = [1usize, 2, 4][rng.below(3) as usize];
        let cap = 1 + rng.below(3) as usize;
        let (pt, ct) = (1 + rng.below(4) as usize, 1 + rng.below(3) as usize);
        let k = [0usize, 1, 7, 50, 100_000][rng.below(5) as usize];
        let o = run_exchange(seed, morsels, k, threads, cap, pt, ct, None);
        assert!(o.result.is_ok(), "seed {seed}: {:?}", o.result);
        let expected = filtered(&o.all, &o.filters);
        assert_eq!(o.rows.len(), k.min(expected.len()), "seed {seed} row count");
        let ids: std::collections::HashSet<i64> = expected.iter().map(|r| r.id).collect();
        let mut seen = std::collections::HashSet::new();
        for r in &o.rows {
            assert!(ids.contains(&r.row.id) && seen.insert(r.row.id), "seed {seed}: row {} not in result or duplicated", r.row.id);
            assert_eq!(&r.row, expected.iter().find(|e| e.id == r.row.id).expect("row"));
        }
        assert_eq!(o.finished, o.prod_tasks, "seed {seed}: every producer task ended");
        assert_eq!(o.uncompacted, 0, "seed {seed}: {} of {} batches entered the exchange with a selection vector", o.uncompacted, o.pushed);
    }
}

#[test]
fn early_finish_crosses_the_exchange_and_stops_the_scan() {
    let total = 3000;
    for &(threads, pt, ct, cap) in &[(1usize, 1usize, 1usize, 1usize), (2, 2, 1, 1), (4, 3, 2, 2), (4, 4, 4, 1)] {
        for k in [1usize, 10, 100] {
            for rep in 0..10u64 {
                let o = run_exchange(rep, total, k, threads, cap, pt, ct, Some((64, 8)));
                assert!(o.result.is_ok(), "{:?}", o.result);
                assert_eq!(o.rows.len(), k);
                // Producers may run ahead of the consumer by the channel capacity plus one batch per task.
                let bound = k.div_ceil(8) + cap + 3 * (pt + ct);
                assert!(o.claimed <= bound && o.claimed < total, "scan read {} of {total} morsels (bound {bound}) threads={threads} pt={pt} ct={ct} cap={cap} k={k}", o.claimed);
                assert_eq!(o.finished, pt);
            }
        }
    }
}

#[test]
fn compaction_before_the_exchange_applies_the_selection_and_the_computed_columns() {
    let mut data = SynthCfg::random(5);
    data.morsels = 1;
    data.max_batches_per_morsel = 1;
    data.max_rows = 80;
    data.stripe = Some((50, 9));
    let (m, _) = generate(&data);
    let b = Batch::new(&m[0][0]);
    let mut out = Batch::default();
    let mut f = F::A(1, 1).op().make();
    f.execute(&b, &mut out).expect("filter");
    let mut p = project_op().make();
    let mut out2 = Batch::default();
    p.execute(&out, &mut out2).expect("project");
    let c = out2.compact().expect("compact");
    assert_eq!((c.rows(), c.len(), c.sel().is_none()), (9, 9, true), "after compaction the batch has exactly the live rows and no selection");
    assert_eq!(read_rows(&[out2]), read_rows(&[c]), "same rows, same computed column");
}
