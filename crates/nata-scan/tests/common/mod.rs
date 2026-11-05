//! Test support: a deterministic generator of row groups, statistics as real writers produce them, predicates, and a brute-force row-by-row oracle
//! with SQL (DuckDB 2.0.0) semantics. The oracle shares no code with the library: its comparisons are written again, and decimals are compared as
//! digit strings, not by rescaling.
#![allow(dead_code)]

use std::cmp::Ordering;
use std::fmt::Write as _;

use nata_scan::{
    Bound, CmpOp, ColumnStats, ColumnType, Decision, Predicate, RowGroupStats, Term, Value,
};

// ------------------------------------------------------------------------------------------------ the generator

/// splitmix64: small, fast, and the same on every platform.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (n > 0), with a negligible modulo bias.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }

    pub fn idx(&mut self, n: usize) -> usize {
        (self.next_u64() % (n as u64)) as usize
    }

    pub fn chance(&mut self, num: u64, den: u64) -> bool {
        self.below(den) < num
    }

    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.idx(xs.len())]
    }

    /// Uniform in `lo..=hi`.
    pub fn range_i64(&mut self, lo: i64, hi: i64) -> i64 {
        let span = (hi as i128 - lo as i128 + 1) as u128;
        let r = ((self.next_u64() as u128) << 64 | self.next_u64() as u128) % span;
        (lo as i128 + r as i128) as i64
    }

    pub fn u128(&mut self) -> u128 {
        (self.next_u64() as u128) << 64 | self.next_u64() as u128
    }
}

/// The RNG of one case: independent of the other cases, so one case replays alone.
pub fn case_rng(seed: u64, case: u64) -> Rng {
    let mut r = Rng::new(seed ^ case.wrapping_mul(0xD1B5_4A32_D192_ED03));
    r.next_u64();
    r
}

#[derive(Clone, Debug)]
pub struct Column {
    pub name: String,
    pub ty: ColumnType,
    pub cells: Vec<Option<Value>>,
}

#[derive(Clone, Debug)]
pub struct Table {
    pub rows: usize,
    pub columns: Vec<Column>,
}

pub fn pow10(p: u32) -> i128 {
    10i128.pow(p)
}

pub fn gen_type(rng: &mut Rng) -> ColumnType {
    match rng.below(11) {
        0 => ColumnType::Bool,
        1 => ColumnType::Int32,
        2 => ColumnType::Int64,
        3 => ColumnType::Float32,
        4 | 5 => ColumnType::Float64,
        6 => {
            let precision = *rng.pick(&[1u8, 2, 3, 5, 9, 10, 18, 19, 20, 38]);
            let scale = if rng.chance(1, 4) {
                *rng.pick(&[0, precision])
            } else {
                rng.below(u64::from(precision) + 1) as u8
            };
            ColumnType::Decimal { precision, scale }
        }
        7 => {
            if rng.chance(1, 2) {
                ColumnType::Date32
            } else {
                ColumnType::TimestampMicros
            }
        }
        8 | 9 => ColumnType::Utf8,
        _ => ColumnType::Binary,
    }
}

const STRING_PIECES: [&str; 12] = [
    "a",
    "b",
    "c",
    "ab",
    "é",
    "日",
    "𝄞",
    "\u{0}",
    "\u{7f}",
    "z",
    "\u{10FFFF}",
    "ab",
];

pub fn rand_string(rng: &mut Rng) -> String {
    let n = if rng.chance(1, 10) {
        rng.below(25)
    } else {
        rng.below(6)
    };
    let mut s = String::new();
    for _ in 0..n {
        s.push_str(rng.pick(&STRING_PIECES));
    }
    s
}

pub fn rand_bytes(rng: &mut Rng) -> Vec<u8> {
    let n = if rng.chance(1, 10) {
        rng.below(20)
    } else {
        rng.below(6)
    };
    (0..n)
        .map(|_| *rng.pick(&[0u8, 1, 0x61, 0x7f, 0x80, 0xC3, 0xFE, 0xFF, 0xFF]))
        .collect()
}

