//! The four hard cases from ADR 0001's open check, plus cancellation and failure.
//! Every query runs under a watchdog: a deadlock or a lost wake-up fails the test, it does not hang.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use spike_operator_trait::ops::*;
use spike_operator_trait::*;

fn run(pipelines: Vec<Pipeline>, threads: usize) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(Query::new(pipelines).run(threads));
    });
    let secs = std::env::var("WATCHDOG_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    rx.recv_timeout(Duration::from_secs(secs))
        .expect("query hung: deadlock or lost wake-up")
}

fn pipeline(
    name: &str,
    source: Arc<dyn SourceFactory>,
    ops: Vec<Arc<dyn OperatorFactory>>,
    sink: Arc<dyn GlobalSink>,
    deps: &[usize],
    tasks: usize,
) -> Pipeline {
    Pipeline {
        name: name.into(),
        source,
        ops,
        sink,
        deps: deps.to_vec(),
        tasks,
    }
}

fn rows_gen(
    morsels: usize,
    rows: usize,
    f: impl Fn(usize) -> (i64, i64) + Send + Sync + 'static,
) -> Generator {
    Generator::new(morsels, move |m| {
        let mut b = Batch::new(2);
        for j in 0..rows {
            let (k, v) = f(m * rows + j);
            b.push_row(&[k, v]);
        }
        b
    })
}

fn wait_with_timeout(running: RunningQuery, secs: u64) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(running.wait());
    });
    rx.recv_timeout(Duration::from_secs(secs))
        .expect("wait() hung: a task was never woken after cancel")
}

// ------------------------------------------------------------------ LIMIT

#[test]
fn limit_stops_the_scan_early_and_is_exact() {
    for threads in [1, 4] {
        let source_gen = rows_gen(10_000, 100, |i| (i as i64, 1));
        let out = Collect::new();
        let p = pipeline(
            "scan-limit",
            Arc::new(source_gen.clone()),
            vec![Arc::new(Limit::new(777))],
            Arc::new(out.clone()),
            &[],
            8,
        );
        run(vec![p], threads).expect("query");
        let rows = out.rows();
        assert_eq!(
            rows.len(),
            777,
            "LIMIT must be exact across tasks (threads={threads})"
        );
        let mut keys: Vec<i64> = rows.iter().map(|r| r[0]).collect();
        keys.dedup();
        assert_eq!(keys.len(), 777, "no row may be emitted twice");
        println!("limit: threads={threads} morsels scanned {} of 10000", source_gen.produced());
        assert!(
            source_gen.produced() < 100,
            "scan should stop early, but produced {} of 10000 morsels",
            source_gen.produced()
        );
    }
}

#[test]
fn limit_is_exact_under_heavy_contention() {
    // Many tiny batches and more tasks than threads, so several tasks reach LIMIT at once.
    for round in 0..300 {
        let source_gen = rows_gen(5_000, 10, |i| (i as i64, 1));
        let out = Collect::new();
        let p = pipeline(
            "scan-limit",
            Arc::new(source_gen),
            vec![Arc::new(Limit::new(500))],
            Arc::new(out.clone()),
            &[],
            8,
        );
        run(vec![p], 8).expect("query");
        assert_eq!(out.rows().len(), 500, "round {round}");
    }
}

// ------------------------------------------------------------------ hash join

fn reference_join(build: &[(i64, i64)], probe: &[(i64, i64)]) -> Vec<Vec<i64>> {
    let mut table: HashMap<i64, Vec<i64>> = HashMap::new();
    for (k, v) in build {
        table.entry(*k).or_default().push(*v);
    }
    let mut out = Vec::new();
    for (k, v) in probe {
        for b in table.get(k).into_iter().flatten() {
            out.push(vec![*k, *v, *b]);
        }
    }
    out.sort();
    out
}

