//! `CAST` and the implicit widenings of the binder (`docs/dialect.md` section 6, `docs/decimal-rules.md` section 6, differences in dialect section 9).
//! A value that does not convert is an error at the position of the CAST, never a NULL and never a value in the message.

use nata_parquet::civil;
use nata_sql::Pos;
use nata_sql::types::SqlType;

use crate::error::{EvalError, R};
use crate::num::{
    FloatToDecimal, ParseNumber, decimal_to_f64, f64_to_scaled, fits, parse_integer, parse_scaled,
    pow10, rescale,
};
use crate::value::{Col, Data, StrData};

fn int_range(t: SqlType) -> Option<(i64, i64)> {
    Some((t.int_min()?, t.int_max()?))
}

fn no_fit(pos: Pos, from: SqlType, to: SqlType) -> EvalError {
    EvalError::at(
        pos,
        format!(
            "a {from} value does not fit {to} in this conversion. Use a wider type, or restrict the rows"
        ),
    )
}

fn not_a_number(pos: Pos, to: SqlType) -> EvalError {
    EvalError::at(
        pos,
        format!(
            "a VARCHAR value is not a valid {to} (Nata reads only the plain form: an optional sign and digits{}). Check the data, or restrict the rows",
            if matches!(
                to,
                SqlType::Decimal { .. } | SqlType::Double | SqlType::Float
            ) {
                ", a decimal point and an exponent"
            } else {
                ""
            }
        ),
    )
}

fn from_parse(e: ParseNumber, pos: Pos, from: SqlType, to: SqlType) -> EvalError {
    match e {
        ParseNumber::NotANumber => not_a_number(pos, to),
        ParseNumber::TooLarge => no_fit(pos, from, to),
    }
}

fn from_float(e: FloatToDecimal, pos: Pos, from: SqlType, to: SqlType) -> EvalError {
    match e {
        FloatToDecimal::NotFinite => EvalError::at(
            pos,
            format!("a {from} value that is not a finite number cannot be converted to {to}"),
        ),
        FloatToDecimal::TooLarge => no_fit(pos, from, to),
    }
}

fn out_valid(c: &Col, sel: &[u32]) -> Vec<bool> {
    let mut v = vec![false; c.len()];
    for &r in sel {
        let i = r as usize;
        v[i] = c.valid[i];
    }
    v
}

/// Convert the rows `sel` of `c` to type `to`. Rows outside `sel` come back NULL.
pub fn cast_col(c: &Col, to: SqlType, sel: &[u32], pos: Pos) -> R<Col> {
    if c.ty == to {
        return Ok(c.clone());
    }
    let mut out = Col::empty(to, c.len());
    cast_into(c, to, sel, pos, &mut out)?;
    Ok(out)
}

