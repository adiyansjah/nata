//! SQL types and the typing rules of `docs/dialect.md` sections 3 to 6 and `docs/decimal-rules.md` sections 2 to 6: result types of arithmetic, which
//! types may be compared, how CASE results unify, which casts exist.

use std::fmt;

use nata_parquet::schema::Kind;

/// The type of an expression. `Null` is the type of a NULL literal whose context has not given it a type YET: it exists only while binding and never
/// appears in a bound query (a NULL that never gets a type is `not supported yet`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SqlType {
    Null,
    Boolean,
    TinyInt,
    SmallInt,
    Integer,
    BigInt,
    /// 32-bit float: a column type only (there is no `CAST ... AS FLOAT`).
    Float,
    Double,
    Decimal {
        precision: u8,
        scale: u8,
    },
    Varchar,
    Date,
}

/// Why an operation on types is refused: a bind error (the dialect forbids it) or `not supported yet` (the dialect does not say).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeError {
    Bind(String),
    NotSupported(String),
}

type TResult<T> = Result<T, TypeError>;

fn bind<T>(m: String) -> TResult<T> {
    Err(TypeError::Bind(m))
}

fn nsy<T>(m: String) -> TResult<T> {
    Err(TypeError::NotSupported(m))
}

impl fmt::Display for SqlType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SqlType::Null => f.write_str("NULL"),
            SqlType::Boolean => f.write_str("BOOLEAN"),
            SqlType::TinyInt => f.write_str("TINYINT"),
            SqlType::SmallInt => f.write_str("SMALLINT"),
            SqlType::Integer => f.write_str("INTEGER"),
            SqlType::BigInt => f.write_str("BIGINT"),
            SqlType::Float => f.write_str("FLOAT"),
            SqlType::Double => f.write_str("DOUBLE"),
            SqlType::Decimal { precision, scale } => write!(f, "DECIMAL({precision},{scale})"),
            SqlType::Varchar => f.write_str("VARCHAR"),
            SqlType::Date => f.write_str("DATE"),
        }
    }
}

impl SqlType {
    /// The type of a Parquet column, or the `<what>` of `not supported yet: <what>` for a column kind the dialect has no type for.
    pub fn from_kind(k: Kind) -> Result<SqlType, String> {
        Ok(match k {
            Kind::Bool => SqlType::Boolean,
            Kind::Int {
                bits: 8,
                signed: true,
            } => SqlType::TinyInt,
            Kind::Int {
                bits: 16,
                signed: true,
            } => SqlType::SmallInt,
            Kind::Int {
                bits: 32,
                signed: true,
            } => SqlType::Integer,
            Kind::Int { signed: true, .. } => SqlType::BigInt,
            Kind::Int { signed: false, .. } => {
                return Err(format!("unsigned integer columns ({})", k.sql_name()));
            }
            Kind::Float32 => SqlType::Float,
            Kind::Float64 => SqlType::Double,
            Kind::Decimal { precision, scale } => SqlType::Decimal { precision, scale },
            Kind::Date => SqlType::Date,
            Kind::Timestamp(_) => return Err("TIMESTAMP columns".into()),
            Kind::Utf8 => SqlType::Varchar,
            Kind::Binary => return Err("BLOB (binary) columns".into()),
        })
    }

    pub fn is_integer(self) -> bool {
        self.int_rank().is_some()
    }

    pub fn is_decimal(self) -> bool {
        matches!(self, SqlType::Decimal { .. })
    }

    /// An integer or a decimal: the types that mix (decimal-rules section 4).
    pub fn is_exact_numeric(self) -> bool {
        self.is_integer() || self.is_decimal()
    }

    pub fn is_float(self) -> bool {
        matches!(self, SqlType::Float | SqlType::Double)
    }

    /// 1 to 4 for TINYINT to BIGINT.
    pub fn int_rank(self) -> Option<u8> {
        match self {
            SqlType::TinyInt => Some(1),
            SqlType::SmallInt => Some(2),
            SqlType::Integer => Some(3),
            SqlType::BigInt => Some(4),
            _ => None,
        }
    }

