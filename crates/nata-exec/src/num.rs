//! Exact number helpers: powers of ten, decimal fit checks, division rounded half away from zero, conversions between decimals, integers, doubles
//! and text (`docs/decimal-rules.md` sections 2 and 6). Everything is checked; a value that does not fit is `None` or an `Err`, never a wrapped number.

pub use nata_kernels::decimal::{div_round_away, fits, pow10, rescale};

/// The text of an unscaled decimal with exactly `scale` fraction digits (no DuckDB leading-zero rule: see `nata_parquet::csv::format_decimal_typed`).
pub fn plain_decimal(v: i128, scale: u8) -> String {
    nata_parquet::csv::format_decimal(v, scale)
}

/// The nearest double to `v / 10^scale` (correctly rounded: exact when both parts are exactly representable, else through the decimal text).
pub fn decimal_to_f64(v: i128, scale: u8) -> f64 {
    // 10^0..10^22 are integers exactly representable in binary64. Literal
    // conversion is correctly rounded; unlike powi it has no libm variability.
    const TEN: [f64; 23] = [
        1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
        1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
    ];
    const TWO53: u128 = 1 << 53;
    if v.unsigned_abs() <= TWO53 && scale <= 22 {
        // both numbers are exact doubles, so the one division is the correctly rounded quotient
        if let Ok(x) = i64::try_from(v) {
            return int_to_f64(x) / TEN[usize::from(scale)];
        }
    }
    plain_decimal(v, scale).parse().unwrap_or(f64::NAN)
}