fn join_case(
    build_rows: usize,
    build_key_mod: i64,
    probe_rows: usize,
    probe_key_mod: i64,
    tasks: (usize, usize),
    threads: usize,
) -> (usize, usize) {
    let bm = 50;
    let pm = 100;
    let build_gen = rows_gen(build_rows / bm, bm, move |i| {
        (i as i64 % build_key_mod, i as i64)
    });
    let probe_gen = rows_gen(probe_rows / pm, pm, move |i| {
        (i as i64 % probe_key_mod, 1_000_000 + i as i64)
    });
    let build: Vec<(i64, i64)> = (0..build_rows)
        .map(|i| (i as i64 % build_key_mod, i as i64))
        .collect();
    let probe: Vec<(i64, i64)> = (0..probe_rows)
        .map(|i| (i as i64 % probe_key_mod, 1_000_000 + i as i64))
        .collect();

    let jb = JoinBuild::new();
    let out = Collect::new();
    let p_build = pipeline(
        "build",
        Arc::new(build_gen),
        vec![],
        Arc::new(jb.clone()),
        &[],
        tasks.0,
    );
    let p_probe = pipeline(
        "probe",
        Arc::new(probe_gen),
        vec![jb.probe()],
        Arc::new(out.clone()),
        &[0],
        tasks.1,
    );
    run(vec![p_build, p_probe], threads).expect("join query");
    let expected = reference_join(&build, &probe);
    let got = out.rows();
    assert_eq!(got.len(), expected.len(), "row count");
    assert_eq!(got, expected, "join result differs from the reference");
    (got.len(), probe_rows / pm)
}

#[test]
fn hash_join_waits_for_the_build_side_and_matches_the_reference() {
    for threads in [1, 3] {
        // About 3 matches per probe row: output per input batch stays below one output batch.
        let (n, _) = join_case(1000, 300, 4000, 400, (3, 4), threads);
        assert!(n > 0);
    }
}

#[test]
fn hash_join_fan_out_needs_many_calls_per_input_batch() {
    // 2000 build rows over 5 keys: 400 matches per probe row, so one 100-row probe batch yields
    // 40,000 output rows, delivered over about 40 `HaveMoreOutput` calls.
    for threads in [1, 4] {
        let (n, batches) = join_case(2000, 5, 1000, 5, (2, 3), threads);
        assert_eq!(n, 1000 * 400);
        assert!(
            n / batches > 10 * OUT_ROWS,
            "this case must exercise HaveMoreOutput"
        );
    }
}

// ------------------------------------------------------------------ blocking exchange

fn exchange_query(
    morsels: usize,
    prod_tasks: usize,
    cons_tasks: usize,
    cap: usize,
    delay: Option<Duration>,
    consumer_first: bool,
) -> (Vec<Vec<i64>>, usize) {
    let source_gen = rows_gen(morsels, 10, |i| (i as i64, 1));
    let (sink, source, chan) = exchange(cap, prod_tasks, cons_tasks);
    let out = Collect::new();
    let mut cons_ops: Vec<Arc<dyn OperatorFactory>> = Vec::new();
    if let Some(d) = delay {
        cons_ops.push(Arc::new(Delay(d)));
    }
    let producer = pipeline(
        "producer",
        Arc::new(source_gen),
        vec![],
        sink,
        &[],
        prod_tasks,
    );
    let consumer = pipeline(
        "consumer",
        source,
        cons_ops,
        Arc::new(out.clone()),
        &[],
        cons_tasks,
    );
    let threads = 1;
    let pipelines = if consumer_first {
        vec![consumer, producer]
    } else {
        vec![producer, consumer]
    };
    // threads is set by the caller via a thread-local override below
    let _ = threads;
    THREADS
        .with(|t| run(pipelines, t.get()))
        .expect("exchange query");
    (
        out.rows(),
        chan.pushed.load(std::sync::atomic::Ordering::SeqCst),
    )
}

thread_local! {
    static THREADS: std::cell::Cell<usize> = const { std::cell::Cell::new(1) };
}

#[test]
fn a_blocked_consumer_frees_the_only_worker_thread() {
    // One OS thread. The consumer is scheduled first and blocks on an empty exchange; if blocking
    // held the thread, the producer could never run.
    THREADS.with(|t| t.set(1));
    let (rows, pushed) = exchange_query(200, 2, 2, 2, None, true);
    assert_eq!(rows.len(), 2000);
    assert_eq!(pushed, 200);
}

#[test]
fn a_full_exchange_blocks_the_producer_until_the_consumer_drains_it() {
    // Capacity 1 and a slow consumer on one thread: the producer must repeatedly block and be woken.
    THREADS.with(|t| t.set(1));
    let (rows, _) = exchange_query(60, 3, 1, 1, Some(Duration::from_millis(1)), false);
    assert_eq!(rows.len(), 600);
}

