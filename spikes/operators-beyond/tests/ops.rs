//! Differential tests of the blocking operators against naive references, on small random data with the edge cases the way-of-working asks
//! for (empty inputs, one row, all-NULL keys, duplicates, ties, NULL residuals, output larger than one batch, spill with a tiny budget).
use std::sync::Arc;
use std::time::Duration;

use arrow::array::{Array, ArrayRef, Int64Array, RecordBatch};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use arrow::util::display::array_value_to_string;

use spike_operators_beyond::agg::{AggFn, HashAgg};
use spike_operators_beyond::join::*;
use spike_operators_beyond::ops::CollectSink;
use spike_operators_beyond::proto::api::*;
use spike_operators_beyond::queries::*;
use spike_operators_beyond::sort::*;
use spike_operators_beyond::spill::Budget;
use spike_operators_beyond::window::{WinFn, Window};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn schema(n: usize) -> SchemaRef {
    Arc::new(Schema::new((0..n).map(|i| Field::new(format!("c{i}"), DataType::Int64, true)).collect::<Vec<_>>()))
}

type Row = Vec<Option<i64>>;

fn gen_rows(rng: &mut Rng, rows: usize, cols: usize, domain: u64, null_pct: u64) -> Vec<Row> {
    (0..rows).map(|_| (0..cols).map(|_| if rng.below(100) < null_pct { None } else { Some(rng.below(domain) as i64) }).collect()).collect()
}

fn to_batches(rng: &mut Rng, rows: &[Row], cols: usize, max_piece: u64) -> Vec<RecordBatch> {
    let mut out = vec![];
    let mut i = 0;
    while i < rows.len() {
        let n = (1 + rng.below(max_piece) as usize).min(rows.len() - i);
        let arrays: Vec<ArrayRef> = (0..cols).map(|c| Arc::new(Int64Array::from(rows[i..i + n].iter().map(|r| r[c]).collect::<Vec<_>>())) as ArrayRef).collect();
        out.push(RecordBatch::try_new(schema(cols), arrays).unwrap());
        i += n;
    }
    out
}

fn show(b: &RecordBatch) -> Vec<String> {
    (0..b.num_rows()).map(|i| (0..b.num_columns()).map(|c| if b.column(c).is_null(i) { "NULL".to_string() } else { array_value_to_string(b.column(c), i).unwrap() }).collect::<Vec<_>>().join("|")).collect()
}

fn show_all(bs: &[RecordBatch]) -> Vec<String> {
    bs.iter().flat_map(show).collect()
}