/// A random value of the type, favouring the boundary values that break rules.
pub fn rand_value(rng: &mut Rng, ty: ColumnType) -> Value {
    match ty {
        ColumnType::Bool => Value::Bool(rng.chance(1, 2)),
        ColumnType::Int32 => Value::Int32(match rng.below(4) {
            0 => *rng.pick(&[i32::MIN, i32::MIN + 1, -1, 0, 1, i32::MAX - 1, i32::MAX]),
            1 | 2 => rng.range_i64(-6, 6) as i32,
            _ => rng.range_i64(i32::MIN.into(), i32::MAX.into()) as i32,
        }),
        ColumnType::Int64 => Value::Int64(match rng.below(4) {
            0 => *rng.pick(&[
                i64::MIN,
                i64::MIN + 1,
                -1,
                0,
                1,
                i64::MAX - 1,
                i64::MAX,
                i64::from(i32::MIN),
                i64::from(i32::MAX),
            ]),
            1 | 2 => rng.range_i64(-6, 6),
            _ => rng.range_i64(i64::MIN, i64::MAX),
        }),
        ColumnType::Date32 => Value::Date32(match rng.below(3) {
            0 => *rng.pick(&[i32::MIN, 0, i32::MAX, -1, 1]),
            _ => rng.range_i64(-6, 6) as i32,
        }),
        ColumnType::TimestampMicros => Value::TimestampMicros(match rng.below(3) {
            0 => *rng.pick(&[i64::MIN, 0, i64::MAX, -1, 1]),
            _ => rng.range_i64(-6, 6),
        }),
        ColumnType::Float64 => Value::Float64(rand_f64(rng)),
        ColumnType::Float32 => Value::Float32(rand_f32(rng)),
        ColumnType::Decimal { precision, scale } => Value::Decimal {
            unscaled: rand_unscaled(rng, u32::from(precision)),
            precision,
            scale,
        },
        ColumnType::Utf8 => Value::Utf8(rand_string(rng)),
        ColumnType::Binary => Value::Binary(rand_bytes(rng)),
        ColumnType::Unsupported => Value::Bool(false),
    }
}

pub fn rand_f64(rng: &mut Rng) -> f64 {
    match rng.below(10) {
        0 => f64::NAN,
        1 => -0.0,
        2 => 0.0,
        3 => *rng.pick(&[
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::MAX,
            f64::MIN,
            f64::MIN_POSITIVE,
            5e-324,
            -5e-324,
        ]),
        4..=7 => rng.range_i64(-8, 8) as f64 / 2.0,
        _ => (rng.range_i64(-1_000_000, 1_000_000) as f64) / 1000.0,
    }
}

pub fn rand_f32(rng: &mut Rng) -> f32 {
    match rng.below(10) {
        0 => f32::NAN,
        1 => -0.0,
        2 => 0.0,
        3 => *rng.pick(&[
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::MAX,
            f32::MIN,
            f32::MIN_POSITIVE,
            1e-45,
            -1e-45,
        ]),
        4..=7 => rng.range_i64(-8, 8) as f32 / 2.0,
        _ => (rng.range_i64(-1_000_000, 1_000_000) as f32) / 1000.0,
    }
}

/// An unscaled decimal value that fits `precision` digits, often at the extremes.
pub fn rand_unscaled(rng: &mut Rng, precision: u32) -> i128 {
    let max = pow10(precision) - 1;
    match rng.below(5) {
        0 => *rng.pick(&[
            0,
            1,
            -1,
            max,
            -max,
            pow10(precision - 1),
            -pow10(precision - 1),
        ]),
        1 | 2 => i128::from(rng.range_i64(-6, 6)).clamp(-max, max),
        _ => {
            let m = (rng.u128() % (max as u128 + 1)) as i128;
            if rng.chance(1, 2) { -m } else { m }
        }
    }
}