#[test]
fn exchange_stress_finds_no_lost_wake_up_or_deadlock() {
    let mut runs = 0;
    for threads in 1..=4 {
        for prod in 1..=3 {
            for cons in 1..=3 {
                for cap in 1..=3 {
                    for round in 0..8 {
                        THREADS.with(|t| t.set(threads));
                        let delay = (round % 4 == 0).then(|| Duration::from_micros(200));
                        let (rows, pushed) =
                            exchange_query(40, prod, cons, cap, delay, round % 2 == 0);
                        assert_eq!(
                            rows.len(),
                            400,
                            "threads={threads} prod={prod} cons={cons} cap={cap}"
                        );
                        assert_eq!(pushed, 40);
                        runs += 1;
                    }
                }
            }
        }
    }
    assert_eq!(runs, 4 * 3 * 3 * 3 * 8);
}

/// A load smoke test: twice as many spinning threads as CPUs while many small queries start, run
/// and shut down. It passed on the buggy scheduler in three trials, so it does not prove the
/// absence of lost-notification bugs; `tests/shutdown.rs` is the deterministic test for that one.
#[test]
fn exchange_stress_under_cpu_contention() {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    let stop = Arc::new(AtomicBool::new(false));
    let cpus = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    let hogs: Vec<_> = (0..2 * cpus)
        .map(|_| {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(SeqCst) {
                    std::hint::spin_loop();
                }
            })
        })
        .collect();
    for round in 0..400 {
        THREADS.with(|t| t.set(1 + round % 4));
        let (rows, pushed) = exchange_query(
            8,
            1 + round % 3,
            1 + (round / 3) % 3,
            1 + round % 2,
            None,
            round % 2 == 0,
        );
        assert_eq!(rows.len(), 80, "round {round}");
        assert_eq!(pushed, 8);
    }
    stop.store(true, SeqCst);
    hogs.into_iter().for_each(|h| h.join().expect("hog"));
}

#[test]
fn a_limit_above_an_exchange_stops_the_producers() {
    let source_gen = rows_gen(100_000, 10, |i| (i as i64, 1));
    let (sink, source, _chan) = exchange(2, 3, 2);
    let out = Collect::new();
    let producer = pipeline(
        "producer",
        Arc::new(source_gen.clone()),
        vec![],
        sink,
        &[],
        3,
    );
    let consumer = pipeline(
        "consumer",
        source,
        vec![Arc::new(Limit::new(50))],
        Arc::new(out.clone()),
        &[],
        2,
    );
    run(vec![producer, consumer], 2).expect("query");
    assert_eq!(out.rows().len(), 50);
    assert!(
        source_gen.produced() < 1000,
        "producers must stop once the consumer is done, but produced {}",
        source_gen.produced()
    );
}

// ------------------------------------------------------------------ finalize with spill

fn agg_case(
    budget: usize,
    partitions: usize,
    keys: usize,
) -> (Aggregate, Vec<Vec<i64>>, Vec<Vec<i64>>) {
    let rows = 500;
    let morsels = 100;
    let f = move |i: usize| ((i % keys) as i64, 1 + (i % 7) as i64);
    let source_gen = rows_gen(morsels, rows, f);
    let mut expect: HashMap<i64, i64> = HashMap::new();
    for i in 0..morsels * rows {
        let (k, v) = f(i);
        *expect.entry(k).or_insert(0) += v;
    }
    let mut expected: Vec<Vec<i64>> = expect.into_iter().map(|(k, v)| vec![k, v]).collect();
    expected.sort();

    let agg = Aggregate::new(AggConfig {
        budget_groups: budget,
        partitions,
    });
    let out = Collect::new();
    let scan = pipeline(
        "scan-agg",
        Arc::new(source_gen),
        vec![],
        Arc::new(agg.clone()),
        &[],
        4,
    );
    let emit = pipeline(
        "emit",
        agg.result_source(),
        vec![],
        Arc::new(out.clone()),
        &[0],
        2,
    );
    run(vec![scan, emit], 3).expect("aggregate query");
    (agg, out.rows(), expected)
}

#[test]
fn finalize_spills_when_over_budget_and_stays_within_it() {
    let (agg, got, expected) = agg_case(200, 256, 20_000);
    assert_eq!(got, expected, "aggregate result differs from the reference");
    let s = agg.stats();
    println!("spill stats: {s:?}");
    assert!(
        s.spilled_entries > 0 && s.spill_files > 0,
        "the budget must force spilling: {s:?}"
    );
    assert!(
        s.peak_local_groups <= 200,
        "a task kept more groups than its budget: {s:?}"
    );
    assert!(
        s.peak_partition_groups <= 200,
        "finalize held more than a budget's worth of groups: {s:?}"
    );
    let leftovers = std::fs::read_dir(agg.spill_dir())
        .map(|d| d.count())
        .unwrap_or(0);
    assert_eq!(leftovers, 0, "spill files must be deleted once read back");
    let dir = agg.spill_dir();
    drop(agg);
    assert!(
        !dir.exists(),
        "the spill directory must disappear with the query state"
    );
}

