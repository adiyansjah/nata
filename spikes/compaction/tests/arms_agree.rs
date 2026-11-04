//! Every arm must produce the same result as an independent scalar reference, for every type and selection shape,
//! with NULLs under the selection, with garbage under the NULL slots, and with an empty selection. And the checks must be
//! able to fail: each deliberate kernel breakage is detected.

use compaction::arms::{Fixture, How, IDS, arm_select, compact, run_arm};
use compaction::data::{Kind, Sel, Shape, make_column, make_rows, make_selection, selected_count};
use compaction::grid::{BreakEven, Cell, GridOpts, break_even, csv, expected, report, run_cell, run_grid, verify};
use compaction::harness::{Arm, Verdict, measure, median, selftest, verdict};
use compaction::kernels::{SelRef, pass, pass_mutant, view};
use compaction::reference::{logical, reference};

const SIZES: [usize; 8] = [1, 2, 63, 64, 65, 200, 1000, 4097];
const NULLS: [f64; 5] = [0.0, 3.0, 10.0, 50.0, 100.0];
const SELS: [f64; 6] = [0.1, 1.0, 10.0, 50.0, 90.0, 100.0];

fn fixture(kind: Kind, shape: Shape, n: usize, k: usize, nulls: f64, seed: u64) -> Fixture {
    Fixture { col: make_column(kind, n, nulls, seed), sel: make_selection(shape, n, k, seed ^ 7), n, k }
}

fn fixture_rows(kind: Kind, shape: Shape, n: usize, rows: &[u32], nulls: f64, seed: u64) -> Fixture {
    Fixture { col: make_column(kind, n, nulls, seed), sel: Sel::from_rows(shape, n, rows), n, k: rows.len() }
}

#[test]
fn all_arms_agree_for_every_type_shape_selectivity_and_null_density() {
    let mut cells = 0;
    for kind in Kind::ALL {
        for shape in Shape::ALL {
            for n in SIZES {
                for nulls in NULLS {
                    for sel in SELS {
                        let k = selected_count(n, sel);
                        let fx = fixture(kind, shape, n, k, nulls, (cells as u64) * 31 + 5);
                        assert_eq!(fx.sel.count(), k, "selection has k rows");
                        verify(&fx).unwrap_or_else(|e| panic!("{} {} n={n} nulls={nulls}% sel={sel}%: {e}", kind.key(), shape.key()));
                        cells += 1;
                    }
                }
            }
        }
    }
    assert!(cells > 3000, "grid ran: {cells}");
}

#[test]
fn an_empty_selection_gives_empty_results_in_every_arm() {
    for kind in Kind::ALL {
        for shape in Shape::ALL {
            for n in [1usize, 64, 100, 1000] {
                for nulls in [0.0, 30.0, 100.0] {
                    let fx = fixture_rows(kind, shape, n, &[], nulls, 11);
                    assert_eq!(fx.sel.count(), 0);
                    verify(&fx).unwrap();
                    for id in ["A", "A3", "Bt", "Bf", "Ct", "Cf", "Et", "Ef"] {
                        assert_eq!(run_arm(&fx, id), 0, "{id} on an empty selection ({} {} n={n})", kind.key(), shape.key());
                    }
                    for how in [How::Take, How::Filter] {
                        assert_eq!(compact(&fx, how).len(), 0);
                    }
                }
            }
        }
    }
}

#[test]
fn awkward_selections_first_last_word_boundaries_all_and_one() {
    let n = 200usize;
    let cases: Vec<Vec<u32>> = vec![vec![0], vec![199], vec![63], vec![64], vec![63, 64], (0..64).collect(), (0..65).collect(), (64..128).collect(), (0..200).collect(), (136..200).collect(), vec![0, 199], (0..200).step_by(2).collect()];
    for kind in Kind::ALL {
        for shape in Shape::ALL {
            for rows in &cases {
                for nulls in [0.0, 25.0, 100.0] {
                    let fx = fixture_rows(kind, shape, n, rows, nulls, 99);
                    verify(&fx).unwrap_or_else(|e| panic!("{} {} {} rows from {}: {e}", kind.key(), shape.key(), rows.len(), rows[0]));
                }
            }
        }
    }
}

