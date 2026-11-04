//! Iceberg primitive types, single values, their total order and the binary single-value serialization of Appendix D.
use std::cmp::Ordering;

use crate::{Res, err};

#[derive(Clone, Debug, PartialEq)]
pub enum Type { Boolean, Int, Long, Float, Double, Decimal(u8, u8), Date, Timestamp, TimestampTz, String, Uuid, Fixed(usize), Binary, Other(String) }

impl Type {
    pub fn to_json(&self) -> serde_json::Value {
        use Type::*;
        serde_json::Value::String(match self {
            Boolean => "boolean".into(), Int => "int".into(), Long => "long".into(), Float => "float".into(), Double => "double".into(),
            Decimal(p, s) => format!("decimal({p}, {s})"), Date => "date".into(), Timestamp => "timestamp".into(), TimestampTz => "timestamptz".into(),
            String => "string".into(), Uuid => "uuid".into(), Fixed(n) => format!("fixed[{n}]"), Binary => "binary".into(), Other(s) => s.clone(),
        })
    }

    pub fn parse(s: &str) -> Res<Type> {
        Ok(match s {
            "boolean" => Type::Boolean, "int" => Type::Int, "long" => Type::Long, "float" => Type::Float, "double" => Type::Double, "date" => Type::Date,
            "timestamp" => Type::Timestamp, "timestamptz" => Type::TimestampTz, "string" => Type::String, "uuid" => Type::Uuid, "binary" => Type::Binary,
            _ if s.starts_with("decimal(") => {
                let inner = s.trim_start_matches("decimal(").trim_end_matches(')');
                let (p, sc) = inner.split_once(',').ok_or("bad decimal")?;
                Type::Decimal(p.trim().parse().map_err(|_| "bad precision")?, sc.trim().parse().map_err(|_| "bad scale")?)
            }
            _ if s.starts_with("fixed[") => Type::Fixed(s.trim_start_matches("fixed[").trim_end_matches(']').parse().map_err(|_| "bad fixed length")?),
            _ => return err(format!("unsupported type {s}")),
        })
    }

    /// Nested and unknown types are kept as `Other`: columns of such types are never used for pruning.
    pub fn parse_lenient(j: &serde_json::Value) -> Type {
        match j.as_str() { Some(s) => Type::parse(s).unwrap_or_else(|_| Type::Other(s.to_string())), None => Type::Other(j.to_string()) }
    }
}

#[derive(Clone, Debug)]
pub enum Value { Bool(bool), Int(i32), Long(i64), Float(f32), Double(f64), Decimal(i128), Date(i32), Ts(i64), Str(String), Bin(Vec<u8>), Uuid([u8; 16]) }

/// The order used for bounds and comparisons: floats in IEEE totalOrder with every NaN equal and above +inf (-0.0 sorts before +0.0).
pub fn fcmp(a: f64, b: f64) -> Ordering {
    match (a.is_nan(), b.is_nan()) { (true, true) => Ordering::Equal, (true, false) => Ordering::Greater, (false, true) => Ordering::Less, _ => a.total_cmp(&b) }
}

pub fn vcmp(a: &Value, b: &Value) -> Ordering {
    use Value::*;
    match (a, b) {
        (Bool(x), Bool(y)) => x.cmp(y),
        (Int(x), Int(y)) | (Date(x), Date(y)) => x.cmp(y),
        (Long(x), Long(y)) | (Ts(x), Ts(y)) => x.cmp(y),
        (Float(x), Float(y)) => fcmp(*x as f64, *y as f64),
        (Double(x), Double(y)) => fcmp(*x, *y),
        (Decimal(x), Decimal(y)) => x.cmp(y),
        (Str(x), Str(y)) => x.as_bytes().cmp(y.as_bytes()),
        (Bin(x), Bin(y)) => x.cmp(y),
        (Uuid(x), Uuid(y)) => x.cmp(y),
        _ => panic!("comparing values of different types: {a:?} vs {b:?}"),
    }
}

impl PartialEq for Value { fn eq(&self, o: &Value) -> bool { std::mem::discriminant(self) == std::mem::discriminant(o) && vcmp(self, o) == Ordering::Equal } }

pub fn is_nan(v: &Value) -> bool { matches!(v, Value::Float(x) if x.is_nan()) || matches!(v, Value::Double(x) if x.is_nan()) }

/// Minimum number of bytes of the two's-complement big-endian encoding of `v`.
pub fn dec_min_be(v: i128) -> Vec<u8> {
    let b = v.to_be_bytes();
    let mut i = 0;
    while i < 15 && ((b[i] == 0 && b[i + 1] & 0x80 == 0) || (b[i] == 0xFF && b[i + 1] & 0x80 != 0)) { i += 1; }
    b[i..].to_vec()
}

pub fn dec_from_be(b: &[u8]) -> i128 {
    let mut v: i128 = if !b.is_empty() && b[0] & 0x80 != 0 { -1 } else { 0 };
    for &x in b { v = (v << 8) | x as i128; }
    v
}

/// Appendix D, binary single-value serialization.
pub fn serialize(v: &Value) -> Vec<u8> {
    use Value::*;
    match v {
        Bool(b) => vec![*b as u8], Int(x) | Date(x) => x.to_le_bytes().to_vec(), Long(x) | Ts(x) => x.to_le_bytes().to_vec(),
        Float(x) => x.to_le_bytes().to_vec(), Double(x) => x.to_le_bytes().to_vec(), Decimal(x) => dec_min_be(*x),
        Str(s) => s.as_bytes().to_vec(), Bin(b) => b.clone(), Uuid(u) => u.to_vec(),
    }
}

pub fn deserialize(t: &Type, b: &[u8]) -> Res<Value> {
    let need = |n: usize| if b.len() == n { Ok(()) } else { err(format!("{t:?}: {} bytes, expected {n}", b.len())) };
    Ok(match t {
        Type::Boolean => { need(1)?; Value::Bool(b[0] != 0) }
        Type::Int => { need(4)?; Value::Int(i32::from_le_bytes(b.try_into().unwrap())) }
        Type::Date => { need(4)?; Value::Date(i32::from_le_bytes(b.try_into().unwrap())) }
        Type::Long => { need(8)?; Value::Long(i64::from_le_bytes(b.try_into().unwrap())) }
        Type::Timestamp | Type::TimestampTz => { need(8)?; Value::Ts(i64::from_le_bytes(b.try_into().unwrap())) }
        Type::Float => { need(4)?; Value::Float(f32::from_le_bytes(b.try_into().unwrap())) }
        Type::Double => { need(8)?; Value::Double(f64::from_le_bytes(b.try_into().unwrap())) }
        Type::Decimal(..) => { if b.is_empty() || b.len() > 16 { return err("bad decimal length"); } Value::Decimal(dec_from_be(b)) }
        Type::String => Value::Str(String::from_utf8(b.to_vec()).map_err(|e| format!("bound is not UTF-8: {e}"))?),
        Type::Binary | Type::Fixed(_) => Value::Bin(b.to_vec()),
        Type::Uuid => { need(16)?; Value::Uuid(b.try_into().unwrap()) }
        Type::Other(o) => return err(format!("no bound deserialization for {o}")),
    })
}

// ---------------------------------------------------------------- civil dates (proleptic Gregorian), Howard Hinnant's algorithms

pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub const MICROS_PER_DAY: i64 = 86_400_000_000;
pub const MICROS_PER_HOUR: i64 = 3_600_000_000;
