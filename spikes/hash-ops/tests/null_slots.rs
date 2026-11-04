//! Arrow leaves the value under a NULL slot unspecified. For a dictionary column that means the KEY under a
//! NULL slot may be anything: out of range (99), negative (-1), i32::MIN or i32::MAX, and when the
//! dictionary is empty and every key is NULL, any key is out of range. `DictionaryArray::try_new` accepts all
//! of these (it validates only the valid slots). An engine that indexes the dictionary with such a key before
//! testing the NULL bit panics (index out of bounds) or reads a wrong value.
//!
//! These tests feed exactly those inputs through every path that reads a dictionary key: the string filter,
//! the CASE flag (`Step::FlagIn`), the aggregate key, the join key on both sides and a dictionary join
//! payload, and compare with hand-computed expectations and the scalar reference (`hash_ops::reference`).
use std::sync::Arc;

use arrow::array::{ArrayRef, Decimal64Array, DictionaryArray, Int32Array, Int64Array, RecordBatch, RecordBatchOptions, StringArray};
use arrow::buffer::NullBuffer;
use arrow::datatypes::{Field, Int32Type, Schema};
use hash_ops::agg::{HashAgg, merge_all, to_output};
use hash_ops::engine::*;
use hash_ops::join::{JoinBuild, Probe, build_table};
use hash_ops::kernels::R;
use hash_ops::reference::{self as rf, Row, SumKind, V};

/// One batch worth of a dictionary column: the dictionary values, the logical cells (None = NULL slot) and
/// the key stored under each NULL slot (ignored for valid slots).
#[derive(Clone)]
struct DictSpec {
    values: Vec<&'static str>,
    cells: Vec<Option<&'static str>>,
    junk: Vec<i32>,
}

fn dict_col(s: &DictSpec) -> ArrayRef {
    assert!(s.cells.len() == s.junk.len());
    let keys: Vec<i32> = s
        .cells
        .iter()
        .zip(&s.junk)
        .map(|(c, &j)| match c {
            Some(v) => s.values.iter().position(|x| x == v).expect("value is in the dictionary") as i32,
            None => j,
        })
        .collect();
    let valid = NullBuffer::from(s.cells.iter().map(Option::is_some).collect::<Vec<bool>>());
    let keys = Int32Array::new(keys.into(), Some(valid));
    Arc::new(DictionaryArray::<Int32Type>::try_new(keys, Arc::new(StringArray::from(s.values.clone()))).expect("Arrow accepts any key under a NULL slot"))
}

fn int_col(v: Vec<i64>) -> ArrayRef {
    Arc::new(Int64Array::from(v))
}

fn dec_col(v: Vec<i64>) -> ArrayRef {
    Arc::new(Decimal64Array::from(v).with_precision_and_scale(15, 2).unwrap())
}

/// Columns `{p}k`, `{p}x` (two dictionary columns), `{p}g` (group), `{p}v` (decimal), `{p}id`.
/// `x` is `k` rotated by one cell with its own junk keys, so it is a second NULL-holding dictionary.
fn batch(p: &str, first_id: i64, k: &DictSpec) -> (RecordBatch, Vec<Row>) {
    let n = k.cells.len();
    let mut x = k.clone();
    x.cells.rotate_left(1);
    // Junk under x's NULL slots: i32::MAX at even positions (arrow's dictionary concat adds an offset to every
    // key, NULL slots included, so this overflows when overflow checks are on) and 99 (out of range) at odd ones.
    // Build rows whose k is valid and whose x is NULL (position 0 and 3 of the first batch) keep these keys.
    x.junk = (0..n).map(|i| if i % 2 == 0 { i32::MAX } else { 99 }).collect();
    let ids: Vec<i64> = (0..n as i64).map(|i| first_id + i).collect();
    let cols: Vec<(String, ArrayRef)> = vec![
        (format!("{p}k"), dict_col(k)),
        (format!("{p}x"), dict_col(&x)),
        (format!("{p}g"), int_col(ids.iter().map(|i| i % 2).collect())),
        (format!("{p}v"), dec_col(ids.iter().map(|i| (i + 1) * 10).collect())),
        (format!("{p}id"), int_col(ids.clone())),
    ];
    let schema = Arc::new(Schema::new(cols.iter().map(|(n, a)| Field::new(n.as_str(), a.data_type().clone(), true)).collect::<Vec<_>>()));
    let rb = RecordBatch::try_new_with_options(schema, cols.into_iter().map(|(_, a)| a).collect(), &RecordBatchOptions::new().with_row_count(Some(n))).unwrap();
    let cell = |c: &Option<&'static str>| c.map(|s| V::S(s.to_string()));
    let rows = (0..n).map(|i| vec![cell(&k.cells[i]), cell(&x.cells[i]), Some(V::I(ids[i] % 2)), Some(V::I((ids[i] + 1) * 10)), Some(V::I(ids[i]))]).collect();
    (rb, rows)
}

