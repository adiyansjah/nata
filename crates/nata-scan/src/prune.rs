//! The pruning decision. See the crate docs (`lib.rs`) for the rule table and the contract.

use std::cmp::Ordering;

use crate::predicate::{CmpOp, Predicate, Term};
use crate::stats::{Bound, ColumnStats, ColumnType, RowGroupStats, Value};

/// Why a row group is skipped: what was proven.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// The row group has no rows.
    EmptyRowGroup,
    /// Every row is NULL (`null_count == num_rows`), so no comparison and no `IS NOT NULL` can be true.
    AllNull,
    /// `IS NULL` and the null count is 0.
    NoNulls,
    /// The minimum or the maximum puts the literal out of reach.
    OutOfRange,
    /// The term is false for every value (`x > NaN`).
    Unsatisfiable,
    /// The term can only be true for a NaN row, and the NaN count is 0.
    NoNan,
}

/// Why a row group is kept. `MayMatch` is the ordinary case: the statistics do not exclude a match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeepReason {
    MayMatch,
    /// The predicate has no terms (and the row group has rows).
    EmptyPredicate,
    UnknownColumn,
    /// The literal does not have a type that is comparable with the column (see the crate docs), or it is malformed.
    TypeMismatch,
    /// The column type is not supported by pruning.
    UnsupportedType,
    /// The statistic that the rule needs is absent.
    NoStatistics,
    /// The statistics contradict themselves or the row count, or hold a NaN bound: not trusted.
    UnsoundStatistics,
    /// NaN rows may exist and the predicate may be true for them.
    NanRows,
}

/// The answer: `Skip` only if the statistics PROVE that no row of the row group satisfies the predicate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Skip(SkipReason),
    Keep(KeepReason),
}

impl Decision {
    pub fn is_skip(&self) -> bool {
        matches!(self, Decision::Skip(_))
    }
}

/// Can the row group be skipped? The predicate is an AND: one term that no row can satisfy is enough.
pub fn can_skip(stats: &RowGroupStats, predicate: &Predicate) -> Decision {
    if stats.num_rows == 0 {
        return Decision::Skip(SkipReason::EmptyRowGroup);
    }
    let mut first_keep = None;
    for term in &predicate.terms {
        match decide_term(stats, term) {
            Decision::Skip(reason) => return Decision::Skip(reason),
            Decision::Keep(reason) => {
                first_keep.get_or_insert(reason);
            }
        }
    }
    Decision::Keep(first_keep.unwrap_or(KeepReason::EmptyPredicate))
}

fn decide_term(rg: &RowGroupStats, term: &Term) -> Decision {
    let name = match term {
        Term::Cmp { column, .. } | Term::IsNull { column } | Term::IsNotNull { column } => column,
    };
    let Some(col) = rg.column(name) else {
        return Decision::Keep(KeepReason::UnknownColumn);
    };
    // Nested columns count nulls per leaf, not per row, so even the null counts of an unsupported column are not used.
    if matches!(col.ty, ColumnType::Unsupported) || !column_type_is_valid(col.ty) {
        return Decision::Keep(KeepReason::UnsupportedType);
    }
    let rows = rg.num_rows;
    // A null count above the row count cannot be right.
    let nulls = match col.null_count {
        Some(n) if n > rows => return Decision::Keep(KeepReason::UnsoundStatistics),
        n => n,
    };
    let all_null = nulls == Some(rows);
    // A column of only NULLs has no minimum and maximum; a writer that gives them anyway contradicts its own null count.
    let contradiction = all_null && (col.min.is_some() || col.max.is_some());
    match term {
        Term::IsNull { .. } => match nulls {
            Some(0) => Decision::Skip(SkipReason::NoNulls),
            Some(_) => Decision::Keep(KeepReason::MayMatch),
            None => Decision::Keep(KeepReason::NoStatistics),
        },
        Term::IsNotNull { .. } => {
            if contradiction {
                Decision::Keep(KeepReason::UnsoundStatistics)
            } else if all_null {
                Decision::Skip(SkipReason::AllNull)
            } else if nulls.is_none() {
                Decision::Keep(KeepReason::NoStatistics)
            } else {
                Decision::Keep(KeepReason::MayMatch)
            }
        }
        Term::Cmp { op, literal, .. } => {
            let Some(v) = literal_key(col.ty, literal) else {
                return Decision::Keep(KeepReason::TypeMismatch);
            };
            if contradiction {
                return Decision::Keep(KeepReason::UnsoundStatistics);
            }
            if all_null {
                return Decision::Skip(SkipReason::AllNull);
            }
            decide_cmp(col, rows, nulls, *op, &v)
        }
    }
}