pub fn gen_table(rng: &mut Rng) -> Table {
    let rows = match rng.below(100) {
        0..=2 => 0,
        3..=12 => 1,
        13..=52 => 2 + rng.idx(5),
        _ => 7 + rng.idx(34),
    };
    let ncols = 1 + rng.idx(3);
    let mut columns = Vec::new();
    for i in 0..ncols {
        let ty = gen_type(rng);
        let pool: Vec<Value> = (0..3 + rng.idx(6)).map(|_| rand_value(rng, ty)).collect();
        let null_share = *rng.pick(&[0u64, 0, 0, 0, 0, 2, 2, 5, 10]); // tenths
        let mut cells = Vec::with_capacity(rows);
        for _ in 0..rows {
            if rng.chance(null_share, 10) {
                cells.push(None);
            } else if rng.chance(8, 10) {
                cells.push(Some(rng.pick(&pool).clone()));
            } else {
                cells.push(Some(rand_value(rng, ty)));
            }
        }
        columns.push(Column {
            name: format!("c{i}"),
            ty,
            cells,
        });
    }
    Table { rows, columns }
}

// ------------------------------------------------------------------------------------------------ the oracle

fn float_of(v: &Value) -> Option<f64> {
    match v {
        Value::Float32(x) => Some(f64::from(*x)),
        Value::Float64(x) => Some(*x),
        _ => None,
    }
}

fn int_of(v: &Value) -> Option<i128> {
    match v {
        Value::Int32(x) | Value::Date32(x) => Some(i128::from(*x)),
        Value::Int64(x) | Value::TimestampMicros(x) => Some(i128::from(*x)),
        _ => None,
    }
}

/// sign, integer digits without leading zeros, fraction digits padded to 40.
fn dec_digits(unscaled: i128, scale: u8) -> (i8, String, String) {
    let sign = unscaled.signum() as i8;
    let mut digits = unscaled.unsigned_abs().to_string();
    let scale = usize::from(scale);
    while digits.len() <= scale {
        digits.insert(0, '0');
    }
    let split = digits.len() - scale;
    let int = digits[..split].trim_start_matches('0').to_string();
    let mut frac = digits[split..].to_string();
    while frac.len() < 40 {
        frac.push('0');
    }
    (sign, int, frac)
}

fn cmp_dec_digits(a: (i128, u8), b: (i128, u8)) -> Ordering {
    let (sa, ia, fa) = dec_digits(a.0, a.1);
    let (sb, ib, fb) = dec_digits(b.0, b.1);
    if sa != sb {
        return sa.cmp(&sb);
    }
    let mag = ia
        .len()
        .cmp(&ib.len())
        .then_with(|| ia.cmp(&ib))
        .then_with(|| fa.cmp(&fb));
    if sa < 0 { mag.reverse() } else { mag }
}

/// The SQL order of two values of comparable types (DuckDB 2.0.0): NaN equals NaN and is above every number, -0.0 equals +0.0, decimals are compared exactly.
pub fn sql_cmp(a: &Value, b: &Value) -> Ordering {
    if let (Some(x), Some(y)) = (float_of(a), float_of(b)) {
        return match (x.is_nan(), y.is_nan()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            _ => {
                if x < y {
                    Ordering::Less
                } else if x > y {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            }
        };
    }
    match (a, b) {
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Utf8(x), Value::Utf8(y)) => x.as_bytes().cmp(y.as_bytes()),
        (Value::Binary(x), Value::Binary(y)) => x.cmp(y),
        (
            Value::Decimal {
                unscaled: x,
                scale: sx,
                ..
            },
            Value::Decimal {
                unscaled: y,
                scale: sy,
                ..
            },
        ) => cmp_dec_digits((*x, *sx), (*y, *sy)),
        (
            Value::Decimal {
                unscaled: x,
                scale: sx,
                ..
            },
            other,
        ) => match int_of(other) {
            Some(y) => cmp_dec_digits((*x, *sx), (y, 0)),
            None => panic!("incomparable {a:?} {b:?}"),
        },
        (
            other,
            Value::Decimal {
                unscaled: y,
                scale: sy,
                ..
            },
        ) => match int_of(other) {
            Some(x) => cmp_dec_digits((x, 0), (*y, *sy)),
            None => panic!("incomparable {a:?} {b:?}"),
        },
        _ => match (int_of(a), int_of(b)) {
            (Some(x), Some(y)) => x.cmp(&y),
            _ => panic!("incomparable {a:?} {b:?}"),
        },
    }
}