pub(crate) fn cast_into(c: &Col, to: SqlType, sel: &[u32], pos: Pos, out: &mut Col) -> R<()> {
    let from = c.ty;
    if from == to {
        return out.assign(c, sel);
    }
    let valid = out_valid(c, sel);
    let rows: Vec<usize> = sel
        .iter()
        .map(|r| *r as usize)
        .filter(|i| valid[*i])
        .collect();
    let internal =
        || EvalError::internal(format!("CAST from {from} to {to} has no implementation"));
    match (&c.data, to, &mut out.data) {
        // ---------------------------------------------------------------------------------------------- integers
        (Data::Int(s), t, Data::Int(d)) if t.is_integer() => {
            let (lo, hi) = int_range(t).ok_or_else(internal)?;
            nata_kernels::decimal::unary(s, &c.valid, sel, d, &mut out.valid, |v| {
                if v < lo || v > hi {
                    Err(no_fit(pos, from, to))
                } else {
                    Ok(v)
                }
            })?;
        }
        (Data::Int(s), SqlType::Decimal { precision, scale }, Data::Dec(d)) => {
            let p = pow10(u32::from(scale)).ok_or_else(internal)?;
            let limit = pow10(u32::from(precision))
                .ok_or_else(internal)?
                .unsigned_abs();
            nata_kernels::decimal::unary(s, &c.valid, sel, d, &mut out.valid, |v| {
                i128::from(v)
                    .checked_mul(p)
                    .filter(|v| v.unsigned_abs() < limit)
                    .ok_or_else(|| no_fit(pos, from, to))
            })?;
        }
        (Data::Int(s), SqlType::Double, Data::F64(d)) => {
            for &i in &rows {
                d[i] = decimal_to_f64(i128::from(s[i]), 0);
            }
        }
        (Data::Int(s), SqlType::Float, Data::F32(d)) => {
            for &i in &rows {
                d[i] = to_f32(decimal_to_f64(i128::from(s[i]), 0), pos, from, to)?;
            }
        }
        (Data::Int(s), SqlType::Varchar, Data::Str(StrData::Owned(d))) => {
            for &i in &rows {
                d[i] = s[i].to_string();
            }
        }
        // ---------------------------------------------------------------------------------------------- decimals
        (Data::Dec(s), t, Data::Int(d)) if t.is_integer() => {
            let SqlType::Decimal { scale, .. } = from else {
                return Err(internal());
            };
            let (lo, hi) = int_range(t).ok_or_else(internal)?;
            nata_kernels::decimal::unary(s, &c.valid, sel, d, &mut out.valid, |v| {
                rescale(v, scale, 0)
                    .and_then(|v| i64::try_from(v).ok())
                    .filter(|v| *v >= lo && *v <= hi)
                    .ok_or_else(|| no_fit(pos, from, to))
            })?;
        }
        (Data::Dec(s), SqlType::Decimal { precision, scale }, Data::Dec(d)) => {
            let SqlType::Decimal { scale: s1, .. } = from else {
                return Err(internal());
            };
            nata_kernels::decimal::unary(s, &c.valid, sel, d, &mut out.valid, |v| {
                rescale(v, s1, scale)
                    .filter(|v| fits(*v, precision))
                    .ok_or_else(|| no_fit(pos, from, to))
            })?;
        }
        (Data::Dec(s), SqlType::Double, Data::F64(d)) => {
            let SqlType::Decimal { scale, .. } = from else {
                return Err(internal());
            };
            for &i in &rows {
                d[i] = decimal_to_f64(s[i], scale);
            }
        }
        (Data::Dec(s), SqlType::Float, Data::F32(d)) => {
            let SqlType::Decimal { scale, .. } = from else {
                return Err(internal());
            };
            for &i in &rows {
                d[i] = to_f32(decimal_to_f64(s[i], scale), pos, from, to)?;
            }
        }
        (Data::Dec(s), SqlType::Varchar, Data::Str(StrData::Owned(d))) => {
            let SqlType::Decimal { precision, scale } = from else {
                return Err(internal());
            };
            for &i in &rows {
                d[i] = nata_parquet::csv::format_decimal_typed(s[i], precision, scale);
            }
        }
        // ---------------------------------------------------------------------------------------------- floating point
        (Data::F32(s), SqlType::Double, Data::F64(d)) => {
            for &i in &rows {
                d[i] = f64::from(s[i]);
            }
        }
        (Data::F64(s), SqlType::Float, Data::F32(d)) => {
            for &i in &rows {
                d[i] = to_f32(s[i], pos, from, to)?;
            }
        }
        (Data::F32(_) | Data::F64(_), t, Data::Int(d)) if t.is_integer() => {
            let (lo, hi) = int_range(t).ok_or_else(internal)?;
            for &i in &rows {
                let x = float_at(&c.data, i);
                let v = f64_to_scaled(x, 0)
                    .map_err(|e| from_float(e, pos, from, to))
                    .and_then(|v| i64::try_from(v).map_err(|_| no_fit(pos, from, to)))?;
                if v < lo || v > hi {
                    return Err(no_fit(pos, from, to));
                }
                d[i] = v;
            }
        }
        (Data::F32(_) | Data::F64(_), SqlType::Decimal { precision, scale }, Data::Dec(d)) => {
            for &i in &rows {
                let v = f64_to_scaled(float_at(&c.data, i), scale)
                    .map_err(|e| from_float(e, pos, from, to))?;
                if !fits(v, precision) {
                    return Err(no_fit(pos, from, to));
                }
                d[i] = v;
            }
        }
        // ---------------------------------------------------------------------------------------------- text to value
        (Data::Str(s), t, Data::Int(d)) if t.is_integer() => {
            let (lo, hi) = int_range(t).ok_or_else(internal)?;
            for &i in &rows {
                let v = parse_integer(s.get(i)).map_err(|e| from_parse(e, pos, from, to))?;
                let v = i64::try_from(v)
                    .ok()
                    .filter(|v| *v >= lo && *v <= hi)
                    .ok_or_else(|| no_fit(pos, from, to))?;
                d[i] = v;
            }
        }
        (Data::Str(s), SqlType::Decimal { precision, scale }, Data::Dec(d)) => {
            for &i in &rows {
                let v = parse_scaled(s.get(i), scale).map_err(|e| from_parse(e, pos, from, to))?;
                if !fits(v, precision) {
                    return Err(no_fit(pos, from, to));
                }
                d[i] = v;
            }
        }
        (Data::Str(s), SqlType::Double, Data::F64(d)) => {
            for &i in &rows {
                d[i] = parse_float(s.get(i)).ok_or_else(|| not_a_number(pos, to))?;
            }
        }
        (Data::Str(s), SqlType::Float, Data::F32(d)) => {
            for &i in &rows {
                let x = parse_float(s.get(i)).ok_or_else(|| not_a_number(pos, to))?;
                d[i] = to_f32(x, pos, from, to)?;
            }
        }
        (Data::Str(s), SqlType::Boolean, Data::Bool(d)) => {
            for &i in &rows {
                let t = s.get(i);
                d[i] = if t.eq_ignore_ascii_case("true") {
                    true
                } else if t.eq_ignore_ascii_case("false") {
                    false
                } else {
                    return Err(EvalError::at(
                        pos,
                        "a VARCHAR value is not a valid BOOLEAN (Nata reads only true and false, in any case). Check the data, or restrict the rows",
                    ));
                };
            }
        }
        (Data::Str(s), SqlType::Date, Data::Date(d)) => {
            for &i in &rows {
                d[i] = civil::parse_date(s.get(i).trim_matches(' ')).map_err(|_| {
                    EvalError::at(
                        pos,
                        "a VARCHAR value is not a valid DATE (Nata reads only YYYY-MM-DD, of a date that exists). Check the data, or restrict the rows",
                    )
                })?;
            }
        }
        // ---------------------------------------------------------------------------------------------- value to text
        (Data::Bool(s), SqlType::Varchar, Data::Str(StrData::Owned(d))) => {
            for &i in &rows {
                d[i] = if s[i] { "true" } else { "false" }.to_string();
            }
        }
        (Data::Date(s), SqlType::Varchar, Data::Str(StrData::Owned(d))) => {
            for &i in &rows {
                d[i] = civil::format_date(s[i]);
            }
        }
        _ => return Err(internal()),
    }
    out.valid = valid;
    Ok(())
}

