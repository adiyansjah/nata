//! Filters: the text form `col op literal`, its typing against a column, and the EXACT ROW FILTER that runs on the batches of the kept row groups.
//!
//! # Syntax
//!
//! `column <op> literal`, `column IS NULL`, `column IS NOT NULL`. Operators: `<`, `<=`, `>`, `>=`, `=`, `<>` (and `!=`). The column is a bare name
//! (no whitespace, none of `<>=!`) or a double-quoted name (`"a b"`, `""` for a quote); names are matched exactly, case included. The literal is typed
//! FROM THE COLUMN and nothing is guessed:
//!
//! | Column | Literal | Notes |
//! |---|---|---|
//! | integer types | `-12`, `+7` | no fraction (`5.5` is an error, never rounded); up to 38 digits |
//! | FLOAT, DOUBLE | `1.5`, `-2e10`, `nan`, `inf`, `-inf` | cast to the column's width (a FLOAT column compares with the `f32` nearest to the literal, as DuckDB casts an untyped literal); text that overflows is an error |
//! | DECIMAL(p,s) | `12.50`, `-0.05`, `7` | EXACT: the literal keeps its own scale and is never rounded to the column's; no exponent; at most 38 digits |
//! | DATE | `1994-01-01` or `'1994-01-01'` | years 0001 to 9999 |
//! | TIMESTAMP | `1994-01-01 10:30:00.5` (quotes optional, `T` allowed) | at most 6 fraction digits |
//! | VARCHAR | `'text'` | single quotes required, `''` for a quote |
//! | BLOB | `X'00FF'` | hex pairs |
//! | BOOLEAN | `true`, `false` | |
//!
//! # Row semantics (DuckDB's, not Arrow's)
//!
//! A comparison with a NULL value is never true (also `<>`). `IS NULL` / `IS NOT NULL` look at validity only. Floats: `NaN = NaN`, NaN is greater than every
//! number (infinity included), `-0.0 = +0.0`. A FLOAT column is compared as the `f64` it widens to, against a literal that was already cast to FLOAT.
//! Decimals are compared EXACTLY across scales (if rescaling one side overflows 128 bits the other side is smaller in magnitude, so the sign decides).
//! Integers compare as 128-bit numbers (no cast of the literal to the column's width). Text and binary compare bytewise as unsigned bytes. Dictionary
//! columns (`Dictionary(Int32, Utf8)`) are compared WITHOUT expanding: the predicate is evaluated once per dictionary entry and looked up per row.
//! Timestamps in milliseconds are compared in microseconds, exactly.
//!
//! The rows of a batch that satisfy EVERY term are kept (the terms are an AND).

use std::cmp::Ordering;

use arrow_array::cast::AsArray;
use arrow_array::types::{
    Date32Type, Decimal64Type, Decimal128Type, Float32Type, Float64Type, Int8Type, Int16Type,
    Int32Type, Int64Type, TimestampMicrosecondType, TimestampMillisecondType, UInt8Type,
    UInt16Type, UInt32Type, UInt64Type,
};
use arrow_array::{Array, ArrowPrimitiveType, RecordBatch};
use arrow_buffer::NullBuffer;
use arrow_schema::{DataType, TimeUnit};
use nata_scan::{CmpOp, Term as ScanTerm, Value};

use crate::civil;
use crate::schema::Kind;

/// A literal, typed from its column.
#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    Bool(bool),
    Int(i128),
    F32(f32),
    F64(f64),
    Decimal {
        unscaled: i128,
        precision: u8,
        scale: u8,
    },
    Date(i32),
    TimestampMicros(i64),
    Text(String),
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Test {
    Cmp(CmpOp, Literal),
    IsNull,
    IsNotNull,
}

/// One filter term: a column name and a test.
#[derive(Clone, Debug, PartialEq)]
pub struct Term {
    pub column: String,
    pub test: Test,
}

/// The syntax of a filter before its literal is typed.
#[derive(Clone, Debug, PartialEq)]
pub enum RawTest {
    Cmp(CmpOp, String),
    IsNull,
    IsNotNull,
}

