//! Hash quality of composite keys. A hash that ignores a key column (or part of it) still gives the right
//! RESULTS, because equality compares every column, so the property tests against the scalar reference cannot
//! see it: it only turns a hash table into a linked list. These tests measure the collision behaviour instead.
//!
//! Stated thresholds (the expected values for a 64-bit hash are 0 and about 1 comparison per row):
//! - keys that differ ONLY in the second (or third) column: fewer than 1% of the distinct keys may share a
//!   hash with another key (for 10,000 keys: under 100 collisions);
//! - join build on such keys: at most n/4 key comparisons in total for n rows (measured: about n/25, hashbrown
//!   compares only on a matching 7-bit tag; a hash that ignores a column needs about n^2/2), and the fullest of the 64
//!   partitions holds at most 1/16 of the rows (the mean is 1/64);
//! - and each of these detectors must FAIL on a deliberately weakened hash (the hash of the first column only;
//!   the test hook `with_hash_mask`), so a pass means something.
use hash_ops::join::{BuildStats, JoinBuild, build_table};
use hash_ops::keys::{KCol, PARTS, hash_cols};
use hash_ops::reference::{Row, V};
use hash_ops::testkit::*;

use arrow::array::RecordBatch;

const N: usize = 10_000;

/// Rows of `n` keys that are constant in every column except column `vary`, which takes `n` distinct values.
fn rows_varying(tys: &[Ty], vary: usize, n: usize) -> Vec<Row> {
    (0..n)
        .map(|i| {
            tys.iter()
                .enumerate()
                .map(|(c, t)| {
                    let v = if c == vary { i as i64 } else { 7 };
                    Some(match t {
                        Ty::Dict | Ty::Str => V::S(format!("v{v}")),
                        _ => V::I(v),
                    })
                })
                .collect()
        })
        .collect()
}

fn batches(rows: &[Row], tys: &[Ty], names: &[&str], chunk: usize) -> Vec<RecordBatch> {
    to_batches(&mut seeded(0x4A5), rows, tys, names, chunk)
}

/// All rows in ONE batch (to_batches splits them at random).
fn one_batch(rows: &[Row], tys: &[Ty], names: &[&str]) -> RecordBatch {
    let bs = batches(rows, tys, names, 1000);
    arrow::compute::concat_batches(&bs[0].schema(), &bs).expect("concat")
}

/// Share of hashes that collide with another key's: 1 - distinct hashes / keys (keys are all distinct).
fn collision_rate(hashes: &[u64]) -> f64 {
    let mut v = hashes.to_vec();
    v.sort_unstable();
    v.dedup();
    1.0 - v.len() as f64 / hashes.len() as f64
}

fn hashes_of(rb: &RecordBatch, ncols: usize) -> Vec<u64> {
    let cols: Vec<KCol> = (0..ncols).map(|i| KCol::from_array(rb.column(i).as_ref()).unwrap()).collect();
    let mut out = Vec::new();
    hash_cols(&cols, rb.num_rows(), None, u64::MAX, &mut out);
    out
}

const TYPE_SETS: [[Ty; 3]; 4] = [[Ty::I64, Ty::I64, Ty::I64], [Ty::Str, Ty::Dict, Ty::I32], [Ty::Dict, Ty::Date, Ty::Str], [Ty::Dec, Ty::Str, Ty::Dict]];

#[test]
fn keys_that_differ_only_in_the_second_or_third_column_hash_differently() {
    for tys in TYPE_SETS {
        for vary in [1usize, 2] {
            let rows = rows_varying(&tys, vary, N);
            let rb = one_batch(&rows, &tys, &["a", "b", "c"]);
            assert_eq!(rb.num_rows(), N);
            let rate = collision_rate(&hashes_of(&rb, 3));
            println!("measured: {tys:?} varying column {vary}: collision rate {:.4}%", rate * 100.0);
            assert!(rate < 0.01, "{tys:?} varying column {vary}: {:.2}% of the hashes collide", rate * 100.0);
        }
    }
}