/// The NULL slots hold real, non-zero data (otherwise a kernel that lets NULL slots in would not change a sum), and NULLs
/// really are under the selection.
#[test]
fn nulls_are_under_the_selection_and_their_slots_hold_garbage() {
    for kind in Kind::ALL {
        let n = 1000;
        let fx = fixture(kind, Shape::Random, n, 500, 30.0, 3);
        let rows = fx.sel.rows();
        let nulls_selected = rows.iter().filter(|&&i| fx.col.is_null(i)).count();
        assert!(nulls_selected > 50 && nulls_selected < 250, "{}: {nulls_selected} NULLs among 500 selected rows", kind.key());
        // Ignoring validity changes the result (checked through the broken kernel below); here: the slot is non-trivial.
        let v = view(fx.col.as_ref());
        let all_valid_free = pass_mutant(1, &v, 0, SelRef::All);
        assert_ne!(all_valid_free, pass(&v, 0, SelRef::All), "{}: NULL slots must hold data", kind.key());
    }
}

#[test]
fn compaction_keeps_exactly_the_selected_rows_with_their_values_and_nulls() {
    for kind in Kind::ALL {
        for shape in Shape::ALL {
            for (n, sel) in [(300usize, 10.0), (300, 50.0), (300, 100.0), (1, 100.0), (130, 90.0)] {
                let fx = fixture(kind, shape, n, selected_count(n, sel), 20.0, 17);
                let rows = fx.sel.rows();
                let want: Vec<_> = rows.iter().map(|&i| logical(fx.col.as_ref(), i)).collect();
                for how in [How::Take, How::Filter] {
                    let c = compact(&fx, how);
                    assert_eq!(c.data_type(), fx.col.data_type());
                    assert_eq!(c.len(), rows.len());
                    let got: Vec<_> = (0..c.len()).map(|i| logical(c.as_ref(), i)).collect();
                    assert_eq!(got, want, "{} {} {how:?} n={n} sel={sel}", kind.key(), shape.key());
                }
            }
        }
    }
}

use arrow::array::Array;

#[test]
fn the_selection_shapes_are_what_they_say() {
    for n in [1usize, 5, 64, 1000, 8192] {
        for k in [1usize, 2, 3, n / 2, n] {
            let k = k.clamp(1, n);
            for shape in Shape::ALL {
                let rows = make_rows(shape, n, k, 42);
                assert_eq!(rows.len(), k);
                assert!(rows.windows(2).all(|w| w[0] < w[1]), "sorted and distinct");
                assert!((*rows.last().unwrap() as usize) < n);
                if shape == Shape::Runs {
                    let runs = 1 + rows.windows(2).filter(|w| w[1] != w[0] + 1).count();
                    assert!(runs <= 4, "at most 4 runs, got {runs} (n={n}, k={k})");
                }
                let sel = Sel::from_rows(shape, n, &rows);
                assert_eq!(sel.rows(), rows.iter().map(|&i| i as usize).collect::<Vec<_>>());
                assert_eq!(sel.count(), k);
                // the two conversions agree with the native form
                assert_eq!(sel.to_indices().values().to_vec(), rows);
                let b = sel.to_bool(n);
                assert_eq!((0..n).filter(|&i| b.value(i)).count(), k);
                assert!(rows.iter().all(|&i| b.value(i as usize)));
            }
        }
    }
    // the benchmark's selectivities map to the intended row counts
    assert_eq!(selected_count(1024, 0.1), 1);
    assert_eq!(selected_count(8192, 0.1), 8);
    assert_eq!(selected_count(65536, 1.0), 655);
    assert_eq!(selected_count(8192, 100.0), 8192);
}