#[test]
fn no_spill_when_everything_fits() {
    let (agg, got, expected) = agg_case(1_000_000, 8, 20_000);
    assert_eq!(got, expected);
    assert_eq!(agg.stats().spilled_entries, 0);
}

// ------------------------------------------------------------------ cancellation and failure

#[test]
fn cancelling_mid_query_stops_work_and_cleans_up_spill() {
    let source_gen = Generator::new(100_000, |m| {
        std::thread::sleep(Duration::from_millis(2));
        let mut b = Batch::new(2);
        for j in 0..100 {
            b.push_row(&[(m * 100 + j) as i64, 1]);
        }
        b
    });
    let agg = Aggregate::new(AggConfig {
        budget_groups: 100,
        partitions: 16,
    });
    let scan = pipeline(
        "scan-agg",
        Arc::new(source_gen.clone()),
        vec![],
        Arc::new(agg.clone()),
        &[],
        4,
    );
    let running = Query::new(vec![scan]).start(4).expect("start");
    std::thread::sleep(Duration::from_millis(150));
    running.cancel();
    let started = Instant::now();
    let result = wait_with_timeout(running, 10);
    assert_eq!(result, Err(QueryError("cancelled".into())));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "cancellation took {:?}",
        started.elapsed()
    );
    assert!(
        source_gen.produced() < 1000,
        "work must stop: produced {}",
        source_gen.produced()
    );
    assert!(
        agg.stats().spilled_entries > 0,
        "the test must have spilled before cancelling"
    );
    let dir = agg.spill_dir();
    assert!(dir.exists());
    drop(agg);
    assert!(!dir.exists(), "cancelled query left spill files behind");
}

#[test]
fn cancelling_wakes_tasks_that_are_parked_on_an_empty_exchange() {
    // The producer is slow (100 ms per morsel), so the consumer tasks sit parked.
    let source_gen = Generator::new(50, |m| {
        std::thread::sleep(Duration::from_millis(100));
        Batch::from_cols(vec![vec![m as i64], vec![1]])
    });
    let (sink, source, _c) = exchange(2, 1, 3);
    let out = Collect::new();
    let producer = pipeline("producer", Arc::new(source_gen), vec![], sink, &[], 1);
    let consumer = pipeline("consumer", source, vec![], Arc::new(out), &[], 3);
    let running = Query::new(vec![producer, consumer])
        .start(2)
        .expect("start");
    std::thread::sleep(Duration::from_millis(50));
    running.cancel();
    let started = Instant::now();
    assert_eq!(
        wait_with_timeout(running, 10),
        Err(QueryError("cancelled".into()))
    );
    assert!(
        started.elapsed() < Duration::from_millis(900),
        "parked tasks were not woken: {:?}",
        started.elapsed()
    );
}

#[test]
fn an_error_in_one_task_cancels_the_rest_and_is_reported() {
    let source_gen = rows_gen(100_000, 10, |i| (i as i64, 1));
    let out = Collect::new();
    let fail = FailAt {
        at: 5,
        seen: Arc::new(AtomicUsize::new(0)),
    };
    let p = pipeline(
        "scan",
        Arc::new(source_gen.clone()),
        vec![Arc::new(fail)],
        Arc::new(out),
        &[],
        4,
    );
    let e = run(vec![p], 4).expect_err("must fail");
    assert!(e.0.contains("overflow"), "{e}");
    assert!(
        source_gen.produced() < 1000,
        "the other tasks must stop: produced {}",
        source_gen.produced()
    );
}

#[test]
fn a_dependency_cycle_is_rejected() {
    let mk = |deps: &[usize]| {
        pipeline(
            "p",
            Arc::new(rows_gen(1, 1, |i| (i as i64, 1))),
            vec![],
            Arc::new(Collect::new()),
            deps,
            1,
        )
    };
    let e = Query::new(vec![mk(&[1]), mk(&[0])])
        .run(1)
        .expect_err("cycle");
    assert!(e.0.contains("cycle"), "{e}");
}