// ------------------------------------------------------------------------------------------------- keys and their order

/// A value in the domain in which it is compared. Integers, dates, timestamps and booleans share `Int`.
#[derive(Clone, Copy, Debug)]
enum Key<'a> {
    Int(i128),
    Float(f64),
    /// Unscaled value and scale.
    Dec(i128, u8),
    Bytes(&'a [u8]),
}

fn column_type_is_valid(ty: ColumnType) -> bool {
    match ty {
        ColumnType::Decimal { precision, scale } => decimal_is_valid(precision, scale),
        _ => true,
    }
}

fn decimal_is_valid(precision: u8, scale: u8) -> bool {
    (1..=38).contains(&precision) && scale <= precision
}

/// A decimal value must fit its stated precision.
fn decimal_fits(unscaled: i128, precision: u8, scale: u8) -> bool {
    if !decimal_is_valid(precision, scale) {
        return false;
    }
    match 10i128.checked_pow(u32::from(precision)) {
        Some(limit) => unscaled > -limit && unscaled < limit,
        None => false,
    }
}

/// The key of a STATISTIC value: it must have exactly the type of the column.
fn stat_key(ty: ColumnType, value: &Value) -> Option<Key<'_>> {
    match (ty, value) {
        (ColumnType::Bool, Value::Bool(b)) => Some(Key::Int(i128::from(*b))),
        (ColumnType::Int32, Value::Int32(x)) => Some(Key::Int(i128::from(*x))),
        (ColumnType::Int64, Value::Int64(x)) => Some(Key::Int(i128::from(*x))),
        (ColumnType::Date32, Value::Date32(x)) => Some(Key::Int(i128::from(*x))),
        (ColumnType::TimestampMicros, Value::TimestampMicros(x)) => Some(Key::Int(i128::from(*x))),
        (ColumnType::Float32, Value::Float32(x)) => Some(Key::Float(f64::from(*x))),
        (ColumnType::Float64, Value::Float64(x)) => Some(Key::Float(*x)),
        (
            ColumnType::Decimal { precision, scale },
            Value::Decimal {
                unscaled,
                precision: p,
                scale: s,
            },
        ) if p == &precision && s == &scale && decimal_fits(*unscaled, *p, *s) => {
            Some(Key::Dec(*unscaled, *s))
        }
        (ColumnType::Utf8, Value::Utf8(s)) => Some(Key::Bytes(s.as_bytes())),
        (ColumnType::Binary, Value::Binary(b)) => Some(Key::Bytes(b)),
        _ => None,
    }
}

/// The key of a LITERAL. Besides the column's own type, these widenings are exact and are accepted: an `Int32` or `Int64` literal against
/// an `Int32` or `Int64` column; a `Float32` or `Float64` literal against a `Float32` or `Float64` column; a decimal of any precision
/// and scale (or an integer, as a decimal of scale 0) against a decimal column. Everything else is a type mismatch: the binder must cast.
fn literal_key(ty: ColumnType, value: &Value) -> Option<Key<'_>> {
    match (ty, value) {
        (ColumnType::Int32 | ColumnType::Int64, Value::Int32(x)) => Some(Key::Int(i128::from(*x))),
        (ColumnType::Int32 | ColumnType::Int64, Value::Int64(x)) => Some(Key::Int(i128::from(*x))),
        (ColumnType::Float32 | ColumnType::Float64, Value::Float32(x)) => {
            Some(Key::Float(f64::from(*x)))
        }
        (ColumnType::Float32 | ColumnType::Float64, Value::Float64(x)) => Some(Key::Float(*x)),
        (
            ColumnType::Decimal { .. },
            Value::Decimal {
                unscaled,
                precision,
                scale,
            },
        ) if decimal_fits(*unscaled, *precision, *scale) => Some(Key::Dec(*unscaled, *scale)),
        (ColumnType::Decimal { .. }, Value::Int32(x)) => Some(Key::Dec(i128::from(*x), 0)),
        (ColumnType::Decimal { .. }, Value::Int64(x)) => Some(Key::Dec(i128::from(*x), 0)),
        (ColumnType::Decimal { .. }, _) => None,
        _ => stat_key(ty, value),
    }
}

