//! The soundness property: whenever `can_skip` says Skip, no row of the row group satisfies the predicate (SQL semantics), on random data with
//! statistics as writers produce them. Plus the negative controls: deliberately unsound readers that the same test must reject.

mod common;

use common::*;
use nata_scan::{
    Bound, ColumnStats, ColumnType, Decision, Predicate, RowGroupStats, Value, can_skip,
};

fn real(stats: &RowGroupStats, predicate: &Predicate) -> Decision {
    can_skip(stats, predicate)
}

/// Replay one case with `NATA_SCAN_SEED=<seed> NATA_SCAN_ONLY=<case>`.
#[test]
fn can_skip_is_sound_against_a_row_by_row_oracle() {
    let seed = seed();
    if let Some(case) = env_u64("NATA_SCAN_ONLY") {
        let (table, stats, predicate) = gen_case(seed, case);
        let d = can_skip(&stats, &predicate);
        eprintln!(
            "seed={seed} case={case}\npredicate: {predicate:?}\nstats: {stats:?}\ndecision: {d:?}\nmatching rows: {}",
            count_matches(&table, &predicate)
        );
        for c in &table.columns {
            eprintln!("column {} {:?}: {:?}", c.name, c.ty, c.cells);
        }
        assert!(
            !d.is_skip() || count_matches(&table, &predicate) == 0,
            "UNSOUND (seed={seed} case={case})"
        );
        return;
    }
    let n = cases();
    eprintln!("soundness: seed={seed} (NATA_SCAN_SEED), {n} cases (NATA_SCAN_CASES)");
    match run_cases(seed, 0, n, &real) {
        Ok(s) => {
            eprintln!(
                "soundness: {} cases, {} skipped ({:.1}%), no unsound skip",
                s.cases,
                s.skipped,
                100.0 * s.skipped as f64 / s.cases as f64
            );
            assert!(
                s.cases >= 200_000
                    || cfg!(debug_assertions)
                    || env_u64("NATA_SCAN_CASES").is_some(),
                "release runs must be at least 200,000 cases"
            );
            eprintln!("soundness: skips by reason: {:?}", s.by_reason);
            for reason in [
                "OutOfRange",
                "AllNull",
                "NoNulls",
                "NoNan",
                "Unsatisfiable",
                "EmptyRowGroup",
            ] {
                let n = s.by_reason.get(reason).copied().unwrap_or(0);
                assert!(
                    n * 2000 >= s.cases || env_u64("NATA_SCAN_CASES").is_some(),
                    "skip reason {reason} was reached only {n} times"
                );
            }
            // The run must exercise Skip at all: a decider that always keeps would pass the property.
            assert!(
                s.skipped * 20 > s.cases,
                "fewer than 5% of the cases skipped: the generator or the rules are too weak ({s:?})"
            );
        }
        Err(f) => panic!("{}", describe(&f)),
    }
}

// ------------------------------------------------------------------------------------------------ negative controls
// Each control takes the statistics and corrupts them in one way that models a real reader or writer fault, then asks the real function.
// The soundness run must find a counterexample for every one of them, or the property is too weak to be evidence.

fn map_columns(stats: &RowGroupStats, f: impl Fn(&mut ColumnStats)) -> RowGroupStats {
    let mut s = stats.clone();
    for (_, c) in &mut s.columns {
        f(c);
    }
    s
}

fn must_fail(name: &str, decide: Decider<'_>) {
    let n = 150_000;
    match run_cases(seed(), 0, n, decide) {
        Ok(s) => panic!(
            "negative control `{name}` was NOT rejected in {} cases: the soundness test cannot see this fault",
            s.cases
        ),
        Err(f) => eprintln!(
            "negative control `{name}` rejected at case {} (seed {}): {:?} but {} row(s) match",
            f.case, f.seed, f.decision, f.matching_rows
        ),
    }
}

/// A reader that believes the float bounds describe all rows (it does not know that NaN rows are left out of them).
#[test]
fn control_nan_blind_reader_is_rejected() {
    must_fail("nan-blind", &|s, p| {
        can_skip(
            &map_columns(s, |c| {
                if matches!(c.ty, ColumnType::Float32 | ColumnType::Float64) {
                    c.nan_count = Some(0)
                }
            }),
            p,
        )
    });
}

/// A reader that takes a truncated maximum for the maximum.
#[test]
fn control_truncated_as_exact_reader_is_rejected() {
    must_fail("truncated-as-exact", &|s, p| {
        can_skip(
            &map_columns(s, |c| {
                if let Some(m) = &mut c.max {
                    m.exact = true
                }
            }),
            p,
        )
    });
}

/// A reader that takes an unknown null count for zero.
#[test]
fn control_missing_null_count_as_zero_is_rejected() {
    must_fail("missing-null-count-as-zero", &|s, p| {
        can_skip(
            &map_columns(s, |c| c.null_count = c.null_count.or(Some(0))),
            p,
        )
    });
}

