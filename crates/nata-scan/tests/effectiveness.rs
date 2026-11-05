//! Effectiveness: "always Keep" is sound, so soundness alone proves nothing. These tests fix how much a correct reader MUST skip.
//!  * R2 of the card: the year predicate `d >= a AND d < b` on sorted dates and sorted integers split into many row groups skips at least 60%
//!    of the row groups (and, on sorted data with exact bounds, exactly the row groups without a match);
//!  * TIGHTNESS: with exact, sound statistics the decision equals the brute-force answer "no value of [min, max] satisfies the term" (integers, dates,
//!    timestamps, booleans, decimals at other scales, floats with and without a NaN count, text). A rule that keeps more than it has to fails here.
//!  * clustered random data: the skip count has a stated minimum.

mod common;

use common::*;
use nata_scan::{CmpOp, ColumnStats, ColumnType, Predicate, RowGroupStats, Term, Value, can_skip};
use std::cmp::Ordering;

fn range_pred(col: &str, lo: Value, hi: Value) -> Predicate {
    Predicate::new(vec![
        Term::cmp(col, CmpOp::Ge, lo),
        Term::cmp(col, CmpOp::Lt, hi),
    ])
}

// ------------------------------------------------------------------------------------------------ R2: sorted columns

/// TPC-H SF1 `lineitem`: 6,001,215 rows, 122,880 rows per row group, sorted by `l_shipdate` (1992-01-02 .. 1998-12-01), as a day number.
#[test]
fn r2_year_predicate_on_sorted_dates_skips_at_least_60_percent_and_exactly_the_empty_groups() {
    const ROWS: u64 = 6_001_215;
    const GROUP: u64 = 122_880;
    const FIRST_DAY: i64 = 8_035; // 1992-01-02
    const DAYS: i64 = 2_526; // to 1998-12-01
    let day_of = |r: u64| -> i32 { (FIRST_DAY + (r as i64 * DAYS) / ROWS as i64) as i32 };
    // statistics of each row group: exact min and max of a sorted column
    let mut groups = Vec::new();
    let mut start = 0;
    while start < ROWS {
        let end = (start + GROUP).min(ROWS);
        let (lo, hi) = (day_of(start), day_of(end - 1));
        groups.push((
            start,
            end,
            RowGroupStats::new(end - start).with_column(
                "d",
                ColumnStats::none(ColumnType::Date32)
                    .with_null_count(0)
                    .with_range(Value::Date32(lo), Value::Date32(hi)),
            ),
        ));
        start = end;
    }
    assert_eq!(groups.len(), 49);
    // 1994-01-01 = 8766, 1995-01-01 = 9131
    for (year_start, year_end) in [
        (8035, 8401),
        (8401, 8766),
        (8766, 9131),
        (9131, 9496),
        (9496, 9862),
        (9862, 10227),
        (10227, 10592),
        (8766, 8796),
        (9000, 9001),
    ] {
        let p = range_pred("d", Value::Date32(year_start), Value::Date32(year_end));
        let mut skipped = 0;
        for (start, end, rg) in &groups {
            let has_match = (*start..*end).any(|r| (year_start..year_end).contains(&day_of(r)));
            let d = can_skip(rg, &p);
            assert!(!d.is_skip() || !has_match == d.is_skip(), "unsound");
            if d.is_skip() {
                skipped += 1;
            }
            assert_eq!(
                d.is_skip(),
                !has_match,
                "sorted data with exact bounds: skip exactly the groups without a match ([{year_start},{year_end}) rows {start}..{end})"
            );
        }
        let share = f64::from(skipped) / groups.len() as f64;
        eprintln!(
            "R2 dates [{year_start},{year_end}): skipped {skipped} of {} row groups ({:.1}%)",
            groups.len(),
            100.0 * share
        );
        if year_end - year_start >= 365 {
            assert!(
                share >= 0.60,
                "R2: a one-year range must skip at least 60% of the row groups, got {share:.3}"
            );
        }
    }
}

#[test]
fn r2_range_predicate_on_sorted_integers_skips_at_least_60_percent() {
    const ROWS: i64 = 1_000_000;
    const GROUP: i64 = 8_192;
    let mut groups = Vec::new();
    let mut start = 0;
    while start < ROWS {
        let end = (start + GROUP).min(ROWS);
        groups.push((
            start,
            end - 1,
            RowGroupStats::new((end - start) as u64).with_column(
                "k",
                ColumnStats::none(ColumnType::Int64)
                    .with_null_count(0)
                    .with_range(Value::Int64(start), Value::Int64(end - 1)),
            ),
        ));
        start = end;
    }
    let mut rng = Rng::new(7);
    for i in 0..200 {
        // a range of 14% of the keys
        let a = rng.range_i64(0, ROWS - ROWS / 7);
        let b = a + ROWS / 7;
        let p = range_pred("k", Value::Int64(a), Value::Int64(b));
        let skipped = groups
            .iter()
            .filter(|(_, _, rg)| can_skip(rg, &p).is_skip())
            .count();
        let must = groups
            .iter()
            .filter(|(lo, hi, _)| *hi < a || *lo >= b)
            .count();
        assert_eq!(
            skipped, must,
            "range #{i} [{a},{b}): sorted data, exact bounds: skip exactly the disjoint groups"
        );
        assert!(
            skipped as f64 / groups.len() as f64 >= 0.60,
            "range #{i}: skipped {skipped} of {}",
            groups.len()
        );
    }
}