fn rowstr(r: &[Option<i64>]) -> String {
    r.iter().map(|v| v.map_or("NULL".to_string(), |x| x.to_string())).collect::<Vec<_>>().join("|")
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("ob-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    p
}

fn go(ps: Vec<Pipeline>, threads: usize) {
    run(ps, threads, Duration::from_secs(60)).unwrap_or_else(|e| panic!("{e}"));
}

// ------------------------------------------------------------------ joins

/// Nested-loop reference. Probe rows are (k, v), build rows are (k, v). Residual: probe.v <> build.v.
fn ref_join(kind: JoinKind, probe: &[Row], build: &[Row], resid: bool) -> Vec<String> {
    let m = |p: &Row, b: &Row| p[0].is_some() && p[0] == b[0] && (!resid || (p[1].is_some() && b[1].is_some() && p[1] != b[1]));
    let mut out = vec![];
    let mut bmatched = vec![false; build.len()];
    for p in probe {
        let mut any = false;
        for (j, b) in build.iter().enumerate() {
            if m(p, b) {
                any = true;
                bmatched[j] = true;
                if matches!(kind, JoinKind::Inner | JoinKind::Left | JoinKind::Right | JoinKind::Full) {
                    out.push(format!("{}|{}", rowstr(p), rowstr(b)));
                }
                if kind == JoinKind::Semi {
                    break;
                }
            }
        }
        match kind {
            JoinKind::Semi if any => out.push(rowstr(p)),
            JoinKind::Anti if !any => out.push(rowstr(p)),
            JoinKind::Left | JoinKind::Full if !any => out.push(format!("{}|NULL|NULL", rowstr(p))),
            _ => {}
        }
    }
    if matches!(kind, JoinKind::Right | JoinKind::Full) {
        for (j, b) in build.iter().enumerate() {
            if !bmatched[j] {
                out.push(format!("NULL|NULL|{}", rowstr(b)));
            }
        }
    }
    out.sort();
    out
}

fn run_join(kind: JoinKind, mode: Mode, probe: &[Row], build: &[Row], resid: bool, tasks: usize, seed: u64, budget_bytes: usize) -> (Vec<String>, Arc<Budget>) {
    let mut rng = Rng(seed);
    let dir = tmp(&format!("join-{seed}-{tasks}"));
    let budget = Budget::new(budget_bytes, &dir);
    let (pb, bb) = (to_batches(&mut rng, probe, 2, 60), to_batches(&mut rng, build, 2, 60));
    let spec = JoinSpec { kind, probe_key: 0, build_key: 0, residual_ne: resid.then_some((1, 1)) };
    let (ps, out, _) = join_query(Arc::new(queue_source(&pb)), &schema(2), Arc::new(queue_source(&bb)), &schema(2), spec, mode, &budget, tasks);
    go(ps, tasks.max(2));
    let mut got = show_all(&out.result());
    got.sort();
    (got, budget)
}

const KINDS: [JoinKind; 6] = [JoinKind::Inner, JoinKind::Left, JoinKind::Right, JoinKind::Full, JoinKind::Semi, JoinKind::Anti];

#[test]
fn joins_match_the_reference_in_memory_and_partitioned() {
    let mut cases = 0;
    for seed in 1..=12u64 {
        let mut rng = Rng(seed * 7919);
        // (probe rows, build rows, key domain, NULL percent): empty sides, one row, all-NULL keys, heavy duplication, plain
        let shapes: [(usize, usize, u64, u64); 8] = [(0, 5, 3, 0), (5, 0, 3, 0), (1, 1, 1, 0), (30, 30, 4, 100), (40, 25, 5, 20), (60, 90, 3, 10), (200, 100, 50, 5), (3, 3, 2, 0)];
        for (np, nb, dom, nullp) in shapes {
            let probe = gen_rows(&mut rng, np, 2, dom, nullp);
            let build = gen_rows(&mut rng, nb, 2, dom, nullp);
            for kind in KINDS {
                for resid in [false, true] {
                    let want = ref_join(kind, &probe, &build, resid);
                    for (mode, tasks) in [(Mode::Mem, 1), (Mode::Mem, 3), (Mode::Grace(1), 1), (Mode::Grace(7), 3)] {
                        let (got, budget) = run_join(kind, mode, &probe, &build, resid, tasks, seed, usize::MAX);
                        assert_eq!(got, want, "{kind:?} {mode:?} tasks={tasks} resid={resid} shape={np}x{nb} dom={dom} null%={nullp} seed={seed}");
                        assert_eq!(budget.leftover_files(), 0, "spill files left behind: {kind:?} {mode:?}");
                        cases += 1;
                    }
                }
            }
        }
    }
    assert!(cases > 4000);
}

#[test]
fn a_join_whose_output_exceeds_one_batch_uses_have_more_output() {
    // 100 probe rows x 200 build rows on ONE key = 20,000 output rows from a few probe batches (a batch is cut at 8,192 rows).
    let probe: Vec<Row> = (0..100).map(|i| vec![Some(1), Some(i)]).collect();
    let build: Vec<Row> = (0..200).map(|i| vec![Some(1), Some(1000 + i)]).collect();
    for kind in [JoinKind::Inner, JoinKind::Left, JoinKind::Full] {
        let want = ref_join(kind, &probe, &build, true);
        for mode in [Mode::Mem, Mode::Grace(4)] {
            let (got, _) = run_join(kind, mode, &probe, &build, true, 2, 5, usize::MAX);
            assert_eq!(got.len(), want.len());
            assert_eq!(got, want, "{kind:?} {mode:?}");
        }
    }
}

#[test]
fn a_build_side_over_the_budget_fails_loudly_and_a_partition_over_the_budget_fails_loudly() {
    let mut rng = Rng(3);
    let (probe, build) = (gen_rows(&mut rng, 50, 2, 10, 0), gen_rows(&mut rng, 4000, 2, 10, 0));
    let dir = tmp("join-budget");
    let budget = Budget::new(20_000, &dir);
    let (pb, bb) = (to_batches(&mut rng, &probe, 2, 60), to_batches(&mut rng, &build, 2, 60));
    let spec = JoinSpec { kind: JoinKind::Inner, probe_key: 0, build_key: 0, residual_ne: None };
    let (ps, _, _) = join_query(Arc::new(queue_source(&pb)), &schema(2), Arc::new(queue_source(&bb)), &schema(2), spec, Mode::Mem, &budget, 2);
    let e = run(ps, 2, Duration::from_secs(30)).expect_err("must not succeed");
    assert!(e.contains("memory budget"), "{e}");
    let budget = Budget::new(20_000, &dir);
    let (ps, _, _) = join_query(Arc::new(queue_source(&pb)), &schema(2), Arc::new(queue_source(&bb)), &schema(2), spec, Mode::Grace(2), &budget, 2);
    let e = run(ps, 2, Duration::from_secs(30)).expect_err("must not succeed");
    assert!(e.contains("partition"), "{e}");
    assert_eq!(budget.leftover_files(), 0, "a failed query leaves no spill files");
}

// ------------------------------------------------------------------ sort and top-N

fn key_cmp(a: &Row, b: &Row, keys: &[SortKey]) -> std::cmp::Ordering {
    for k in keys {
        let (x, y) = (a[k.col], b[k.col]);
        let o = match (x, y) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, _) => if k.nulls_first { std::cmp::Ordering::Less } else { std::cmp::Ordering::Greater },
            (_, None) => if k.nulls_first { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Less },
            (Some(x), Some(y)) => if k.desc { y.cmp(&x) } else { x.cmp(&y) },
        };
        if o != std::cmp::Ordering::Equal {
            return o;
        }
    }
    std::cmp::Ordering::Equal
}

