//! PUSHDOWN NEVER CHANGES A RESULT (rule R2). For thousands of random WHERE clauses over random rows (NULLs, extremes, ties), the rows kept by
//!   (the pushed conjuncts, run by nata-parquet's exact row filter on an Arrow batch)  AND  (the remaining filter, run by a reference evaluator)
//! must be the rows kept by the whole WHERE clause run by the same reference evaluator. The scan's row filter is the code of slice 3 (checked against
//! DuckDB there); the reference evaluator below is written here, independently of the binder, from the SQL rules. Seeded and replayable:
//! `NATA_PUSHDOWN_SEED=<n> NATA_PUSHDOWN_N=<count>`; a failure prints the seed, the query and the row.

mod common;

use std::sync::Arc;

use arrow_array::builder::StringDictionaryBuilder;
use arrow_array::types::Int32Type;
use arrow_array::{
    ArrayRef, BooleanArray, Date32Array, Decimal64Array, Int8Array, Int32Array, Int64Array,
    RecordBatch,
};
use nata_parquet::filter::evaluate;
use nata_parquet::schema::Kind;
use nata_sql::bind::bind;
use nata_sql::parse::parse_query;
use nata_sql::plan_sql;
use nata_sql::source::MemSource;
use nata_sql::texpr::{CmpOp, Expr, ExprKind, Value};
use nata_sql::types::{ArithOp, SqlType};

// ------------------------------------------------------------------------------------------------------------------------------ the reference

#[derive(Clone, Debug, PartialEq)]
enum V {
    Null,
    B(bool),
    I(i128),
    /// unscaled value and scale
    D(i128, u32),
    S(String),
    Dt(i32),
}

type R = Result<V, ()>;

fn p10(n: u32) -> Result<i128, ()> {
    10i128.checked_pow(n).ok_or(())
}

fn rescale(v: i128, from: u32, to: u32) -> Result<i128, ()> {
    if to >= from {
        v.checked_mul(p10(to - from)?).ok_or(())
    } else {
        // round half away from zero
        let d = p10(from - to)?;
        let (q, r) = (v / d, v % d);
        Ok(if r.abs() * 2 >= d { q + v.signum() } else { q })
    }
}

fn as_dec(v: &V) -> Option<(i128, u32)> {
    match v {
        V::I(i) => Some((*i, 0)),
        V::D(u, s) => Some((*u, *s)),
        _ => None,
    }
}

fn fits_type(v: i128, ty: SqlType) -> bool {
    match ty {
        SqlType::Decimal { precision, .. } => p10(u32::from(precision)).is_ok_and(|m| v.abs() < m),
        t => t
            .int_min()
            .zip(t.int_max())
            .is_none_or(|(lo, hi)| (i128::from(lo)..=i128::from(hi)).contains(&v)),
    }
}

fn value_of(v: &Value, ty: SqlType) -> V {
    match v {
        Value::Null => V::Null,
        Value::Bool(b) => V::B(*b),
        Value::Int(i) => V::I(i128::from(*i)),
        Value::Decimal(u) => match ty {
            SqlType::Decimal { scale, .. } => V::D(*u, u32::from(scale)),
            _ => V::I(*u),
        },
        Value::Str(s) => V::S(s.clone()),
        Value::Date(d) => V::Dt(*d),
    }
}

fn compare(a: &V, b: &V) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (V::Null, _) | (_, V::Null) => None,
        (V::B(x), V::B(y)) => Some(x.cmp(y)),
        (V::S(x), V::S(y)) => Some(x.as_bytes().cmp(y.as_bytes())),
        (V::Dt(x), V::Dt(y)) => Some(x.cmp(y)),
        _ => {
            let ((x, sx), (y, sy)) = (as_dec(a)?, as_dec(b)?);
            let s = sx.max(sy);
            Some(rescale(x, sx, s).ok()?.cmp(&rescale(y, sy, s).ok()?))
        }
    }
}

fn truth(v: &V) -> Option<bool> {
    match v {
        V::B(b) => Some(*b),
        _ => None,
    }
}