// ------------------------------------------------------------------------------------------------ clustered random data

#[test]
fn clustered_random_data_skips_the_disjoint_groups() {
    // 2,000 row groups, each holding 1,000 values drawn uniformly from a window of width 1,000 at a random place in [0, 1,000,000);
    // 500 range predicates of width 10,000. Stated minimum: at least 95% of all row groups skipped, and at least 97% of the groups that hold no
    // matching row (brute force over the actual values).
    let mut rng = Rng::new(0xC1);
    let mut groups = Vec::new();
    for _ in 0..2_000 {
        let base = rng.range_i64(0, 999_000);
        let values: Vec<i64> = (0..1_000).map(|_| base + rng.range_i64(0, 999)).collect();
        let (lo, hi) = (
            *values.iter().min().unwrap_or(&0),
            *values.iter().max().unwrap_or(&0),
        );
        groups.push((
            values,
            RowGroupStats::new(1_000).with_column(
                "k",
                ColumnStats::none(ColumnType::Int64)
                    .with_null_count(0)
                    .with_range(Value::Int64(lo), Value::Int64(hi)),
            ),
        ));
    }
    let (mut total, mut skipped, mut empty, mut empty_skipped) = (0u64, 0u64, 0u64, 0u64);
    for _ in 0..500 {
        let a = rng.range_i64(0, 990_000);
        let p = range_pred("k", Value::Int64(a), Value::Int64(a + 10_000));
        for (values, rg) in &groups {
            let has_match = values.iter().any(|v| (a..a + 10_000).contains(v));
            let s = can_skip(rg, &p).is_skip();
            assert!(!s || !has_match, "unsound");
            total += 1;
            skipped += u64::from(s);
            if !has_match {
                empty += 1;
                empty_skipped += u64::from(s);
            }
        }
    }
    eprintln!(
        "clustered: skipped {skipped} of {total} ({:.2}%); of {empty} groups without a match {empty_skipped} skipped ({:.2}%)",
        100.0 * skipped as f64 / total as f64,
        100.0 * empty_skipped as f64 / empty as f64
    );
    assert!(skipped as f64 >= 0.95 * total as f64);
    assert!(empty_skipped as f64 >= 0.97 * empty as f64);
}

// ------------------------------------------------------------------------------------------------ tightness

const OPS: [CmpOp; 6] = [
    CmpOp::Lt,
    CmpOp::Le,
    CmpOp::Gt,
    CmpOp::Ge,
    CmpOp::Eq,
    CmpOp::Ne,
];

/// Brute force over every integer in `[lo, hi]` (column scale): does any of them satisfy `x <op> literal` under SQL semantics?
fn any_in_range(ty: ColumnType, lo: i64, hi: i64, op: CmpOp, literal: &Value) -> bool {
    let at = |u: i64| match ty {
        ColumnType::Bool => Value::Bool(u != 0),
        ColumnType::Int32 => Value::Int32(u as i32),
        ColumnType::Int64 => Value::Int64(u),
        ColumnType::Date32 => Value::Date32(u as i32),
        ColumnType::TimestampMicros => Value::TimestampMicros(u),
        ColumnType::Decimal { precision, scale } => Value::Decimal {
            unscaled: i128::from(u),
            precision,
            scale,
        },
        _ => unreachable!(),
    };
    // The statistics describe an interval, not a grid: a literal that lies inside [min, max] is a possible value (a decimal literal between two
    // values of the column's scale cannot be told apart from a value by min and max alone).
    let literal_inside = sql_cmp(&at(lo), literal) != Ordering::Greater
        && sql_cmp(&at(hi), literal) != Ordering::Less;
    (literal_inside && op_holds(op, Ordering::Equal))
        || (lo..=hi).any(|u| {
            let v = match ty {
                ColumnType::Bool => Value::Bool(u != 0),
                ColumnType::Int32 => Value::Int32(u as i32),
                ColumnType::Int64 => Value::Int64(u),
                ColumnType::Date32 => Value::Date32(u as i32),
                ColumnType::TimestampMicros => Value::TimestampMicros(u),
                ColumnType::Decimal { precision, scale } => Value::Decimal {
                    unscaled: i128::from(u),
                    precision,
                    scale,
                },
                _ => unreachable!(),
            };
            op_holds(op, sql_cmp(&v, literal))
        })
}

