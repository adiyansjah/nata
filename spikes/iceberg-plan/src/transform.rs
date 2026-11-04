//! Partition transforms, the 32-bit Murmur3 hash behind `bucket`, and the INCLUSIVE PROJECTION of a predicate onto a transformed partition value:
//! if a row satisfies `col OP lit` then its partition value satisfies the projected predicate. Checked against the spec's Appendix B vectors and,
//! as a property, against random values (including negative ones, which is where floor-versus-truncate bugs live).
use crate::types::*;
use crate::{Res, err};

pub fn murmur3_32(data: &[u8], seed: u32) -> u32 {
    const C1: u32 = 0xcc9e2d51;
    const C2: u32 = 0x1b873593;
    let mut h = seed;
    let chunks = data.chunks_exact(4);
    let tail = chunks.remainder();
    for c in chunks {
        let mut k = u32::from_le_bytes(c.try_into().unwrap());
        k = k.wrapping_mul(C1).rotate_left(15).wrapping_mul(C2);
        h ^= k;
        h = h.rotate_left(13).wrapping_mul(5).wrapping_add(0xe6546b64);
    }
    let mut k: u32 = 0;
    for (i, &b) in tail.iter().enumerate() { k ^= (b as u32) << (8 * i); }
    if !tail.is_empty() { k = k.wrapping_mul(C1).rotate_left(15).wrapping_mul(C2); h ^= k; }
    h ^= data.len() as u32;
    h ^= h >> 16; h = h.wrapping_mul(0x85ebca6b);
    h ^= h >> 13; h = h.wrapping_mul(0xc2b2ae35);
    h ^ (h >> 16)
}

pub fn hash_long(v: i64) -> i32 { murmur3_32(&v.to_le_bytes(), 0) as i32 }

/// Appendix B: the hash of a value of a bucketable type.
pub fn iceberg_hash(v: &Value) -> i32 {
    use Value::*;
    match v {
        Int(x) | Date(x) => hash_long(*x as i64),
        Long(x) | Ts(x) => hash_long(*x),
        Decimal(x) => murmur3_32(&dec_min_be(*x), 0) as i32,
        Str(s) => murmur3_32(s.as_bytes(), 0) as i32,
        Bin(b) => murmur3_32(b, 0) as i32,
        Uuid(u) => murmur3_32(u, 0) as i32,
        Bool(b) => hash_long(*b as i64),
        Float(x) => hash_long(canon_double(*x as f64)),
        Double(x) => hash_long(canon_double(*x)),
    }
}

fn canon_double(x: f64) -> i64 { if x.is_nan() { 0x7ff8000000000000 } else if x == 0.0 { 0 } else { x.to_bits() as i64 } }

#[derive(Clone, Debug, PartialEq)]
pub enum Transform { Identity, Bucket(u32), Truncate(u32), Year, Month, Day, Hour, Void }

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op { Eq, Lt, Le, Gt, Ge, StartsWith }

impl Transform {
    pub fn parse(s: &str) -> Res<Transform> {
        Ok(match s {
            "identity" => Transform::Identity, "year" => Transform::Year, "month" => Transform::Month, "day" => Transform::Day, "hour" => Transform::Hour, "void" => Transform::Void,
            _ if s.starts_with("bucket[") => Transform::Bucket(s[7..s.len() - 1].parse().map_err(|_| "bad bucket")?),
            _ if s.starts_with("truncate[") => Transform::Truncate(s[9..s.len() - 1].parse().map_err(|_| "bad truncate")?),
            _ => return err(format!("unknown transform {s}")),
        })
    }

    pub fn name(&self) -> String {
        match self {
            Transform::Identity => "identity".into(), Transform::Bucket(n) => format!("bucket[{n}]"), Transform::Truncate(w) => format!("truncate[{w}]"),
            Transform::Year => "year".into(), Transform::Month => "month".into(), Transform::Day => "day".into(), Transform::Hour => "hour".into(), Transform::Void => "void".into(),
        }
    }