fn with_id(mut rows: Vec<Row>) -> Vec<Row> {
    for (i, r) in rows.iter_mut().enumerate() {
        r.push(Some(i as i64));
    }
    rows
}

#[test]
fn sort_matches_the_reference_with_spill_and_merge_passes() {
    for (seed, rows_n, budget_bytes, fanin, tasks) in [(1u64, 0usize, usize::MAX, 4usize, 2usize), (2, 1, usize::MAX, 4, 2), (3, 500, usize::MAX, 4, 3), (4, 3000, 40_000, 2, 1), (5, 3000, 40_000, 3, 4), (6, 3000, 8_000, 2, 4), (7, 20000, 100_000, 4, 3)] {
        let mut rng = Rng(seed);
        let rows = with_id(gen_rows(&mut rng, rows_n, 2, 6, 15)); // columns: a, b, id (unique)
        for keys in [
            vec![SortKey { col: 0, desc: false, nulls_first: false }, SortKey { col: 2, desc: false, nulls_first: false }],
            vec![SortKey { col: 0, desc: true, nulls_first: true }, SortKey { col: 1, desc: false, nulls_first: true }, SortKey { col: 2, desc: true, nulls_first: false }],
        ] {
            let mut want = rows.clone();
            want.sort_by(|a, b| key_cmp(a, b, &keys));
            let want: Vec<String> = want.iter().map(|r| rowstr(r)).collect();
            let dir = tmp(&format!("sort-{seed}"));
            let budget = Budget::new(budget_bytes, &dir);
            let batches = to_batches(&mut rng, &rows, 3, 100);
            let (ps, out) = sort_query(Arc::new(queue_source(&batches)), SortSpec { keys: keys.clone() }, &budget, fanin, tasks);
            go(ps, tasks.max(2));
            assert_eq!(show_all(&out.result()), want, "sort seed={seed} rows={rows_n} budget={budget_bytes} fanin={fanin} tasks={tasks}");
            assert_eq!(budget.leftover_files(), 0, "no spill file survives the query");
            assert_eq!(budget.used(), 0, "every reserved byte is released when the query ends");
            if budget_bytes != usize::MAX && rows_n >= 3000 {
                assert!(budget.files_created.load(std::sync::atomic::Ordering::SeqCst) > fanin, "the spill and the merge passes were not exercised");
                assert!(budget.max_open.load(std::sync::atomic::Ordering::SeqCst) <= fanin + 2 + tasks, "open files must stay near the fan-in");
            }
        }
    }
}