// ------------------------------------------------------------------------------------------------ the checks can fail

/// Each deliberate breakage of the selection-aware kernel is caught by comparing with the reference on some cell.
#[test]
fn every_kernel_mutant_is_detected_by_the_agreement_check() {
    // (bug, what it breaks, shapes where it must show)
    let cases: [(u8, &str, &[Shape]); 4] = [
        (1, "validity ignored", &[Shape::Random, Shape::Runs, Shape::Mask]),
        (2, "bitmap path without the validity word", &[Shape::Mask]),
        (3, "index path drops the last index", &[Shape::Random, Shape::Runs]),
        (4, "bitmap path drops the last word", &[Shape::Mask]),
    ];
    for (bug, what, shapes) in cases {
        for kind in Kind::ALL {
            for &shape in shapes {
                // A breakage can hide when the one row it affects happens to be a NULL, so allow several seeds; the real
                // kernel must agree on every one of them.
                let mut caught = false;
                for seed in 0..8u64 {
                    let fx = fixture(kind, shape, 1000, 400, 25.0, 21 + seed);
                    let right = expected(&fx, "A");
                    assert_eq!(arm_select(&fx, 1, 0), right, "the real kernel agrees");
                    caught |= arm_select(&fx, 1, bug) != right;
                }
                assert!(caught, "mutant {bug} ({what}) survived on {} {}", kind.key(), shape.key());
            }
        }
    }
    // mutant 5 lives in the dense pass (partial last word)
    for kind in Kind::ALL {
        let col = make_column(kind, 100, 30.0, 4);
        let v = view(col.as_ref());
        let all: Vec<usize> = (0..100).collect();
        let right = reference(&col, &all, 1);
        assert_eq!(pass(&v, 0, SelRef::All), right);
        assert_ne!(pass_mutant(5, &v, 0, SelRef::All), right, "mutant 5 survived on {}", kind.key());
    }
}

/// The checksums depend on the data (a constant checksum would make every agreement test pass).
#[test]
fn every_arm_checksum_depends_on_the_data() {
    let fx = fixture(Kind::Int64, Shape::Random, 500, 100, 10.0, 1);
    assert!(verify(&fx).is_ok());
    let right = expected(&fx, "A");
    // a fixture whose selection differs from the one the reference sees would be caught: swap the column under the arms
    let other = Fixture { col: make_column(Kind::Int64, 500, 10.0, 2), sel: fx.sel.clone(), n: 500, k: 100 };
    assert_ne!(run_arm(&other, "A"), right);
    for id in IDS {
        // every arm's checksum depends on the data (the E arms on the row count)
        if id != "Et" && id != "Ef" {
            assert_ne!(run_arm(&other, id), run_arm(&fx, id), "arm {id} does not depend on the data");
        }
    }
}

// ------------------------------------------------------------------------------------------------ harness

#[test]
fn the_harness_detects_an_injected_cost_by_the_expected_amount() {
    let fx = fixture(Kind::Int64, Shape::Random, 8192, 819, 10.0, 7);
    let (rows, base, ok) = selftest(&fx, 25);
    assert!(base > 0.0);
    assert_eq!(rows.len(), 4);
    for r in &rows {
        assert!(r.ok, "{}: injected {} ns, measured {:.0} ns (tolerance {:.0}), verdict {:?} (expected {:?})", r.label, r.injected_ns, r.measured_delta_ns, r.tolerance_ns, r.verdict, r.expected);
    }
    assert!(ok);
}