/// Split `col op literal` / `col IS [NOT] NULL` into column, operator and literal text. Errors are plain sentences (the caller adds the filter text).
pub fn parse_syntax(text: &str) -> Result<(String, RawTest), String> {
    let t = text.trim_start();
    let (column, rest) = if let Some(quoted) = t.strip_prefix('"') {
        let mut name = String::new();
        let mut chars = quoted.char_indices().peekable();
        let mut end = None;
        while let Some((i, c)) = chars.next() {
            if c == '"' {
                if matches!(chars.peek(), Some((_, '"'))) {
                    name.push('"');
                    chars.next();
                } else {
                    end = Some(i + 1);
                    break;
                }
            } else {
                name.push(c);
            }
        }
        let end = end.ok_or("the quoted column name is not closed")?;
        (name, &quoted[end..])
    } else {
        let end = t
            .find(|c: char| c.is_whitespace() || "<>=!".contains(c))
            .unwrap_or(t.len());
        (t[..end].to_string(), &t[end..])
    };
    if column.is_empty() {
        return Err("expected a column name first".into());
    }
    let rest = rest.trim_start();
    for (symbol, op) in [
        ("<=", CmpOp::Le),
        (">=", CmpOp::Ge),
        ("<>", CmpOp::Ne),
        ("!=", CmpOp::Ne),
        ("<", CmpOp::Lt),
        (">", CmpOp::Gt),
        ("=", CmpOp::Eq),
    ] {
        if let Some(lit) = rest.strip_prefix(symbol) {
            let lit = lit.trim();
            if lit.is_empty() {
                return Err(format!("expected a literal after `{symbol}`"));
            }
            return Ok((column, RawTest::Cmp(op, lit.to_string())));
        }
    }
    let words: Vec<String> = rest
        .split_whitespace()
        .map(str::to_ascii_uppercase)
        .collect();
    match words
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["IS", "NULL"] => Ok((column, RawTest::IsNull)),
        ["IS", "NOT", "NULL"] => Ok((column, RawTest::IsNotNull)),
        _ => Err("expected one of < <= > >= = <> IS NULL IS NOT NULL after the column name".into()),
    }
}

/// Type a literal text from the column's kind. Never guesses: anything that is not exactly the documented form is an error.
pub fn type_literal(kind: Kind, text: &str) -> Result<Literal, String> {
    let name = kind.sql_name();
    match kind {
        Kind::Bool => match text.to_ascii_lowercase().as_str() {
            "true" => Ok(Literal::Bool(true)),
            "false" => Ok(Literal::Bool(false)),
            _ => Err("write true or false".to_string()),
        },
        Kind::Int { .. } => {
            let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
            if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
                if digits.bytes().all(|c| c.is_ascii_digit() || c == b'.') && digits.contains('.') {
                    return Err(
                        "the literal has a fraction: it is never rounded, write an integer"
                            .to_string(),
                    );
                }
                return Err("write an integer such as -12".to_string());
            }
            text.strip_prefix('+')
                .unwrap_or(text)
                .parse::<i128>()
                .map(Literal::Int)
                .map_err(|_| "the integer is too large (at most 38 digits)".to_string())
        }
        Kind::Float32 | Kind::Float64 => float_literal(kind == Kind::Float32, text, &name),
        Kind::Decimal { .. } => decimal_literal(text),
        Kind::Date => parse_unquoted(text).and_then(|t| civil::parse_date(t).map(Literal::Date)),
        Kind::Timestamp(_) => parse_unquoted(text)
            .and_then(|t| civil::parse_timestamp_micros(t).map(Literal::TimestampMicros)),
        Kind::Utf8 => {
            let inner = text
                .strip_prefix('\'')
                .and_then(|t| t.strip_suffix('\''))
                .filter(|_| text.len() >= 2)
                .ok_or_else(|| "write the text in single quotes, 'like this'".to_string())?;
            // inside the quotes only doubled quotes are allowed
            let mut out = String::new();
            let mut chars = inner.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\'' && chars.next() != Some('\'') {
                    return Err("a single quote inside the text must be doubled ('')".into());
                }
                out.push(c);
            }
            Ok(Literal::Text(out))
        }
        Kind::Binary => {
            let hex = text
                .strip_prefix(['x', 'X'])
                .and_then(|t| t.strip_prefix('\''))
                .and_then(|t| t.strip_suffix('\''))
                .ok_or_else(|| "write the bytes as X'00FF'".to_string())?;
            if hex.len() % 2 != 0 || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
                return Err("the bytes must be an even number of hex digits".into());
            }
            let bytes = (0..hex.len())
                .step_by(2)
                .map(|i| {
                    u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| "bad hex digit".to_string())
                })
                .collect::<Result<Vec<u8>, String>>()?;
            Ok(Literal::Bytes(bytes))
        }
    }
}