    pub fn int_of_rank(r: u8) -> SqlType {
        match r {
            1 => SqlType::TinyInt,
            2 => SqlType::SmallInt,
            3 => SqlType::Integer,
            _ => SqlType::BigInt,
        }
    }

    /// The largest value of an integer type.
    pub fn int_max(self) -> Option<i64> {
        match self {
            SqlType::TinyInt => Some(i64::from(i8::MAX)),
            SqlType::SmallInt => Some(i64::from(i16::MAX)),
            SqlType::Integer => Some(i64::from(i32::MAX)),
            SqlType::BigInt => Some(i64::MAX),
            _ => None,
        }
    }

    pub fn int_min(self) -> Option<i64> {
        self.int_max().map(|m| -m - 1)
    }

    /// (precision, scale): an integer is an exact decimal when it takes part in arithmetic WITH a decimal (decimal-rules section 2).
    pub fn as_decimal(self) -> Option<(u8, u8)> {
        match self {
            SqlType::TinyInt => Some((3, 0)),
            SqlType::SmallInt => Some((5, 0)),
            SqlType::Integer => Some((10, 0)),
            SqlType::BigInt => Some((19, 0)),
            SqlType::Decimal { precision, scale } => Some((precision, scale)),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    /// `//`, integers only.
    IntDiv,
}

impl ArithOp {
    pub fn symbol(self) -> &'static str {
        match self {
            ArithOp::Add => "+",
            ArithOp::Sub => "-",
            ArithOp::Mul => "*",
            ArithOp::Div => "/",
            ArithOp::Mod => "%",
            ArithOp::IntDiv => "//",
        }
    }

    fn verb(self) -> &'static str {
        match self {
            ArithOp::Add => "add",
            ArithOp::Sub => "subtract",
            ArithOp::Mul => "multiply",
            ArithOp::Div => "divide",
            ArithOp::Mod => "take the remainder of",
            ArithOp::IntDiv => "integer-divide",
        }
    }
}

const MAX_DIGITS: u32 = 38;

fn dec(p: u32, s: u32) -> SqlType {
    SqlType::Decimal {
        precision: u8::try_from(p).unwrap_or(38),
        scale: u8::try_from(s).unwrap_or(38),
    }
}

/// Result type of a decimal operation, `decimal-rules.md` section 3. `(p1, s1)` and `(p2, s2)` are the operand types (integers as (p, 0)).
fn decimal_result(op: ArithOp, (p1, s1): (u8, u8), (p2, s2): (u8, u8)) -> TResult<SqlType> {
    let (p1, s1, p2, s2) = (u32::from(p1), u32::from(s1), u32::from(p2), u32::from(s2));
    let (a, b) = (
        SqlType::Decimal {
            precision: u8::try_from(p1).unwrap_or(38),
            scale: u8::try_from(s1).unwrap_or(38),
        },
        SqlType::Decimal {
            precision: u8::try_from(p2).unwrap_or(38),
            scale: u8::try_from(s2).unwrap_or(38),
        },
    );
    match op {
        ArithOp::Add | ArithOp::Sub => {
            let s = s1.max(s2);
            let p = (p1 - s1).max(p2 - s2) + s + 1;
            Ok(dec(p.min(MAX_DIGITS), s))
        }
        ArithOp::Mul => {
            let s = s1 + s2;
            if s > MAX_DIGITS {
                return bind(format!(
                    "cannot multiply {a} by {b}: the result scale would be {s}, above the maximum of {MAX_DIGITS}. Cast an operand to a smaller scale, for example CAST(x AS DECIMAL(20,10))."
                ));
            }
            let p = if p1 <= 18 && p2 <= 18 && s < 18 {
                (p1 + p2).min(18)
            } else {
                (p1 + p2).min(MAX_DIGITS)
            };
            Ok(dec(p, s))
        }
        ArithOp::Div => {
            let s = (s1 + 6).min(MAX_DIGITS);
            let p = (p1 - s1) + s2 + s;
            if p > MAX_DIGITS {
                return bind(format!(
                    "cannot divide {a} by {b}: the result needs {p} digits, above the maximum of {MAX_DIGITS}. Cast the operands to narrower decimals first."
                ));
            }
            Ok(dec(p, s))
        }
        ArithOp::Mod => {
            let s = s1.max(s2);
            if (p1 - s1).max(p2 - s2) + s > MAX_DIGITS {
                return nsy(format!(
                    "the remainder of {a} and {b}: aligning the operands to scale {s} needs {} digits, above the maximum of {MAX_DIGITS} (cast one operand to a narrower decimal)",
                    (p1 - s1).max(p2 - s2) + s
                ));
            }
            Ok(dec((p1 - s1).min(p2 - s2) + s, s))
        }
        ArithOp::IntDiv => nsy(format!(
            "// on decimals ({a} // {b}): use integers, or / for an exact quotient"
        )),
    }
}

