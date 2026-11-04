//! Filter expressions and their three evaluators:
//!  * `eval_row`: the ROW-LEVEL meaning (the oracle): Kleene three-valued logic, floats in either of two semantics (see `FloatSem`);
//!  * `might_match_bounds`: the INCLUSIVE METRICS evaluator on a column's bounds and counts (a file, or a manifest's partition summary);
//!  * `project`: the INCLUSIVE PROJECTION of a filter onto a partition spec.
//! A planner is sound if whenever a row matches by `eval_row`, the file holding it is not pruned by the other two.
use crate::meta::{Schema, Spec};
use crate::transform::{Op, Transform, holds};
use crate::types::*;

#[derive(Clone, Debug)]
pub enum P { Cmp(Op, Value), Ne(Value), In(Vec<Value>), NotIn(Vec<Value>), IsNull, NotNull, IsNaN, NotNaN, NotStartsWith(Value) }

#[derive(Clone, Debug)]
pub enum Expr { True, False, And(Box<Expr>, Box<Expr>), Or(Box<Expr>, Box<Expr>), Not(Box<Expr>), Pred(String, P) }

pub fn and(a: Expr, b: Expr) -> Expr { Expr::And(Box::new(a), Box::new(b)) }
pub fn or(a: Expr, b: Expr) -> Expr { Expr::Or(Box::new(a), Box::new(b)) }
pub fn not(a: Expr) -> Expr { Expr::Not(Box::new(a)) }
pub fn pred(c: &str, p: P) -> Expr { Expr::Pred(c.to_string(), p) }
pub fn cmp(c: &str, op: Op, v: Value) -> Expr { pred(c, P::Cmp(op, v)) }

/// `Total`: NaN equals itself and is above everything (Java, DuckDB). `Ieee`: every comparison with NaN is false except `!=`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FloatSem { Total, Ieee }

fn leaf(p: &P, v: &Option<Value>, sem: FloatSem) -> Option<bool> {
    // SQL: a comparison with NULL is unknown; IS NULL and IS NOT NULL are never unknown
    match p {
        P::IsNull => return Some(v.is_none()),
        P::NotNull => return Some(v.is_some()),
        _ => {}
    }
    let v = v.as_ref()?;
    let nan = is_nan(v);
    Some(match p {
        P::IsNaN => nan,
        P::NotNaN => !nan,
        P::Cmp(op, lit) => { if sem == FloatSem::Ieee && (nan || is_nan(lit)) { false } else { holds(v, *op, lit) } }
        P::Ne(lit) => { if sem == FloatSem::Ieee && (nan || is_nan(lit)) { true } else { !holds(v, Op::Eq, lit) } }
        P::In(ls) => ls.iter().any(|l| if sem == FloatSem::Ieee && (nan || is_nan(l)) { false } else { holds(v, Op::Eq, l) }),
        P::NotIn(ls) => !ls.iter().any(|l| if sem == FloatSem::Ieee && (nan || is_nan(l)) { false } else { holds(v, Op::Eq, l) }),
        P::NotStartsWith(l) => !holds(v, Op::StartsWith, l),
        P::IsNull | P::NotNull => unreachable!(),
    })
}

/// Row-level truth with Kleene logic; `get(column)` gives the row's value (None = NULL).
pub fn eval_row(e: &Expr, get: &dyn Fn(&str) -> Option<Value>, sem: FloatSem) -> Option<bool> {
    match e {
        Expr::True => Some(true),
        Expr::False => Some(false),
        Expr::Not(a) => eval_row(a, get, sem).map(|b| !b),
        Expr::And(a, b) => match (eval_row(a, get, sem), eval_row(b, get, sem)) { (Some(false), _) | (_, Some(false)) => Some(false), (Some(true), Some(true)) => Some(true), _ => None },
        Expr::Or(a, b) => match (eval_row(a, get, sem), eval_row(b, get, sem)) { (Some(true), _) | (_, Some(true)) => Some(true), (Some(false), Some(false)) => Some(false), _ => None },
        Expr::Pred(c, p) => leaf(p, &get(c), sem),
    }
}

/// A row matches when the filter is TRUE (unknown filters the row out), under either float semantics (the planner must keep the file for both).
pub fn row_matches_any_sem(e: &Expr, get: &dyn Fn(&str) -> Option<Value>) -> bool {
    eval_row(e, get, FloatSem::Total) == Some(true) || eval_row(e, get, FloatSem::Ieee) == Some(true)
}