/// A literal that may be wrapped in single quotes.
fn parse_unquoted(text: &str) -> Result<&str, String> {
    match text.strip_prefix('\'') {
        Some(rest) => rest
            .strip_suffix('\'')
            .ok_or_else(|| "the closing quote is missing".to_string()),
        None => Ok(text),
    }
}

fn float_literal(single: bool, text: &str, name: &str) -> Result<Literal, String> {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let lower = unsigned.to_ascii_lowercase();
    let special = matches!(lower.as_str(), "nan" | "inf" | "infinity");
    if !special {
        let b = unsigned.as_bytes();
        let ok = !b.is_empty()
            && b.iter()
                .all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-'))
            && b.iter().any(u8::is_ascii_digit);
        if !ok {
            return Err("write a number such as 1.5, 1e10, nan, inf or -inf".to_string());
        }
    }
    if single {
        let v: f32 = text.parse().map_err(|_| "not a number".to_string())?;
        if v.is_infinite() && !special {
            return Err(format!("the number is out of range for {name}"));
        }
        Ok(Literal::F32(v))
    } else {
        let v: f64 = text.parse().map_err(|_| "not a number".to_string())?;
        if v.is_infinite() && !special {
            return Err(format!("the number is out of range for {name}"));
        }
        Ok(Literal::F64(v))
    }
}

fn decimal_literal(text: &str) -> Result<Literal, String> {
    let (negative, body) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let (int_part, frac_part) = body.split_once('.').unwrap_or((body, ""));
    let well_formed = !(int_part.is_empty() && frac_part.is_empty())
        && int_part.bytes().all(|c| c.is_ascii_digit())
        && frac_part.bytes().all(|c| c.is_ascii_digit())
        && (body.contains('.') || !int_part.is_empty());
    if !well_formed {
        return Err(
            "write the number with digits and an optional point, such as 12.50 (no exponent)"
                .to_string(),
        );
    }
    let digits = format!("{int_part}{frac_part}");
    let significant = digits.trim_start_matches('0').len();
    let scale = frac_part.len();
    let precision = significant.max(scale).max(1);
    if digits.len() > 60 || precision > 38 {
        return Err("a decimal literal has at most 38 digits".into());
    }
    let magnitude: i128 = digits
        .parse()
        .map_err(|_| "a decimal literal has at most 38 digits".to_string())?;
    let (Ok(scale), Ok(precision)) = (u8::try_from(scale), u8::try_from(precision)) else {
        return Err("a decimal literal has at most 38 digits".into());
    };
    Ok(Literal::Decimal {
        unscaled: if negative { -magnitude } else { magnitude },
        precision,
        scale,
    })
}

impl Term {
    /// The pruning term of `nata-scan` for this filter, or `None` if its literal has no representation there (an integer beyond 64 bits): the row
    /// filter still applies it, the row group is just not pruned by it.
    pub fn scan_term(&self, kind: Kind) -> Option<ScanTerm> {
        let (op, lit) = match &self.test {
            Test::IsNull => return Some(ScanTerm::is_null(&self.column)),
            Test::IsNotNull => return Some(ScanTerm::is_not_null(&self.column)),
            Test::Cmp(op, lit) => (*op, lit),
        };
        let value = match (lit, kind) {
            (Literal::Bool(b), _) => Value::Bool(*b),
            (Literal::Int(i), _) => Value::Int64(i64::try_from(*i).ok()?),
            (Literal::F32(f), Kind::Float32) => Value::Float32(*f),
            (Literal::F64(f), Kind::Float64) => Value::Float64(*f),
            (Literal::F32(_) | Literal::F64(_), _) => return None,
            (
                Literal::Decimal {
                    unscaled,
                    precision,
                    scale,
                },
                _,
            ) => Value::Decimal {
                unscaled: *unscaled,
                precision: *precision,
                scale: *scale,
            },
            (Literal::Date(d), _) => Value::Date32(*d),
            (Literal::TimestampMicros(t), _) => Value::TimestampMicros(*t),
            (Literal::Text(s), _) => Value::Utf8(s.clone()),
            (Literal::Bytes(b), _) => Value::Binary(b.clone()),
        };
        Some(ScanTerm::cmp(&self.column, op, value))
    }
}