    pub fn result_type(&self, src: &Type) -> Type {
        match self { Transform::Identity | Transform::Truncate(_) | Transform::Void => src.clone(), Transform::Day => Type::Date, _ => Type::Int }
    }

    /// The partition value for a (non-null) source value; None for the void transform.
    pub fn apply(&self, v: &Value) -> Option<Value> {
        use Value::*;
        Some(match self {
            Transform::Identity => v.clone(),
            Transform::Void => return None,
            Transform::Bucket(n) => Int(((iceberg_hash(v) & i32::MAX) as i64 % *n as i64) as i32),
            Transform::Truncate(w) => match v {
                Int(x) => Int(x - x.rem_euclid(*w as i32)),
                Long(x) => Long(x - x.rem_euclid(*w as i64)),
                Decimal(x) => Decimal(x - x.rem_euclid(*w as i128)),
                Str(s) => Str(s.chars().take(*w as usize).collect()),
                Bin(b) => Bin(b.iter().take(*w as usize).copied().collect()),
                other => panic!("truncate of {other:?}"),
            },
            Transform::Year | Transform::Month | Transform::Day | Transform::Hour => {
                let (days, micros): (i64, Option<i64>) = match v { Date(d) => (*d as i64, None), Ts(t) => (t.div_euclid(MICROS_PER_DAY), Some(*t)), other => panic!("date transform of {other:?}") };
                let (y, m, _) = civil_from_days(days);
                match self {
                    Transform::Year => Int((y - 1970) as i32),
                    Transform::Month => Int(((y - 1970) * 12 + (m - 1)) as i32),
                    Transform::Day => Date(days as i32),
                    _ => Int(micros.expect("hour needs a timestamp").div_euclid(MICROS_PER_HOUR) as i32),
                }
            }
        })
    }

    /// Inclusive projection of `src OP lit` onto this transform's partition value. None = the predicate cannot be projected (the partition filter is true).
    pub fn project(&self, op: Op, lit: &Value) -> Option<(Op, Value)> {
        match self {
            Transform::Void => None,
            Transform::Identity => Some((op, lit.clone())),
            Transform::Bucket(_) => (op == Op::Eq).then(|| (Op::Eq, self.apply(lit).unwrap())),
            Transform::Truncate(w) => match op {
                Op::Eq => Some((Op::Eq, self.apply(lit)?)),
                Op::Lt | Op::Le => Some((Op::Le, self.apply(lit)?)),
                Op::Gt | Op::Ge => Some((Op::Ge, self.apply(lit)?)),
                Op::StartsWith => match lit {
                    Value::Str(s) if s.chars().count() >= *w as usize => Some((Op::Eq, self.apply(lit)?)),
                    _ => Some((Op::StartsWith, lit.clone())),
                },
            },
            Transform::Year | Transform::Month | Transform::Day | Transform::Hour => match op {
                Op::Eq => Some((Op::Eq, self.apply(lit)?)),
                Op::Lt | Op::Le => Some((Op::Le, self.apply(lit)?)),
                Op::Gt | Op::Ge => Some((Op::Ge, self.apply(lit)?)),
                Op::StartsWith => None,
            },
        }
    }
}

/// Does `v OP lit` hold? (the semantics of the row-level predicates; floats in totalOrder, strings by UTF-8 bytes)
pub fn holds(v: &Value, op: Op, lit: &Value) -> bool {
    match op {
        Op::Eq => vcmp(v, lit).is_eq(), Op::Lt => vcmp(v, lit).is_lt(), Op::Le => vcmp(v, lit).is_le(), Op::Gt => vcmp(v, lit).is_gt(), Op::Ge => vcmp(v, lit).is_ge(),
        Op::StartsWith => match (v, lit) { (Value::Str(a), Value::Str(b)) => a.starts_with(b.as_str()), (Value::Bin(a), Value::Bin(b)) => a.starts_with(b), _ => false },
    }
}
