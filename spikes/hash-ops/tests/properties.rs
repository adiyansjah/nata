//! Property and edge tests of the hash aggregate and the hash join against the scalar reference
//! (`hash_ops::reference`): random small tables with duplicate keys, NULL keys, empty sides, skew,
//! selection vectors, differing dictionaries per batch, sliced arrays, many groups, forced hash collisions.
use std::sync::Arc;

use arrow::array::RecordBatch;
use hash_ops::agg::{HashAgg, merge_all, to_output};
use hash_ops::engine::*;
use hash_ops::join::{JoinBuild, Probe, build_table};
use hash_ops::kernels::R;
use hash_ops::out::{KVal, Output};
use hash_ops::reference::{self as rf, Row, SumKind, V};
use hash_ops::testkit::*;

const KEYS: [&str; 3] = ["k0", "k1", "k2"];
const SETS: [&[&str]; 3] = [&["k0", "k3", ""], &["same-prefix-0", "same-prefix-0x", "é2"], &["k5", "k12", "k"]];

fn visit_of(batches: &[RecordBatch]) -> impl Fn(usize, &mut dyn FnMut(&RecordBatch)) + Sync + '_ {
    move |m, f| f(&batches[m])
}

fn dec_dom(rng: &mut Rng, distinct: u64) -> Dom {
    Dom { distinct, hot_pct: 0, null_pct: rng.pick(&[0, 0, 10, 40]) }
}

fn key_dom(rng: &mut Rng, null_pct: u64) -> Dom {
    Dom { distinct: rng.pick(&[1, 2, 3, 5, 12, 40, 400]), hot_pct: rng.pick(&[0, 0, 50, 90]), null_pct }
}

fn pick_mask(rng: &mut Rng) -> u64 {
    rng.pick(&[u64::MAX, u64::MAX, u64::MAX, 0, 0x3, 0xFF_0000_0000])
}

#[derive(Clone, Copy, Debug)]
enum SumSpec {
    V1,
    V2,
    Mul,
}

fn engine_sum_refs(specs: &[SumSpec]) -> (Vec<Step>, Vec<Ref>) {
    let mul = specs.iter().any(|s| matches!(s, SumSpec::Mul));
    let steps = if mul { vec![Step::SubConstLeft { c: 100, a: Ref::Col("v2") }, Step::Mul { a: Ref::Col("v1"), b: Ref::Tmp(0) }] } else { vec![] };
    let refs = specs
        .iter()
        .map(|s| match s {
            SumSpec::V1 => Ref::Col("v1"),
            SumSpec::V2 => Ref::Col("v2"),
            SumSpec::Mul => Ref::Tmp(1),
        })
        .collect();
    (steps, refs)
}

fn ref_sums(specs: &[SumSpec]) -> Vec<SumKind> {
    specs
        .iter()
        .map(|s| match s {
            SumSpec::V1 => SumKind::Col(3),
            SumSpec::V2 => SumKind::Col(4),
            SumSpec::Mul => SumKind::PriceTimes100MinusDisc { price: 3, disc: 4 },
        })
        .collect()
}