/// The order of two doubles for pruning: numbers as numbers (-0.0 equals +0.0, DuckDB 2.0.0), and NO order for NaN. A NaN literal and a NaN bound are
/// handled before any comparison; if one ever got here, a comparison with it proves nothing.
fn cmp_f64(a: f64, b: f64) -> Option<Ordering> {
    a.partial_cmp(&b)
}

/// Exact comparison of two decimals of different scales: bring both to the larger scale. If the rescaled value does not fit 128 bits
/// its magnitude is above every 128-bit value, so the other side decides by sign alone.
fn cmp_dec(a: i128, sa: u8, b: i128, sb: u8) -> Ordering {
    match sa.cmp(&sb) {
        Ordering::Equal => a.cmp(&b),
        Ordering::Less => match rescale(a, sb.saturating_sub(sa)) {
            Some(a2) => a2.cmp(&b),
            None => sign_order(a),
        },
        Ordering::Greater => match rescale(b, sa.saturating_sub(sb)) {
            Some(b2) => a.cmp(&b2),
            None => sign_order(b).reverse(),
        },
    }
}

/// `x * 10^k`, or `None` if it does not fit.
fn rescale(x: i128, k: u8) -> Option<i128> {
    if x == 0 {
        return Some(0);
    }
    10i128
        .checked_pow(u32::from(k))
        .and_then(|p| x.checked_mul(p))
}

/// The order of an overflowed value against any value that fits: by its sign.
fn sign_order(overflowed: i128) -> Ordering {
    if overflowed > 0 {
        Ordering::Greater
    } else {
        Ordering::Less
    }
}

/// The order of two keys of the same kind. `None` for different kinds (which validation prevents): the caller then proves nothing.
fn cmp_key(a: &Key<'_>, b: &Key<'_>) -> Option<Ordering> {
    match (a, b) {
        (Key::Int(x), Key::Int(y)) => Some(x.cmp(y)),
        (Key::Float(x), Key::Float(y)) => cmp_f64(*x, *y),
        (Key::Dec(x, sx), Key::Dec(y, sy)) => Some(cmp_dec(*x, *sx, *y, *sy)),
        (Key::Bytes(x), Key::Bytes(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

fn lt(a: &Key<'_>, b: &Key<'_>) -> bool {
    cmp_key(a, b) == Some(Ordering::Less)
}

fn le(a: &Key<'_>, b: &Key<'_>) -> bool {
    matches!(cmp_key(a, b), Some(Ordering::Less | Ordering::Equal))
}

fn gt(a: &Key<'_>, b: &Key<'_>) -> bool {
    cmp_key(a, b) == Some(Ordering::Greater)
}

fn ge(a: &Key<'_>, b: &Key<'_>) -> bool {
    matches!(cmp_key(a, b), Some(Ordering::Greater | Ordering::Equal))
}

fn eq(a: &Key<'_>, b: &Key<'_>) -> bool {
    cmp_key(a, b) == Some(Ordering::Equal)
}

/// The smallest byte string that is above every string that starts with `prefix`: drop trailing 0xFF bytes and add one to the last
/// byte left. `None` if there is none (empty, or all 0xFF).
fn increment(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut v = prefix.to_vec();
    while let Some(last) = v.pop() {
        if let Some(next) = last.checked_add(1) {
            v.push(next);
            return Some(v);
        }
    }
    None
}

// ------------------------------------------------------------------------------------------------- the rules

/// A bound as a key, with its exactness.
struct Stat<'a> {
    key: Key<'a>,
    exact: bool,
}

fn stat_of<'a>(ty: ColumnType, bound: &'a Option<Bound>) -> Result<Option<Stat<'a>>, ()> {
    match bound {
        None => Ok(None),
        Some(b) => match stat_key(ty, &b.value) {
            Some(key) => Ok(Some(Stat {
                key,
                exact: b.exact,
            })),
            // A bound whose type is not the column's type: the statistics are not trusted.
            None => Err(()),
        },
    }
}

fn decide_cmp(
    col: &ColumnStats,
    rows: u64,
    nulls: Option<u64>,
    op: CmpOp,
    v: &Key<'_>,
) -> Decision {
    let (Ok(min), Ok(max)) = (stat_of(col.ty, &col.min), stat_of(col.ty, &col.max)) else {
        return Decision::Keep(KeepReason::UnsoundStatistics);
    };
    // Lower bound above upper bound: the statistics contradict themselves.
    if let (Some(lo), Some(hi)) = (&min, &max)
        && gt(&lo.key, &hi.key)
    {
        return Decision::Keep(KeepReason::UnsoundStatistics);
    }
    if let Key::Float(x) = v {
        // The non-NaN rows are at most `rows - nulls`; more NaN rows than that is a contradiction.
        let non_null = rows.saturating_sub(nulls.unwrap_or(0));
        if col.nan_count.is_some_and(|n| n > non_null) {
            return Decision::Keep(KeepReason::UnsoundStatistics);
        }
        let nan_free = col.nan_count == Some(0);
        if x.is_nan() {
            return match op {
                // Nothing is above NaN, not even NaN.
                CmpOp::Gt => Decision::Skip(SkipReason::Unsatisfiable),
                // Only a NaN row can be equal to NaN or at least NaN.
                CmpOp::Eq | CmpOp::Ge => {
                    if nan_free {
                        Decision::Skip(SkipReason::NoNan)
                    } else {
                        Decision::Keep(KeepReason::NanRows)
                    }
                }
                // Every number is below NaN; NaN <= NaN: these match without any NaN-free argument.
                CmpOp::Lt | CmpOp::Le | CmpOp::Ne => Decision::Keep(KeepReason::NanRows),
            };
        }
        // A NaN bound (a writer that lets NaN into the minimum or maximum, for example by merging the statistics of a page of NaN
        // with the others) says nothing about the numbers.
        let nan_bound = |s: &Option<Stat<'_>>| {
            s.as_ref()
                .is_some_and(|s| matches!(s.key, Key::Float(f) if f.is_nan()))
        };
        if nan_bound(&min) || nan_bound(&max) {
            return Decision::Keep(KeepReason::UnsoundStatistics);
        }
        // A NaN row is above every number: it satisfies `>`, `>=` and `<>` against any number, and the bounds do not show it.
        if matches!(op, CmpOp::Gt | CmpOp::Ge | CmpOp::Ne) && !nan_free {
            return Decision::Keep(KeepReason::NanRows);
        }
    }
    decide_ordered(op, v, &min, &max)
}