pub fn op_holds(op: CmpOp, ord: Ordering) -> bool {
    match op {
        CmpOp::Lt => ord == Ordering::Less,
        CmpOp::Le => ord != Ordering::Greater,
        CmpOp::Gt => ord == Ordering::Greater,
        CmpOp::Ge => ord != Ordering::Less,
        CmpOp::Eq => ord == Ordering::Equal,
        CmpOp::Ne => ord != Ordering::Equal,
    }
}

/// SQL truth of one term on one row: true only if the term is TRUE (NULL and false both filter the row out).
pub fn term_true(table: &Table, term: &Term, row: usize) -> bool {
    let col_of = |name: &str| {
        table
            .columns
            .iter()
            .find(|c| c.name == name)
            .expect("column")
    };
    match term {
        Term::Cmp {
            column,
            op,
            literal,
        } => match &col_of(column).cells[row] {
            None => false,
            Some(v) => op_holds(*op, sql_cmp(v, literal)),
        },
        Term::IsNull { column } => col_of(column).cells[row].is_none(),
        Term::IsNotNull { column } => col_of(column).cells[row].is_some(),
    }
}

pub fn count_matches(table: &Table, predicate: &Predicate) -> usize {
    (0..table.rows)
        .filter(|&r| predicate.terms.iter().all(|t| term_true(table, t, r)))
        .count()
}

// ------------------------------------------------------------------------------------------------ statistics as writers write them

fn is_nan_value(v: &Value) -> bool {
    float_of(v).is_some_and(f64::is_nan)
}

fn char_boundary_at_most(s: &str, n: usize) -> usize {
    let mut i = n.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// The smallest string above every string with the prefix `p`, as a valid string: add one to the last character, dropping characters that cannot grow.
pub fn increment_utf8(p: &str) -> Option<String> {
    let mut chars: Vec<char> = p.chars().collect();
    while let Some(c) = chars.pop() {
        let mut next = c as u32 + 1;
        if (0xD800..=0xDFFF).contains(&next) {
            next = 0xE000;
        }
        if let Some(n) = char::from_u32(next) {
            chars.push(n);
            return Some(chars.into_iter().collect());
        }
    }
    None
}

pub fn increment_bytes(p: &[u8]) -> Option<Vec<u8>> {
    let mut v = p.to_vec();
    while let Some(last) = v.pop() {
        if last != 0xFF {
            v.push(last + 1);
            return Some(v);
        }
    }
    None
}

/// How a writer cuts a long bound.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Truncation {
    None,
    /// Prefix of the minimum; prefix of the maximum with the increment the format asks for.
    Incrementing(usize),
    /// Prefix of the minimum; prefix of the maximum WITHOUT the increment (a faulty writer), flagged as truncated.
    NotIncrementing(usize),
}

fn loosen_lower(rng: &mut Rng, v: &Value) -> Value {
    match v {
        Value::Bool(_) => Value::Bool(false),
        Value::Int32(x) => Value::Int32(if rng.chance(1, 3) {
            i32::MIN
        } else {
            x.saturating_sub(rng.below(100) as i32)
        }),
        Value::Int64(x) => Value::Int64(if rng.chance(1, 3) {
            i64::MIN
        } else {
            x.saturating_sub(rng.below(100) as i64)
        }),
        Value::Date32(x) => Value::Date32(x.saturating_sub(rng.below(100) as i32)),
        Value::TimestampMicros(x) => {
            Value::TimestampMicros(x.saturating_sub(rng.below(100) as i64))
        }
        Value::Float32(x) => Value::Float32(if rng.chance(1, 3) {
            f32::NEG_INFINITY
        } else {
            x - 1.5 - x.abs()
        }),
        Value::Float64(x) => Value::Float64(if rng.chance(1, 3) {
            f64::NEG_INFINITY
        } else {
            x - 1.5 - x.abs()
        }),
        Value::Decimal {
            unscaled,
            precision,
            scale,
        } => {
            let max = pow10(u32::from(*precision)) - 1;
            Value::Decimal {
                unscaled: unscaled
                    .saturating_sub(i128::from(rng.below(100) as i64))
                    .max(-max),
                precision: *precision,
                scale: *scale,
            }
        }
        Value::Utf8(s) => {
            Value::Utf8(s[..char_boundary_at_most(s, rng.idx(s.len() + 1))].to_string())
        }
        Value::Binary(b) => Value::Binary(b[..rng.idx(b.len() + 1)].to_vec()),
    }
}

