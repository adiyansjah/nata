//! Versioned logical records inside the validated Arrow IPC envelope. The bound
//! plan is the schema authority; records carry SQL type and aggregate tags too,
//! so a valid checksum cannot turn one kind of state into another.
use super::{Atom, State};
use nata_kernels::aggregate::ExactSum;
use nata_sql::{bind::Aggregate, types::SqlType};
use std::io;

const VERSION: u8 = 1;
const MAX_RECORD: usize = 24 << 10;
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid typed aggregate spill state",
    )
}
fn type_tag(ty: SqlType) -> [u8; 3] {
    match ty {
        SqlType::Null => [0, 0, 0],
        SqlType::Boolean => [1, 0, 0],
        SqlType::TinyInt => [2, 0, 0],
        SqlType::SmallInt => [3, 0, 0],
        SqlType::Integer => [4, 0, 0],
        SqlType::BigInt => [5, 0, 0],
        SqlType::Float => [6, 0, 0],
        SqlType::Double => [7, 0, 0],
        SqlType::Decimal { precision, scale } => [8, precision, scale],
        SqlType::Varchar => [9, 0, 0],
        SqlType::Date => [10, 0, 0],
    }
}
fn kind(name: &str) -> io::Result<u8> {
    match name {
        "count" => Ok(0),
        "sum" => Ok(1),
        "avg" => Ok(2),
        "min" => Ok(3),
        "max" => Ok(4),
        _ => Err(invalid()),
    }
}
fn atom_size(v: &Atom) -> usize {
    4 + match v {
        Atom::Null => 0,
        Atom::Bool(_) => 1,
        Atom::Int(_) | Atom::Float(_) => 8,
        Atom::Dec(_) => 16,
        Atom::Date(_) => 4,
        Atom::Str(s) => 4 + s.len(),
    }
}
/// Bound before constructing a frame or copying any key/state bytes. The caller
/// owns a separately reserved encode workspace covering these allocations.
pub(super) fn size(key: &[Atom], states: &[State]) -> io::Result<usize> {
    let n = key
        .iter()
        .map(atom_size)
        .chain(states.iter().map(|s| {
            7 + match s {
                State::Count(_) => 8,
                State::Sum(_) => 40,
                State::Extreme(v) => atom_size(v),
                State::Final(_) => MAX_RECORD + 1,
            }
        }))
        .try_fold(10usize, |a, b| a.checked_add(b).ok_or_else(invalid))?;
    if n > MAX_RECORD {
        return Err(io::Error::other(
            "resource error: aggregate spill record exceeds minimum workspace",
        ));
    }
    Ok(n)
}
fn put_atom(out: &mut Vec<u8>, v: &Atom, ty: SqlType) -> io::Result<()> {
    // Validate through the same typed decoder, including canonical floats and
    // precision/range. This is a small bounded buffer inside the encode grant.
    out.extend_from_slice(&type_tag(ty));
    out.push(u8::from(!matches!(v, Atom::Null)));
    match v {
        Atom::Null => (),
        Atom::Bool(v) => out.push(u8::from(*v)),
        Atom::Int(v) => out.extend_from_slice(&v.to_le_bytes()),
        Atom::Dec(v) => out.extend_from_slice(&v.to_le_bytes()),
        Atom::Float(v) => out.extend_from_slice(&v.to_le_bytes()),
        Atom::Date(v) => out.extend_from_slice(&v.to_le_bytes()),
        Atom::Str(v) => {
            out.extend_from_slice(&u32::try_from(v.len()).map_err(|_| invalid())?.to_le_bytes());
            out.extend_from_slice(v.as_bytes());
        }
    }
    Ok(())
}
pub(super) fn encode(
    key: &[Atom],
    states: &[State],
    a: &Aggregate,
) -> io::Result<(Vec<u8>, Vec<u8>)> {
    size(key, states)?;
    if key.len() != a.keys.len() || states.len() != a.aggregates.len() {
        return Err(invalid());
    }
    let mut keys = Vec::with_capacity(key.iter().map(atom_size).sum::<usize>() + 5);
    keys.push(VERSION);
    keys.extend_from_slice(
        &u32::try_from(key.len())
            .map_err(|_| invalid())?
            .to_le_bytes(),
    );
    for (v, expr) in key.iter().zip(&a.keys) {
        put_atom(&mut keys, v, expr.ty)?;
    }
    let mut values = Vec::with_capacity(size(&[], states)?);
    values.push(VERSION);
    values.extend_from_slice(
        &u32::try_from(states.len())
            .map_err(|_| invalid())?
            .to_le_bytes(),
    );
    for (s, expr) in states.iter().zip(&a.aggregates) {
        values.push(kind(&expr.name)?);
        values.extend_from_slice(&type_tag(expr.arg.as_ref().map_or(SqlType::Null, |a| a.ty)));
        values.extend_from_slice(&type_tag(expr.ty));
        match s {
            State::Count(n) if expr.name == "count" && *n <= i64::MAX as u64 => {
                values.extend_from_slice(&n.to_le_bytes())
            }
            State::Sum(s) if expr.name == "sum" || expr.name == "avg" => {
                values.extend_from_slice(&s.spill_sum());
                values.extend_from_slice(&s.count().to_le_bytes());
            }
            State::Extreme(v) if expr.name == "min" || expr.name == "max" => {
                put_atom(&mut values, v, expr.ty)?
            }
            _ => return Err(invalid()),
        }
    }
    Ok((keys, values))
}
struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let out = self.0.get(..n).ok_or_else(invalid)?;
        self.0 = &self.0[n..];
        Ok(out)
    }
    fn fixed<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        self.take(N)?.try_into().map_err(|_| invalid())
    }
    fn byte(&mut self) -> io::Result<u8> {
        Ok(self.fixed::<1>()?[0])
    }
    fn header(&mut self, n: usize) -> io::Result<()> {
        if self.byte()? != VERSION
            || usize::try_from(u32::from_le_bytes(self.fixed()?)).map_err(|_| invalid())? != n
        {
            return Err(invalid());
        }
        Ok(())
    }
    fn atom(&mut self, ty: SqlType) -> io::Result<Atom> {
        if self.fixed::<3>()? != type_tag(ty) {
            return Err(invalid());
        }
        match self.byte()? {
            0 => return Ok(Atom::Null),
            1 => (),
            _ => return Err(invalid()),
        }
        Ok(match ty {
            SqlType::Null => return Err(invalid()),
            SqlType::Boolean => Atom::Bool(match self.byte()? {
                0 => false,
                1 => true,
                _ => return Err(invalid()),
            }),
            ty if ty.is_integer() => {
                let n = i64::from_le_bytes(self.fixed()?);
                if n < ty.int_min().ok_or_else(invalid)? || n > ty.int_max().ok_or_else(invalid)? {
                    return Err(invalid());
                }
                Atom::Int(n)
            }
            SqlType::Decimal { precision, scale } => {
                let n = i128::from_le_bytes(self.fixed()?);
                if !(1..=38).contains(&precision)
                    || scale > precision
                    || n.unsigned_abs() >= 10u128.pow(u32::from(precision))
                {
                    return Err(invalid());
                }
                Atom::Dec(n)
            }
            SqlType::Float | SqlType::Double => {
                let bits = u64::from_le_bytes(self.fixed()?);
                let v = f64::from_bits(bits);
                if Atom::float(v) != Atom::Float(bits) {
                    return Err(invalid());
                }
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "Validate exact widening from the declared REAL input"
                )]
                if ty == SqlType::Float && !v.is_nan() && f64::from(v as f32) != v {
                    return Err(invalid());
                }
                Atom::Float(bits)
            }
            SqlType::Date => Atom::Date(i32::from_le_bytes(self.fixed()?)),
            SqlType::Varchar => {
                let n =
                    usize::try_from(u32::from_le_bytes(self.fixed()?)).map_err(|_| invalid())?;
                Atom::Str(
                    std::str::from_utf8(self.take(n)?)
                        .map_err(|_| invalid())?
                        .to_owned(),
                )
            }
            _ => return Err(invalid()),
        })
    }
    fn end(&self) -> io::Result<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(invalid())
        }
    }
}
/// Bound key/state framing before copying typed key bytes. State cursors are
/// consumed separately into the destination table under its own grants.
pub(super) fn key(keys: &[u8], values: &[u8], a: &Aggregate) -> io::Result<Vec<Atom>> {
    if keys
        .len()
        .checked_add(values.len())
        .is_none_or(|n| n > MAX_RECORD)
    {
        return Err(invalid());
    }
    if a.keys.len() > keys.len().saturating_sub(5) / 4
        || a.aggregates.len() > values.len().saturating_sub(5) / 7
    {
        return Err(invalid());
    }
    let mut r = Reader(keys);
    r.header(a.keys.len())?;
    let key = a
        .keys
        .iter()
        .map(|e| r.atom(e.ty))
        .collect::<io::Result<Vec<_>>>()?;
    r.end()?;
    Ok(key)
}
#[cfg(test)]
pub(super) fn decode(
    keys: &[u8],
    values: &[u8],
    a: &Aggregate,
) -> io::Result<(Vec<Atom>, Vec<State>)> {
    let key = key(keys, values, a)?;
    let mut states = States::new(values, a)?;
    let out = a
        .aggregates
        .iter()
        .map(|e| states.next(e))
        .collect::<io::Result<Vec<_>>>()?;
    states.end()?;
    Ok((key, out))
}
/// Borrowed cursor over a row's encoded states. The batch replay path sends
/// each decoded state directly to its charged group, without a temporary Vec.
pub(super) struct States<'a>(Reader<'a>);
impl<'a> States<'a> {
    pub fn new(values: &'a [u8], a: &Aggregate) -> io::Result<Self> {
        let mut r = Reader(values);
        r.header(a.aggregates.len())?;
        Ok(Self(r))
    }
    pub fn next(&mut self, e: &nata_sql::bind::AggregateExpr) -> io::Result<State> {
        let r = &mut self.0;
        if r.byte()? != kind(&e.name)?
            || r.fixed::<3>()? != type_tag(e.arg.as_ref().map_or(SqlType::Null, |a| a.ty))
            || r.fixed::<3>()? != type_tag(e.ty)
        {
            return Err(invalid());
        }
        Ok(match e.name.as_str() {
            "count" => {
                let n = u64::from_le_bytes(r.fixed()?);
                if n > i64::MAX as u64 {
                    return Err(invalid());
                }
                State::Count(n)
            }
            "sum" | "avg" => State::Sum(
                ExactSum::from_spill(r.fixed()?, u64::from_le_bytes(r.fixed()?))
                    .map_err(|_| invalid())?,
            ),
            "min" | "max" => State::Extreme(r.atom(e.ty)?),
            _ => return Err(invalid()),
        })
    }
    pub fn end(&self) -> io::Result<()> {
        self.0.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nata_sql::{
        Pos,
        bind::AggregateExpr,
        texpr::{Expr, ExprKind, Value},
    };
    fn expr(ty: SqlType) -> Expr {
        Expr {
            kind: ExprKind::Literal(Value::Null),
            ty,
            pos: Pos::NONE,
        }
    }
    fn plan(ty: SqlType) -> Aggregate {
        Aggregate {
            keys: vec![expr(ty)],
            aggregates: vec![AggregateExpr {
                name: "min".into(),
                arg: Some(expr(ty)),
                ty,
                pos: Pos::NONE,
            }],
            having: None,
        }
    }
    #[test]
    fn typed_records_reject_truncation_trailing_bytes_wrong_tags_and_invalid_values() {
        let cases = [
            (SqlType::Boolean, Atom::Bool(true)),
            (SqlType::TinyInt, Atom::Int(-128)),
            (SqlType::SmallInt, Atom::Int(-32768)),
            (SqlType::Integer, Atom::Int(i64::from(i32::MIN))),
            (SqlType::BigInt, Atom::Int(i64::MIN)),
            (SqlType::Float, Atom::float(f64::from(f32::MAX))),
            (SqlType::Double, Atom::float(f64::INFINITY)),
            (SqlType::Double, Atom::float(f64::NAN)),
            (SqlType::Double, Atom::float(-0.0)),
            (
                SqlType::Decimal {
                    precision: 38,
                    scale: 17,
                },
                Atom::Dec(-10i128.pow(37)),
            ),
            (SqlType::Varchar, Atom::Str("key\0雪".into())),
            (SqlType::Varchar, Atom::Str(String::new())),
            (SqlType::Date, Atom::Date(-1)),
        ];
        for (ty, v) in cases {
            let a = plan(ty);
            for v in [v, Atom::Null] {
                let (k, s) = encode(std::slice::from_ref(&v), &[State::Extreme(v.clone())], &a)
                    .expect("encode");
                let (key, states) = decode(&k, &s, &a).expect("decode");
                assert_eq!(key.as_slice(), std::slice::from_ref(&v));
                assert!(matches!(&states[0], State::Extreme(x) if *x == v));
                for n in 0..k.len() {
                    assert!(decode(&k[..n], &s, &a).is_err());
                }
                for n in 0..s.len() {
                    assert!(decode(&k, &s[..n], &a).is_err());
                }
                let mut extra = s.clone();
                extra.push(0);
                assert!(decode(&k, &extra, &a).is_err());
                let mut wrong = k.clone();
                wrong[5] = 255;
                assert!(decode(&wrong, &s, &a).is_err());
            }
        }
        for (ty, v) in [
            (SqlType::TinyInt, Atom::Int(128)),
            (
                SqlType::Decimal {
                    precision: 2,
                    scale: 0,
                },
                Atom::Dec(100),
            ),
            (SqlType::Double, Atom::Float((-0.0f64).to_bits())),
            (SqlType::Float, Atom::float(1.0000000001)),
        ] {
            let a = plan(ty);
            let (k, s) = encode(&[v], &[State::Extreme(Atom::Null)], &a).expect("record");
            assert!(decode(&k, &s, &a).is_err());
        }
    }
    #[test]
    fn replay_rejects_impossible_wide_states_and_wrong_aggregate_descriptor() {
        let a = Aggregate {
            keys: vec![],
            aggregates: vec![AggregateExpr {
                name: "sum".into(),
                arg: Some(expr(SqlType::Decimal {
                    precision: 38,
                    scale: 0,
                })),
                ty: SqlType::Decimal {
                    precision: 38,
                    scale: 0,
                },
                pos: Pos::NONE,
            }],
            having: None,
        };
        let mut sum = ExactSum::default();
        sum.add(10i128.pow(38) - 1).expect("sum");
        sum.add(10i128.pow(38) - 1).expect("wide");
        let (k, s) = encode(&[], &[State::Sum(sum)], &a).expect("encode");
        assert!(matches!(decode(&k,&s,&a).expect("wide replay").1[0], State::Sum(v) if v == sum));
        let mut bad = s.clone();
        bad[6] = 255;
        assert!(decode(&k, &bad, &a).is_err());
        let mut bad = s.clone();
        let n = bad.len();
        bad[n - 8..].fill(0);
        assert!(decode(&k, &bad, &a).is_err());
        let mut bad = s.clone();
        bad[n - 8..].fill(255);
        assert!(decode(&k, &bad, &a).is_err());
    }
}
