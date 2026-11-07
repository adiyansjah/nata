//! PostgreSQL extra_float_digits >= 1 layout, independent of CLI CSV formatting.
pub(crate) fn text(s: &str, single: bool) -> String {
    let x = s.parse::<f64>().expect("server float text");
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x < 0.0 { "-Infinity" } else { "Infinity" }.into();
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0" } else { "0" }.into();
    }
    let shortest = if single {
        format!("{:e}", (x as f32).abs())
    } else {
        format!("{:e}", x.abs())
    };
    // PostgreSQL disables Ryu's acceptBounds: a shortest decimal on a binary
    // midpoint is excluded even when ties-to-even would roundtrip. Rust also
    // chooses a different last digit on some decimal ties. Round explicitly at
    // the shortest precision, ties-to-even, then grow only for an open endpoint.
    let n = shortest
        .split_once('e')
        .expect("scientific")
        .0
        .bytes()
        .filter(|b| b.is_ascii_digit())
        .count();
    let (m, e2, mm_shift) = if single {
        let bits = (x as f32).abs().to_bits();
        let exponent = (bits >> 23) as i32;
        let mantissa = u64::from(bits & 0x7fffff);
        (
            if exponent == 0 {
                mantissa
            } else {
                mantissa | (1 << 23)
            },
            exponent.max(1) - 127 - 23 - 2,
            mantissa != 0 || exponent <= 1,
        )
    } else {
        let bits = x.abs().to_bits();
        let exponent = (bits >> 52) as i32;
        let mantissa = bits & 0xfffffffffffff;
        (
            if exponent == 0 {
                mantissa
            } else {
                mantissa | (1 << 52)
            },
            exponent.max(1) - 1023 - 52 - 2,
            mantissa != 0 || exponent <= 1,
        )
    };
    let mut shortest = shortest;
    for precision in n..=if single { 9 } else { 17 } {
        shortest = if single {
            format!("{:.*e}", precision - 1, (x as f32).abs())
        } else {
            format!("{:.*e}", precision - 1, x.abs())
        };
        let (mantissa, exp) = shortest.split_once('e').expect("scientific");
        let digits = mantissa.replace('.', "");
        let q = exp.parse::<i32>().expect("exponent") - digits.len() as i32 + 1;
        let d = digits.parse::<u64>().expect("digits");
        let lower = 4 * m - 1 - u64::from(mm_shift);
        let upper = 4 * m + 2;
        if !midpoint(d, q, lower, e2) && !midpoint(d, q, upper, e2) {
            break;
        }
    }
    let (mantissa, exp) = shortest.split_once('e').expect("scientific float");
    let digits = mantissa.replace('.', "");
    let e: i32 = exp.parse().expect("exponent");
    let precision = if single { 6 } else { 15 };
    let mut out = if x < 0.0 {
        "-".to_owned()
    } else {
        String::new()
    };
    if e < -4 || e >= precision {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push_str(&format!(
            "e{}{:02}",
            if e < 0 { '-' } else { '+' },
            e.unsigned_abs()
        ));
    } else if e >= 0 {
        let n = e as usize + 1;
        if digits.len() <= n {
            out.push_str(&digits);
            out.push_str(&"0".repeat(n - digits.len()));
        } else {
            out.push_str(&digits[..n]);
            out.push('.');
            out.push_str(&digits[n..]);
        }
    } else {
        out.push_str("0.");
        out.push_str(&"0".repeat((-e - 1) as usize));
        out.push_str(&digits);
    }
    out
}

// Exact equality of decimal d*10^q and binary m*2^e, without float rounding.
// Cancel factors of five first; intermediates are bounded by the 55-bit m.
fn midpoint(mut d: u64, q: i32, mut m: u64, mut e: i32) -> bool {
    let mz = m.trailing_zeros();
    m >>= mz;
    e += mz as i32;
    if q < 0 {
        for _ in q..0 {
            if !d.is_multiple_of(5) {
                return false;
            }
            d /= 5;
        }
    }
    let dz = d.trailing_zeros();
    d >>= dz;
    if q >= 0 {
        for _ in 0..q {
            let Some(next) = d.checked_mul(5) else {
                return false;
            };
            d = next;
        }
    }
    d == m && q + dz as i32 == e
}