fn loosen_upper(rng: &mut Rng, v: &Value) -> Value {
    match v {
        Value::Bool(_) => Value::Bool(true),
        Value::Int32(x) => Value::Int32(if rng.chance(1, 3) {
            i32::MAX
        } else {
            x.saturating_add(rng.below(100) as i32)
        }),
        Value::Int64(x) => Value::Int64(if rng.chance(1, 3) {
            i64::MAX
        } else {
            x.saturating_add(rng.below(100) as i64)
        }),
        Value::Date32(x) => Value::Date32(x.saturating_add(rng.below(100) as i32)),
        Value::TimestampMicros(x) => {
            Value::TimestampMicros(x.saturating_add(rng.below(100) as i64))
        }
        Value::Float32(x) => Value::Float32(if rng.chance(1, 3) {
            f32::INFINITY
        } else {
            x + 1.5 + x.abs()
        }),
        Value::Float64(x) => Value::Float64(if rng.chance(1, 3) {
            f64::INFINITY
        } else {
            x + 1.5 + x.abs()
        }),
        Value::Decimal {
            unscaled,
            precision,
            scale,
        } => {
            let max = pow10(u32::from(*precision)) - 1;
            Value::Decimal {
                unscaled: unscaled
                    .saturating_add(i128::from(rng.below(100) as i64))
                    .min(max),
                precision: *precision,
                scale: *scale,
            }
        }
        Value::Utf8(s) => Value::Utf8(format!("{s}\u{10FFFF}")),
        Value::Binary(b) => {
            let mut b = b.clone();
            b.push(0xFF);
            Value::Binary(b)
        }
    }
}

fn zero_sign(rng: &mut Rng, v: &Value) -> Value {
    let neg = rng.chance(1, 2);
    match v {
        Value::Float64(x) if *x == 0.0 => Value::Float64(if neg { -0.0 } else { 0.0 }),
        Value::Float32(x) if *x == 0.0 => Value::Float32(if neg { -0.0 } else { 0.0 }),
        o => o.clone(),
    }
}