fn float_at(d: &Data, i: usize) -> f64 {
    match d {
        Data::F32(v) => f64::from(v[i]),
        Data::F64(v) => v[i],
        _ => f64::NAN,
    }
}

/// A double as a REAL: the nearest one; a finite number that becomes infinite is an overflow.
#[allow(
    clippy::cast_possible_truncation,
    reason = "the narrowing of a double to a REAL is the nearest float, checked for overflow on the next line"
)]
fn to_f32(x: f64, pos: Pos, from: SqlType, to: SqlType) -> R<f32> {
    let y = x as f32;
    if x.is_finite() && y.is_infinite() {
        return Err(no_fit(pos, from, to));
    }
    Ok(y)
}

/// A double from text: spaces trimmed, then what Rust reads (sign, digits, point, exponent, `inf`, `infinity`, `nan`, any case); nothing else.
fn parse_float(text: &str) -> Option<f64> {
    let t = text.trim_matches(' ');
    if t.is_empty() || !t.is_ascii() {
        return None;
    }
    if t.bytes().any(|c| c == b'_' || c.is_ascii_whitespace()) {
        return None;
    }
    t.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int_col(v: &[Option<i64>], ty: SqlType) -> Col {
        let mut c = Col::empty(ty, v.len());
        if let Data::Int(d) = &mut c.data {
            for (i, x) in v.iter().enumerate() {
                if let Some(x) = x {
                    d[i] = *x;
                    c.valid[i] = true;
                }
            }
        }
        c
    }

    fn all(n: usize) -> Vec<u32> {
        (0..u32::try_from(n).unwrap_or(0)).collect()
    }

    fn text_col(v: &[&str]) -> Col {
        let mut c = Col::empty(SqlType::Varchar, v.len());
        if let Data::Str(StrData::Owned(d)) = &mut c.data {
            for (i, x) in v.iter().enumerate() {
                d[i] = (*x).to_string();
                c.valid[i] = true;
            }
        }
        c
    }

    #[test]
    fn integers_narrow_with_a_range_check_and_a_null_stays_null() {
        let c = int_col(&[Some(5), None, Some(40000)], SqlType::BigInt);
        let sel = all(3);
        let r = cast_col(&c, SqlType::Integer, &sel, Pos::START).expect("fits");
        assert_eq!(r.valid, vec![true, false, true]);
        let e = cast_col(&c, SqlType::SmallInt, &sel, Pos { line: 1, col: 9 });
        let e = e.expect_err("40000 is not a SMALLINT");
        assert_eq!(e.pos, Pos { line: 1, col: 9 });
        assert!(!e.message.contains("40000"), "{e}");
        // an error is raised only for a selected row
        let r =
            cast_col(&c, SqlType::SmallInt, &[0, 1], Pos::START).expect("row 2 is not selected");
        assert_eq!(r.valid, vec![true, false, false]);
    }

    #[test]
    fn text_to_numbers_dates_and_booleans() {
        let c = text_col(&["12", " -7 ", "x", "1.5"]);
        assert!(cast_col(&c, SqlType::Integer, &[0, 1], Pos::START).is_ok());
        assert!(cast_col(&c, SqlType::Integer, &[2], Pos::START).is_err());
        assert!(cast_col(&c, SqlType::Integer, &[3], Pos::START).is_err());
        let d = SqlType::Decimal {
            precision: 5,
            scale: 1,
        };
        let r = cast_col(&c, d, &[0, 1, 3], Pos::START).expect("decimals");
        match r.data {
            Data::Dec(v) => assert_eq!((v[0], v[1], v[3]), (120, -70, 15)),
            _ => panic!("not decimal"),
        }
        let c = text_col(&["2024-02-29", "2023-02-29", "2024-2-9", " 2024-02-29 "]);
        assert!(cast_col(&c, SqlType::Date, &[0, 3], Pos::START).is_ok());
        assert!(cast_col(&c, SqlType::Date, &[1], Pos::START).is_err());
        assert!(cast_col(&c, SqlType::Date, &[2], Pos::START).is_err());
        let c = text_col(&["TRUE", "false", "t"]);
        assert!(cast_col(&c, SqlType::Boolean, &[0, 1], Pos::START).is_ok());
        assert!(cast_col(&c, SqlType::Boolean, &[2], Pos::START).is_err());
    }

    #[test]
    fn decimals_round_half_away_and_report_what_does_not_fit() {
        let d = SqlType::Decimal {
            precision: 5,
            scale: 3,
        };
        let mut c = Col::empty(d, 3);
        if let Data::Dec(v) = &mut c.data {
            v[0] = 12345;
            v[1] = -12345;
            v[2] = 99999;
        }
        c.valid = vec![true; 3];
        let to = SqlType::Decimal {
            precision: 4,
            scale: 2,
        };
        let r = cast_col(&c, to, &[0, 1], Pos::START).expect("1234.5 rounds");
        match r.data {
            Data::Dec(v) => assert_eq!((v[0], v[1]), (1235, -1235)),
            _ => panic!(),
        }
        assert!(
            cast_col(&c, to, &[2], Pos::START).is_err(),
            "99.999 rounds to 100.00, 5 digits"
        );
        let r = cast_col(&c, SqlType::Integer, &[0, 1], Pos::START).expect("to integer");
        match r.data {
            Data::Int(v) => assert_eq!((v[0], v[1]), (12, -12)),
            _ => panic!(),
        }
    }
}