#[test]
fn tightness_integers_dates_booleans_and_decimals() {
    let n = if cfg!(debug_assertions) {
        40_000
    } else {
        200_000
    };
    let mut skips = 0u64;
    for case in 0..n {
        let mut rng = case_rng(0x7167, case);
        let ty = match rng.below(6) {
            0 => ColumnType::Bool,
            1 => ColumnType::Int32,
            2 => ColumnType::Int64,
            3 => ColumnType::Date32,
            4 => ColumnType::TimestampMicros,
            _ => {
                let precision = *rng.pick(&[3u8, 5, 9, 18, 38]);
                ColumnType::Decimal {
                    precision,
                    scale: rng.below(3) as u8,
                }
            }
        };
        let (lo, hi) = if matches!(ty, ColumnType::Bool) {
            let lo = rng.range_i64(0, 1);
            (lo, rng.range_i64(lo, 1))
        } else {
            let lo = rng.range_i64(-8, 8);
            (lo, lo + rng.range_i64(0, 5))
        };
        let rows = 1 + rng.below(6);
        let nulls = if rng.chance(1, 2) { 0 } else { rng.below(rows) };
        let wrap = |u: i64| match ty {
            ColumnType::Bool => Value::Bool(u != 0),
            ColumnType::Int32 => Value::Int32(u as i32),
            ColumnType::Int64 => Value::Int64(u),
            ColumnType::Date32 => Value::Date32(u as i32),
            ColumnType::TimestampMicros => Value::TimestampMicros(u),
            ColumnType::Decimal { precision, scale } => Value::Decimal {
                unscaled: i128::from(u),
                precision,
                scale,
            },
            _ => unreachable!(),
        };
        let rg = RowGroupStats::new(rows).with_column(
            "x",
            ColumnStats::none(ty)
                .with_null_count(nulls)
                .with_range(wrap(lo), wrap(hi)),
        );
        // literal: same type, or another width / scale
        let literal = match ty {
            ColumnType::Bool => Value::Bool(rng.chance(1, 2)),
            ColumnType::Int32 | ColumnType::Int64 => {
                let u = rng.range_i64(-10, 12);
                if rng.chance(1, 2) {
                    Value::Int64(u)
                } else {
                    Value::Int32(u as i32)
                }
            }
            ColumnType::Date32 => Value::Date32(rng.range_i64(-10, 12) as i32),
            ColumnType::TimestampMicros => Value::TimestampMicros(rng.range_i64(-10, 12)),
            ColumnType::Decimal { scale, .. } => {
                // the same grid or a finer one (more fraction digits), or an integer
                let s2 = rng.below(5) as u8;
                if rng.chance(1, 5) {
                    Value::Int64(rng.range_i64(-3, 3))
                } else {
                    let u = rng.range_i64(-1200, 1200);
                    let _ = scale;
                    Value::Decimal {
                        unscaled: i128::from(u),
                        precision: 10,
                        scale: s2.min(10),
                    }
                }
            }
            _ => unreachable!(),
        };
        let op = *rng.pick(&OPS);
        let d = can_skip(
            &rg,
            &Predicate::new(vec![Term::cmp("x", op, literal.clone())]),
        );
        // Expected: a comparison needs a non-NULL row (a group of only NULLs is skipped), then a value of [lo, hi] that satisfies it.
        let all_null = nulls == rows;
        let satisfiable = !all_null && any_in_range(ty, lo, hi, op, &literal);
        // For decimals the unscaled grid differs from the literal's grid, so the brute force must enumerate the column's values at the column's scale (done:
        // `any_in_range` builds column values and compares them with the literal exactly).
        assert_eq!(
            d.is_skip(),
            !satisfiable,
            "case {case}: {ty:?} [{lo},{hi}] nulls {nulls}/{rows} x {op:?} {literal:?} -> {d:?}"
        );
        skips += u64::from(d.is_skip());
    }
    eprintln!("tightness (integers etc.): {n} cases, {skips} skipped");
    assert!(
        skips * 5 > n,
        "too few skips for the check to mean anything"
    );
}