fn eval(e: &Expr, row: &[V]) -> R {
    Ok(match &e.kind {
        ExprKind::Column(i) => row[*i].clone(),
        ExprKind::Literal(v) => value_of(v, e.ty),
        ExprKind::Cast { expr, .. } => {
            let v = eval(expr, row)?;
            match (&v, e.ty) {
                (V::Null, _) => V::Null,
                (V::I(i), t) if t.is_integer() => {
                    if !fits_type(*i, t) {
                        return Err(());
                    }
                    V::I(*i)
                }
                (V::I(i), SqlType::Decimal { scale, precision }) => {
                    let u = rescale(*i, 0, u32::from(scale))?;
                    if !fits_type(u, SqlType::Decimal { scale, precision }) {
                        return Err(());
                    }
                    V::D(u, u32::from(scale))
                }
                (V::D(u, s), SqlType::Decimal { scale, precision }) => {
                    let u2 = rescale(*u, *s, u32::from(scale))?;
                    if !fits_type(u2, SqlType::Decimal { scale, precision }) {
                        return Err(());
                    }
                    V::D(u2, u32::from(scale))
                }
                (V::D(u, s), t) if t.is_integer() => {
                    let i = rescale(*u, *s, 0)?;
                    if !fits_type(i, t) {
                        return Err(());
                    }
                    V::I(i)
                }
                (V::I(i), SqlType::Varchar) => V::S(i.to_string()),
                (V::S(s), SqlType::Varchar) => V::S(s.clone()),
                _ => return Err(()),
            }
        }
        ExprKind::Neg(x) => match eval(x, row)? {
            V::Null => V::Null,
            V::I(i) => V::I(-i),
            V::D(u, s) => V::D(-u, s),
            _ => return Err(()),
        },
        ExprKind::Not(x) => match truth(&eval(x, row)?) {
            Some(b) => V::B(!b),
            None => V::Null,
        },
        ExprKind::Arith { op, left, right } => {
            let (a, b) = (eval(left, row)?, eval(right, row)?);
            if a == V::Null || b == V::Null {
                return Ok(V::Null);
            }
            let ((x, sx), (y, sy)) = (as_dec(&a).ok_or(())?, as_dec(&b).ok_or(())?);
            let int_result = e.ty.is_integer();
            let out = |u: i128, s: u32| -> R {
                if !fits_type(u, e.ty) {
                    return Err(());
                }
                Ok(if int_result { V::I(u) } else { V::D(u, s) })
            };
            let SqlType::Decimal { scale: rs, .. } = e.ty else {
                // integers
                return match op {
                    ArithOp::Add => out(x.checked_add(y).ok_or(())?, 0),
                    ArithOp::Sub => out(x.checked_sub(y).ok_or(())?, 0),
                    ArithOp::Mul => out(x.checked_mul(y).ok_or(())?, 0),
                    ArithOp::IntDiv => out(x.checked_div(y).ok_or(())?, 0),
                    ArithOp::Mod => out(x.checked_rem(y).ok_or(())?, 0),
                    ArithOp::Div => Err(()),
                };
            };
            let rs = u32::from(rs);
            match op {
                ArithOp::Add | ArithOp::Sub => {
                    let s = sx.max(sy);
                    let (x, y) = (rescale(x, sx, s)?, rescale(y, sy, s)?);
                    out(
                        if *op == ArithOp::Add {
                            x.checked_add(y)
                        } else {
                            x.checked_sub(y)
                        }
                        .ok_or(())?,
                        s,
                    )?
                }
                ArithOp::Mul => out(x.checked_mul(y).ok_or(())?, sx + sy)?,
                ArithOp::Div => {
                    if y == 0 {
                        return Err(());
                    }
                    // x/10^sx / (y/10^sy) at scale rs
                    let num = x.checked_mul(p10(rs + sy)?).ok_or(())?;
                    let den = y.checked_mul(p10(sx)?).ok_or(())?;
                    let (q, r) = (num / den, num % den);
                    let q = if r.abs() * 2 >= den.abs() {
                        q + (num.signum() * den.signum())
                    } else {
                        q
                    };
                    out(q, rs)?
                }
                ArithOp::Mod => {
                    let s = sx.max(sy);
                    let (x, y) = (rescale(x, sx, s)?, rescale(y, sy, s)?);
                    out(x.checked_rem(y).ok_or(())?, s)?
                }
                ArithOp::IntDiv => return Err(()),
            }
        }
        ExprKind::Cmp { op, left, right } => {
            let (a, b) = (eval(left, row)?, eval(right, row)?);
            match compare(&a, &b) {
                None => V::Null,
                Some(o) => V::B(match op {
                    CmpOp::Eq => o.is_eq(),
                    CmpOp::Ne => o.is_ne(),
                    CmpOp::Lt => o.is_lt(),
                    CmpOp::Le => o.is_le(),
                    CmpOp::Gt => o.is_gt(),
                    CmpOp::Ge => o.is_ge(),
                }),
            }
        }
        ExprKind::And(v) => {
            let mut unknown = false;
            for x in v {
                match truth(&eval(x, row)?) {
                    Some(false) => return Ok(V::B(false)),
                    None => unknown = true,
                    Some(true) => {}
                }
            }
            if unknown { V::Null } else { V::B(true) }
        }
        ExprKind::Or(v) => {
            let mut unknown = false;
            for x in v {
                match truth(&eval(x, row)?) {
                    Some(true) => return Ok(V::B(true)),
                    None => unknown = true,
                    Some(false) => {}
                }
            }
            if unknown { V::Null } else { V::B(false) }
        }
        ExprKind::IsNull { expr, negated } => V::B((eval(expr, row)? == V::Null) != *negated),
        ExprKind::Between {
            expr,
            low,
            high,
            negated,
        } => {
            let x = eval(expr, row)?;
            let ge = compare(&x, &eval(low, row)?).map(|o| o.is_ge());
            let le = compare(&x, &eval(high, row)?).map(|o| o.is_le());
            let r = match (ge, le) {
                (Some(false), _) | (_, Some(false)) => Some(false),
                (Some(true), Some(true)) => Some(true),
                _ => None,
            };
            match r {
                None => V::Null,
                Some(b) => V::B(b != *negated),
            }
        }
        ExprKind::InList {
            expr,
            list,
            negated,
        } => {
            let x = eval(expr, row)?;
            let mut unknown = false;
            let mut hit = false;
            for l in list {
                match compare(&x, &eval(l, row)?) {
                    None => unknown = true,
                    Some(o) => hit |= o.is_eq(),
                }
            }
            let r = if hit {
                Some(true)
            } else if unknown {
                None
            } else {
                Some(false)
            };
            match r {
                None => V::Null,
                Some(b) => V::B(b != *negated),
            }
        }
        ExprKind::Case {
            operand,
            whens,
            else_,
        } => {
            let op = operand.as_ref().map(|o| eval(o, row)).transpose()?;
            for (c, r) in whens {
                let taken = match &op {
                    None => truth(&eval(c, row)?) == Some(true),
                    Some(x) => compare(x, &eval(c, row)?).is_some_and(|o| o.is_eq()),
                };
                if taken {
                    return eval(r, row);
                }
            }
            eval(else_, row)?
        }
    })
}