/// A reader that takes a missing minimum or maximum for zero.
#[test]
fn control_missing_bound_as_zero_is_rejected() {
    fn zero(ty: ColumnType) -> Option<Bound> {
        Some(Bound::exact(match ty {
            ColumnType::Int32 => Value::Int32(0),
            ColumnType::Int64 => Value::Int64(0),
            ColumnType::Date32 => Value::Date32(0),
            ColumnType::TimestampMicros => Value::TimestampMicros(0),
            ColumnType::Float32 => Value::Float32(0.0),
            ColumnType::Float64 => Value::Float64(0.0),
            ColumnType::Decimal { precision, scale } => Value::Decimal {
                unscaled: 0,
                precision,
                scale,
            },
            _ => return None,
        }))
    }
    must_fail("missing-bound-as-zero", &|s, p| {
        can_skip(
            &map_columns(s, |c| {
                if c.min.is_none() {
                    c.min = zero(c.ty);
                }
                if c.max.is_none() {
                    c.max = zero(c.ty);
                }
            }),
            p,
        )
    });
}

/// A reader that treats a bound that is NaN as an ordinary (very large) number: the reference implementation of the NaN trap, written out here
/// independently of the library as a stand-alone decider for float columns with exact integer-valued literals.
#[test]
fn control_nan_trusting_reference_is_rejected() {
    // For a one-column float row group and one `x < v` term: skip when min >= v where NaN counts as the largest value (the SQL order).
    must_fail("nan-bound-trusted", &|s, p| {
        if let (
            [(_, c)],
            [
                nata_scan::Term::Cmp {
                    op: nata_scan::CmpOp::Lt,
                    literal,
                    ..
                },
            ],
        ) = (s.columns.as_slice(), p.terms.as_slice())
            && let (Some(min), Some(v)) = (&c.min, float_of(literal))
            && let Some(lo) = float_of(&min.value)
            && !v.is_nan()
            && (lo.is_nan() || lo >= v)
            && s.num_rows > 0
        {
            return Decision::Skip(nata_scan::SkipReason::OutOfRange);
        }
        Decision::Keep(nata_scan::KeepReason::MayMatch)
    });
}

fn float_of(v: &Value) -> Option<f64> {
    match v {
        Value::Float32(x) => Some(f64::from(*x)),
        Value::Float64(x) => Some(*x),
        _ => None,
    }
}

/// A sanity check of the generator itself: it must produce the situations the rules are about, at a useful rate.
#[test]
fn generator_reaches_the_hard_situations() {
    let seed = seed();
    let (
        mut nan_rows,
        mut zero_signs,
        mut truncated_max,
        mut all_null,
        mut empty,
        mut contaminated,
        mut missing,
    ) = (0u32, 0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
    for case in 0..20_000u64 {
        let (table, stats, _) = gen_case(seed, case);
        if table.rows == 0 {
            empty += 1;
        }
        for (c, (_, st)) in table.columns.iter().zip(&stats.columns) {
            if c.cells
                .iter()
                .flatten()
                .any(|v| float_of(v).is_some_and(f64::is_nan))
            {
                nan_rows += 1;
            }
            if c.cells
                .iter()
                .flatten()
                .any(|v| float_of(v).is_some_and(|x| x == 0.0 && x.is_sign_negative()))
                && c.cells
                    .iter()
                    .flatten()
                    .any(|v| float_of(v).is_some_and(|x| x == 0.0 && x.is_sign_positive()))
            {
                zero_signs += 1;
            }
            if st.max.as_ref().is_some_and(|m| !m.exact)
                && matches!(c.ty, ColumnType::Utf8 | ColumnType::Binary)
            {
                truncated_max += 1;
            }
            if !c.cells.is_empty() && c.cells.iter().all(Option::is_none) {
                all_null += 1;
            }
            if st
                .min
                .as_ref()
                .is_some_and(|m| float_of(&m.value).is_some_and(f64::is_nan))
            {
                contaminated += 1;
            }
            if st.min.is_none() || st.null_count.is_none() {
                missing += 1;
            }
        }
    }
    eprintln!(
        "generator over 20,000 cases: nan_rows={nan_rows} both_zero_signs={zero_signs} truncated_max={truncated_max} all_null={all_null} empty={empty} nan_bounds={contaminated} missing={missing}"
    );
    for (name, n) in [
        ("nan_rows", nan_rows),
        ("both_zero_signs", zero_signs),
        ("truncated_max", truncated_max),
        ("all_null", all_null),
        ("empty", empty),
        ("nan_bounds", contaminated),
        ("missing", missing),
    ] {
        assert!(
            n >= 100,
            "generator rarely produces `{name}` ({n} of 20,000 cases)"
        );
    }
}