fn spec(values: &[&'static str], cells: &[Option<&'static str>], junk: &[i32]) -> DictSpec {
    DictSpec { values: values.to_vec(), cells: cells.to_vec(), junk: junk.to_vec() }
}

/// Three batches: out-of-range / negative / extreme keys under NULLs; an EMPTY dictionary with every key NULL;
/// a third with a key (5) that is out of range for its 2-entry dictionary.
fn fixture(p: &str) -> (Vec<RecordBatch>, Vec<Row>) {
    let a = spec(&["a", "b"], &[Some("a"), None, None, Some("b"), None, None, Some("a")], &[0, 99, -1, 0, i32::MAX, i32::MIN, 0]);
    let b = spec(&[], &[None, None, None], &[0, 99, -1]);
    let c = spec(&["b", "zz"], &[Some("b"), None, Some("zz")], &[0, 5, 0]);
    let (mut batches, mut rows) = (Vec::new(), Vec::new());
    let mut next = 0;
    for s in [a, b, c] {
        let (rb, r) = batch(p, next, &s);
        next += rb.num_rows() as i64;
        batches.push(rb);
        rows.extend(r);
    }
    (batches, rows)
}

fn visit_of(batches: &[RecordBatch]) -> impl Fn(usize, &mut dyn FnMut(&RecordBatch)) + Sync + '_ {
    move |m, f| f(&batches[m])
}

/// Records, for every live row, [id, tmp0, tmp1, ...].
struct Seen(Vec<Vec<i64>>);
impl Sink for Seen {
    fn consume(&mut self, b: &Batch) -> R<()> {
        let (id, _) = b.dec(&Ref::Col("kid"));
        let live: Vec<usize> = match &b.sel {
            None => (0..b.rows).collect(),
            Some(s) => s.iter().map(|&i| i as usize).collect(),
        };
        for i in live {
            let mut r = vec![id[i]];
            r.extend(b.tmp.iter().map(|t| t.v[i]));
            self.0.push(r);
        }
        Ok(())
    }
}

/// Fixture for the filter, flag and aggregate tests: columns kk (dictionary), kx, kg, kv, kid.
fn kfixture() -> (Vec<RecordBatch>, Vec<Row>) {
    fixture("k")
}

fn run_seen(batches: &[RecordBatch], threads: usize, mk_ops: &(dyn Fn() -> Vec<Box<dyn Op>> + Sync)) -> Vec<Vec<i64>> {
    let v = visit_of(batches);
    let sinks = run_pipeline(batches.len(), &v, threads, mk_ops, &|| Seen(vec![])).expect("no panic and no error on junk under NULL slots");
    let mut all: Vec<Vec<i64>> = sinks.into_iter().flat_map(|s| s.0).collect();
    all.sort();
    all
}

