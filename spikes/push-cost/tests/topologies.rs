//! One test per topology: every arm must produce the SAME checksum, equal to the scalar reference, for both kernels,
//! several chain lengths, odd batch sizes, batch counts that do not divide by the thread count, and thread counts above
//! the number of batches. Plus: the check itself can fail (a wrong arm is caught), and the interleaving rotates.
//!
//! Every test runs under a timeout so that a deadlock in an arm fails the test instead of hanging the run.

use std::sync::Arc;
use std::time::Duration;

use spike_push_cost::arms::*;
use spike_push_cost::bench::{Opts, interleave};
use spike_push_cost::branch::*;
use spike_push_cost::kernels::*;

fn with_timeout<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(Duration::from_secs(secs)) {
        Ok(v) => v,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => panic!("timed out after {secs} s (a deadlock?)"),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => panic!("the work panicked (see above)"),
    }
}

fn chain_arms() -> Vec<Arm> {
    let mut v = CHAIN_ARMS.to_vec();
    v.push(Arm::ControlSpin(300));
    v
}

#[test]
fn linear_chain_every_arm_equals_the_scalar_reference() {
    with_timeout(300, || {
        // (rows per batch, batches, threads): power-of-two and odd sizes, fewer batches than threads, one row per batch
        let shapes = [(1024usize, 37usize, 1usize), (100, 53, 4), (8192, 9, 3), (64, 2, 4), (1, 5, 2), (333, 1, 4)];
        for kind in Kind::ALL {
            for len in [1usize, 3, 6] {
                for &(batch, morsels, threads) in &shapes {
                    let total_rows = batch * morsels;
                    let cell = Cell { kind, len, batch, threads, total_rows };
                    let tmpl = Arc::new(template(batch));
                    let want = reference(kind, len, batch, morsels, &tmpl);
                    match kind {
                        Kind::Cheap => assert_eq!(want.count as usize, total_rows),
                        Kind::Medium => {
                            assert!(want.count as usize <= total_rows);
                            if total_rows >= 1000 {
                                assert!(want.count > 0, "the medium kernel filtered every row");
                            }
                        }
                    }
                    for arm in chain_arms() {
                        let got = run_chain(arm, &cell, &tmpl);
                        assert_eq!(
                            got,
                            want,
                            "arm {} kernel {kind:?} len {len} batch {batch} morsels {morsels} threads {threads}",
                            arm.name()
                        );
                    }
                }
            }
        }
    });
}

#[test]
fn linear_chain_with_no_input_gives_the_empty_checksum_in_every_arm() {
    with_timeout(120, || {
        for kind in Kind::ALL {
            for len in [1usize, 3, 6] {
                for threads in [1usize, 4] {
                    let cell = Cell { kind, len, batch: 1024, threads, total_rows: 0 };
                    let tmpl = Arc::new(template(1024));
                    for arm in chain_arms() {
                        assert_eq!(run_chain(arm, &cell, &tmpl), Acc::default(), "arm {}", arm.name());
                    }
                }
            }
        }
    });
}

#[test]
fn branching_topology_every_arm_equals_the_scalar_reference() {
    with_timeout(300, || {
        let shapes = [(1024usize, 40usize), (100, 57), (4096, 8), (1, 30), (256, 1)];
        for kind in Kind::ALL {
            for &(batch, morsels) in &shapes {
                for cap in [1usize, 2, 4] {
                    let cell = BCell { kind, batch, cap, total_rows: batch * morsels };
                    let tmpl = Arc::new(template(batch));
                    let want = reference(kind, BRANCH_LEN, batch, morsels, &tmpl);
                    for arm in BRANCH_ARMS {
                        let got = run_branch(arm, &cell, &tmpl);
                        assert_eq!(got, want, "arm {} kernel {kind:?} batch {batch} morsels {morsels} cap {cap}", arm.name());
                    }
                }
            }
        }
    });
}