/// Push NOT down to the leaves (so that pruning sees only positive structure). Valid for the Total float semantics; rows that differ under Ieee
/// are NaN rows, which the bounds evaluator never prunes for the predicates this can turn into `>`, `>=` or `!=`.
pub fn rewrite_not(e: &Expr) -> Expr {
    fn neg(e: &Expr) -> Expr {
        match e {
            Expr::True => Expr::False, Expr::False => Expr::True, Expr::Not(a) => rewrite_not(a),
            Expr::And(a, b) => or(neg(a), neg(b)), Expr::Or(a, b) => and(neg(a), neg(b)),
            Expr::Pred(c, p) => match p {
                P::Cmp(Op::Eq, v) => pred(c, P::Ne(v.clone())),
                // NOT (x < c) is x >= c for every NaN-free value; a NaN row satisfies it under IEEE semantics (NOT false), so the NaN rows are kept: `OR x IS NaN`
                // (provably false for a column that is not floating point)
                P::Cmp(Op::Lt, v) => or(cmp(c, Op::Ge, v.clone()), pred(c, P::IsNaN)), P::Cmp(Op::Le, v) => or(cmp(c, Op::Gt, v.clone()), pred(c, P::IsNaN)),
                P::Cmp(Op::Gt, v) => or(cmp(c, Op::Le, v.clone()), pred(c, P::IsNaN)), P::Cmp(Op::Ge, v) => or(cmp(c, Op::Lt, v.clone()), pred(c, P::IsNaN)), P::Cmp(Op::StartsWith, v) => pred(c, P::NotStartsWith(v.clone())),
                P::Ne(v) => cmp(c, Op::Eq, v.clone()), P::In(vs) => pred(c, P::NotIn(vs.clone())), P::NotIn(vs) => pred(c, P::In(vs.clone())),
                P::IsNull => pred(c, P::NotNull), P::NotNull => pred(c, P::IsNull), P::IsNaN => pred(c, P::NotNaN), P::NotNaN => pred(c, P::IsNaN),
                P::NotStartsWith(v) => cmp(c, Op::StartsWith, v.clone()),
            },
        }
    }
    match e {
        Expr::Not(a) => neg(a),
        Expr::And(a, b) => and(rewrite_not(a), rewrite_not(b)),
        Expr::Or(a, b) => or(rewrite_not(a), rewrite_not(b)),
        o => o.clone(),
    }
}

// ---------------------------------------------------------------- the bounds evaluator

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Count { Zero, Some_, Unknown }

/// What a manifest says about one column (or one partition field) of a file or of a set of files.
#[derive(Clone, Debug)]
pub struct Bounds { pub lower: Option<Value>, pub upper: Option<Value>, pub nulls: Count, pub nans: Count, pub nulls_only: bool, pub nans_only: bool, pub is_float: bool }

impl Bounds {
    pub fn unknown(is_float: bool) -> Bounds { Bounds { lower: None, upper: None, nulls: Count::Unknown, nans: Count::Unknown, nulls_only: false, nans_only: false, is_float } }
}

/// Could any row described by `b` satisfy the predicate? false only when the bounds PROVE that none can.
pub fn might_match_bounds(p: &P, b: &Bounds) -> bool {
    let no_values = b.nulls_only || b.nans_only;
    let nan_possible = b.is_float && b.nans != Count::Zero && !b.nulls_only;           // a NaN may be present: bounds say nothing about it
    match p {
        P::IsNull => b.nulls != Count::Zero,
        P::NotNull => !b.nulls_only,
        P::IsNaN => b.is_float && b.nans != Count::Zero,
        P::NotNaN => !b.nans_only,
        P::Ne(_) | P::NotIn(_) | P::NotStartsWith(_) => true,
        P::Cmp(op, lit) => {
            if is_nan(lit) {
                // a NaN literal: under Total order NaN is the largest value; under IEEE every comparison but != is false. Keep the file if either could hold.
                return match op { Op::Eq | Op::Ge => b.nans != Count::Zero, Op::Gt => false, Op::Le => !b.nulls_only, Op::Lt => !b.nulls_only && !b.nans_only, Op::StartsWith => false };
            }
            if no_values && !(nan_possible && matches!(op, Op::Gt | Op::Ge)) { return false; }
            match op {
                Op::Lt => b.lower.as_ref().is_none_or(|l| vcmp(l, lit).is_lt()),
                Op::Le => b.lower.as_ref().is_none_or(|l| vcmp(l, lit).is_le()),
                Op::Gt => nan_possible || b.upper.as_ref().is_none_or(|u| vcmp(u, lit).is_gt()),
                Op::Ge => nan_possible || b.upper.as_ref().is_none_or(|u| vcmp(u, lit).is_ge()),
                Op::Eq => b.lower.as_ref().is_none_or(|l| vcmp(l, lit).is_le()) && b.upper.as_ref().is_none_or(|u| vcmp(u, lit).is_ge()),
                Op::StartsWith => starts_with_possible(b, lit),
            }
        }
        P::In(ls) => {
            if no_values { return false; }
            ls.iter().any(|l| if is_nan(l) { b.nans != Count::Zero } else { b.lower.as_ref().is_none_or(|x| vcmp(x, l).is_le()) && b.upper.as_ref().is_none_or(|u| vcmp(u, l).is_ge()) })
        }
    }
}

