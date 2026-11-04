//! The toy protocol under simulation, the oracles, determinism, and the real-runtime run.
use std::collections::BTreeSet;

use nata_sim::engine;
use nata_sim::frame::encode_i64s;
use nata_sim::protocol::{Config, checksum};
use nata_sim::real::{RealPlan, run_real};
use nata_sim::rng::Rng;
use nata_sim::scenario::*;

fn manifest_of(rows: &[i64]) -> Vec<u8> {
    let mut m = Vec::new();
    m.extend_from_slice(&(rows.len() as u64).to_le_bytes());
    m.extend_from_slice(&checksum(rows).to_le_bytes());
    m
}

#[test]
fn the_unmutated_protocol_passes_every_family() {
    // 8 families x 40 seeds. The full control sweep (20,000 seeds) is in gcp-check.sh.
    for seed in 0..320u64 {
        let o = run_seed(seed, false);
        assert!(o.fail.is_none(), "{}", o.line());
    }
}

#[test]
fn a_seed_is_a_complete_description_of_a_run() {
    for seed in [0u64, 1, 2, 3, 4, 5, 6, 7, 99, 1234] {
        let a = run_seed(seed, false);
        let _noise = run_seed(seed + 1, false); // another run in between must not matter
        let b = run_seed(seed, true); // keeping the log must not matter either
        assert_eq!(a.line(), b.line(), "seed {seed}");
        assert_eq!(a.events, b.events);
        assert!(!b.log.is_empty());
    }
}

#[test]
fn different_seeds_give_different_traces() {
    let hashes: BTreeSet<u64> = (0..200u64).map(|s| run_seed(s, false).hash).collect();
    assert_eq!(hashes.len(), 200, "two seeds produced the same trace hash");
}

#[test]
fn every_family_exercises_its_faults() {
    // seed % 8 picks the family; check that the characteristic event of each family happens at least once
    // in a handful of seeds of that family.
    let want: [(&str, &[&str]); 8] = [
        ("calm", &["task_complete"]),
        ("lossy", &["net_drop", "net_dup", "dup_frame"]),
        ("worker-crash", &["crash"]),
        ("crash-after-first-frame", &["crash_point", "retry"]),
        ("crash-at-final-frame", &["crash_point"]),
        ("partition", &["partition", "heal", "stale_data"]),
        ("cancel", &["cancel_requested", "cancel_seen"]),
        ("store-faults", &["store_retry"]),
    ];
    for (fam, kinds) in want.iter().enumerate() {
        let mut seen: BTreeSet<&'static str> = BTreeSet::new();
        for k in 0..40u64 {
            let o = run_seed(fam as u64 + 8 * k, false);
            assert!(o.fail.is_none(), "{}", o.line());
            seen.extend(o.stats.keys().copied());
        }
        for kind in kinds.1 {
            assert!(seen.contains(kind), "family {} never produced the event `{kind}` in 40 seeds", kinds.0);
        }
    }
}

#[test]
fn both_outcomes_happen() {
    let mut completed = 0;
    let mut cancelled = 0;
    for seed in 0..160u64 {
        match run_seed(seed, false).status {
            "completed" => completed += 1,
            "cancelled" => cancelled += 1,
            s => panic!("seed {seed}: status {s}"),
        }
    }
    assert!(completed > 50 && cancelled > 5, "completed {completed}, cancelled {cancelled}");
}

#[test]
fn the_result_oracle_can_fail() {
    let expected: Vec<i64> = (0..50).collect();
    let rows = encode_i64s(&expected);
    let ok = verify_result(1, Some(rows.clone()), Some(manifest_of(&expected)), &expected);
    assert!(ok.is_none(), "{ok:?}");
    // a lost row
    let lost: Vec<i64> = expected[1..].to_vec();
    let f = verify_result(1, Some(encode_i64s(&lost)), Some(manifest_of(&lost)), &expected).expect("must fail");
    assert_eq!(f.kind, "rows");
    assert!(f.msg.contains("lost 1"), "{}", f.msg);
    // a duplicated row
    let mut dup = expected.clone();
    dup.push(7);
    let f = verify_result(1, Some(encode_i64s(&dup)), Some(manifest_of(&dup)), &expected).expect("must fail");
    assert!(f.msg.contains("duplicated or invented 1"), "{}", f.msg);
    // a manifest that does not describe the rows
    let f = verify_result(1, Some(rows.clone()), Some(manifest_of(&lost)), &expected).expect("must fail");
    assert_eq!(f.kind, "result");
    // missing objects, a manifest after a cancel, no outcome
    assert!(verify_result(1, None, None, &expected).is_some());
    assert!(verify_result(2, None, Some(manifest_of(&expected)), &expected).is_some());
    assert!(verify_result(2, None, None, &expected).is_none());
    assert!(verify_result(0, None, None, &expected).is_some());
}

#[test]
fn the_real_scheduler_agrees_with_the_plain_reference() {
    let mut r = Rng::new(42);
    for case in 0..60 {
        let n = if case < 3 { case } else { r.below(300) as usize };
        let ids: Vec<i64> = (0..n).map(|_| r.below(1000) as i64 - 500).collect();
        let batch = r.range(1, 20) as usize;
        let threads = 1 + (case % 3);
        assert_eq!(engine::process(&ids, batch, threads), engine::reference(&ids), "case {case}: n={n} batch={batch} threads={threads}");
    }
}

fn real_cfg() -> (Config, Vec<Vec<i64>>) {
    let sc = Scenario::from_seed(0);
    (sc.cfg, sc.files)
}

#[test]
fn the_same_protocol_runs_on_the_real_runtime() {
    let (cfg, files) = real_cfg();
    let o = run_real(1, &cfg, &files, &RealPlan { kill: None }, 20_000);
    assert!(o.fail.is_none(), "{:?}", o.fail);
    assert_eq!(o.status, "completed");
}

#[test]
fn the_real_runtime_survives_a_killed_worker() {
    let (cfg, files) = real_cfg();
    let o = run_real(2, &cfg, &files, &RealPlan { kill: Some((2, 3, None)) }, 20_000);
    assert!(o.fail.is_none(), "{:?}", o.fail);
    assert_eq!(o.status, "completed");
}

#[test]
fn the_real_runtime_survives_a_killed_and_restarted_worker() {
    let (cfg, files) = real_cfg();
    let o = run_real(3, &cfg, &files, &RealPlan { kill: Some((1, 4, Some(30))) }, 20_000);
    assert!(o.fail.is_none(), "{:?}", o.fail);
    assert_eq!(o.status, "completed");
}