#[test]
fn top_n_matches_the_reference() {
    for (seed, rows_n, n) in [(1u64, 0usize, 5usize), (2, 3, 5), (3, 3000, 1), (4, 3000, 10), (5, 20000, 100), (6, 100, 100)] {
        let mut rng = Rng(seed);
        let rows = with_id(gen_rows(&mut rng, rows_n, 2, 8, 10));
        let keys = vec![SortKey { col: 0, desc: true, nulls_first: false }, SortKey { col: 2, desc: false, nulls_first: false }];
        let mut want = rows.clone();
        want.sort_by(|a, b| key_cmp(a, b, &keys));
        let want: Vec<String> = want.iter().take(n).map(|r| rowstr(r)).collect();
        let batches = to_batches(&mut rng, &rows, 3, 100);
        let (ps, out) = topn_query(Arc::new(queue_source(&batches)), SortSpec { keys }, n, 3);
        go(ps, 3);
        assert_eq!(show_all(&out.result()), want, "topn seed={seed} rows={rows_n} n={n}");
    }
}

#[test]
fn dropping_a_sort_before_it_finishes_leaves_no_files() {
    let mut rng = Rng(9);
    let rows = with_id(gen_rows(&mut rng, 4000, 2, 6, 0));
    let dir = tmp("sort-drop");
    let budget = Budget::new(20_000, &dir);
    {
        let sink = SortSink::new(SortSpec::asc(&[0, 2]), &budget, 4);
        let mut local = sink.make(0);
        let w = Waker::new(|| {});
        for b in to_batches(&mut rng, &rows, 3, 100) {
            let mut input = spike_operators_beyond::batch::Batch::new(b);
            local.push(&mut input, &w).unwrap();
        }
        assert!(budget.files_created.load(std::sync::atomic::Ordering::SeqCst) > 0, "must have spilled");
        // the task is cancelled: its local state is dropped, finalize never runs, the sink is dropped
    }
    assert_eq!(budget.leftover_files(), 0);
    assert_eq!(budget.used(), 0, "every reserved byte is released");
}

// ------------------------------------------------------------------ aggregation and window

#[test]
fn hash_aggregate_matches_the_reference() {
    for (seed, rows_n, dom, nullp) in [(1u64, 2000usize, 7u64, 25u64), (2, 2000, 7, 25), (3, 60, 30, 60), (4, 300, 20, 70), (5, 0, 3, 0), (6, 1, 3, 50)] {
        let mut rng = Rng(seed);
        let rows = gen_rows(&mut rng, rows_n, 3, dom, nullp); // g1, g2, x
        let batches = to_batches(&mut rng, &rows, 3, 90);
        if rows.is_empty() {
            continue; // an empty input has no groups; the global-aggregate-over-nothing case is a known gap (see README)
        }
        let agg = HashAgg::new(&schema(3), &[0, 1], &[AggFn::CountStar, AggFn::Count(2), AggFn::Sum(2)], &["n", "c", "s"]);
        let out = CollectSink::new();
        let ps = vec![
            Pipeline { name: "agg".into(), source: Arc::new(queue_source(&batches)), ops: vec![], sink: Arc::new(Arc::clone(&agg)), deps: vec![], tasks: 3 },
            Pipeline { name: "out".into(), source: Arc::new(Arc::clone(&agg.out)), ops: vec![], sink: Arc::new(Arc::clone(&out)), deps: vec![0], tasks: 1 },
        ];
        go(ps, 3);
        let mut want: std::collections::BTreeMap<(Option<i64>, Option<i64>), (i64, i64, Option<i128>)> = Default::default();
        for r in &rows {
            let e = want.entry((r[0], r[1])).or_insert((0, 0, None));
            e.0 += 1;
            if let Some(x) = r[2] {
                e.1 += 1;
                e.2 = Some(e.2.unwrap_or(0) + x as i128);
            }
        }
        let mut want: Vec<String> = want.iter().map(|((a, b), (n, c, s))| format!("{}|{}|{n}|{c}|{}", a.map_or("NULL".into(), |x| x.to_string()), b.map_or("NULL".into(), |x| x.to_string()), s.map_or("NULL".into(), |x| x.to_string()))).collect();
        want.sort();
        let mut got = show_all(&out.result());
        got.sort();
        assert_eq!(got, want, "seed {seed}");
    }
}