// ------------------------------------------------------------------------------------------------------------------------------ row semantics

/// DuckDB's order on doubles: NaN equals NaN and is above everything (infinity included); `-0.0` equals `+0.0`.
pub fn cmp_float(a: f64, b: f64) -> Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
    }
}

/// Exact order of two decimals of different scales: `a / 10^sa` against `b / 10^sb`, never rounding either side.
pub fn cmp_decimal(a: i128, sa: u8, b: i128, sb: u8) -> Ordering {
    fn rescale(x: i128, by: u8) -> Option<i128> {
        10i128
            .checked_pow(u32::from(by))
            .and_then(|p| x.checked_mul(p))
    }
    match sa.cmp(&sb) {
        Ordering::Equal => a.cmp(&b),
        // `a` has fewer fraction digits: bring it up to `sb`; if that does not fit, |a| is above anything `b` can be
        Ordering::Less => match rescale(a, sb - sa) {
            Some(a2) => a2.cmp(&b),
            None => sign_order(a),
        },
        Ordering::Greater => match rescale(b, sa - sb) {
            Some(b2) => a.cmp(&b2),
            None => sign_order(b).reverse(),
        },
    }
}

/// The order of an overflowed value against any value that fits: a positive one is greater, a negative one smaller.
fn sign_order(overflowed: i128) -> Ordering {
    if overflowed > 0 {
        Ordering::Greater
    } else {
        Ordering::Less
    }
}

fn holds(op: CmpOp, ord: Ordering) -> bool {
    match op {
        CmpOp::Lt => ord == Ordering::Less,
        CmpOp::Le => ord != Ordering::Greater,
        CmpOp::Gt => ord == Ordering::Greater,
        CmpOp::Ge => ord != Ordering::Less,
        CmpOp::Eq => ord == Ordering::Equal,
        CmpOp::Ne => ord != Ordering::Equal,
    }
}

/// `mask[i] &= (row i is valid) && op(ord(i))`, for the rows that are still set.
fn apply(
    nulls: Option<&NullBuffer>,
    mask: &mut [bool],
    op: CmpOp,
    ord: impl Fn(usize) -> Ordering,
) {
    for (i, m) in mask.iter_mut().enumerate() {
        if *m {
            *m = nulls.is_none_or(|n| n.is_valid(i)) && holds(op, ord(i));
        }
    }
}

fn prim<T: ArrowPrimitiveType>(
    col: &dyn Array,
    op: CmpOp,
    mask: &mut [bool],
    ord: impl Fn(T::Native) -> Ordering,
) -> Result<(), String> {
    let a = col
        .as_primitive_opt::<T>()
        .ok_or("the array is not of the type its schema says")?;
    let values = a.values();
    apply(a.nulls(), mask, op, |i| {
        values.get(i).map_or(Ordering::Equal, |v| ord(*v))
    });
    Ok(())
}

fn mismatch(col: &dyn Array, lit: &Literal) -> String {
    format!(
        "a literal of kind {} cannot be compared with a column of Arrow type {}",
        literal_name(lit),
        col.data_type()
    )
}

fn literal_name(l: &Literal) -> &'static str {
    match l {
        Literal::Bool(_) => "boolean",
        Literal::Int(_) => "integer",
        Literal::F32(_) => "float",
        Literal::F64(_) => "double",
        Literal::Decimal { .. } => "decimal",
        Literal::Date(_) => "date",
        Literal::TimestampMicros(_) => "timestamp",
        Literal::Text(_) => "text",
        Literal::Bytes(_) => "bytes",
    }
}

