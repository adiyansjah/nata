//! CSV output in the exact text DuckDB's command line prints with `-csv`, which is what `nata-check` compares:
//! a header row of the column names, `NULL` for a NULL, a field in double quotes (a quote doubled) if it holds a byte of 1 to 31, a double quote,
//! a single quote, a comma, byte 127 or any non-ASCII byte, rows ended by `\n`. Every rule was checked against DuckDB 2.0.0 (the tests of this crate
//! compare whole files, and one test compares all 128 ASCII characters and 1,000 random floats of each width).
//!
//! Values: integers in decimal; `true` / `false`; floats as the shortest text that reads back to the same number (`1.0`, `0.1`, `1e+20`, `1e-05`,
//! positional between `1e-04` and `1e16`), `inf`, `-inf`, `nan` and `-nan` (sign bit set); decimals with exactly their scale; dates `YYYY-MM-DD`; timestamps without trailing
//! zeros of the fraction; text as is; binary with `\xHH` for every byte outside printable ASCII and for `"`, `'` and `\`.
//!
//! KNOWN DIFFERENCE in float text (cosmetic; the VALUE is the same): where several shortest digit strings exist or the shortest one sits exactly on the
//! edge of the rounding interval, DuckDB's algorithm prints other digits than Rust's (a tie between two shortest strings goes to the even digit in DuckDB;
//! many `FLOAT` values of magnitude above about 1e6 print with MORE digits in DuckDB, e.g. 160227392.0 where Rust's shortest is 160227400.0). Both texts
//! read back to the same number. Measured on 120,000 random values: see `bench/results/20261105-scan-reader.md`. `nata-check` queries over float
//! columns therefore need `-- tolerance:`.

use std::io::Write;

use arrow_array::cast::AsArray;
use arrow_array::types::{
    Date32Type, Decimal64Type, Decimal128Type, Float32Type, Float64Type, Int8Type, Int16Type,
    Int32Type, Int64Type, TimestampMicrosecondType, TimestampMillisecondType, UInt8Type,
    UInt16Type, UInt32Type, UInt64Type,
};
use arrow_array::{Array, RecordBatch};
use arrow_schema::{DataType, TimeUnit};

use crate::civil;
use crate::error::{ErrorKind, ScanError};

fn needs_quotes(s: &str) -> bool {
    s.bytes()
        .any(|b| b <= 31 || b == b'"' || b == b'\'' || b == b',' || b >= 127)
}

/// One CSV field.
pub fn push_field(out: &mut Vec<u8>, s: &str) {
    if needs_quotes(s) {
        out.push(b'"');
        for b in s.bytes() {
            if b == b'"' {
                out.push(b'"');
            }
            out.push(b);
        }
        out.push(b'"');
    } else {
        out.extend_from_slice(s.as_bytes());
    }
}

/// The header row.
pub fn write_header(out: &mut impl Write, names: &[String]) -> std::io::Result<()> {
    let mut line = Vec::new();
    for (i, n) in names.iter().enumerate() {
        if i > 0 {
            line.push(b',');
        }
        push_field(&mut line, n);
    }
    line.push(b'\n');
    out.write_all(&line)
}

/// Shortest-round-trip digits and the decimal exponent of a positive finite number: `(digits, e)` with value = d.ddd * 10^e.
fn shortest(text: &str) -> (String, i32) {
    let (mantissa, exp) = text.split_once('e').unwrap_or((text, "0"));
    (mantissa.replace('.', ""), exp.parse().unwrap_or(0))
}