/// The type of `l <op> r` (dialect section 4, decimal-rules sections 3 and 4).
pub fn arith_type(op: ArithOp, l: SqlType, r: SqlType) -> TResult<SqlType> {
    use SqlType::{Boolean, Date, Varchar};
    let (ln, rn) = (l.is_exact_numeric(), r.is_exact_numeric());
    if ln && rn {
        if l.is_integer() && r.is_integer() {
            if op == ArithOp::Div {
                // `/` of two integers is the decimal rule on (p, 0) and (p, 0): 7 / 2 is DECIMAL(16,6)
                let (a, b) = (l.as_decimal(), r.as_decimal());
                if let (Some(a), Some(b)) = (a, b) {
                    return decimal_result(op, a, b);
                }
            }
            let rank = l.int_rank().unwrap_or(4).max(r.int_rank().unwrap_or(4));
            return Ok(SqlType::int_of_rank(rank));
        }
        if let (Some(a), Some(b)) = (l.as_decimal(), r.as_decimal()) {
            return decimal_result(op, a, b);
        }
    }
    let verb = op.verb();
    // DATE: the date-function slice brings intervals
    if (l == Date && (rn || r == Date)) || (r == Date && ln) {
        return nsy(format!(
            "{l} {} {r}: arithmetic on dates (intervals come with the date-function slice)",
            op.symbol()
        ));
    }
    if l.is_float() || r.is_float() {
        let other = if l.is_float() { r } else { l };
        let fl = if l.is_float() { l } else { r };
        if other.is_decimal() {
            return bind(format!(
                "cannot {verb} {l} and {r}: exact and approximate numbers are not mixed implicitly. Use CAST(x AS DOUBLE) for an approximate result or CAST(y AS DECIMAL(p,s)) for an exact one."
            ));
        }
        if other.is_integer() || other.is_float() {
            return nsy(format!(
                "arithmetic on {fl} ({l} {} {r}): not specified yet; compare the values, or leave the arithmetic to decimals and integers",
                op.symbol()
            ));
        }
    }
    let _ = (Boolean, Varchar);
    bind(format!(
        "cannot {verb} {l} and {r}: arithmetic needs integers or decimals"
    ))
}

pub fn neg_type(t: SqlType) -> TResult<SqlType> {
    if t.is_exact_numeric() {
        Ok(t)
    } else if t.is_float() {
        nsy(format!(
            "unary minus on {t}: arithmetic on floating point is not specified yet"
        ))
    } else {
        bind(format!(
            "cannot negate {t}: unary minus needs an integer or a decimal"
        ))
    }
}

/// May `l` be compared with `r`? (dialect section 5): the same type, or integers and decimals; never strings with numbers or dates; floats not with decimals.
pub fn check_comparable(l: SqlType, r: SqlType) -> TResult<()> {
    if l == r || (l.is_exact_numeric() && r.is_exact_numeric()) {
        return Ok(());
    }
    if (l.is_float() && r.is_decimal()) || (l.is_decimal() && r.is_float()) {
        return bind(format!(
            "cannot compare {l} and {r}: exact and approximate numbers are not mixed implicitly. Use CAST(x AS DOUBLE) for an approximate comparison or CAST(y AS DECIMAL(p,s)) for an exact one."
        ));
    }
    if (l.is_float() && r.is_integer()) || (l.is_integer() && r.is_float()) {
        return nsy(format!(
            "comparing {l} with {r}: not specified yet; write CAST(x AS DOUBLE) on the integer"
        ));
    }
    if l.is_float() && r.is_float() {
        return bind(format!(
            "cannot compare {l} and {r}: the two sides must have the same type. Write CAST(x AS DOUBLE) on the {l} side."
        ));
    }
    bind(format!(
        "cannot compare {l} and {r}: there is no implicit conversion between strings, numbers, dates and booleans. Use CAST(x AS type) on one side."
    ))
}