/// An integer of at most 2^53 in magnitude as a double: exact.
#[allow(
    clippy::cast_precision_loss,
    reason = "only called for magnitudes up to 2^53, where the conversion is exact"
)]
fn int_to_f64(x: i64) -> f64 {
    x as f64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatToDecimal {
    /// NaN or infinity.
    NotFinite,
    /// The result has more than 38 digits.
    TooLarge,
}

/// The exact binary value of `x`, times `10^scale`, rounded half away from zero (so `2.675` as a double, which is a little below 2.675, gives 2.67 at
/// scale 2, and `0.125`, which is exact, gives 0.13). `NaN`, infinity and anything beyond 38 digits are errors.
pub fn f64_to_scaled(x: f64, scale: u8) -> Result<i128, FloatToDecimal> {
    if !x.is_finite() {
        return Err(FloatToDecimal::NotFinite);
    }
    let a = x.abs();
    if a == 0.0 {
        return Ok(0);
    }
    let bits = a.to_bits();
    let exp_bits = i32::try_from((bits >> 52) & 0x7ff).unwrap_or(0);
    let frac = bits & ((1u64 << 52) - 1);
    let (m, mut e) = if exp_bits == 0 {
        (frac, -1074i32)
    } else {
        (frac | (1u64 << 52), exp_bits - 1075)
    };
    let tz = m.trailing_zeros();
    e += i32::try_from(tz).unwrap_or(0);
    // a * 10^s * 2 is an odd integer exactly when e + s + 1 == 0 (m is odd): the value is exactly half way between two results
    let tie = e + i32::from(scale) + 1 == 0;
    let digits_of = |prec: usize| -> Result<u128, FloatToDecimal> {
        let text = format!("{a:.prec$}");
        let digits: String = text.chars().filter(char::is_ascii_digit).collect();
        digits.parse::<u128>().map_err(|_| FloatToDecimal::TooLarge)
    };
    let magnitude: u128 = if tie {
        // the text with one more digit is exact and ends in 5: round up by hand
        digits_of(usize::from(scale) + 1)?
            .checked_div(10)
            .and_then(|q| q.checked_add(1))
            .ok_or(FloatToDecimal::TooLarge)?
    } else {
        digits_of(usize::from(scale))?
    };
    let v = i128::try_from(magnitude).map_err(|_| FloatToDecimal::TooLarge)?;
    Ok(if x < 0.0 { -v } else { v })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseNumber {
    NotANumber,
    TooLarge,
}

/// A decimal number as text (`decimal-rules.md` section 6): spaces trimmed, an optional sign, digits with an optional point, an optional exponent;
/// rounded half away from zero at `scale`. The result may have more digits than any column holds: the caller checks `fits`.
pub fn parse_scaled(text: &str, scale: u8) -> Result<i128, ParseNumber> {
    let t = text.trim_matches(' ');
    let b = t.as_bytes();
    let mut i = 0;
    let neg = match b.first() {
        Some(b'-') => {
            i = 1;
            true
        }
        Some(b'+') => {
            i = 1;
            false
        }
        _ => false,
    };
    let mut digits = String::new();
    let mut frac_len: i64 = 0;
    let mut seen_point = false;
    let mut any_digit = false;
    while let Some(&c) = b.get(i) {
        match c {
            b'0'..=b'9' => {
                digits.push(char::from(c));
                any_digit = true;
                if seen_point {
                    frac_len += 1;
                }
            }
            b'.' if !seen_point => seen_point = true,
            _ => break,
        }
        i += 1;
    }
    if !any_digit {
        return Err(ParseNumber::NotANumber);
    }
    let mut exp: i64 = 0;
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        let exp_neg = match b.get(i) {
            Some(b'-') => {
                i += 1;
                true
            }
            Some(b'+') => {
                i += 1;
                false
            }
            _ => false,
        };
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if start == i {
            return Err(ParseNumber::NotANumber);
        }
        let n: i64 = t[start..i].parse().unwrap_or(i64::MAX / 4);
        exp = if exp_neg { -n } else { n };
    }
    if i != b.len() {
        return Err(ParseNumber::NotANumber);
    }
    let d = digits.trim_start_matches('0');
    if d.is_empty() {
        return Ok(0);
    }
    let shift = exp
        .checked_sub(frac_len)
        .and_then(|x| x.checked_add(i64::from(scale)))
        .ok_or(ParseNumber::TooLarge)?;
    let len = i64::try_from(d.len()).map_err(|_| ParseNumber::TooLarge)?;
    let magnitude: u128 = if shift >= 0 {
        if len + shift > 38 {
            return Err(ParseNumber::TooLarge);
        }
        let base: u128 = d.parse().map_err(|_| ParseNumber::TooLarge)?;
        let p = u32::try_from(shift).map_err(|_| ParseNumber::TooLarge)?;
        base.checked_mul(10u128.checked_pow(p).ok_or(ParseNumber::TooLarge)?)
            .ok_or(ParseNumber::TooLarge)?
    } else {
        let drop = -shift;
        if drop > len {
            0
        } else {
            let keep = usize::try_from(len - drop).map_err(|_| ParseNumber::TooLarge)?;
            let kept: u128 = if keep == 0 {
                0
            } else {
                d[..keep].parse().map_err(|_| ParseNumber::TooLarge)?
            };
            let first_dropped = d.as_bytes().get(keep).copied().unwrap_or(b'0');
            if first_dropped >= b'5' {
                kept.checked_add(1).ok_or(ParseNumber::TooLarge)?
            } else {
                kept
            }
        }
    };
    let v = i128::try_from(magnitude).map_err(|_| ParseNumber::TooLarge)?;
    Ok(if neg { -v } else { v })
}

/// An integer as text for `CAST('12' AS INTEGER)`: spaces trimmed, an optional sign, digits only. (DuckDB also reads `'1.5'`, `'1e2'`, `'0x10'` and
/// `'1_000'` as numbers; Nata does not: `docs/dialect.md` section 9.)
pub fn parse_integer(text: &str) -> Result<i128, ParseNumber> {
    let t = text.trim_matches(' ');
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return Err(ParseNumber::NotANumber);
    }
    let v: u128 = digits.trim_start_matches('0').parse().or_else(|_| {
        if digits.bytes().all(|c| c == b'0') {
            Ok(0)
        } else {
            Err(ParseNumber::TooLarge)
        }
    })?;
    let v = i128::try_from(v).map_err(|_| ParseNumber::TooLarge)?;
    Ok(if neg { -v } else { v })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn division_rounds_half_away_from_zero() {
        assert_eq!(div_round_away(5, 2), Some(3));
        assert_eq!(div_round_away(-5, 2), Some(-3));
        assert_eq!(div_round_away(5, -2), Some(-3));
        assert_eq!(div_round_away(-5, -2), Some(3));
        assert_eq!(div_round_away(4, 3), Some(1));
        assert_eq!(div_round_away(-4, 3), Some(-1));
        assert_eq!(div_round_away(7, 0), None);
        assert_eq!(div_round_away(i128::MIN, -1), None);
        assert_eq!(div_round_away(1, 3), Some(0));
        assert_eq!(div_round_away(2, 3), Some(1));
    }

    #[test]
    fn rescale_both_ways() {
        assert_eq!(rescale(12345, 3, 2), Some(1235));
        assert_eq!(rescale(-12345, 3, 2), Some(-1235));
        assert_eq!(rescale(12344, 3, 2), Some(1234));
        assert_eq!(rescale(5, 0, 2), Some(500));
        assert_eq!(rescale(i128::MAX, 0, 2), None);
        assert!(fits(99, 2) && !fits(100, 2) && fits(-99, 2) && !fits(-100, 2));
        assert!(fits(10i128.pow(38) - 1, 38) && !fits(10i128.pow(38), 38));
    }

    #[test]
    fn floats_become_decimals_by_their_exact_binary_value() {
        assert_eq!(f64_to_scaled(2.675, 2), Ok(267)); // the double is a little below 2.675
        assert_eq!(f64_to_scaled(0.125, 2), Ok(13)); // exact tie: away from zero
        assert_eq!(f64_to_scaled(-0.125, 2), Ok(-13));
        assert_eq!(f64_to_scaled(2.5, 0), Ok(3));
        assert_eq!(f64_to_scaled(-2.5, 0), Ok(-3));
        assert_eq!(f64_to_scaled(3.5, 0), Ok(4));
        assert_eq!(f64_to_scaled(0.5, 0), Ok(1));
        assert_eq!(f64_to_scaled(1.5, 1), Ok(15));
        assert_eq!(f64_to_scaled(0.0, 5), Ok(0));
        assert_eq!(f64_to_scaled(-0.0, 5), Ok(0));
        assert_eq!(f64_to_scaled(1e300, 2), Err(FloatToDecimal::TooLarge));
        assert_eq!(f64_to_scaled(f64::NAN, 2), Err(FloatToDecimal::NotFinite));
        assert_eq!(
            f64_to_scaled(f64::NEG_INFINITY, 2),
            Err(FloatToDecimal::NotFinite)
        );
        assert_eq!(f64_to_scaled(1e-300, 2), Ok(0));
        assert_eq!(f64_to_scaled(123456789.0, 0), Ok(123_456_789));
    }

    #[test]
    fn decimals_become_the_nearest_double() {
        assert_eq!(decimal_to_f64(15, 1), 1.5);
        assert_eq!(decimal_to_f64(-15, 1), -1.5);
        assert_eq!(decimal_to_f64(1, 1), 0.1);
        assert_eq!(decimal_to_f64(0, 3), 0.0);
        assert_eq!(
            decimal_to_f64(1_234_567_890_123_456_785, 1),
            1.2345678901234568e17
        );
        assert_eq!(
            decimal_to_f64(i128::from(i64::MAX), 0),
            9.223_372_036_854_776e18
        );
        assert_eq!(decimal_to_f64(-(1 << 53), 0), -9007199254740992.0);
    }

    #[test]
    fn text_becomes_a_decimal_rounded_half_away_from_zero() {
        assert_eq!(parse_scaled("1.005", 2), Ok(101));
        assert_eq!(parse_scaled("-1.005", 2), Ok(-101));
        assert_eq!(parse_scaled("1e2", 2), Ok(10_000));
        assert_eq!(parse_scaled(" 1 ", 2), Ok(100));
        assert_eq!(parse_scaled(".5", 2), Ok(50));
        assert_eq!(parse_scaled("5.", 2), Ok(500));
        assert_eq!(parse_scaled("+1.5", 2), Ok(150));
        assert_eq!(parse_scaled("0.995", 2), Ok(100));
        assert_eq!(parse_scaled("1e-3", 2), Ok(0));
        assert_eq!(parse_scaled("5e-3", 2), Ok(1));
        assert_eq!(parse_scaled("0.0", 0), Ok(0));
        assert_eq!(parse_scaled("0.5", 0), Ok(1));
        assert_eq!(parse_scaled("0.04", 1), Ok(0));
        assert_eq!(parse_scaled("0.05", 1), Ok(1));
        assert_eq!(parse_scaled("", 2), Err(ParseNumber::NotANumber));
        assert_eq!(parse_scaled(".", 2), Err(ParseNumber::NotANumber));
        assert_eq!(parse_scaled("1e", 2), Err(ParseNumber::NotANumber));
        assert_eq!(parse_scaled("abc", 2), Err(ParseNumber::NotANumber));
        assert_eq!(parse_scaled("1_0", 2), Err(ParseNumber::NotANumber));
        assert_eq!(parse_scaled("1.2.3", 2), Err(ParseNumber::NotANumber));
        assert_eq!(
            parse_scaled("1e99999999999999999999", 2),
            Err(ParseNumber::TooLarge)
        );
        assert_eq!(parse_scaled(&"9".repeat(40), 0), Err(ParseNumber::TooLarge));
        assert_eq!(parse_scaled(&"9".repeat(38), 0), Ok(10i128.pow(38) - 1));
        assert_eq!(parse_scaled("0e99999", 2), Ok(0));
    }

    #[test]
    fn integers_as_text() {
        assert_eq!(parse_integer(" 12 "), Ok(12));
        assert_eq!(parse_integer("+12"), Ok(12));
        assert_eq!(parse_integer("-0"), Ok(0));
        assert_eq!(parse_integer("00012"), Ok(12));
        assert_eq!(parse_integer("1.5"), Err(ParseNumber::NotANumber));
        assert_eq!(parse_integer("1e2"), Err(ParseNumber::NotANumber));
        assert_eq!(parse_integer(""), Err(ParseNumber::NotANumber));
        assert_eq!(parse_integer("-"), Err(ParseNumber::NotANumber));
        assert_eq!(parse_integer(&"9".repeat(60)), Err(ParseNumber::TooLarge));
        assert_eq!(parse_integer("000"), Ok(0));
    }
}