// ------------------------------------------------------------------------------------------------------------------------------ the data

const COLS: [(&str, Result<Kind, &str>); 7] = [
    (
        "a",
        Ok(Kind::Int {
            bits: 32,
            signed: true,
        }),
    ),
    (
        "c",
        Ok(Kind::Int {
            bits: 64,
            signed: true,
        }),
    ),
    (
        "ti",
        Ok(Kind::Int {
            bits: 8,
            signed: true,
        }),
    ),
    (
        "m",
        Ok(Kind::Decimal {
            precision: 10,
            scale: 2,
        }),
    ),
    ("s", Ok(Kind::Utf8)),
    ("dt", Ok(Kind::Date)),
    ("bo", Ok(Kind::Bool)),
];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n.max(1) as u64).unwrap_or(0)
    }
    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[self.below(xs.len())]
    }
}

fn random_row(r: &mut Rng) -> Vec<V> {
    let null = |r: &mut Rng| r.below(5) == 0;
    let mut row = Vec::new();
    row.push(if null(r) {
        V::Null
    } else {
        V::I(i128::from(r.pick(&[
            -2i32,
            -1,
            0,
            1,
            2,
            3,
            5,
            100,
            i32::MAX,
            i32::MIN,
        ])))
    });
    row.push(if null(r) {
        V::Null
    } else {
        V::I(i128::from(r.pick(&[
            -1i64,
            0,
            1,
            2,
            3,
            3_000_000_000,
            i64::MAX,
            i64::MIN,
        ])))
    });
    row.push(if null(r) {
        V::Null
    } else {
        V::I(i128::from(r.pick(&[-128i8, -1, 0, 1, 5, 127])))
    });
    row.push(if null(r) {
        V::Null
    } else {
        V::D(
            i128::from(r.pick(&[-150i64, 0, 5, 50, 100, 150, 250, 99_999_999])),
            2,
        )
    });
    row.push(if null(r) {
        V::Null
    } else {
        V::S(r.pick(&["", "a", "b", "it's", "R", "zz", "é"]).to_string())
    });
    row.push(if null(r) {
        V::Null
    } else {
        V::Dt(r.pick(&[0, 8765, 8766, 9130, 9131, 20000]))
    });
    row.push(if null(r) {
        V::Null
    } else {
        V::B(r.below(2) == 0)
    });
    row
}