/// Evaluate one comparison on a column into `mask` (rows already false stay false).
fn eval_cmp(col: &dyn Array, op: CmpOp, lit: &Literal, mask: &mut [bool]) -> Result<(), String> {
    match (col.data_type(), lit) {
        (DataType::Boolean, Literal::Bool(l)) => {
            let a = col.as_boolean_opt().ok_or("not a boolean array")?;
            apply(a.nulls(), mask, op, |i| a.value(i).cmp(l));
            Ok(())
        }
        (DataType::Int8, Literal::Int(l)) => {
            prim::<Int8Type>(col, op, mask, |v| i128::from(v).cmp(l))
        }
        (DataType::Int16, Literal::Int(l)) => {
            prim::<Int16Type>(col, op, mask, |v| i128::from(v).cmp(l))
        }
        (DataType::Int32, Literal::Int(l)) => {
            prim::<Int32Type>(col, op, mask, |v| i128::from(v).cmp(l))
        }
        (DataType::Int64, Literal::Int(l)) => {
            prim::<Int64Type>(col, op, mask, |v| i128::from(v).cmp(l))
        }
        (DataType::UInt8, Literal::Int(l)) => {
            prim::<UInt8Type>(col, op, mask, |v| i128::from(v).cmp(l))
        }
        (DataType::UInt16, Literal::Int(l)) => {
            prim::<UInt16Type>(col, op, mask, |v| i128::from(v).cmp(l))
        }
        (DataType::UInt32, Literal::Int(l)) => {
            prim::<UInt32Type>(col, op, mask, |v| i128::from(v).cmp(l))
        }
        (DataType::UInt64, Literal::Int(l)) => {
            prim::<UInt64Type>(col, op, mask, |v| i128::from(v).cmp(l))
        }
        (DataType::Float32, Literal::F32(l)) => {
            let l = f64::from(*l);
            prim::<Float32Type>(col, op, mask, |v| cmp_float(f64::from(v), l))
        }
        (DataType::Float64, Literal::F64(l)) => {
            prim::<Float64Type>(col, op, mask, |v| cmp_float(v, *l))
        }
        (
            DataType::Decimal64(_, s),
            Literal::Decimal {
                unscaled, scale, ..
            },
        ) => {
            let s = u8::try_from(*s).map_err(|_| "negative decimal scale")?;
            prim::<Decimal64Type>(col, op, mask, |v| {
                cmp_decimal(i128::from(v), s, *unscaled, *scale)
            })
        }
        (
            DataType::Decimal128(_, s),
            Literal::Decimal {
                unscaled, scale, ..
            },
        ) => {
            let s = u8::try_from(*s).map_err(|_| "negative decimal scale")?;
            prim::<Decimal128Type>(col, op, mask, |v| cmp_decimal(v, s, *unscaled, *scale))
        }
        (DataType::Date32, Literal::Date(l)) => prim::<Date32Type>(col, op, mask, |v| v.cmp(l)),
        (DataType::Timestamp(TimeUnit::Microsecond, None), Literal::TimestampMicros(l)) => {
            prim::<TimestampMicrosecondType>(col, op, mask, |v| v.cmp(l))
        }
        (DataType::Timestamp(TimeUnit::Millisecond, None), Literal::TimestampMicros(l)) => {
            prim::<TimestampMillisecondType>(col, op, mask, |v| {
                (i128::from(v) * 1000).cmp(&i128::from(*l))
            })
        }
        (DataType::Utf8, Literal::Text(l)) => {
            let a = col.as_string_opt::<i32>().ok_or("not a string array")?;
            apply(a.nulls(), mask, op, |i| {
                a.value(i).as_bytes().cmp(l.as_bytes())
            });
            Ok(())
        }
        (DataType::Binary, Literal::Bytes(l)) => {
            let a = col.as_binary_opt::<i32>().ok_or("not a binary array")?;
            apply(a.nulls(), mask, op, |i| a.value(i).cmp(l.as_slice()));
            Ok(())
        }
        (DataType::Dictionary(k, v), Literal::Text(l))
            if **k == DataType::Int32 && **v == DataType::Utf8 =>
        {
            // the predicate once per dictionary entry, then a lookup per row: the strings are never expanded
            let d = col
                .as_dictionary_opt::<Int32Type>()
                .ok_or("not a dictionary array")?;
            let values = d
                .values()
                .as_string_opt::<i32>()
                .ok_or("the dictionary values are not strings")?;
            let table: Vec<bool> = (0..values.len())
                .map(|j| {
                    values.is_valid(j) && holds(op, values.value(j).as_bytes().cmp(l.as_bytes()))
                })
                .collect();
            let keys = d.keys();
            let mut bad_key = false;
            for (i, m) in mask.iter_mut().enumerate() {
                if *m {
                    *m = keys.is_valid(i)
                        && match usize::try_from(keys.value(i))
                            .ok()
                            .and_then(|k| table.get(k))
                        {
                            Some(hit) => *hit,
                            None => {
                                bad_key = true;
                                false
                            }
                        };
                }
            }
            if bad_key {
                return Err("a dictionary key is out of range".into());
            }
            Ok(())
        }
        _ => Err(mismatch(col, lit)),
    }
}