/// Engine aggregate over the batches (each batch is a morsel).
fn engine_agg(batches: &[RecordBatch], mk_ops: &(dyn Fn() -> Vec<Box<dyn Op>> + Sync), keys: &[&'static str], sums: Vec<Ref>, threads: usize, mask: u64) -> R<Output> {
    let v = visit_of(batches);
    let n = sums.len();
    let locals = run_pipeline(batches.len(), &v, threads, mk_ops, &|| HashAgg::new(keys.to_vec(), sums.clone()).with_hash_mask(mask))?;
    let parts = merge_all(locals, threads)?;
    Ok(to_output(&parts, vec![0; n], true))
}

#[test]
fn aggregate_matches_the_scalar_reference() {
    let mut rng = seeded(0xA66_0001);
    for case in 0..scaled(500) {
        let tys: Vec<Ty> = vec![rng.pick(&KEY_TYS), rng.pick(&KEY_TYS), rng.pick(&KEY_TYS), Ty::Dec, Ty::Dec, Ty::Dec];
        let null_pct = rng.pick(&[0, 0, 5, 30, 100]);
        let doms = vec![key_dom(&mut rng, null_pct), key_dom(&mut rng, null_pct), key_dom(&mut rng, null_pct), dec_dom(&mut rng, 50), dec_dom(&mut rng, 50), dec_dom(&mut rng, 10)];
        let n = rng.pick(&[0usize, 1, 2, 10, 50, 300]);
        let rows = gen_rows(&mut rng, &tys, &doms, n);
        let chunk = rng.pick(&[1usize, 7, 64]);
        let batches = to_batches(&mut rng, &rows, &tys, &["k0", "k1", "k2", "v1", "v2", "f"], chunk);
        let nk = 1 + rng.below(3) as usize;
        let mut kidx: Vec<usize> = vec![0, 1, 2];
        for i in (1..3).rev() {
            kidx.swap(i, rng.below(i as u64 + 1) as usize);
        }
        kidx.truncate(nk);
        let keys: Vec<&'static str> = kidx.iter().map(|&i| KEYS[i]).collect();
        let specs: Vec<SumSpec> = [SumSpec::V1, SumSpec::V2, SumSpec::Mul].into_iter().filter(|_| rng.pct(60)).collect();
        let (lo, hi) = (rng.below(5) as i64 - 3, rng.below(8) as i64 - 2);
        let use_filter = rng.pct(60);
        // A string-key filter too, when the first key is a string column.
        let str_filter = rng.pct(25) && matches!(tys[kidx[0]], Ty::Dict | Ty::Str);
        let set = SETS[rng.below(3) as usize];
        let mask = pick_mask(&mut rng);
        let (steps_proto, refs) = engine_sum_refs(&specs);
        let kcol = KEYS[kidx[0]];
        let mk_ops = move || -> Vec<Box<dyn Op>> {
            let mut ops: Vec<Box<dyn Op>> = Vec::new();
            if use_filter {
                ops.push(Box::new(FilterDec { col: "f", lo, hi }));
            }
            if str_filter {
                ops.push(Box::new(FilterStrIn { col: kcol, set }));
            }
            if !steps_proto.is_empty() {
                ops.push(Box::new(Project(vec![Step::SubConstLeft { c: 100, a: Ref::Col("v2") }, Step::Mul { a: Ref::Col("v1"), b: Ref::Tmp(0) }])));
            }
            ops
        };
        let kept: Vec<Row> = rows
            .iter()
            .filter(|r| {
                let f_ok = !use_filter || matches!(&r[5], Some(V::I(x)) if (lo..=hi).contains(x));
                let s_ok = !str_filter || matches!(&r[kidx[0]], Some(V::S(s)) if set.contains(&s.as_str()));
                f_ok && s_ok
            })
            .cloned()
            .collect();
        let want = rf::agg(&kept, &kidx, &ref_sums(&specs), vec![0; specs.len()], true);
        let one = engine_agg(&batches, &mk_ops, &keys, refs.clone(), 1, mask).expect("1 thread");
        let threads = rng.pick(&[2usize, 3, 5]);
        let many = engine_agg(&batches, &mk_ops, &keys, refs, threads, mask).expect("n threads");
        assert_eq!(one.rows, want.rows, "case {case}: 1 thread differs from the reference (keys {keys:?}, tys {tys:?})");
        assert_eq!(many.rows, want.rows, "case {case}: {threads} threads differ from the reference");
    }
}

/// Collects the rows of every batch it receives, and the batch sizes.
struct Collect {
    rows: Vec<Row>,
    sizes: Vec<usize>,
}

impl Sink for Collect {
    fn consume(&mut self, b: &Batch) -> R<()> {
        assert!(b.sel.is_none(), "join output carries no selection");
        let rb = RecordBatch::try_new_with_options(b.schema.clone(), b.cols.clone(), &arrow::array::RecordBatchOptions::new().with_row_count(Some(b.rows))).unwrap();
        let names: Vec<String> = b.schema.fields().iter().map(|f| f.name().clone()).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        self.rows.extend(rf::rows_of(&[rb], &names));
        self.sizes.push(b.rows);
        Ok(())
    }
}

struct JoinCase {
    probe_rows: Vec<Row>,
    probe: Vec<RecordBatch>,
    build_rows: Vec<Row>,
    build: Vec<RecordBatch>,
    nkeys: usize,
    probe_out: Vec<usize>, // indexes into probe cols [pk0, pk1, pv, pf]
    payload: Vec<usize>,   // indexes into build cols [bk0, bk1, bp0, bp1, bf]
    probe_filter: Option<(i64, i64)>,
    build_filter: Option<(i64, i64)>,
    cap: usize,
    mask: u64,
}

const PNAMES: [&str; 4] = ["pk0", "pk1", "pv", "pf"];
const BNAMES: [&str; 5] = ["bk0", "bk1", "bp0", "bp1", "bf"];

fn gen_join(rng: &mut Rng) -> JoinCase {
    let nkeys = 1 + rng.below(2) as usize;
    let null_pct = rng.pick(&[0, 0, 10, 40, 100]);
    let kt: Vec<Ty> = (0..2).map(|_| rng.pick(&KEY_TYS)).collect();
    let side_ty = |rng: &mut Rng, t: Ty| if matches!(t, Ty::Dict | Ty::Str) { rng.pick(&[Ty::Dict, Ty::Str]) } else { t };
    let ptys = vec![side_ty(rng, kt[0]), side_ty(rng, kt[1]), Ty::Dec, Ty::Dec];
    let bp0 = rng.pick(&[Ty::I64, Ty::Dict, Ty::Str, Ty::Date]);
    let btys = vec![side_ty(rng, kt[0]), side_ty(rng, kt[1]), bp0, Ty::Dec, Ty::Dec];
    let distinct = rng.pick(&[1u64, 2, 4, 8, 30]);
    let kd = |rng: &mut Rng| Dom { distinct, hot_pct: rng.pick(&[0, 0, 60]), null_pct };
    let pdoms = vec![kd(rng), kd(rng), dec_dom(rng, 40), dec_dom(rng, 10)];
    let bdoms = vec![kd(rng), kd(rng), Dom { distinct: 6, hot_pct: 0, null_pct: 20 }, dec_dom(rng, 40), dec_dom(rng, 10)];
    let (np, nb) = (rng.pick(&[0usize, 1, 5, 40, 200]), rng.pick(&[0usize, 1, 5, 40, 200]));
    let probe_rows = gen_rows(rng, &ptys, &pdoms, np);
    let build_rows = gen_rows(rng, &btys, &bdoms, nb);
    let pchunk = rng.pick(&[1usize, 9, 64]);
    let probe = to_batches(rng, &probe_rows, &ptys, &PNAMES, pchunk);
    let bchunk = rng.pick(&[1usize, 9, 64]);
    let build = to_batches(rng, &build_rows, &btys, &BNAMES, bchunk);
    let pick_some = |rng: &mut Rng, from: &[usize]| -> Vec<usize> { from.iter().copied().filter(|_| rng.pct(60)).collect() };
    let flt = |rng: &mut Rng| if rng.pct(50) { Some((rng.below(4) as i64 - 3, rng.below(6) as i64 - 2)) } else { None };
    JoinCase {
        probe_out: pick_some(rng, &[0, 1, 2]),
        payload: pick_some(rng, &[2, 3]),
        probe_filter: flt(rng),
        build_filter: flt(rng),
        cap: rng.pick(&[1usize, 2, 3, 7, 8192]),
        mask: pick_mask(rng),
        probe_rows,
        probe,
        build_rows,
        build,
        nkeys,
    }
}

fn in_range(c: &rf::Cell, f: Option<(i64, i64)>) -> bool {
    match f {
        None => true,
        Some((lo, hi)) => matches!(c, Some(V::I(x)) if (lo..=hi).contains(x)),
    }
}

/// Runs build and probe pipelines and returns the join output's pipeline sinks, and the table.
fn engine_join<S: Sink>(
    c: &JoinCase,
    bt: usize,
    pt: usize,
    after: &(dyn Fn() -> Vec<Box<dyn Op>> + Sync),
    mk_sink: &(dyn Fn() -> S + Sync),
) -> R<Vec<S>> {
    let pk: Vec<&'static str> = PNAMES[..c.nkeys].to_vec();
    let bk: Vec<&'static str> = BNAMES[..c.nkeys].to_vec();
    let payload: Vec<&'static str> = c.payload.iter().map(|&i| BNAMES[i]).collect();
    let probe_out: Vec<&'static str> = c.probe_out.iter().map(|&i| PNAMES[i]).collect();
    let bf = c.build_filter;
    let pf = c.probe_filter;
    let bv = visit_of(&c.build);
    let mask = c.mask;
    let sinks = run_pipeline(
        c.build.len(),
        &bv,
        bt,
        &move || -> Vec<Box<dyn Op>> {
            match bf {
                Some((lo, hi)) => vec![Box::new(FilterDec { col: "bf", lo, hi })],
                None => vec![],
            }
        },
        &|| JoinBuild::new(bk.clone(), payload.clone()).with_hash_mask(mask),
    )?;
    let table = Arc::new(build_table(sinks, bt)?);
    let pv = visit_of(&c.probe);
    let cap = c.cap;
    run_pipeline(
        c.probe.len(),
        &pv,
        pt,
        &|| {
            let mut ops: Vec<Box<dyn Op>> = Vec::new();
            if let Some((lo, hi)) = pf {
                ops.push(Box::new(FilterDec { col: "pf", lo, hi }));
            }
            ops.push(Box::new(Probe::new(table.clone(), pk.clone(), probe_out.clone()).with_cap(cap)));
            ops.extend(after());
            ops
        },
        mk_sink,
    )
}

fn ref_join(c: &JoinCase, indexed: bool) -> Vec<Row> {
    let probe: Vec<Row> = c.probe_rows.iter().filter(|r| in_range(&r[3], c.probe_filter)).cloned().collect();
    let build: Vec<Row> = c.build_rows.iter().filter(|r| in_range(&r[4], c.build_filter)).cloned().collect();
    let k: Vec<usize> = (0..c.nkeys).collect();
    if indexed { rf::join_indexed(&probe, &k, &build, &k, &c.probe_out, &c.payload) } else { rf::join_nested(&probe, &k, &build, &k, &c.probe_out, &c.payload) }
}

#[test]
fn the_indexed_reference_join_equals_the_nested_loop() {
    let mut rng = seeded(0x1D3_0001);
    for _ in 0..scaled(300) {
        let c = gen_join(&mut rng);
        assert_eq!(rf::sorted(ref_join(&c, true)), rf::sorted(ref_join(&c, false)));
    }
}

#[test]
fn join_output_matches_the_nested_loop_reference() {
    let mut rng = seeded(0x501_0001);
    let mut fanned_out = 0;
    for case in 0..scaled(600) {
        let c = gen_join(&mut rng);
        let (bt, pt) = (rng.pick(&[1usize, 2, 4]), rng.pick(&[1usize, 2, 3]));
        let sinks = engine_join(&c, bt, pt, &|| vec![], &|| Collect { rows: vec![], sizes: vec![] }).expect("join");
        let want = rf::sorted(ref_join(&c, false));
        let mut got: Vec<Row> = Vec::new();
        for s in &sinks {
            assert!(s.sizes.iter().all(|&n| n >= 1 && n <= c.cap), "case {case}: batch sizes {:?} violate cap {}", s.sizes, c.cap);
            got.extend(s.rows.iter().cloned());
        }
        if want.len() > c.probe_rows.len() {
            fanned_out += 1;
        }
        assert_eq!(rf::sorted(got), want, "case {case}: nkeys {} cap {} mask {:#x}", c.nkeys, c.cap, c.mask);
    }
    assert!(fanned_out > 20, "the generator must produce fan-out cases ({fanned_out})");
}

#[test]
fn join_then_aggregate_matches_the_reference() {
    let mut rng = seeded(0x7A6_0001);
    for case in 0..scaled(400) {
        let mut c = gen_join(&mut rng);
        // Output rows: [pk0, pv] ++ [bp0, bp1]; v1 = pv, v2 = bp1 for the sums.
        c.probe_out = vec![0, 2];
        c.payload = vec![2, 3];
        let both = rng.pick(&[0usize, 1, 2]);
        let (keys, kidx): (Vec<&'static str>, Vec<usize>) = match both {
            0 => (vec!["pk0"], vec![0]),
            1 => (vec!["bp0"], vec![2]),
            _ => (vec!["pk0", "bp0"], vec![0, 2]),
        };
        let mask = pick_mask(&mut rng);
        let threads = rng.pick(&[1usize, 3, 4]);
        let after = || -> Vec<Box<dyn Op>> {
            vec![Box::new(Project(vec![Step::SubConstLeft { c: 100, a: Ref::Col("bp1") }, Step::Mul { a: Ref::Col("pv"), b: Ref::Tmp(0) }]))]
        };
        let refs = vec![Ref::Col("pv"), Ref::Col("bp1"), Ref::Tmp(1)];
        let k2 = keys.clone();
        let sinks = engine_join(&c, 2, threads, &after, &|| HashAgg::new(k2.clone(), refs.clone()).with_hash_mask(mask)).expect("join agg");
        let parts = merge_all(sinks, threads).expect("merge");
        let got = to_output(&parts, vec![0; 3], true);
        let joined = ref_join(&c, false);
        let want = rf::agg(
            &joined,
            &kidx.iter().map(|&k| if k == 0 { 0 } else { 2 }).collect::<Vec<_>>(),
            &[SumKind::Col(1), SumKind::Col(3), SumKind::PriceTimes100MinusDisc { price: 1, disc: 3 }],
            vec![0; 3],
            true,
        );
        assert_eq!(got.rows, want.rows, "case {case}: keys {keys:?}");
    }
}

// ---------- deterministic edge cases ----------

fn simple_join(build: &[(Option<i64>, i64, i64)], probe: &[(Option<i64>, i64)], cap: usize, f_range: (i64, i64)) -> Vec<Row> {
    // build rows: (key, payload, f); probe rows: (key, v)
    let brows: Vec<Row> = build.iter().map(|&(k, p, f)| vec![k.map(V::I), Some(V::I(p)), Some(V::I(f))]).collect();
    let prows: Vec<Row> = probe.iter().map(|&(k, v)| vec![k.map(V::I), Some(V::I(v))]).collect();
    let mut rng = seeded(77);
    let bb = to_batches(&mut rng, &brows, &[Ty::I64, Ty::Dec, Ty::Dec], &["bk0", "bp1", "bf"], 3);
    let pb = to_batches(&mut rng, &prows, &[Ty::I64, Ty::Dec], &["pk0", "pv"], 3);
    let bv = visit_of(&bb);
    let sinks = run_pipeline(
        bb.len(),
        &bv,
        2,
        &move || -> Vec<Box<dyn Op>> { vec![Box::new(FilterDec { col: "bf", lo: f_range.0, hi: f_range.1 })] },
        &|| JoinBuild::new(vec!["bk0"], vec!["bp1"]),
    )
    .unwrap();
    let table = Arc::new(build_table(sinks, 2).unwrap());
    let pv = visit_of(&pb);
    let outs = run_pipeline(
        pb.len(),
        &pv,
        2,
        &|| -> Vec<Box<dyn Op>> { vec![Box::new(Probe::new(table.clone(), vec!["pk0"], vec!["pv"]).with_cap(cap))] },
        &|| Collect { rows: vec![], sizes: vec![] },
    )
    .unwrap();
    let mut rows = Vec::new();
    for s in outs {
        assert!(s.sizes.iter().all(|&n| (1..=cap).contains(&n)), "batch sizes {:?} cap {cap}", s.sizes);
        rows.extend(s.rows);
    }
    rf::sorted(rows)
}

fn r2(v: i64, p: i64) -> Row {
    vec![Some(V::I(v)), Some(V::I(p))]
}

#[test]
fn fan_out_emits_every_duplicate_exactly_once_in_bounded_batches() {
    // key 1 appears 10 times on the build side; 7 probe rows carry key 1: 70 output rows.
    let build: Vec<_> = (0..10).map(|i| (Some(1), i, 0)).chain([(Some(2), 99, 0)]).collect();
    let probe: Vec<_> = (0..7).map(|i| (Some(1), 100 + i)).chain([(Some(3), 500)]).collect();
    for cap in [1usize, 2, 3, 7, 10, 69, 70, 71, 8192] {
        let got = simple_join(&build, &probe, cap, (0, 0));
        let mut want: Vec<Row> = Vec::new();
        for i in 0..7 {
            for j in 0..10 {
                want.push(r2(100 + i, j));
            }
        }
        assert_eq!(got, rf::sorted(want), "cap {cap}");
    }
}

#[test]
fn null_keys_never_match_and_empty_sides_give_nothing() {
    let b = [(None, 1, 0), (Some(5), 2, 0)];
    let p = [(None, 10), (Some(5), 11), (None, 12)];
    assert_eq!(simple_join(&b, &p, 4, (0, 0)), vec![r2(11, 2)], "NULL = NULL must not match");
    assert!(simple_join(&[], &p, 4, (0, 0)).is_empty(), "empty build");
    assert!(simple_join(&b, &[], 4, (0, 0)).is_empty(), "empty probe");
    assert!(simple_join(&[], &[], 4, (0, 0)).is_empty(), "both empty");
    assert!(simple_join(&[(None, 1, 0)], &[(None, 1)], 4, (0, 0)).is_empty(), "only NULL keys");
}

#[test]
fn the_build_selection_is_respected() {
    // Both build rows have key 1; only the second passes the build filter f in [5, 5].
    let b = [(Some(1), 111, 0), (Some(1), 222, 5)];
    let p = [(Some(1), 7)];
    assert_eq!(simple_join(&b, &p, 4, (5, 5)), vec![r2(7, 222)]);
    assert!(simple_join(&b, &p, 4, (9, 9)).is_empty(), "no build row passes: nothing joins");
}

#[test]
fn the_probe_selection_is_respected() {
    let build = vec![vec![Some(V::I(1)), Some(V::I(10))]];
    let probe: Vec<Row> = vec![vec![Some(V::I(1)), Some(V::I(0))], vec![Some(V::I(1)), Some(V::I(5))], vec![Some(V::I(1)), Some(V::I(0))]];
    let mut rng = seeded(5);
    let bb = to_batches(&mut rng, &build, &[Ty::I64, Ty::Dec], &["bk0", "bp1"], 8);
    let pb = to_batches(&mut rng, &probe, &[Ty::I64, Ty::Dec], &["pk0", "pf"], 8);
    let bv = visit_of(&bb);
    let sinks = run_pipeline(bb.len(), &bv, 1, &|| vec![], &|| JoinBuild::new(vec!["bk0"], vec!["bp1"])).unwrap();
    let table = Arc::new(build_table(sinks, 1).unwrap());
    let pv = visit_of(&pb);
    let outs = run_pipeline(
        pb.len(),
        &pv,
        1,
        &|| -> Vec<Box<dyn Op>> { vec![Box::new(FilterDec { col: "pf", lo: 5, hi: 5 }), Box::new(Probe::new(table.clone(), vec!["pk0"], vec!["pf"]))] },
        &|| Collect { rows: vec![], sizes: vec![] },
    )
    .unwrap();
    let rows: Vec<Row> = outs.into_iter().flat_map(|s| s.rows).collect();
    assert_eq!(rows, vec![r2(5, 10)]);
}

#[test]
fn string_keys_join_across_dictionary_and_plain_representations() {
    let mut rng = seeded(9);
    let brows: Vec<Row> = ["a", "b", "b", ""].iter().enumerate().map(|(i, s)| vec![Some(V::S(s.to_string())), Some(V::I(i as i64))]).collect();
    let prows: Vec<Row> = ["b", "", "c", "a", "b"].iter().map(|s| vec![Some(V::S(s.to_string()))]).collect();
    for (bt, pt) in [(Ty::Dict, Ty::Str), (Ty::Str, Ty::Dict), (Ty::Dict, Ty::Dict), (Ty::Str, Ty::Str)] {
        let bb = to_batches(&mut rng, &brows, &[bt, Ty::Dec], &["bk0", "bp1"], 2);
        let pb = to_batches(&mut rng, &prows, &[pt], &["pk0"], 2);
        let bv = visit_of(&bb);
        let sinks = run_pipeline(bb.len(), &bv, 2, &|| vec![], &|| JoinBuild::new(vec!["bk0"], vec!["bp1"])).unwrap();
        let table = Arc::new(build_table(sinks, 2).unwrap());
        let pv = visit_of(&pb);
        let outs = run_pipeline(
            pb.len(),
            &pv,
            2,
            &|| -> Vec<Box<dyn Op>> { vec![Box::new(Probe::new(table.clone(), vec!["pk0"], vec!["pk0"]))] },
            &|| Collect { rows: vec![], sizes: vec![] },
        )
        .unwrap();
        let got = rf::sorted(outs.into_iter().flat_map(|s| s.rows).collect());
        let want = rf::sorted(rf::join_nested(&prows, &[0], &brows, &[0], &[0], &[1]));
        assert_eq!(got, want, "{bt:?} build, {pt:?} probe");
        // Hand count: probe b (x2 rows) matches the two build b rows = 4, plus '' = 1, plus a = 1; c matches nothing.
        assert_eq!(want.len(), 6, "b matches twice for each of two probe rows (4), plus '' (1) and a (1)");
    }
}

#[test]
fn a_null_group_key_is_its_own_group_and_empty_string_is_not_null() {
    let rows: Vec<Row> = vec![
        vec![None, Some(V::I(1))],
        vec![Some(V::S(String::new())), Some(V::I(2))],
        vec![None, Some(V::I(4))],
        vec![Some(V::S("x".into())), None],
    ];
    let mut rng = seeded(3);
    for ty in [Ty::Str, Ty::Dict] {
        let batches = to_batches(&mut rng, &rows, &[ty, Ty::Dec], &["k0", "v1"], 2);
        let out = engine_agg(&batches, &(|| vec![]), &["k0"], vec![Ref::Col("v1")], 3, u64::MAX).unwrap();
        let g = |k: KVal| out.rows.get(&vec![k]).cloned().unwrap_or_else(|| panic!("missing group {ty:?}"));
        assert_eq!((g(KVal::Null).count, g(KVal::Null).sums[0]), (2, (5, 2)));
        assert_eq!((g(KVal::S(String::new())).count, g(KVal::S(String::new())).sums[0]), (1, (2, 1)));
        assert_eq!(g(KVal::S("x".into())).sums[0], (0, 0), "a sum of only NULLs is NULL");
        assert_eq!(out.rows.len(), 3);
    }
}

#[test]
fn many_groups_across_batches_and_threads_match_the_reference() {
    let mut rng = seeded(0xB16);
    for ty in [Ty::I64, Ty::Str, Ty::Dict, Ty::Date] {
        let tys = [ty, Ty::Dec, Ty::Dec];
        let doms = [Dom { distinct: 30_000, hot_pct: 20, null_pct: 2 }, dec_dom(&mut rng, 100), dec_dom(&mut rng, 100)];
        let rows = gen_rows(&mut rng, &tys, &doms, 60_000);
        let batches = to_batches(&mut rng, &rows, &tys, &["k0", "v1", "v2"], 8192);
        let want = rf::agg(&rows, &[0], &[SumKind::Col(1), SumKind::Col(2)], vec![0; 2], true);
        assert!(want.rows.len() > 5_000, "{ty:?}: only {} groups", want.rows.len());
        for threads in [1usize, 4, 8] {
            let got = engine_agg(&batches, &(|| vec![]), &["k0"], vec![Ref::Col("v1"), Ref::Col("v2")], threads, u64::MAX).unwrap();
            assert_eq!(got.rows, want.rows, "{ty:?} with {threads} threads");
        }
    }
}

#[test]
fn an_aggregate_over_no_rows_has_no_groups_and_one_row_has_one() {
    let mut rng = seeded(11);
    let batches = to_batches(&mut rng, &[], &[Ty::I64, Ty::Dec], &["k0", "v1"], 4);
    let out = engine_agg(&batches, &(|| vec![]), &["k0"], vec![Ref::Col("v1")], 4, u64::MAX).unwrap();
    assert!(out.rows.is_empty());
    let one: Vec<Row> = vec![vec![Some(V::I(7)), Some(V::I(5))]];
    let batches = to_batches(&mut rng, &one, &[Ty::I64, Ty::Dec], &["k0", "v1"], 4);
    let out = engine_agg(&batches, &(|| vec![]), &["k0"], vec![Ref::Col("v1")], 4, u64::MAX).unwrap();
    assert_eq!(out.rows.len(), 1);
}

#[test]
fn the_aggregate_selection_vector_excludes_unselected_rows_from_keys_and_sums() {
    // Rows with f outside [1,1] must not create groups nor add to sums.
    let rows: Vec<Row> = vec![
        vec![Some(V::I(1)), Some(V::I(10)), Some(V::I(1))],
        vec![Some(V::I(2)), Some(V::I(20)), Some(V::I(0))],
        vec![Some(V::I(1)), Some(V::I(30)), Some(V::I(0))],
        vec![Some(V::I(3)), Some(V::I(40)), Some(V::I(1))],
    ];
    let mut rng = seeded(21);
    let batches = to_batches(&mut rng, &rows, &[Ty::I64, Ty::Dec, Ty::Dec], &["k0", "v1", "f"], 8);
    let out = engine_agg(&batches, &(|| -> Vec<Box<dyn Op>> { vec![Box::new(FilterDec { col: "f", lo: 1, hi: 1 })] }), &["k0"], vec![Ref::Col("v1")], 2, u64::MAX).unwrap();
    let want = rf::agg(&[rows[0].clone(), rows[3].clone()], &[0], &[SumKind::Col(1)], vec![0], true);
    assert_eq!(out.rows, want.rows);
    assert_eq!(out.rows.len(), 2);
}