#[test]
fn the_string_filter_ignores_keys_under_null_slots() {
    let (batches, _) = kfixture();
    for threads in [1usize, 3] {
        // "a" is at ids 0 and 6; ids 1, 2, 4, 5 are NULL slots with keys 99, -1, MAX, MIN; ids 7..=9 are NULL in an empty dictionary.
        let got = run_seen(&batches, threads, &|| vec![Box::new(FilterStrIn { col: "kk", set: &["a"] })]);
        assert_eq!(got, vec![vec![0], vec![6]], "{threads} threads");
        // b (id 3, 10) and zz (id 12); id 11 is a NULL slot whose key 5 is out of range.
        let got = run_seen(&batches, threads, &|| vec![Box::new(FilterStrIn { col: "kk", set: &["b", "zz"] })]);
        assert_eq!(got, vec![vec![3], vec![10], vec![12]], "{threads} threads");
    }
}

#[test]
fn the_case_flag_ignores_keys_under_null_slots_and_is_zero_for_null_in_both_polarities() {
    let (batches, _) = kfixture();
    // Hand-computed from the cells: [id, in-set flag, not-in-set flag]. A NULL takes the ELSE branch in both.
    let mut want: Vec<Vec<i64>> = vec![
        vec![0, 1, 0],
        vec![1, 0, 0],
        vec![2, 0, 0],
        vec![3, 0, 1],
        vec![4, 0, 0],
        vec![5, 0, 0],
        vec![6, 1, 0],
        vec![7, 0, 0],
        vec![8, 0, 0],
        vec![9, 0, 0],
        vec![10, 0, 1],
        vec![11, 0, 0],
        vec![12, 0, 1],
    ];
    want.sort();
    for threads in [1usize, 3] {
        let got = run_seen(&batches, threads, &|| {
            vec![Box::new(Project(vec![Step::FlagIn { col: "kk", set: &["a"], negate: false }, Step::FlagIn { col: "kk", set: &["a"], negate: true }]))]
        });
        assert_eq!(got, want, "{threads} threads");
    }
}

