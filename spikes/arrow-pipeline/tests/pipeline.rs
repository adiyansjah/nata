//! The protocol carrying Arrow batches: no copies, selection-aware errors, error propagation.
mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering::SeqCst;

use arrow::array::AsArray;
use arrow::datatypes::Decimal64Type;
use common::*;
use spike_arrow_pipeline::batch::Batch;
use spike_arrow_pipeline::ops::{FilterDec, truncate};
use spike_arrow_pipeline::proto::api::*;
use spike_arrow_pipeline::sources::Chaos;
use spike_arrow_pipeline::synth::*;

#[test]
fn operators_share_the_column_buffers_and_never_copy_data() {
    let mut cfg = SynthCfg::random(7);
    cfg.morsels = 1;
    cfg.max_rows = 50;
    cfg.max_batches_per_morsel = 1;
    let (morsels, _) = generate(&SynthCfg { stripe: Some((40, 10)), ..cfg });
    let input = Batch::new(&morsels[0][0]);
    let mut out = Batch::default();
    let mut op = FilterDec { col: "a", lo: 1, hi: 1 }.make();
    assert!(matches!(op.execute(&input, &mut out), Ok(OpResult::NeedMoreInput)));
    assert_eq!(out.len(), 10);
    assert_eq!(input.len(), 40, "the input batch is not modified");
    let ptr = |b: &Batch| b.column("a").as_primitive::<Decimal64Type>().values().as_ptr();
    assert_eq!(ptr(&input), ptr(&out), "the filter output must point at the same column buffer");
    // LIMIT's truncation also only cuts the selection.
    let cut = truncate(&out, 3);
    assert_eq!((cut.len(), ptr(&cut) == ptr(&input)), (3, true));
}

fn overflow_case(huge_c_pct: u64, filters: Vec<F>, seed: u64) -> Case {
    let mut data = SynthCfg::random(seed);
    data.morsels = 30;
    data.huge_c_pct = huge_c_pct;
    // k large: read everything, so any error is found
    Case { data, filters, project: true, k: usize::MAX / 2, threads: 4, tasks: 4, sink_limit: false, chaos: Chaos::default(), sink_chaos: None }
}

#[test]
fn an_overflow_in_a_row_the_filter_removed_is_not_an_error_but_a_selected_one_is() {
    // c is huge in ~30% of the rows, so a * c overflows there. Guard: c in [-20, 20] removes exactly those rows.
    for seed in 0..40 {
        let guarded = overflow_case(30, vec![F::C(-20, 20)], seed);
        let run = run_case(&guarded, watchdog());
        check_run(&guarded, &run); // result Ok, rows equal the reference, p equals checked_mul
        // Without the guard, a selected valid row with a huge c overflows: the query must fail, with an error.
        let unguarded = overflow_case(30, vec![], seed);
        let (_, all) = generate(&unguarded.data);
        let should_fail = all.iter().any(|r| ref_p(r).is_err());
        let run = run_case(&unguarded, watchdog());
        match (&run.result, should_fail) {
            (Err(Some(e)), true) => assert!(e.0.contains("overflow"), "{e}"),
            (Ok(()), false) => {}
            (r, f) => panic!("seed {seed}: expected failure={f}, got {r:?}"),
        }
    }
}

#[test]
fn an_error_in_one_task_stops_the_other_tasks_and_reports_it() {
    let mut data = SynthCfg::random(3);
    data.morsels = 400;
    data.huge_c_pct = 5;
    data.max_rows = 40;
    let (_, all) = generate(&data);
    assert!(all.iter().any(|r| ref_p(r).is_err()), "the data must contain an overflow");
    let case = Case { data, filters: vec![], project: true, k: usize::MAX / 2, threads: 4, tasks: 8, sink_limit: false, chaos: Chaos::default(), sink_chaos: None };
    let run = run_case(&case, watchdog());
    let e = run.result.expect_err("overflow must fail the query").expect("an error, not a timeout");
    assert!(e.0.contains("overflow"));
    assert!(run.stats.claimed.load(SeqCst) < 400, "the other tasks should have stopped");
    assert_eq!(run.stats.finished.load(SeqCst), 8, "all tasks ended and called Source::finish");
}

#[test]
fn a_blocked_sink_leaves_the_arrow_batch_untouched_and_loses_no_row() {
    use spike_arrow_pipeline::sinks::SinkChaos;
    for seed in 0..200u64 {
        let mut c = random_case(seed);
        c.k = usize::MAX / 2; // no LIMIT effect: every filtered row must arrive
        c.sink_limit = false;
        c.sink_chaos = Some(SinkChaos { seed, block_per_mille: 400, max_block_us: 50 });
        let run = run_case(&c, watchdog());
        check_run(&c, &run);
        let n = filtered(&run.all, &c.filters).len();
        assert_eq!(run.rows.len(), n);
    }
}

#[test]
fn rows_are_not_lost_without_a_limit_across_threads_tasks_and_nulls() {
    for seed in 1000..1300u64 {
        let mut c = random_case(seed);
        c.k = usize::MAX / 2;
        let run = run_case(&c, watchdog());
        check_run(&c, &run);
        assert_eq!(run.stats.claimed.load(SeqCst), run.total_morsels, "no LIMIT: every morsel is read");
        let _ = Arc::new(0);
    }
}

#[test]
fn overflow_checks_are_on_in_this_build() {
    // ADR 0003 decision 6: `[profile.release] overflow-checks = true`. Every test, the DuckDB diff, the mutation
    // run and the benchmark of this spike must run with it. A plain `+` on i64::MAX must panic.
    let r = std::panic::catch_unwind(|| std::hint::black_box(i64::MAX) + std::hint::black_box(1));
    assert!(r.is_err(), "integer overflow did not panic: this build has overflow-checks OFF");
}