/// Statistics of one column the way real writers produce them, with the faults that are known in the field:
/// NaN left out of the bounds (the Parquet rule); bounds widened; `-0.0`/`+0.0` written either way round; bounds contaminated by NaN
/// (a page of NaN merged with others); string bounds truncated with or without the increment; any statistic missing.
pub fn make_stats(
    rng: &mut Rng,
    ty: ColumnType,
    cells: &[Option<Value>],
) -> (ColumnStats, Truncation) {
    let nulls = cells.iter().filter(|c| c.is_none()).count() as u64;
    let nans = cells.iter().flatten().filter(|v| is_nan_value(v)).count() as u64;
    let values: Vec<&Value> = cells
        .iter()
        .flatten()
        .filter(|v| !is_nan_value(v))
        .collect();
    let mut min: Option<Value> = None;
    let mut max: Option<Value> = None;
    for v in &values {
        if min.as_ref().is_none_or(|m| sql_cmp(v, m) == Ordering::Less) {
            min = Some((*v).clone());
        }
        if max
            .as_ref()
            .is_none_or(|m| sql_cmp(v, m) == Ordering::Greater)
        {
            max = Some((*v).clone());
        }
    }
    let mut min_b = min.clone().map(Bound::exact);
    let mut max_b = max.clone().map(Bound::exact);
    let is_float = matches!(ty, ColumnType::Float32 | ColumnType::Float64);
    if is_float {
        if let Some(m) = &min {
            min_b = Some(Bound::exact(zero_sign(rng, m)));
        }
        if let Some(m) = &max {
            max_b = Some(Bound::exact(zero_sign(rng, m)));
        }
        // A page of NaN merged with the others: NaN gets into the bounds and stays there.
        if nans > 0 && rng.chance(1, 4) {
            let nan = if matches!(ty, ColumnType::Float32) {
                Value::Float32(f32::NAN)
            } else {
                Value::Float64(f64::NAN)
            };
            if rng.chance(3, 4) {
                min_b = Some(Bound::exact(nan.clone()));
            }
            if rng.chance(3, 4) || min_b.as_ref().is_none_or(|b| !is_nan_value(&b.value)) {
                max_b = Some(Bound::exact(nan));
            }
        }
    }
    // Widened bounds.
    if rng.chance(1, 6)
        && let Some(m) = &min
    {
        min_b = Some(Bound::truncated(loosen_lower(rng, m)));
    }
    if rng.chance(1, 6)
        && let Some(m) = &max
    {
        max_b = Some(Bound::truncated(loosen_upper(rng, m)));
    }
    // Truncation of text.
    let mut truncation = Truncation::None;
    if matches!(ty, ColumnType::Utf8 | ColumnType::Binary) && rng.chance(2, 5) {
        let len = rng.idx(9);
        truncation = if rng.chance(3, 5) {
            Truncation::Incrementing(len)
        } else {
            Truncation::NotIncrementing(len)
        };
        if let (Some(m), true) = (&min, min_b.as_ref().is_some_and(|b| b.exact)) {
            min_b = match m {
                Value::Utf8(s) if s.len() > len => Some(Bound::truncated(Value::Utf8(
                    s[..char_boundary_at_most(s, len)].to_string(),
                ))),
                Value::Binary(b) if b.len() > len => {
                    Some(Bound::truncated(Value::Binary(b[..len].to_vec())))
                }
                _ => min_b,
            };
        }
        if let (Some(m), true) = (&max, max_b.as_ref().is_some_and(|b| b.exact)) {
            let inc = matches!(truncation, Truncation::Incrementing(_));
            max_b = match m {
                Value::Utf8(s) if s.len() > len => {
                    let p = &s[..char_boundary_at_most(s, len)];
                    if inc {
                        increment_utf8(p).map(|x| Bound::truncated(Value::Utf8(x)))
                    } else {
                        Some(Bound::truncated(Value::Utf8(p.to_string())))
                    }
                }
                Value::Binary(b) if b.len() > len => {
                    let p = &b[..len];
                    if inc {
                        increment_bytes(p).map(|x| Bound::truncated(Value::Binary(x)))
                    } else {
                        Some(Bound::truncated(Value::Binary(p.to_vec())))
                    }
                }
                _ => max_b,
            };
        }
    }
    // Missing statistics.
    if rng.chance(1, 8) {
        min_b = None;
    }
    if rng.chance(1, 8) {
        max_b = None;
    }
    // An all-NULL chunk has no bounds.
    if nulls as usize == cells.len() {
        min_b = None;
        max_b = None;
    }
    let stats = ColumnStats {
        ty,
        null_count: if rng.chance(1, 8) { None } else { Some(nulls) },
        nan_count: if is_float && rng.chance(1, 2) {
            Some(nans)
        } else {
            None
        },
        min: min_b,
        max: max_b,
    };
    (stats, truncation)
}

pub fn make_row_group(rng: &mut Rng, table: &Table) -> RowGroupStats {
    let mut rg = RowGroupStats::new(table.rows as u64);
    for c in &table.columns {
        let (s, _) = make_stats(rng, c.ty, &c.cells);
        rg = rg.with_column(&c.name, s);
    }
    rg
}

// ------------------------------------------------------------------------------------------------ predicates