#[test]
fn random_composite_keys_hash_differently() {
    let mut rng = seeded(0x5EED);
    for tys in TYPE_SETS {
        // 10,000 random pairs/triples from small per-column domains, deduplicated (equal keys must collide).
        let mut seen = std::collections::BTreeSet::new();
        let mut rows: Vec<Row> = Vec::new();
        while rows.len() < N {
            let key: Vec<u64> = (0..3).map(|_| rng.below(60)).collect();
            if seen.insert(key.clone()) {
                rows.push(tys.iter().zip(&key).map(|(t, &v)| Some(if matches!(t, Ty::Dict | Ty::Str) { V::S(format!("s{v}")) } else { V::I(v as i64) })).collect());
            }
        }
        let rb = one_batch(&rows, &tys, &["a", "b", "c"]);
        let rate = collision_rate(&hashes_of(&rb, 3));
        assert!(rate < 0.01, "{tys:?}: {:.2}% of the hashes collide", rate * 100.0);
    }
}

#[test]
fn the_collision_detector_fails_on_weakened_hashes() {
    let tys = [Ty::I64, Ty::Str, Ty::Dict];
    let rows = rows_varying(&tys, 1, N);
    let rb = one_batch(&rows, &tys, &["a", "b", "c"]);
    // Weakened 1: a hash of the first column only (what a hash that forgets the other columns computes).
    let first_only = collision_rate(&hashes_of(&rb, 1));
    println!("measured: first-column-only hash collision rate {:.2}%", first_only * 100.0);
    assert!(first_only > 0.99, "first-column-only hash: {:.2}% collide, expected nearly all", first_only * 100.0);
    // Weakened 2: a hash cut to 8 bits.
    let cols: Vec<KCol> = (0..3).map(|i| KCol::from_array(rb.column(i).as_ref()).unwrap()).collect();
    let mut out = Vec::new();
    hash_cols(&cols, N, None, 0xFF, &mut out);
    let narrow = collision_rate(&out);
    println!("measured: 8-bit hash collision rate {:.2}%", narrow * 100.0);
    assert!(narrow > 0.9, "8-bit hash: {:.2}% collide", narrow * 100.0);
    // And the good hash passes the threshold on the same data.
    assert!(collision_rate(&hashes_of(&rb, 3)) < 0.01);
}

fn build_stats(tys: &[Ty], names: &'static [&'static str], vary: usize, n: usize, threads: usize, mask: u64) -> BuildStats {
    let rows = rows_varying(tys, vary, n);
    let bs = batches(&rows, tys, names, 1000);
    let v = |m: usize, f: &mut dyn FnMut(&RecordBatch)| f(&bs[m]);
    let sinks = hash_ops::engine::run_pipeline(bs.len(), &v, threads, &|| vec![], &|| JoinBuild::new(names.to_vec(), vec![]).with_hash_mask(mask)).unwrap();
    build_table(sinks, threads).unwrap().stats()
}

#[test]
fn a_join_build_on_keys_that_differ_only_in_later_columns_has_short_chains() {
    const NAMES: &[&str] = &["a", "b", "c"];
    let n = 5_000;
    for tys in [[Ty::I64, Ty::I64, Ty::I64], [Ty::Str, Ty::Dict, Ty::I32]] {
        for vary in [1usize, 2] {
            for threads in [1usize, 4] {
                let s = build_stats(&tys, NAMES, vary, n, threads, u64::MAX);
                println!("measured: {tys:?} varying {vary}, {threads} threads: {s:?}");
                assert_eq!(s.rows, n);
                assert!(s.key_compares <= n as u64 / 4, "{tys:?} varying {vary}: {} key comparisons for {n} rows", s.key_compares);
                assert!(s.max_partition_rows <= n / 16, "{tys:?} varying {vary}: the fullest of {PARTS} partitions has {} of {n} rows", s.max_partition_rows);
            }
        }
    }
}

#[test]
fn the_chain_detector_fails_on_a_weakened_hash() {
    // The test hook cuts every hash to 2 bits: all keys collide, so the build degenerates into one list.
    const NAMES: &[&str] = &["a", "b", "c"];
    let n = 5_000;
    let s = build_stats(&[Ty::I64, Ty::I64, Ty::I64], NAMES, 1, n, 1, 0x3);
    println!("measured: weakened (mask 0x3): {s:?}");
    assert_eq!(s.max_partition_rows, n, "all rows land in one partition");
    assert!(s.key_compares > 50 * n as u64, "only {} comparisons: the counter does not see the collisions", s.key_compares);
}