fn starts_with_possible(b: &Bounds, lit: &Value) -> bool {
    if b.nulls_only { return false; }
    let (lo, hi) = (&b.lower, &b.upper);
    match lit {
        Value::Str(s) => {
            let n = s.len();   // compare as UTF-8 bytes: truncating a bound to n bytes never makes it bigger
            let ok_lo = lo.as_ref().is_none_or(|l| match l { Value::Str(l) => l.as_bytes()[..n.min(l.len())] <= *s.as_bytes(), _ => true });
            let ok_hi = hi.as_ref().is_none_or(|h| match h { Value::Str(h) => h.as_bytes()[..n.min(h.len())] >= *s.as_bytes(), _ => true });
            ok_lo && ok_hi
        }
        Value::Bin(s) => {
            let n = s.len();
            lo.as_ref().is_none_or(|l| match l { Value::Bin(l) => l[..n.min(l.len())] <= **s, _ => true }) && hi.as_ref().is_none_or(|h| match h { Value::Bin(h) => h[..n.min(h.len())] >= **s, _ => true })
        }
        _ => true,
    }
}

/// Evaluate a (not-free) expression against per-column bounds supplied by `bounds(column) -> Option<Bounds>`; None = unknown column = might match.
pub fn might_match(e: &Expr, bounds: &dyn Fn(&str) -> Option<Bounds>) -> bool {
    match e {
        Expr::True => true,
        Expr::False => false,
        Expr::And(a, b) => might_match(a, bounds) && might_match(b, bounds),
        Expr::Or(a, b) => might_match(a, bounds) || might_match(b, bounds),
        Expr::Not(_) => true,      // callers rewrite NOT away first; an unrewritten NOT is never pruned
        Expr::Pred(c, p) => bounds(c).is_none_or(|b| might_match_bounds(p, &b)),
    }
}

// ---------------------------------------------------------------- inclusive projection onto a partition spec

/// A projected filter over the partition tuple: leaves name a partition field by position.
#[derive(Clone, Debug)]
pub enum PProj { True, False, And(Box<PProj>, Box<PProj>), Or(Box<PProj>, Box<PProj>), Leaf(usize, P, Option<u32>) }

pub fn project(e: &Expr, schema: &Schema, spec: &Spec) -> PProj {
    match e {
        Expr::True => PProj::True,
        Expr::False => PProj::False,
        Expr::And(a, b) => PProj::And(Box::new(project(a, schema, spec)), Box::new(project(b, schema, spec))),
        Expr::Or(a, b) => PProj::Or(Box::new(project(a, schema, spec)), Box::new(project(b, schema, spec))),
        Expr::Not(_) => PProj::True,
        Expr::Pred(c, p) => {
            let Some(f) = schema.by_name(c) else { return PProj::True };
            let mut out = PProj::True;
            for (i, pf) in spec.fields.iter().enumerate().filter(|(_, pf)| pf.source_id == f.id) {
                let proj: Option<P> = match p {
                    P::Cmp(op, v) => pf.transform.project(*op, v).map(|(o, v)| P::Cmp(o, v)),
                    P::In(vs) => {
                        let ps: Option<Vec<Value>> = vs.iter().map(|v| pf.transform.project(Op::Eq, v).map(|(_, v)| v)).collect();
                        ps.map(P::In)
                    }
                    P::IsNull => (pf.transform != Transform::Void).then_some(P::IsNull),
                    P::NotNull => None,                 // a non-null source gives a non-null partition value except for void: nothing to prune on
                    P::IsNaN if !matches!(f.ty, Type::Float | Type::Double) => { return PProj::False; }      // a column that is not floating point never holds NaN
                    P::IsNaN if pf.transform == Transform::Identity => Some(P::IsNaN),
                    _ => None,
                };
                if let Some(pp) = proj {
                    let guard = if let Transform::Truncate(w) = pf.transform { Some(w) } else { None };
                    let leaf = PProj::Leaf(i, pp, guard);
                    out = if matches!(out, PProj::True) { leaf } else { PProj::And(Box::new(out), Box::new(leaf)) };
                }
            }
            out
        }
    }
}

/// Evaluate a projected filter on a partition TUPLE.
pub fn tuple_might_match(p: &PProj, tuple: &[Option<Value>]) -> bool {
    match p {
        PProj::True => true,
        PProj::False => false,
        PProj::And(a, b) => tuple_might_match(a, tuple) && tuple_might_match(b, tuple),
        PProj::Or(a, b) => tuple_might_match(a, tuple) || tuple_might_match(b, tuple),
        PProj::Leaf(i, pp, guard) => match tuple.get(*i) {
            None => true,
            // a truncate partition value that is not a multiple of its width is the wrapped result of an overflow: it says nothing about the rows
            Some(Some(v)) if guard.is_some_and(|w| !crate::transform::plausible_truncate_value(v, w)) => true,
            Some(v) => leaf(pp, v, FloatSem::Total).unwrap_or(false) || matches!(pp, P::Cmp(..) | P::In(_)) && v.as_ref().is_some_and(is_nan),
        },
    }
}