/// The common type of the results of a CASE (dialect section 6): one type, or integers and decimals widened without exceeding 38 digits.
pub fn unify_results(types: &[SqlType]) -> TResult<SqlType> {
    let mut it = types.iter().copied().filter(|t| *t != SqlType::Null);
    let Some(first) = it.next() else {
        return Ok(SqlType::Null);
    };
    let rest: Vec<SqlType> = it.collect();
    if rest.iter().all(|t| *t == first) {
        return Ok(first);
    }
    let all: Vec<SqlType> = std::iter::once(first).chain(rest).collect();
    if all.iter().all(|t| t.is_exact_numeric()) {
        if all.iter().all(|t| t.is_integer()) {
            let rank = all.iter().filter_map(|t| t.int_rank()).max().unwrap_or(4);
            return Ok(SqlType::int_of_rank(rank));
        }
        // scale max(s_i), integer digits max(p_i - s_i), precision their sum; an error above 38, never a silent rounding
        let mut scale = 0u32;
        let mut int_digits = 0u32;
        for t in &all {
            if let Some((p, s)) = t.as_decimal() {
                scale = scale.max(u32::from(s));
                int_digits = int_digits.max(u32::from(p) - u32::from(s));
            }
        }
        if int_digits + scale > MAX_DIGITS {
            let names: Vec<String> = all.iter().map(ToString::to_string).collect();
            return bind(format!(
                "the CASE results ({}) have no common decimal type within {MAX_DIGITS} digits: it would need {} digits. Cast the results to a common type, for example CAST(x AS DECIMAL(20,10)).",
                names.join(", "),
                int_digits + scale
            ));
        }
        return Ok(dec(int_digits + scale, scale));
    }
    let a = all[0];
    let b = all.iter().find(|t| **t != a).copied().unwrap_or(a);
    if (a.is_float() && b.is_decimal()) || (a.is_decimal() && b.is_float()) {
        return bind(format!(
            "the CASE results have the types {a} and {b}: exact and approximate numbers are not mixed implicitly. Use CAST to make them one type."
        ));
    }
    bind(format!(
        "the CASE results have different types ({a} and {b}): all results must have one type. Use CAST(x AS type) on one of them."
    ))
}

/// May a value of type `from` be cast to `to`? (dialect section 6, decimal-rules section 6.) A cast that is meaningless is a bind error; one that is
/// meaningful but whose result the dialect does not describe is `not supported yet`.
pub fn check_cast(from: SqlType, to: SqlType) -> TResult<()> {
    use SqlType::{Boolean, Date, Varchar};
    if from == to {
        return Ok(());
    }
    let num = |t: SqlType| t.is_exact_numeric() || t == SqlType::Double || t == SqlType::Float;
    match (from, to) {
        (f, t) if num(f) && num(t) => Ok(()),
        (Varchar, t) if num(t) || t == Boolean || t == Date => Ok(()),
        (Date, Varchar) | (Boolean, Varchar) => Ok(()),
        (f, Varchar) if f.is_exact_numeric() => Ok(()),
        (f, Varchar) if f.is_float() => nsy(format!(
            "CAST({f} AS VARCHAR): the text form of floating point numbers is not specified yet"
        )),
        (Boolean, t) | (t, Boolean) if num(t) => nsy(format!(
            "CAST({from} AS {to}): conversions between booleans and numbers are not specified yet"
        )),
        _ => bind(format!("cannot cast {from} to {to}")),
    }
}