/// The rows of `batch` that satisfy every term.
pub fn evaluate(terms: &[Term], batch: &RecordBatch) -> Result<Vec<bool>, String> {
    let mut mask = vec![true; batch.num_rows()];
    for term in terms {
        let col = batch
            .column_by_name(&term.column)
            .ok_or("a filter column is missing from the batch")?;
        if col.len() != mask.len() {
            return Err("a column has a different length than the batch".into());
        }
        match &term.test {
            Test::IsNull => {
                let nulls = col.logical_nulls();
                for (i, m) in mask.iter_mut().enumerate() {
                    *m = *m && nulls.as_ref().is_some_and(|n| n.is_null(i));
                }
            }
            Test::IsNotNull => {
                let nulls = col.logical_nulls();
                for (i, m) in mask.iter_mut().enumerate() {
                    *m = *m && nulls.as_ref().is_none_or(|n| n.is_valid(i));
                }
            }
            Test::Cmp(op, lit) => eval_cmp(col.as_ref(), *op, lit, &mut mask)?,
        }
    }
    Ok(mask)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::RecordBatch;

    fn lit(kind: Kind, text: &str) -> Result<Literal, String> {
        type_literal(kind, text)
    }

    #[test]
    fn syntax_accepts_the_documented_forms() {
        assert_eq!(
            parse_syntax("a < 5"),
            Ok(("a".into(), RawTest::Cmp(CmpOp::Lt, "5".into())))
        );
        assert_eq!(
            parse_syntax("a>=1994-01-01"),
            Ok(("a".into(), RawTest::Cmp(CmpOp::Ge, "1994-01-01".into())))
        );
        assert_eq!(
            parse_syntax("  \"a b\" <> 'x y'"),
            Ok(("a b".into(), RawTest::Cmp(CmpOp::Ne, "'x y'".into())))
        );
        assert_eq!(
            parse_syntax("a != 1"),
            Ok(("a".into(), RawTest::Cmp(CmpOp::Ne, "1".into())))
        );
        assert_eq!(
            parse_syntax("a is not null"),
            Ok(("a".into(), RawTest::IsNotNull))
        );
        assert_eq!(parse_syntax("a IS NULL"), Ok(("a".into(), RawTest::IsNull)));
        assert_eq!(
            parse_syntax("\"a\"\"b\" = 1"),
            Ok(("a\"b".into(), RawTest::Cmp(CmpOp::Eq, "1".into())))
        );
    }

    #[test]
    fn syntax_errors_say_what_is_wrong() {
        assert!(parse_syntax("").is_err());
        assert!(
            parse_syntax("a")
                .expect_err("an error")
                .contains("expected one of")
        );
        assert!(
            parse_syntax("a <")
                .expect_err("an error")
                .contains("expected a literal")
        );
        assert!(parse_syntax("a LIKE 'x'").is_err());
        assert!(
            parse_syntax("\"a < 1")
                .expect_err("an error")
                .contains("not closed")
        );
        assert!(parse_syntax("< 1").is_err());
    }

    #[test]
    fn literals_are_typed_from_the_column_and_nothing_is_guessed() {
        let int = Kind::Int {
            bits: 32,
            signed: true,
        };
        assert_eq!(lit(int, "-12"), Ok(Literal::Int(-12)));
        assert_eq!(lit(int, "+7"), Ok(Literal::Int(7)));
        assert!(
            lit(int, "5.5")
                .expect_err("an error")
                .contains("never rounded")
        );
        assert!(lit(int, "abc").is_err());
        assert!(lit(int, "").is_err());
        assert!(lit(int, "1e3").is_err());
        assert!(
            lit(int, "9".repeat(40).as_str())
                .expect_err("an error")
                .contains("too large")
        );
        let dec = Kind::Decimal {
            precision: 10,
            scale: 2,
        };
        assert_eq!(
            lit(dec, "1.505"),
            Ok(Literal::Decimal {
                unscaled: 1505,
                precision: 4,
                scale: 3
            })
        );
        assert_eq!(
            lit(dec, "-0.05"),
            Ok(Literal::Decimal {
                unscaled: -5,
                precision: 2,
                scale: 2
            })
        );
        assert_eq!(
            lit(dec, "7"),
            Ok(Literal::Decimal {
                unscaled: 7,
                precision: 1,
                scale: 0
            })
        );
        assert_eq!(
            lit(dec, ".5"),
            Ok(Literal::Decimal {
                unscaled: 5,
                precision: 1,
                scale: 1
            })
        );
        assert!(lit(dec, "1e3").is_err());
        assert!(lit(dec, ".").is_err());
        assert!(lit(dec, &"9".repeat(39)).is_err());
        assert!(lit(dec, &"9".repeat(38)).is_ok());
        assert_eq!(lit(Kind::Date, "1994-01-01"), Ok(Literal::Date(8766)));
        assert_eq!(lit(Kind::Date, "'1994-01-01'"), Ok(Literal::Date(8766)));
        assert!(lit(Kind::Date, "1994-02-30").is_err());
        assert!(lit(Kind::Date, "94-01-01").is_err());
        assert_eq!(lit(Kind::Utf8, "'it''s'"), Ok(Literal::Text("it's".into())));
        assert!(
            lit(Kind::Utf8, "plain")
                .expect_err("an error")
                .contains("single quotes")
        );
        assert!(lit(Kind::Utf8, "'a'b'").is_err());
        assert!(lit(Kind::Utf8, "'").is_err());
        assert_eq!(
            lit(Kind::Binary, "X'00ff'"),
            Ok(Literal::Bytes(vec![0, 255]))
        );
        assert!(lit(Kind::Binary, "X'0'").is_err());
        assert_eq!(lit(Kind::Bool, "TRUE"), Ok(Literal::Bool(true)));
        assert!(lit(Kind::Bool, "1").is_err());
        assert!(matches!(lit(Kind::Float64, "nan"), Ok(Literal::F64(x)) if x.is_nan()));
        assert_eq!(
            lit(Kind::Float64, "-inf"),
            Ok(Literal::F64(f64::NEG_INFINITY))
        );
        assert_eq!(lit(Kind::Float32, "0.1"), Ok(Literal::F32(0.1f32)));
        assert!(
            lit(Kind::Float64, "1e999")
                .expect_err("an error")
                .contains("out of range")
        );
        assert!(lit(Kind::Float64, "1.5x").is_err());
        assert!(lit(Kind::Float64, "--1").is_err());
        assert!(lit(Kind::Timestamp(crate::schema::Unit::Micros), "1994-01-01").is_err());
    }

    fn batch(cols: Vec<(&str, arrow_array::ArrayRef)>) -> RecordBatch {
        RecordBatch::try_from_iter(cols).expect("batch")
    }

    fn rows(terms: &[Term], b: &RecordBatch) -> Vec<usize> {
        evaluate(terms, b)
            .expect("evaluate")
            .iter()
            .enumerate()
            .filter(|(_, m)| **m)
            .map(|(i, _)| i)
            .collect()
    }

    fn cmp(col: &str, op: CmpOp, lit: Literal) -> Term {
        Term {
            column: col.to_string(),
            test: Test::Cmp(op, lit),
        }
    }

    #[test]
    fn plain_text_binary_and_unsigned_columns_use_unsigned_bytes_and_unsigned_numbers() {
        use arrow_array::{BinaryArray, StringArray, UInt64Array};
        use std::sync::Arc;
        let b = batch(vec![
            (
                "s",
                Arc::new(StringArray::from(vec![
                    Some("a"),
                    Some("é"),
                    None,
                    Some("z"),
                ])),
            ),
            (
                "b",
                Arc::new(BinaryArray::from_opt_vec(vec![
                    Some(&[0x7fu8][..]),
                    Some(&[0x80]),
                    None,
                    Some(&[0xff]),
                ])),
            ),
            (
                "u",
                Arc::new(UInt64Array::from(vec![
                    Some(1u64),
                    Some(u64::MAX),
                    None,
                    Some(1 << 63),
                ])),
            ),
        ]);
        // 'é' (0xC3 0xA9) is above 'z' bytewise (a signed comparison would put it below)
        assert_eq!(
            rows(&[cmp("s", CmpOp::Gt, Literal::Text("z".into()))], &b),
            [1]
        );
        assert_eq!(
            rows(&[cmp("s", CmpOp::Lt, Literal::Text("b".into()))], &b),
            [0]
        );
        assert_eq!(
            rows(&[cmp("s", CmpOp::Ne, Literal::Text("a".into()))], &b),
            [1, 3],
            "NULL <> 'a' is not true"
        );
        assert_eq!(
            rows(&[cmp("b", CmpOp::Ge, Literal::Bytes(vec![0x80]))], &b),
            [1, 3]
        );
        assert_eq!(
            rows(&[cmp("b", CmpOp::Lt, Literal::Bytes(vec![0x80]))], &b),
            [0]
        );
        assert_eq!(
            rows(&[cmp("u", CmpOp::Gt, Literal::Int(5))], &b),
            [1, 3],
            "u64::MAX is not -1"
        );
        assert_eq!(
            rows(
                &[cmp("u", CmpOp::Eq, Literal::Int(i128::from(u64::MAX)))],
                &b
            ),
            [1]
        );
        assert_eq!(
            rows(
                &[Term {
                    column: "u".into(),
                    test: Test::IsNull
                }],
                &b
            ),
            [2]
        );
        assert_eq!(
            rows(
                &[Term {
                    column: "s".into(),
                    test: Test::IsNotNull
                }],
                &b
            ),
            [0, 1, 3]
        );
    }

    #[test]
    fn small_integer_columns_compare_without_casting_the_literal() {
        use arrow_array::{Int8Array, Int32Array};
        use std::sync::Arc;
        let b = batch(vec![
            ("i8", Arc::new(Int8Array::from(vec![44i8, -1, 5]))),
            ("i32", Arc::new(Int32Array::from(vec![0i32, 1, i32::MAX]))),
        ]);
        assert_eq!(
            rows(&[cmp("i8", CmpOp::Eq, Literal::Int(300))], &b),
            Vec::<usize>::new(),
            "300 is not 44 as a tinyint"
        );
        assert_eq!(
            rows(&[cmp("i32", CmpOp::Eq, Literal::Int(4_294_967_296))], &b),
            Vec::<usize>::new(),
            "2^32 is not 0"
        );
        assert_eq!(
            rows(&[cmp("i32", CmpOp::Lt, Literal::Int(5_000_000_000))], &b),
            [0, 1, 2]
        );
    }

    #[test]
    fn float_order_is_duckdbs() {
        let nan = f64::NAN;
        assert_eq!(cmp_float(nan, nan), Ordering::Equal);
        assert_eq!(cmp_float(nan, f64::INFINITY), Ordering::Greater);
        assert_eq!(cmp_float(f64::INFINITY, nan), Ordering::Less);
        assert_eq!(cmp_float(-0.0, 0.0), Ordering::Equal);
        assert_eq!(cmp_float(1.0, 2.0), Ordering::Less);
        assert_eq!(cmp_float(-f64::NAN, 1.0), Ordering::Greater);
    }

    #[test]
    fn decimals_compare_exactly_across_scales() {
        // 1.50 against 1.505
        assert_eq!(cmp_decimal(150, 2, 1505, 3), Ordering::Less);
        assert_eq!(cmp_decimal(150, 2, 15, 1), Ordering::Equal);
        assert_eq!(cmp_decimal(1505, 3, 150, 2), Ordering::Greater);
        // rescaling overflows: the sign decides
        let big = 10i128.pow(37);
        assert_eq!(cmp_decimal(big, 0, 5, 20), Ordering::Greater);
        assert_eq!(cmp_decimal(-big, 0, 5, 20), Ordering::Less);
        assert_eq!(cmp_decimal(5, 20, big, 0), Ordering::Less);
        assert_eq!(cmp_decimal(5, 20, -big, 0), Ordering::Greater);
    }
}