fn layout(negative: bool, digits: &str, e: i32) -> String {
    let mut s = String::new();
    if negative {
        s.push('-');
    }
    if !(-4..16).contains(&e) {
        s.push_str(&digits[..1]);
        if digits.len() > 1 {
            s.push('.');
            s.push_str(&digits[1..]);
        }
        s.push('e');
        s.push(if e < 0 { '-' } else { '+' });
        s.push_str(&format!("{:02}", e.unsigned_abs()));
    } else if e >= 0 {
        let int_len = usize::try_from(e).unwrap_or(0) + 1;
        if digits.len() <= int_len {
            s.push_str(digits);
            s.push_str(&"0".repeat(int_len - digits.len()));
            s.push_str(".0");
        } else {
            s.push_str(&digits[..int_len]);
            s.push('.');
            s.push_str(&digits[int_len..]);
        }
    } else {
        s.push_str("0.");
        s.push_str(&"0".repeat(usize::try_from(-e - 1).unwrap_or(0)));
        s.push_str(digits);
    }
    s
}

pub fn format_f64(x: f64) -> String {
    if x.is_nan() {
        return if x.is_sign_negative() {
            "-nan".into()
        } else {
            "nan".into()
        };
    }
    if x.is_infinite() {
        return if x < 0.0 { "-inf".into() } else { "inf".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() {
            "-0.0".into()
        } else {
            "0.0".into()
        };
    }
    let (digits, e) = shortest(&format!("{:e}", x.abs()));
    layout(x < 0.0, &digits, e)
}

pub fn format_f32(x: f32) -> String {
    if x.is_nan() {
        return if x.is_sign_negative() {
            "-nan".into()
        } else {
            "nan".into()
        };
    }
    if x.is_infinite() {
        return if x < 0.0 { "-inf".into() } else { "inf".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() {
            "-0.0".into()
        } else {
            "0.0".into()
        };
    }
    let (digits, e) = shortest(&format!("{:e}", x.abs()));
    layout(x < 0.0, &digits, e)
}

pub fn format_decimal(unscaled: i128, scale: u8) -> String {
    let mut digits = unscaled.unsigned_abs().to_string();
    let scale = usize::from(scale);
    if scale > 0 {
        while digits.len() <= scale {
            digits.insert(0, '0');
        }
        digits.insert(digits.len() - scale, '.');
    }
    if unscaled < 0 {
        format!("-{digits}")
    } else {
        digits
    }
}

/// The text of a `DECIMAL(precision, scale)` value as DuckDB prints it: `format_decimal`, and no leading zero when every digit is a fraction digit
/// (`DECIMAL(3,3)` prints `.500` and `-.500`, `DECIMAL(4,3)` prints `0.500`; DuckDB 2.0.0).
pub fn format_decimal_typed(unscaled: i128, precision: u8, scale: u8) -> String {
    let text = format_decimal(unscaled, scale);
    if scale > 0 && precision == scale {
        if let Some(rest) = text.strip_prefix("0.") {
            return format!(".{rest}");
        }
        if let Some(rest) = text.strip_prefix("-0.") {
            return format!("-.{rest}");
        }
    }
    text
}

pub fn format_blob(bytes: &[u8]) -> String {
    let mut s = String::new();
    for &b in bytes {
        if (32..=126).contains(&b) && b != b'"' && b != b'\'' && b != b'\\' {
            s.push(char::from(b));
        } else {
            s.push_str(&format!("\\x{b:02X}"));
        }
    }
    s
}

fn unsupported(t: &DataType) -> ScanError {
    ScanError::not_supported(format!("CSV output of a column of Arrow type {t}"))
}

/// The text of row `i` of a column (`None` for NULL), the text the CSV writer prints before quoting.
pub fn cell_text(col: &dyn Array, i: usize) -> Result<Option<String>, ScanError> {
    cell(col, i)
}

/// The text of row `i` of a column (`None` for NULL).
fn cell(col: &dyn Array, i: usize) -> Result<Option<String>, ScanError> {
    if col.is_null(i) {
        return Ok(None);
    }
    let bad = || {
        ScanError::new(ErrorKind::Internal(
            "an array does not match its type".into(),
        ))
    };
    let text = match col.data_type() {
        DataType::Boolean => col.as_boolean_opt().ok_or_else(bad)?.value(i).to_string(),
        DataType::Int8 => col
            .as_primitive_opt::<Int8Type>()
            .ok_or_else(bad)?
            .value(i)
            .to_string(),
        DataType::Int16 => col
            .as_primitive_opt::<Int16Type>()
            .ok_or_else(bad)?
            .value(i)
            .to_string(),
        DataType::Int32 => col
            .as_primitive_opt::<Int32Type>()
            .ok_or_else(bad)?
            .value(i)
            .to_string(),
        DataType::Int64 => col
            .as_primitive_opt::<Int64Type>()
            .ok_or_else(bad)?
            .value(i)
            .to_string(),
        DataType::UInt8 => col
            .as_primitive_opt::<UInt8Type>()
            .ok_or_else(bad)?
            .value(i)
            .to_string(),
        DataType::UInt16 => col
            .as_primitive_opt::<UInt16Type>()
            .ok_or_else(bad)?
            .value(i)
            .to_string(),
        DataType::UInt32 => col
            .as_primitive_opt::<UInt32Type>()
            .ok_or_else(bad)?
            .value(i)
            .to_string(),
        DataType::UInt64 => col
            .as_primitive_opt::<UInt64Type>()
            .ok_or_else(bad)?
            .value(i)
            .to_string(),
        DataType::Float32 => format_f32(
            col.as_primitive_opt::<Float32Type>()
                .ok_or_else(bad)?
                .value(i),
        ),
        DataType::Float64 => format_f64(
            col.as_primitive_opt::<Float64Type>()
                .ok_or_else(bad)?
                .value(i),
        ),
        DataType::Decimal64(p, s) => {
            let s = u8::try_from(*s).map_err(|_| bad())?;
            format_decimal_typed(
                i128::from(
                    col.as_primitive_opt::<Decimal64Type>()
                        .ok_or_else(bad)?
                        .value(i),
                ),
                *p,
                s,
            )
        }
        DataType::Decimal128(p, s) => {
            let s = u8::try_from(*s).map_err(|_| bad())?;
            format_decimal_typed(
                col.as_primitive_opt::<Decimal128Type>()
                    .ok_or_else(bad)?
                    .value(i),
                *p,
                s,
            )
        }
        DataType::Date32 => civil::format_date(
            col.as_primitive_opt::<Date32Type>()
                .ok_or_else(bad)?
                .value(i),
        ),
        DataType::Timestamp(TimeUnit::Microsecond, None) => civil::format_timestamp_micros(
            col.as_primitive_opt::<TimestampMicrosecondType>()
                .ok_or_else(bad)?
                .value(i),
        ),
        DataType::Timestamp(TimeUnit::Millisecond, None) => {
            let ms = col
                .as_primitive_opt::<TimestampMillisecondType>()
                .ok_or_else(bad)?
                .value(i);
            match ms.checked_mul(1000) {
                Some(us) => civil::format_timestamp_micros(us),
                None => {
                    return Err(ScanError::not_supported(
                        "CSV output of a TIMESTAMP_MS value beyond the microsecond range",
                    ));
                }
            }
        }
        DataType::Utf8 => col
            .as_string_opt::<i32>()
            .ok_or_else(bad)?
            .value(i)
            .to_string(),
        DataType::Binary => format_blob(col.as_binary_opt::<i32>().ok_or_else(bad)?.value(i)),
        DataType::Dictionary(k, v) if **k == DataType::Int32 && **v == DataType::Utf8 => {
            let d = col.as_dictionary_opt::<Int32Type>().ok_or_else(bad)?;
            let values = d.values().as_string_opt::<i32>().ok_or_else(bad)?;
            let key = usize::try_from(d.keys().value(i)).map_err(|_| bad())?;
            if key >= values.len() || values.is_null(key) {
                return Err(bad());
            }
            values.value(key).to_string()
        }
        other => return Err(unsupported(other)),
    };
    Ok(Some(text))
}

/// The rows of a batch.
pub fn write_batch(out: &mut impl Write, batch: &RecordBatch) -> Result<(), ScanError> {
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    for row in 0..batch.num_rows() {
        for (c, col) in batch.columns().iter().enumerate() {
            if c > 0 {
                buf.push(b',');
            }
            match cell(col.as_ref(), row)? {
                None => buf.extend_from_slice(b"NULL"),
                Some(text) => push_field(&mut buf, &text),
            }
        }
        buf.push(b'\n');
        if buf.len() >= 60 * 1024 {
            out.write_all(&buf)
                .map_err(|e| ScanError::new(ErrorKind::Io(crate::error::redact(&e.to_string()))))?;
            buf.clear();
        }
    }
    out.write_all(&buf)
        .map_err(|e| ScanError::new(ErrorKind::Io(crate::error::redact(&e.to_string()))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_have_duckdbs_text() {
        // each of these was printed by DuckDB 2.0.0 for the DOUBLE of the same number
        for (x, want) in [
            (1.0, "1.0"),
            (1e20, "1e+20"),
            (1e-5, "1e-05"),
            (0.0001, "0.0001"),
            (-0.0, "-0.0"),
            (123456789.123, "123456789.123"),
            (1e15, "1000000000000000.0"),
            (1e16, "1e+16"),
            (1.5e300, "1.5e+300"),
            (5e-324, "5e-324"),
            (1.2345678901234568e17, "1.2345678901234568e+17"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (f64::NAN, "nan"),
            (-f64::NAN, "-nan"),
            (0.1, "0.1"),
            (-2.5, "-2.5"),
            (100.0, "100.0"),
        ] {
            assert_eq!(format_f64(x), want, "{x:e}");
        }
        for (x, want) in [
            (0.1f32, "0.1"),
            (16_777_216.0, "16777216.0"),
            (1e10, "10000000000.0"),
            (3.4028235e38, "3.4028235e+38"),
            (1e-7, "1e-07"),
        ] {
            assert_eq!(format_f32(x), want, "{x:e}");
        }
    }

    #[test]
    fn decimals_dates_and_blobs() {
        assert_eq!(format_decimal(1250, 2), "12.50");
        assert_eq!(format_decimal(-5, 2), "-0.05");
        assert_eq!(format_decimal(123, 0), "123");
        assert_eq!(format_decimal(0, 3), "0.000");
        // DuckDB prints a decimal whose precision equals its scale without the leading zero
        assert_eq!(format_decimal_typed(500, 3, 3), ".500");
        assert_eq!(format_decimal_typed(-500, 3, 3), "-.500");
        assert_eq!(format_decimal_typed(0, 3, 3), ".000");
        assert_eq!(format_decimal_typed(500, 4, 3), "0.500");
        assert_eq!(format_decimal_typed(1500, 4, 3), "1.500");
        assert_eq!(format_decimal_typed(7, 2, 0), "7");
        assert_eq!(
            format_blob(&[0, 0x41, 0xFF, 0x27, 0x5C, 0x22]),
            "\\x00A\\xFF\\x27\\x5C\\x22"
        );
    }

    #[test]
    fn fields_are_quoted_like_duckdb() {
        let q = |s: &str| {
            let mut v = Vec::new();
            push_field(&mut v, s);
            String::from_utf8(v).expect("utf8")
        };
        assert_eq!(q("plain text; with # and \\"), "plain text; with # and \\");
        assert_eq!(q(" a "), " a ");
        assert_eq!(q(""), "");
        assert_eq!(q("a,b"), "\"a,b\"");
        assert_eq!(q("x\"y"), "\"x\"\"y\"");
        assert_eq!(q("it's"), "\"it's\"");
        assert_eq!(q("é"), "\"é\"");
        assert_eq!(q("a\nb"), "\"a\nb\"");
        assert_eq!(q("a\tb"), "\"a\tb\"");
        assert_eq!(q("a\u{7f}b"), "\"a\u{7f}b\"");
    }
}