fn dec_literal_near(rng: &mut Rng, unscaled: i128, scale: u8) -> Value {
    // The same number at another scale (more or fewer fraction digits), shifted by a delta that sometimes lands between two column values.
    let target = rng.below(39) as u8;
    let delta: i128 = *rng.pick(&[0, 0, 0, 1, -1, 5, -5]);
    let u = if target >= scale {
        let k = u32::from(target - scale);
        match pow10(k.min(38)).checked_mul(unscaled) {
            Some(m) if k <= 38 => m.checked_add(delta).unwrap_or(m),
            _ => {
                return Value::Decimal {
                    unscaled: 0,
                    precision: 1,
                    scale: 0,
                };
            }
        }
    } else {
        unscaled / pow10(u32::from(scale - target))
    };
    let digits = u.unsigned_abs().to_string().len().max(1) as u8;
    let precision = digits.max(target).max(1);
    if precision > 38 || u.unsigned_abs() >= pow10(u32::from(precision)) as u128 {
        return Value::Decimal {
            unscaled: 0,
            precision: 1,
            scale: 0,
        };
    }
    Value::Decimal {
        unscaled: u,
        precision,
        scale: target,
    }
}

pub fn gen_literal(rng: &mut Rng, col: &Column) -> Value {
    let from_cells: Vec<&Value> = col.cells.iter().flatten().collect();
    let cell = if from_cells.is_empty() || rng.chance(1, 4) {
        rand_value(rng, col.ty)
    } else {
        (*rng.pick(&from_cells)).clone()
    };
    match (&cell, col.ty) {
        (Value::Int32(x), _) => match rng.below(6) {
            0 => Value::Int64(i64::from(*x).saturating_add(*rng.pick(&[-1, 0, 1]))),
            1 => Value::Int64(*rng.pick(&[
                i64::MIN,
                i64::MAX,
                i64::from(i32::MAX) + 1,
                i64::from(i32::MIN) - 1,
            ])),
            2 => Value::Int32(x.saturating_add(*rng.pick(&[-1, 1]))),
            _ => cell,
        },
        (Value::Int64(x), ColumnType::Int64) => match rng.below(6) {
            0 => Value::Int32(rng.range_i64(i32::MIN.into(), i32::MAX.into()) as i32),
            1 => Value::Int64(x.saturating_add(*rng.pick(&[-1, 1]))),
            _ => cell,
        },
        (Value::Date32(x), _) => Value::Date32(x.saturating_add(*rng.pick(&[-1, 0, 0, 1]))),
        (Value::TimestampMicros(x), _) => {
            Value::TimestampMicros(x.saturating_add(*rng.pick(&[-1, 0, 0, 1])))
        }
        (Value::Float32(x), _) => match rng.below(6) {
            0 => Value::Float64(f64::from(*x)),
            1 => Value::Float64(f64::from(*x) + 1e-9),
            2 => Value::Float64(rand_f64(rng)),
            3 => Value::Float32(rand_f32(rng)),
            _ => cell,
        },
        (Value::Float64(_), _) => match rng.below(5) {
            0 => Value::Float32(rand_f32(rng)),
            1 => Value::Float64(rand_f64(rng)),
            _ => cell,
        },
        (
            Value::Decimal {
                unscaled, scale, ..
            },
            _,
        ) => match rng.below(6) {
            0..=2 => dec_literal_near(rng, *unscaled, *scale),
            3 => Value::Int64(i64::try_from(unscaled / pow10(u32::from(*scale))).unwrap_or(0)),
            _ => cell,
        },
        (Value::Utf8(s), _) => match rng.below(6) {
            0 => Value::Utf8(s[..char_boundary_at_most(s, rng.idx(s.len() + 1))].to_string()),
            1 => Value::Utf8(format!("{s}\u{0}")),
            2 => Value::Utf8(
                increment_utf8(&s[..char_boundary_at_most(s, rng.idx(s.len() + 1))])
                    .unwrap_or_default(),
            ),
            3 => Value::Utf8(rand_string(rng)),
            _ => cell,
        },
        (Value::Binary(b), _) => match rng.below(6) {
            0 => Value::Binary(b[..rng.idx(b.len() + 1)].to_vec()),
            1 => {
                let mut x = b.clone();
                x.push(0);
                Value::Binary(x)
            }
            2 => Value::Binary(increment_bytes(&b[..rng.idx(b.len() + 1)]).unwrap_or_default()),
            3 => Value::Binary(rand_bytes(rng)),
            _ => cell,
        },
        _ => cell,
    }
}