/// The rules on a range `[min, max]`. A truncated (inexact) maximum of bytes is only trusted up to its increment: a writer that
/// forgot to increment leaves a maximum that is a prefix of, and below, the true maximum; every value is then below `increment(max)`
/// whatever the writer did, and `increment(max)` is an EXCLUSIVE upper bound.
fn decide_ordered(
    op: CmpOp,
    v: &Key<'_>,
    min: &Option<Stat<'_>>,
    max: &Option<Stat<'_>>,
) -> Decision {
    let incremented;
    let (hi, hi_inclusive) = match max {
        None => (None, true),
        Some(m) => match (m.key, m.exact) {
            (Key::Bytes(bytes), false) => {
                incremented = increment(bytes);
                (incremented.as_deref().map(Key::Bytes), false)
            }
            (key, _) => (Some(key), true),
        },
    };
    let lo = min.as_ref().map(|m| m.key);
    let constant_possible =
        min.as_ref().is_some_and(|m| m.exact) && max.as_ref().is_some_and(|m| m.exact);
    let excluded = match op {
        CmpOp::Lt => lo.as_ref().is_some_and(|l| ge(l, v)),
        CmpOp::Le => lo.as_ref().is_some_and(|l| gt(l, v)),
        CmpOp::Gt => hi.as_ref().is_some_and(|h| le(h, v)),
        CmpOp::Ge => hi
            .as_ref()
            .is_some_and(|h| every_value_below(h, hi_inclusive, v)),
        CmpOp::Eq => {
            lo.as_ref().is_some_and(|l| gt(l, v))
                || hi
                    .as_ref()
                    .is_some_and(|h| every_value_below(h, hi_inclusive, v))
        }
        CmpOp::Ne => match (&lo, &hi) {
            (Some(l), Some(h)) => constant_possible && eq(l, h) && eq(l, v),
            _ => false,
        },
    };
    if excluded {
        Decision::Skip(SkipReason::OutOfRange)
    } else if lo.is_none() && hi.is_none() {
        Decision::Keep(KeepReason::NoStatistics)
    } else {
        Decision::Keep(KeepReason::MayMatch)
    }
}

/// Is every value that is `<= h` (inclusive bound) or `< h` (exclusive bound) strictly below `v`?
fn every_value_below(h: &Key<'_>, hi_inclusive: bool, v: &Key<'_>) -> bool {
    if hi_inclusive { lt(h, v) } else { le(h, v) }
}