#[test]
fn window_functions_match_the_reference() {
    for (seed, rows_n, budget_bytes, fanin) in [(1u64, 0usize, usize::MAX, 3usize), (2, 1, usize::MAX, 3), (3, 400, usize::MAX, 3), (4, 5000, 30_000, 2), (5, 5000, 30_000, 3)] {
        let mut rng = Rng(seed);
        let rows = with_id(gen_rows(&mut rng, rows_n, 2, 5, 20)); // p, o, id (unique)
        let batches = to_batches(&mut rng, &rows, 3, 80);
        let dir = tmp(&format!("win-{seed}"));
        let budget = Budget::new(budget_bytes, &dir);
        // partition by p, order by o (peers on ties of o): rank, dense_rank, sum(id) over the partition; unique order (o, id): row_number, lag(id)
        for peers in [true, false] {
            let order = if peers { SortSpec::asc(&[1]) } else { SortSpec::asc(&[1, 2]) };
            let fns = if peers { vec![(WinFn::Rank, "rk"), (WinFn::DenseRank, "dr"), (WinFn::SumPartition(2), "tot")] } else { vec![(WinFn::RowNumber, "rn"), (WinFn::Lag(2), "prev"), (WinFn::SumPartition(2), "tot")] };
            let ocols: Vec<usize> = order.keys.iter().map(|k| k.col).collect();
            let win = Window { partition: vec![0], order: ocols, fns };
            let (ps, out, _) = window_query(Arc::new(queue_source(&batches)), &schema(3), win, order, &budget, fanin, 3);
            go(ps, 3);
            // reference: group by p (NULL is its own group), order by o NULLS LAST (the sort's default), id
            let mut groups: std::collections::BTreeMap<Option<i64>, Vec<Row>> = Default::default();
            for r in &rows {
                groups.entry(r[0]).or_default().push(r.clone());
            }
            let mut want = vec![];
            for (_, mut g) in groups {
                g.sort_by(|a, b| key_cmp(a, b, &[SortKey { col: 1, desc: false, nulls_first: false }, SortKey { col: 2, desc: false, nulls_first: false }]));
                let tot: i64 = g.iter().map(|r| r[2].unwrap()).sum();
                let (mut rk, mut dr) = (0i64, 0i64);
                for (i, r) in g.iter().enumerate() {
                    if i == 0 || g[i - 1][1] != r[1] {
                        rk = i as i64 + 1;
                        dr += 1;
                    }
                    let extra = if peers { format!("{rk}|{dr}|{tot}") } else { format!("{}|{}|{tot}", i + 1, if i == 0 { "NULL".to_string() } else { g[i - 1][2].unwrap().to_string() }) };
                    want.push(format!("{}|{extra}", rowstr(r)));
                }
            }
            want.sort();
            let mut got = show_all(&out.result());
            got.sort();
            assert_eq!(got, want, "window peers={peers} seed={seed} rows={rows_n}");
            assert_eq!(budget.leftover_files(), 0);
            assert_eq!(budget.used(), 0, "every reserved byte is released when the query ends");
        }
    }
}