pub fn gen_predicate(rng: &mut Rng, table: &Table) -> Predicate {
    let nterms = 1 + rng.idx(3);
    let mut terms = Vec::new();
    for _ in 0..nterms {
        let col = rng.pick(&table.columns);
        let term = match rng.below(20) {
            0 => Term::is_null(&col.name),
            1 => Term::is_not_null(&col.name),
            _ => {
                let op = *rng.pick(&[
                    CmpOp::Lt,
                    CmpOp::Le,
                    CmpOp::Gt,
                    CmpOp::Ge,
                    CmpOp::Eq,
                    CmpOp::Ne,
                ]);
                Term::cmp(&col.name, op, gen_literal(rng, col))
            }
        };
        terms.push(term);
    }
    Predicate::new(terms)
}

// ------------------------------------------------------------------------------------------------ the soundness run

pub type Decider<'a> = &'a dyn Fn(&RowGroupStats, &Predicate) -> Decision;

#[derive(Debug)]
pub struct Failure {
    pub seed: u64,
    pub case: u64,
    pub table: Table,
    pub stats: RowGroupStats,
    pub predicate: Predicate,
    pub decision: Decision,
    pub matching_rows: usize,
}

#[derive(Debug, Default, Clone)]
pub struct Summary {
    pub cases: u64,
    pub skipped: u64,
    /// Skips by reason (the Debug name of the `SkipReason`).
    pub by_reason: std::collections::BTreeMap<String, u64>,
}

pub fn gen_case(seed: u64, case: u64) -> (Table, RowGroupStats, Predicate) {
    let mut rng = case_rng(seed, case);
    let table = gen_table(&mut rng);
    let stats = make_row_group(&mut rng, &table);
    let predicate = gen_predicate(&mut rng, &table);
    (table, stats, predicate)
}

/// Run cases `from..from + count`; stop at the first case where `decide` says Skip and a row matches.
pub fn run_cases(
    seed: u64,
    from: u64,
    count: u64,
    decide: Decider<'_>,
) -> Result<Summary, Box<Failure>> {
    let mut summary = Summary::default();
    for case in from..from + count {
        let (table, stats, predicate) = gen_case(seed, case);
        summary.cases += 1;
        let decision = decide(&stats, &predicate);
        let matching_rows = count_matches(&table, &predicate);
        if decision.is_skip() {
            summary.skipped += 1;
            if let Decision::Skip(r) = decision {
                *summary.by_reason.entry(format!("{r:?}")).or_insert(0) += 1;
            }
            if matching_rows > 0 {
                return Err(Box::new(Failure {
                    seed,
                    case,
                    table,
                    stats,
                    predicate,
                    decision,
                    matching_rows,
                }));
            }
        }
    }
    Ok(summary)
}

pub fn describe(f: &Failure) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "UNSOUND: seed={} case={} decision={:?} but {} row(s) match",
        f.seed, f.case, f.decision, f.matching_rows
    );
    let _ = writeln!(
        s,
        "replay: NATA_SCAN_SEED={} NATA_SCAN_ONLY={} cargo test -p nata-scan --test soundness -- --nocapture",
        f.seed, f.case
    );
    let _ = writeln!(s, "predicate: {:?}", f.predicate);
    let _ = writeln!(s, "stats: {:?}", f.stats);
    for c in &f.table.columns {
        let _ = writeln!(s, "column {} {:?}: {:?}", c.name, c.ty, c.cells);
    }
    s
}

pub fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| {
        let v = v.trim();
        if let Some(hex) = v.strip_prefix("0x") {
            u64::from_str_radix(hex, 16).ok()
        } else {
            v.parse().ok()
        }
    })
}

pub const DEFAULT_SEED: u64 = 0x4E41_5441_5343_414E;

pub fn seed() -> u64 {
    env_u64("NATA_SCAN_SEED").unwrap_or(DEFAULT_SEED)
}

/// 400,000 cases in a release build, 40,000 in a debug build, unless `NATA_SCAN_CASES` says otherwise.
pub fn cases() -> u64 {
    env_u64("NATA_SCAN_CASES").unwrap_or(if cfg!(debug_assertions) {
        40_000
    } else {
        400_000
    })
}