#[test]
fn branching_topology_survives_many_park_and_wake_cycles() {
    with_timeout(300, || {
        // capacity 1 and 3000 small batches: nearly every push and pop parks or wakes a task or a thread
        let (batch, morsels) = (256usize, 3000usize);
        for kind in Kind::ALL {
            let cell = BCell { kind, batch, cap: 1, total_rows: batch * morsels };
            let tmpl = Arc::new(template(batch));
            let want = reference(kind, BRANCH_LEN, batch, morsels, &tmpl);
            for rep in 0..10 {
                assert_eq!(run_branch(BArm::Proto, &cell, &tmpl), want, "proto, rep {rep}");
                assert_eq!(run_branch(BArm::Hand, &cell, &tmpl), want, "hand, rep {rep}");
            }
        }
    });
}

#[test]
#[should_panic(expected = "WRONG RESULT")]
fn the_check_fires_when_an_arm_returns_a_wrong_checksum() {
    let tmpl = Arc::new(template(256));
    let cell = Cell { kind: Kind::Medium, len: 3, batch: 256, threads: 2, total_rows: 256 * 20 };
    let want = reference(cell.kind, cell.len, cell.batch, cell.morsels(), &tmpl);
    let names: Vec<String> = ["good", "bad"].iter().map(|s| s.to_string()).collect();
    // the "bad" arm silently drops one morsel
    let short = Cell { total_rows: 256 * 19, ..cell };
    interleave(&names, 3, want, |a| if a == 0 { run_chain(Arm::Control, &cell, &tmpl) } else { run_chain(Arm::Control, &short, &tmpl) });
}

#[test]
#[should_panic(expected = "WRONG RESULT")]
fn the_check_fires_for_a_branching_arm_too() {
    let tmpl = Arc::new(template(256));
    let cell = BCell { kind: Kind::Cheap, batch: 256, cap: 2, total_rows: 256 * 20 };
    let want = reference(cell.kind, BRANCH_LEN, cell.batch, 20, &tmpl);
    let names: Vec<String> = ["hand", "broken"].iter().map(|s| s.to_string()).collect();
    interleave(&names, 3, want, |a| {
        let mut got = run_branch(BArm::Hand, &cell, &tmpl);
        if a == 1 {
            got.sum = got.sum.wrapping_add(1);
        }
        got
    });
}

#[test]
fn interleaving_rotates_the_starting_arm_and_keeps_every_round() {
    let names: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
    let mut order = Vec::new();
    let times = interleave(&names, 3, Acc::default(), |a| {
        order.push(a);
        Acc::default()
    });
    // 1 warm-up round + 3 measured rounds, the first arm rotating
    assert_eq!(order, vec![0, 1, 2, 1, 2, 0, 2, 0, 1, 0, 1, 2]);
    assert!(times.iter().all(|t| t.len() == 3));
}

#[test]
fn options_parse() {
    let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let o = Opts::parse(&args(&["--rounds", "7", "--total-log2", "20", "--csv", "x.csv"])).unwrap();
    assert_eq!((o.rounds, o.total_log2, o.csv.as_deref()), (7, 20, Some("x.csv")));
    assert!(Opts::parse(&args(&["--rounds", "1"])).is_err());
    assert!(Opts::parse(&args(&["--total-log2", "10"])).is_err());
    assert!(Opts::parse(&args(&["--bogus"])).is_err());
    assert!(Opts::parse(&args(&["--rounds"])).is_err());
    let s = Opts::parse(&args(&["--smoke"])).unwrap();
    assert!(s.smoke && s.rounds <= 3 && s.total_log2 <= 18);
}

#[test]
fn the_whole_report_path_runs_on_a_smoke_grid() {
    with_timeout(600, || {
        let path = std::env::temp_dir().join(format!("push-cost-smoke-{}.csv", std::process::id()));
        let args: Vec<String> = ["--smoke", "--csv", path.to_str().unwrap()].iter().map(|s| s.to_string()).collect();
        let o = Opts::parse(&args).unwrap();
        assert_eq!(spike_push_cost::bench::run(&o), 0);
        let csv = std::fs::read_to_string(&path).unwrap();
        assert!(csv.starts_with("section,kernel,len,batch,threads,cap,round,arm,seconds"));
        assert!(csv.contains(",dyn,") && csv.contains(",proto,") && csv.contains(",hand-twin,"));
        let _ = std::fs::remove_file(&path);
    });
}