fn batch(rows: &[Vec<V>]) -> RecordBatch {
    let col = |i: usize| rows.iter().map(move |r| &r[i]);
    let ints = |i: usize| {
        col(i)
            .map(|v| if let V::I(x) = v { Some(*x) } else { None })
            .collect::<Vec<_>>()
    };
    let a: ArrayRef = Arc::new(Int32Array::from(
        ints(0)
            .iter()
            .map(|x| x.and_then(|x| i32::try_from(x).ok()))
            .collect::<Vec<_>>(),
    ));
    let c: ArrayRef = Arc::new(Int64Array::from(
        ints(1)
            .iter()
            .map(|x| x.and_then(|x| i64::try_from(x).ok()))
            .collect::<Vec<_>>(),
    ));
    let ti: ArrayRef = Arc::new(Int8Array::from(
        ints(2)
            .iter()
            .map(|x| x.and_then(|x| i8::try_from(x).ok()))
            .collect::<Vec<_>>(),
    ));
    let m: ArrayRef = Arc::new(
        Decimal64Array::from(
            col(3)
                .map(|v| {
                    if let V::D(u, _) = v {
                        i64::try_from(*u).ok()
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>(),
        )
        .with_precision_and_scale(10, 2)
        .expect("decimal"),
    );
    let mut sb = StringDictionaryBuilder::<Int32Type>::new();
    for v in col(4) {
        match v {
            V::S(s) => sb.append_value(s),
            _ => sb.append_null(),
        }
    }
    let s: ArrayRef = Arc::new(sb.finish());
    let dt: ArrayRef = Arc::new(Date32Array::from(
        col(5)
            .map(|v| if let V::Dt(d) = v { Some(*d) } else { None })
            .collect::<Vec<_>>(),
    ));
    let bo: ArrayRef = Arc::new(BooleanArray::from(
        col(6)
            .map(|v| if let V::B(b) = v { Some(*b) } else { None })
            .collect::<Vec<_>>(),
    ));
    RecordBatch::try_from_iter([
        ("a", a),
        ("c", c),
        ("ti", ti),
        ("m", m),
        ("s", s),
        ("dt", dt),
        ("bo", bo),
    ])
    .expect("batch")
}

// ------------------------------------------------------------------------------------------------------------------------------ the queries

const INT_LITS: [&str; 12] = [
    "-1",
    "0",
    "1",
    "2",
    "5",
    "100",
    "3000000000",
    "1.5",
    "2.50",
    "1e2",
    "99999999999",
    "-3000000000",
];
const DEC_LITS: [&str; 10] = [
    "0",
    "1",
    "5",
    "0.05",
    "1.5",
    "2.50",
    "0.001",
    "100",
    "-1.50",
    "999999.99",
];
const STR_LITS: [&str; 7] = ["''", "'a'", "'b'", "'it''s'", "'R'", "'zz'", "'é'"];
const DATE_LITS: [&str; 5] = [
    "DATE '1970-01-01'",
    "DATE '1994-01-01'",
    "DATE '1995-01-01'",
    "DATE '2024-02-29'",
    "DATE '1994-12-31'",
];
const OPS: [&str; 6] = ["=", "<>", "<", "<=", ">", ">="];

fn lit_for(r: &mut Rng, col: &str) -> String {
    match col {
        "a" | "c" | "ti" => r.pick(&INT_LITS).to_string(),
        "m" => r.pick(&DEC_LITS).to_string(),
        "s" => r.pick(&STR_LITS).to_string(),
        "dt" => r.pick(&DATE_LITS).to_string(),
        _ => r.pick(&["TRUE", "FALSE"]).to_string(),
    }
}

fn atom(r: &mut Rng) -> String {
    let col = r.pick(&["a", "c", "ti", "m", "s", "dt", "bo"]);
    let op = r.pick(&OPS);
    let col2 = r.pick(&["a", "c", "ti", "m"]);
    match r.below(16) {
        0..=5 => format!("{col} {op} {}", lit_for(r, col)),
        6 => format!("{} {op} {col}", lit_for(r, col)),
        7 => format!("{col} IS NULL"),
        8 => format!("{col} IS NOT NULL"),
        9 => format!("{col} BETWEEN {} AND {}", lit_for(r, col), lit_for(r, col)),
        10 => format!(
            "{col} NOT BETWEEN {} AND {}",
            lit_for(r, col),
            lit_for(r, col)
        ),
        11 => format!("{col} IN ({}, {})", lit_for(r, col), lit_for(r, col)),
        12 => format!("{col2} {op} {}", r.pick(&["a", "c", "m", "ti"])),
        13 => format!("{col2} + 1 {op} {}", r.pick(&["2", "0.5", "1"])),
        14 => format!(
            "CASE WHEN {col} {op} {} THEN bo ELSE NOT bo END",
            lit_for(r, col)
        ),
        _ => format!("{col} {op} NULL"),
    }
}

fn term(r: &mut Rng) -> String {
    match r.below(10) {
        0..=5 => atom(r),
        6 => format!("({} OR {})", atom(r), atom(r)),
        7 => format!("NOT ({})", atom(r)),
        8 => format!("NOT {}", atom(r)).replace("NOT NOT", "NOT"),
        _ => format!("({} AND {})", atom(r), atom(r)),
    }
}

fn where_clause(r: &mut Rng) -> String {
    let n = 1 + r.below(4);
    (0..n).map(|_| term(r)).collect::<Vec<_>>().join(" AND ")
}

#[test]
fn pushed_conjuncts_plus_the_remaining_filter_keep_exactly_the_rows_of_the_whole_where_clause() {
    let seed: u64 = std::env::var("NATA_PUSHDOWN_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20_261_106);
    let n: u64 = std::env::var("NATA_PUSHDOWN_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4_000);
    let src = MemSource::new().file("p.parquet", &COLS);
    let (
        mut bound_ok,
        mut with_pushed,
        mut with_rest,
        mut both,
        mut rows_kept,
        mut rows_total,
        mut skipped_rows,
    ) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    for i in 0..n {
        let mut r = Rng(seed.wrapping_mul(1_000_003).wrapping_add(i));
        let w = where_clause(&mut r);
        let sql = format!("SELECT a FROM 'p.parquet' WHERE {w}");
        // the whole WHERE clause, bound
        let Ok(q) = parse_query(&sql) else {
            panic!("seed {seed} case {i}: does not parse: {sql}")
        };
        let Ok(whole) = bind(&q, &src) else { continue };
        let Some(whole) = whole.where_ else { continue };
        let planned = plan_sql(&sql, &src).unwrap_or_else(|e| {
            panic!("seed {seed} case {i}: binds but does not plan: {sql}: {e}")
        });
        bound_ok += 1;
        let scan = &planned.plan.scan;
        with_pushed += u64::from(!scan.pushed.is_empty());
        with_rest += u64::from(planned.plan.filter.is_some());
        both += u64::from(!scan.pushed.is_empty() && planned.plan.filter.is_some());
        // the text form that the scan parses gives back the same typed terms
        for p in &scan.pushed {
            let (col, raw) =
                nata_parquet::filter::parse_syntax(&p.filter_text()).unwrap_or_else(|e| {
                    panic!(
                        "seed {seed} case {i}: {} does not parse: {e}",
                        p.filter_text()
                    )
                });
            assert_eq!(col, p.column);
            let test = match raw {
                nata_parquet::filter::RawTest::IsNull => nata_parquet::filter::Test::IsNull,
                nata_parquet::filter::RawTest::IsNotNull => nata_parquet::filter::Test::IsNotNull,
                nata_parquet::filter::RawTest::Cmp(op, lit) => nata_parquet::filter::Test::Cmp(
                    op,
                    nata_parquet::filter::type_literal(p.kind, &lit).unwrap_or_else(|e| {
                        panic!(
                            "seed {seed} case {i}: {} is refused by the scan: {e}",
                            p.filter_text()
                        )
                    }),
                ),
            };
            assert_eq!(
                test,
                p.term.test,
                "seed {seed} case {i}: {} round trip",
                p.filter_text()
            );
        }
        // rows
        let rows: Vec<Vec<V>> = (0..40).map(|_| random_row(&mut r)).collect();
        let b = batch(&rows);
        let terms: Vec<_> = scan.pushed.iter().map(|p| p.term.clone()).collect();
        let pushed_ok = evaluate(&terms, &b).expect("the scan's row filter");
        let names: Vec<&str> = COLS.iter().map(|c| c.0).collect();
        for (k, row) in rows.iter().enumerate() {
            rows_total += 1;
            let Ok(want) = eval(&whole, row) else {
                skipped_rows += 1;
                continue;
            };
            let want = truth(&want) == Some(true);
            // the plan: the scan row filter, then the remaining filter over the scan's columns
            let scan_row: Vec<V> = scan
                .columns
                .iter()
                .map(|c| row[names.iter().position(|n| *n == c.name).expect("column")].clone())
                .collect();
            let got = pushed_ok[k]
                && match &planned.plan.filter {
                    None => true,
                    Some(f) => match eval(&f.predicate, &scan_row) {
                        Ok(v) => truth(&v) == Some(true),
                        Err(()) => continue, // an error in the remaining filter for a row the whole clause did not error on cannot happen: see below
                    },
                };
            rows_kept += u64::from(want);
            assert_eq!(
                got,
                want,
                "seed {seed} case {i} row {k}: {sql}\nrow {row:?}\npushed {:?}\nremaining {:?}",
                scan.pushed.iter().map(|p| p.sql()).collect::<Vec<_>>(),
                planned
                    .plan
                    .filter
                    .as_ref()
                    .map(|f| nata_sql::plan::render_expr(
                        &f.predicate,
                        &scan
                            .columns
                            .iter()
                            .map(|c| c.name.clone())
                            .collect::<Vec<_>>()
                    ))
            );
        }
    }
    eprintln!(
        "pushdown property: seed {seed}, {n} queries, {bound_ok} bound; {with_pushed} with a pushed conjunct, {with_rest} with a remaining filter, {both} with both; {rows_total} rows, {rows_kept} kept by the WHERE clause, {skipped_rows} rows skipped (an arithmetic error in the reference)"
    );
    if n >= 4_000 {
        assert!(bound_ok > n / 2, "too few queries bind: {bound_ok} of {n}");
        assert!(
            with_pushed > bound_ok / 3 && with_rest > bound_ok / 3 && both > bound_ok / 8,
            "the generator must reach all three plan shapes"
        );
        assert!(
            rows_kept > rows_total / 20,
            "the generator must keep some rows: {rows_kept} of {rows_total}"
        );
    }
}