#[test]
fn the_harness_interleaves_pairs_rounds_and_reports_reps() {
    use std::cell::Cell as StdCell;
    let log = StdCell::new(Vec::<usize>::new());
    let arms: Vec<Arm<'_>> = (0..3)
        .map(|i| Arm {
            id: format!("a{i}"),
            run: Box::new({
                let log = &log;
                move || {
                    let mut v = log.take();
                    v.push(i);
                    log.set(v);
                    i as u64
                }
            }),
        })
        .collect();
    let st = measure(&arms, 6, 0.0); // min sample 0: one call per sample
    assert!(st.iter().all(|s| s.ns_per_call.len() == 6 && s.reps == 1));
    assert_eq!(st.iter().map(|s| s.check).collect::<Vec<_>>(), vec![0, 1, 2]);
    // after the calibration calls (3 arms x (1 check + 1 calibration)), the sampled order rotates: 012, 120, 201, ...
    let order = log.take();
    let sampled = &order[6..];
    assert_eq!(&sampled[..9], &[0, 1, 2, 1, 2, 0, 2, 0, 1], "arm order rotates by one each round (first round is the discarded warm-up)");
}

#[test]
fn the_verdict_needs_three_quarters_of_the_rounds() {
    assert_eq!(verdict(&[1.2, 1.3, 1.1, 1.4, 1.25]), Verdict::Slower);
    assert_eq!(verdict(&[0.8, 0.7, 0.9, 0.85, 0.75]), Verdict::Faster);
    assert_eq!(verdict(&[0.9, 1.1, 1.0, 1.2, 0.8, 1.05]), Verdict::Same);
    assert!((median(&[3.0, 1.0, 2.0]) - 2.0).abs() < 1e-12);
}

#[test]
fn break_even_interpolates_and_reports_never() {
    // ratio 2 at 10%, 0.5 at 100%: crosses 1 at t = (2-1)/(2-0.5) = 2/3 of the way in log10, i.e. 10^(1 + 2/3)
    match break_even(&[(10.0, 2.0), (100.0, 0.5)]) {
        BreakEven::At(s) => assert!((s - 10f64.powf(1.0 + 2.0 / 3.0)).abs() < 1e-6, "{s}"),
        _ => panic!("expected a crossing"),
    }
    assert!(matches!(break_even(&[(0.1, 0.9), (1.0, 0.8)]), BreakEven::AtOrBelow(s) if s == 0.1));
    assert!(matches!(break_even(&[(0.1, 3.0), (1.0, 2.0), (100.0, 1.5)]), BreakEven::Never(r, s) if r == 1.5 && s == 100.0));
    assert!(matches!(break_even(&[(1.0, 1.5), (10.0, 1.0)]), BreakEven::At(s) if (s - 10.0).abs() < 1e-9));
}

#[test]
fn a_tiny_grid_runs_end_to_end_and_the_report_has_the_tables() {
    let mut o = GridOpts::default_grid(10.0, 3);
    o.kinds = vec![Kind::Int64, Kind::Dict];
    o.sels = vec![1.0, 50.0, 100.0];
    o.sizes = vec![256];
    o.min_sample_ns = 1_000.0;
    let (results, load_max) = run_grid(&o).unwrap();
    assert_eq!(results.len(), 2 * 3 * 3);
    let md = report(&results, 3, 10.0, 0.0, load_max, 0.0);
    for needle in ["Break-even selectivity", "| Int64 |", "Dictionary<Int32,Utf8>", "256 rows", "B*/A", "n*"] {
        assert!(md.contains(needle), "report lacks '{needle}'");
    }
    let c = csv(&results);
    assert_eq!(c.lines().count(), 1 + results.len() * IDS.len());
    // a load of 2 or more labels the report
    assert!(report(&results, 3, 10.0, 2.5, 2.5, 2.5).contains("SANITY ONLY"));
    assert!(!md.contains("SANITY ONLY") || load_max >= 2.0);
}

#[test]
fn a_cell_with_a_wrong_arm_is_refused() {
    // run_cell verifies before timing; a healthy cell passes (the failing direction is covered by the mutant test)
    let c = Cell { kind: Kind::Utf8, shape: Shape::Mask, sel_pct: 10.0, n: 300, nulls_pct: 10.0 };
    let r = run_cell(c, 3, 1_000.0, 5).unwrap();
    assert_eq!(r.k, 30);
    assert_eq!(r.arms.len(), IDS.len());
}