fn agg_rows(batches: &[RecordBatch], threads: usize, keys: &[&'static str], sums: Vec<Ref>, mk_ops: &(dyn Fn() -> Vec<Box<dyn Op>> + Sync), scales: usize) -> std::collections::BTreeMap<Vec<hash_ops::out::KVal>, hash_ops::out::GroupOut> {
    let v = visit_of(batches);
    let locals = run_pipeline(batches.len(), &v, threads, mk_ops, &|| HashAgg::new(keys.to_vec(), sums.clone())).expect("no panic and no error");
    to_output(&merge_all(locals, threads).unwrap(), vec![0; scales], true).rows
}

#[test]
fn an_aggregate_over_a_dictionary_key_ignores_keys_under_null_slots() {
    let (batches, rows) = kfixture();
    // GROUP BY k, SUM(v): NULL (10 rows, all NULL slots incl. the empty dictionary) is one group.
    let want = rf::agg(&rows, &[0], &[SumKind::Col(3)], vec![0], true).rows;
    assert_eq!(want.len(), 4, "a, b, zz and NULL");
    for threads in [1usize, 3] {
        assert_eq!(agg_rows(&batches, threads, &["kk"], vec![Ref::Col("kv")], &|| vec![], 1), want, "{threads} threads");
    }
    // GROUP BY g, SUM(CASE WHEN k IN ('a') ..), SUM(CASE WHEN k NOT IN ('a') ..): the Q12 pattern.
    let flags = [SumKind::FlagIn { col: 0, set: &["a"], negate: false }, SumKind::FlagIn { col: 0, set: &["a"], negate: true }];
    let want = rf::agg(&rows, &[2], &flags, vec![0, 0], true).rows;
    for threads in [1usize, 3] {
        let got = agg_rows(
            &batches,
            threads,
            &["kg"],
            vec![Ref::Tmp(0), Ref::Tmp(1)],
            &|| vec![Box::new(Project(vec![Step::FlagIn { col: "kk", set: &["a"], negate: false }, Step::FlagIn { col: "kk", set: &["a"], negate: true }]))],
            2,
        );
        assert_eq!(got, want, "{threads} threads");
    }
}

struct Collect(Vec<Row>);
impl Sink for Collect {
    fn consume(&mut self, b: &Batch) -> R<()> {
        assert!(b.sel.is_none());
        let rb = RecordBatch::try_new_with_options(b.schema.clone(), b.cols.clone(), &RecordBatchOptions::new().with_row_count(Some(b.rows))).unwrap();
        let names: Vec<String> = b.schema.fields().iter().map(|f| f.name().clone()).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        self.0.extend(rf::rows_of(&[rb], &names));
        Ok(())
    }
}

#[test]
fn a_join_on_dictionary_keys_and_payloads_ignores_keys_under_null_slots() {
    let (build, brows) = fixture("b");
    // Probe side: other dictionaries, again with junk under NULL slots and an all-NULL empty-dictionary batch.
    let p1 = spec(&["b", "a"], &[Some("a"), None, Some("b"), None, Some("a")], &[0, 99, 0, -1, 1]);
    let p2 = spec(&[], &[None, None], &[7, i32::MIN]);
    let p3 = spec(&["zz", "c"], &[Some("zz"), Some("c"), None], &[0, 1, 77]);
    let (mut probe, mut prows) = (Vec::new(), Vec::new());
    let mut next = 100;
    for s in [p1, p2, p3] {
        let (rb, r) = batch("p", next, &s);
        next += rb.num_rows() as i64;
        probe.push(rb);
        prows.extend(r);
    }
    // join pk = bk; output probe px, pid and build bx (dictionary payload with NULL slots), bid.
    let want = rf::sorted(rf::join_nested(&prows, &[0], &brows, &[0], &[1, 4], &[1, 4]));
    assert!(!want.is_empty() && want.iter().any(|r| r[2].is_none()), "the expectation includes a NULL dictionary payload");
    for (threads, cap) in [(1usize, 8192usize), (3, 8192), (1, 1), (3, 2)] {
        let bv = visit_of(&build);
        let sinks = run_pipeline(build.len(), &bv, threads, &|| vec![], &|| JoinBuild::new(vec!["bk"], vec!["bx", "bid"])).expect("build: no panic and no error");
        let table = Arc::new(build_table(sinks, threads).expect("build table"));
        let pv = visit_of(&probe);
        let outs = run_pipeline(
            probe.len(),
            &pv,
            threads,
            &|| -> Vec<Box<dyn Op>> { vec![Box::new(Probe::new(table.clone(), vec!["pk"], vec!["px", "pid"]).with_cap(cap))] },
            &|| Collect(vec![]),
        )
        .expect("probe: no panic and no error");
        let got = rf::sorted(outs.into_iter().flat_map(|s| s.0).collect());
        assert_eq!(got, want, "{threads} threads, cap {cap}");
    }
}

#[test]
fn a_build_side_that_is_only_an_empty_dictionary_of_nulls_joins_nothing() {
    let all_null = spec(&[], &[None, None, None], &[0, 99, -1]);
    let (build, _) = batch("b", 0, &all_null);
    let (probe, _) = batch("p", 10, &spec(&["a"], &[Some("a"), None], &[0, 3]));
    let (build, probe) = (vec![build], vec![probe]);
    let bv = visit_of(&build);
    let sinks = run_pipeline(1, &bv, 1, &|| vec![], &|| JoinBuild::new(vec!["bk"], vec!["bid"])).unwrap();
    let table = Arc::new(build_table(sinks, 1).unwrap());
    let pv = visit_of(&probe);
    let outs = run_pipeline(1, &pv, 1, &|| -> Vec<Box<dyn Op>> { vec![Box::new(Probe::new(table.clone(), vec!["pk"], vec!["pid"]))] }, &|| Collect(vec![])).unwrap();
    assert!(outs.into_iter().all(|s| s.0.is_empty()));
}