#[test]
fn tightness_floats() {
    // Values from a small set that includes both zeros and the infinities; NaN rows possible or excluded by a NaN count.
    const SET: [f64; 7] = [f64::NEG_INFINITY, -1.0, -0.0, 0.0, 1.0, 2.0, f64::INFINITY];
    let n = if cfg!(debug_assertions) {
        40_000
    } else {
        200_000
    };
    let mut skips = 0u64;
    for case in 0..n {
        let mut rng = case_rng(0xF107, case);
        let (mut a, mut b) = (*rng.pick(&SET), *rng.pick(&SET));
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        let rows = 1 + rng.below(5);
        let nulls = if rng.chance(1, 2) { 0 } else { rng.below(rows) };
        let nan_known_free = rng.chance(1, 2);
        let mut c = ColumnStats::none(ColumnType::Float64)
            .with_null_count(nulls)
            .with_range(Value::Float64(a), Value::Float64(b));
        if nan_known_free {
            c = c.with_nan_count(0);
        }
        let rg = RowGroupStats::new(rows).with_column("x", c);
        let lit = if rng.chance(1, 8) {
            f64::NAN
        } else {
            *rng.pick(&SET)
        };
        let op = *rng.pick(&OPS);
        let d = can_skip(
            &rg,
            &Predicate::new(vec![Term::cmp("x", op, Value::Float64(lit))]),
        );
        // Candidate rows: every number in [a, b] (a continuum, but only the order matters: the ends and anything strictly between), plus NaN rows if possible.
        let all_null = nulls == rows;
        let numbers_satisfy = if lit.is_nan() {
            // numbers are all below NaN
            matches!(op, CmpOp::Lt | CmpOp::Le | CmpOp::Ne)
        } else {
            let inside = |x: f64| x >= a && x <= b;
            // the ends, plus a point strictly between if the interval is not a single value
            let mut pts = vec![a, b];
            if a < b {
                pts.push(if a.is_infinite() && b.is_infinite() {
                    0.5
                } else if a.is_infinite() {
                    b - 1.0
                } else if b.is_infinite() {
                    a + 1.0
                } else {
                    (a + b) / 2.0
                });
            }
            if inside(lit) {
                pts.push(lit);
            }
            pts.into_iter().filter(|x| inside(*x)).any(|x| op_holds(op, sql_cmp(&Value::Float64(x), &Value::Float64(lit))))
                // x != lit also holds for a second distinct number in the interval
                || (matches!(op, CmpOp::Ne) && a < b)
        };
        let nan_satisfies = !nan_known_free
            && op_holds(op, sql_cmp(&Value::Float64(f64::NAN), &Value::Float64(lit)))
            // a NaN row is only an assumption when the NaN count is unknown; the NaN-only terms below still need it to exist
            ;
        let satisfiable = !all_null && (numbers_satisfy || nan_satisfies);
        // The reader does not use the NaN literal rules `x < NaN` and `x <> NaN` to rule out groups, even when all rows could be NaN: not tight there, by design.
        assert_eq!(
            d.is_skip(),
            !satisfiable,
            "case {case}: [{a:?},{b:?}] nulls {nulls}/{rows} nan_free={nan_known_free} x {op:?} {lit:?} -> {d:?}"
        );
        skips += u64::from(d.is_skip());
    }
    eprintln!("tightness (floats): {n} cases, {skips} skipped");
    assert!(skips * 5 > n);
}

#[test]
fn tightness_text() {
    const WORDS: [&str; 9] = ["", "a", "ab", "b", "é", "ü", "日", "日本", "𝄞"];
    let n = if cfg!(debug_assertions) {
        40_000
    } else {
        200_000
    };
    let mut skips = 0u64;
    for case in 0..n {
        let mut rng = case_rng(0x7E47, case);
        let (mut a, mut b) = (*rng.pick(&WORDS), *rng.pick(&WORDS));
        if a.as_bytes() > b.as_bytes() {
            std::mem::swap(&mut a, &mut b);
        }
        let rows = 1 + rng.below(5);
        let rg = RowGroupStats::new(rows).with_column(
            "x",
            ColumnStats::none(ColumnType::Utf8)
                .with_null_count(0)
                .with_range(Value::Utf8(a.into()), Value::Utf8(b.into())),
        );
        let v = *rng.pick(&WORDS);
        let op = *rng.pick(&OPS);
        let d = can_skip(
            &rg,
            &Predicate::new(vec![Term::cmp("x", op, Value::Utf8(v.into()))]),
        );
        let (ao, bo, vo) = (a.as_bytes(), b.as_bytes(), v.as_bytes());
        // Exists a string s with a <= s <= b and s <op> v?
        let satisfiable = match op {
            CmpOp::Lt => ao < vo,
            CmpOp::Le => ao <= vo,
            CmpOp::Gt => bo > vo,
            CmpOp::Ge => bo >= vo,
            CmpOp::Eq => ao <= vo && vo <= bo,
            CmpOp::Ne => !(ao == bo && ao == vo),
        };
        assert_eq!(
            d.is_skip(),
            !satisfiable,
            "case {case}: [{a:?},{b:?}] x {op:?} {v:?} -> {d:?}"
        );
        assert!(a.as_bytes().cmp(b.as_bytes()) != Ordering::Greater);
        skips += u64::from(d.is_skip());
    }
    eprintln!("tightness (text): {n} cases, {skips} skipped");
    assert!(skips * 5 > n);
}
