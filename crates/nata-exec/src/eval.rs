//! The evaluator: a typed expression of `nata-sql` over the rows of one Arrow batch.
//!
//! Rules (dialect sections 4 to 6, `decimal-rules.md`, ADR 0003 decision 4):
//! * **Selection.** Every node is evaluated for a SELECTION of rows (indices into the batch). An error (overflow, division by zero, a value that does not
//!   convert) is raised only for a row that is selected AND whose operands are valid (not NULL). `AND` evaluates its next operand only for the rows the
//!   earlier ones left undecided, `OR` likewise, `CASE` evaluates a branch only for the rows that take it, `BETWEEN` its upper bound only for the rows
//!   whose lower bound did not already decide. So `WHERE b <> 0 AND a / b > 1` never divides by zero.
//! * **NULL.** Three-valued logic. Arithmetic and comparisons give NULL for a NULL operand.
//! * **Exact numbers.** Integers are checked against the range of their SQL type; decimals are exact `i128` values checked against the digits of the
//!   result type (`decimal-rules.md` section 3); comparisons across scales are exact (`nata_parquet::filter::cmp_decimal`).
//! * **Floats.** Only comparisons: NaN equals NaN and is above everything, `-0.0` equals `0.0` (`nata_parquet::filter::cmp_float`).
//!
//! The result of `eval` has one entry per row of the batch; rows outside the selection are NULL.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

use arrow_array::RecordBatch;
use nata_parquet::filter::{cmp_decimal, cmp_float};
use nata_sql::Pos;
use nata_sql::texpr::{CmpOp, Expr, ExprKind, Value};
use nata_sql::types::{ArithOp, SqlType};

use crate::cast::cast_col;
use crate::error::{EvalError, R};
use crate::value::{Col, Data, StrData};
use nata_kernels::decimal::{Decimal, Fail as DecFail, Op};

#[cfg(test)]
thread_local! {
    pub(crate) static SELECTION_ROWS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub type ColRef = Rc<Col>;

pub struct Evaluator<'a> {
    batch: &'a RecordBatch,
    types: &'a [SqlType],
    cache: RefCell<Vec<Option<ColRef>>>,
    n: usize,
}

impl<'a> Evaluator<'a> {
    /// `types[i]` is the SQL type of column `i` of `batch` (the plan's scan columns).
    pub fn new(batch: &'a RecordBatch, types: &'a [SqlType]) -> Evaluator<'a> {
        Evaluator {
            batch,
            types,
            cache: RefCell::new(vec![None; types.len()]),
            n: batch.num_rows(),
        }
    }

    pub fn rows(&self) -> usize {
        self.n
    }

    /// Every row of the batch.
    pub fn all_rows(&self) -> Vec<u32> {
        #[cfg(test)]
        SELECTION_ROWS.with(|rows| rows.set(rows.get() + self.n));
        (0..self.n).filter_map(|i| u32::try_from(i).ok()).collect()
    }

    fn column(&self, i: usize) -> R<ColRef> {
        if let Some(Some(c)) = self.cache.borrow().get(i) {
            return Ok(Rc::clone(c));
        }
        let ty = *self
            .types
            .get(i)
            .ok_or_else(|| EvalError::internal("a column index beyond the scan's columns"))?;
        let array = self
            .batch
            .columns()
            .get(i)
            .ok_or_else(|| EvalError::internal("the batch has fewer columns than the plan"))?;
        let col = Rc::new(Col::from_arrow(array, ty)?);
        if let Some(slot) = self.cache.borrow_mut().get_mut(i) {
            *slot = Some(Rc::clone(&col));
        }
        Ok(col)
    }

    /// The rows for which the boolean expression is TRUE (NULL and FALSE rows are dropped): the rows a `WHERE` keeps.
    ///
    /// A `WHERE` needs only the rows that are TRUE, so here (and only here, never in a projection) the operands of a top-level `AND` run in sequence on the
    /// rows that are still TRUE, and the operands of a top-level `OR` on the rows no earlier operand made TRUE: a row whose first conjunct is NULL or FALSE
    /// never reaches the second, so an error of the second is not raised for it (as DuckDB's filters).
    pub fn keep_mask(&self, e: &Expr) -> R<Vec<bool>> {
        let kept = self.filter_rows(e, &self.all_rows())?;
        let mut mask = vec![false; self.n];
        for r in kept {
            mask[r as usize] = true;
        }
        Ok(mask)
    }

    /// The rows of `sel` for which `e` is TRUE, in order.
    fn filter_rows(&self, e: &Expr, sel: &[u32]) -> R<Vec<u32>> {
        match &e.kind {
            ExprKind::And(parts) => {
                let mut rows = sel.to_vec();
                for p in parts {
                    if rows.is_empty() {
                        break;
                    }
                    rows = self.filter_rows(p, &rows)?;
                }
                Ok(rows)
            }
            ExprKind::Or(parts) => {
                let mut taken = vec![false; self.n];
                let mut open = sel.to_vec();
                for p in parts {
                    if open.is_empty() {
                        break;
                    }
                    for r in self.filter_rows(p, &open)? {
                        taken[r as usize] = true;
                    }
                    open.retain(|r| !taken[*r as usize]);
                }
                Ok(sel.iter().copied().filter(|r| taken[*r as usize]).collect())
            }
            ExprKind::Between {
                expr,
                low,
                high,
                negated,
            } => self.filter_between(expr, low, high, *negated, e, sel),
            _ => {
                let c = self.eval(e, sel)?;
                let v = c.bools()?;
                Ok(sel
                    .iter()
                    .copied()
                    .filter(|r| c.valid[*r as usize] && v[*r as usize])
                    .collect())
            }
        }
    }

    /// `x BETWEEN lo AND hi` in a `WHERE` is `x >= lo AND x <= hi` (`NOT BETWEEN`: `x < lo OR x > hi`) with the narrowing of a top-level `AND` / `OR`: the upper
    /// bound is evaluated only for the rows whose lower bound is TRUE (`NOT BETWEEN`: not TRUE).
    #[inline(never)]
    fn filter_between(
        &self,
        expr: &Expr,
        low: &Expr,
        high: &Expr,
        negated: bool,
        e: &Expr,
        sel: &[u32],
    ) -> R<Vec<u32>> {
        let x = self.eval(expr, sel)?;
        let lo = self.eval(low, sel)?;
        let (op1, op2) = if negated {
            (CmpOp::Lt, CmpOp::Gt)
        } else {
            (CmpOp::Ge, CmpOp::Le)
        };
        let first = compare(op1, &x, &lo, sel, e.pos)?;
        let fv = first.bools()?;
        let first_true = |r: &u32| first.valid[*r as usize] && fv[*r as usize];
        let open: Vec<u32> = sel
            .iter()
            .copied()
            .filter(|r| first_true(r) != negated)
            .collect();
        let hi = self.eval(high, &open)?;
        let second = compare(op2, &x, &hi, &open, e.pos)?;
        let sv = second.bools()?;
        let second_true = |r: &u32| second.valid[*r as usize] && sv[*r as usize];
        Ok(sel
            .iter()
            .copied()
            .filter(|r| {
                if negated {
                    first_true(r) || second_true(r)
                } else {
                    first_true(r) && second_true(r)
                }
            })
            .collect())
    }

    /// Evaluate `e` for the rows `sel`.
    pub fn eval(&self, e: &Expr, sel: &[u32]) -> R<ColRef> {
        let n = self.n;
        match &e.kind {
            ExprKind::Column(i) => self.column(*i),
            ExprKind::Literal(v) => Ok(Rc::new(literal_col(v, e.ty, n)?)),
            ExprKind::Neg(x) => {
                let c = self.eval(x, sel)?;
                Ok(Rc::new(negate(&c, e, sel)?))
            }
            ExprKind::Not(x) => {
                let c = self.eval(x, sel)?;
                let v = c.bools()?;
                let mut out = Col::bool_col(vec![false; n], vec![false; n]);
                if let Data::Bool(d) = &mut out.data {
                    for &r in sel {
                        let i = r as usize;
                        out.valid[i] = c.valid[i];
                        d[i] = !v[i];
                    }
                }
                Ok(Rc::new(out))
            }
            ExprKind::Arith { op, left, right } => {
                let l = self.eval(left, sel)?;
                let r = self.eval(right, sel)?;
                Ok(Rc::new(arith(*op, &l, &r, e, sel)?))
            }
            ExprKind::Cmp { op, left, right } => {
                let l = self.eval(left, sel)?;
                let r = self.eval(right, sel)?;
                Ok(Rc::new(compare(*op, &l, &r, sel, e.pos)?))
            }
            ExprKind::And(parts) => self.and_or(parts, sel, true),
            ExprKind::Or(parts) => self.and_or(parts, sel, false),
            ExprKind::IsNull { expr, negated } => {
                let c = self.eval(expr, sel)?;
                let mut vals = vec![false; n];
                let mut valid = vec![false; n];
                for &r in sel {
                    let i = r as usize;
                    valid[i] = true;
                    vals[i] = c.valid[i] == *negated;
                }
                Ok(Rc::new(Col::bool_col(vals, valid)))
            }
            ExprKind::Between {
                expr,
                low,
                high,
                negated,
            } => self.between(expr, low, high, *negated, e, sel),
            ExprKind::InList {
                expr,
                list,
                negated,
            } => self.in_list(expr, list, *negated, e, sel),
            ExprKind::Case {
                operand,
                whens,
                else_,
            } => self.case(operand.as_deref(), whens, else_, e, sel),
            ExprKind::Cast { expr, .. } => {
                let c = self.eval(expr, sel)?;
                Ok(Rc::new(cast_col(&c, e.ty, sel, e.pos)?))
            }
        }
    }

    #[inline(never)]
    fn between(
        &self,
        expr: &Expr,
        low: &Expr,
        high: &Expr,
        negated: bool,
        e: &Expr,
        sel: &[u32],
    ) -> R<ColRef> {
        let x = self.eval(expr, sel)?;
        let lo = self.eval(low, sel)?;
        let ge = compare(CmpOp::Ge, &x, &lo, sel, e.pos)?;
        // the upper bound is evaluated only for the rows the lower bound did not already decide as FALSE
        let rest: Vec<u32> = {
            let g = ge.bools()?;
            sel.iter()
                .copied()
                .filter(|r| {
                    let i = *r as usize;
                    !(ge.valid[i] && !g[i])
                })
                .collect()
        };
        let hi = self.eval(high, &rest)?;
        let le = compare(CmpOp::Le, &x, &hi, &rest, e.pos)?;
        let mut out = and3(&ge, &le, sel)?;
        if negated {
            not_in_place(&mut out, sel)?;
        }
        Ok(Rc::new(out))
    }

    #[inline(never)]
    fn in_list(
        &self,
        expr: &Expr,
        list: &[Expr],
        negated: bool,
        e: &Expr,
        sel: &[u32],
    ) -> R<ColRef> {
        let n = self.n;

        let x = self.eval(expr, sel)?;
        let items: Vec<ColRef> = list
            .iter()
            .map(|it| self.eval(it, sel))
            .collect::<R<Vec<_>>>()?;
        let mut vals = vec![false; n];
        let mut valid = vec![false; n];
        // the items of one list are of one family but may have different decimal scales: one comparer per item
        let comparers = items
            .iter()
            .map(|it| Comparer::new(&x, it))
            .collect::<R<Vec<_>>>()?;
        for &r in sel {
            let i = r as usize;
            if !x.valid[i] {
                continue;
            }
            let mut found = false;
            let mut null_seen = false;
            for (it, cmp) in items.iter().zip(&comparers) {
                if !it.valid[i] {
                    null_seen = true;
                } else if cmp.order(&x, it, i, e.pos)? == Ordering::Equal {
                    found = true;
                    break;
                }
            }
            if found {
                valid[i] = true;
                vals[i] = !negated;
            } else if !null_seen {
                valid[i] = true;
                vals[i] = negated;
            }
        }
        Ok(Rc::new(Col::bool_col(vals, valid)))
    }

    /// `AND` (`is_and`) or `OR` of two or more operands, left to right: an operand is evaluated only for the rows that the earlier ones did not decide.
    fn and_or(&self, parts: &[Expr], sel: &[u32], is_and: bool) -> R<ColRef> {
        let mut it = parts.iter();
        let first = it
            .next()
            .ok_or_else(|| EvalError::internal("an AND or OR without operands"))?;
        let mut acc = (*self.eval(first, sel)?).clone();
        for p in it {
            // decided: FALSE for AND, TRUE for OR
            let open: Vec<u32> = {
                let v = acc.bools()?;
                sel.iter()
                    .copied()
                    .filter(|r| {
                        let i = *r as usize;
                        !(acc.valid[i] && v[i] != is_and)
                    })
                    .collect()
            };
            if open.is_empty() {
                break;
            }
            let next = self.eval(p, &open)?;
            acc = if is_and {
                and3(&acc, &next, &open)?
            } else {
                or3(&acc, &next, &open)?
            };
            // rows outside `open` keep their decided value: and3/or3 wrote only `open`, the rest of `acc` is as before
        }
        Ok(Rc::new(acc))
    }

    fn case(
        &self,
        operand: Option<&Expr>,
        whens: &[(Expr, Expr)],
        else_: &Expr,
        e: &Expr,
        sel: &[u32],
    ) -> R<ColRef> {
        let n = self.n;
        let mut out = Col::empty(e.ty, n);
        let subject = match operand {
            Some(o) => Some(self.eval(o, sel)?),
            None => None,
        };
        let mut remaining: Vec<u32> = sel.to_vec();
        for (cond, result) in whens {
            if remaining.is_empty() {
                break;
            }
            let cv = self.eval(cond, &remaining)?;
            let taken: Vec<u32> = match &subject {
                None => {
                    let b = cv.bools()?;
                    remaining
                        .iter()
                        .copied()
                        .filter(|r| cv.valid[*r as usize] && b[*r as usize])
                        .collect()
                }
                Some(s) => {
                    let eq = compare(CmpOp::Eq, s, &cv, &remaining, cond.pos)?;
                    let b = eq.bools()?;
                    remaining
                        .iter()
                        .copied()
                        .filter(|r| eq.valid[*r as usize] && b[*r as usize])
                        .collect()
                }
            };
            if !taken.is_empty() {
                let rv = self.eval(result, &taken)?;
                out.assign(&rv, &taken)?;
                // a row takes the first branch whose condition is TRUE; FALSE and NULL fall through
                let mut is_taken = vec![false; n];
                for &r in &taken {
                    is_taken[r as usize] = true;
                }
                remaining.retain(|r| !is_taken[*r as usize]);
            }
        }
        if !remaining.is_empty() {
            let rv = self.eval(else_, &remaining)?;
            out.assign(&rv, &remaining)?;
        }
        Ok(Rc::new(out))
    }
}

// ------------------------------------------------------------------------------------------------------------------------------ literals

fn literal_col(v: &Value, ty: SqlType, n: usize) -> R<Col> {
    let mut c = Col::empty(ty, n);
    literal_into(v, &mut c)?;
    Ok(c)
}

pub(crate) fn literal_into(v: &Value, c: &mut Col) -> R<()> {
    let n = c.len();
    if matches!(v, Value::Null) {
        return Ok(());
    }
    c.valid.fill(true);
    let bad = || EvalError::internal("a literal does not match its type");
    match (v, &mut c.data) {
        (Value::Bool(b), Data::Bool(d)) => d.fill(*b),
        (Value::Int(x), Data::Int(d)) => d.fill(*x),
        (Value::Decimal(x), Data::Dec(d)) => d.fill(*x),
        // an integer literal where a decimal of scale 0 is typed
        (Value::Int(x), Data::Dec(d)) => d.fill(i128::from(*x)),
        (Value::Str(s), Data::Str(d)) => {
            // one text, one key per row
            *d = StrData::Dict {
                values: std::sync::Arc::new(arrow_array::StringArray::from(vec![s.as_str()])),
                keys: vec![0; n],
            };
        }
        (Value::Date(x), Data::Date(d)) => d.fill(*x),
        _ => return Err(bad()),
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------------------------------------ logic

fn and3(a: &Col, b: &Col, rows: &[u32]) -> R<Col> {
    three(a, b, rows, true)
}

fn or3(a: &Col, b: &Col, rows: &[u32]) -> R<Col> {
    three(a, b, rows, false)
}

/// SQL three-valued AND / OR of two boolean columns for `rows`; the other rows are copied from `a`.
fn three(a: &Col, b: &Col, rows: &[u32], is_and: bool) -> R<Col> {
    let (av, bv) = (a.bools()?, b.bools()?);
    let mut vals = av.to_vec();
    let mut valid = a.valid.clone();
    for &r in rows {
        let i = r as usize;
        let (a_ok, b_ok) = (a.valid[i], b.valid[i]);
        // the value that decides the result: FALSE for AND, TRUE for OR
        let decides = |ok: bool, v: bool| ok && v != is_and;
        let (value, known) = if decides(a_ok, av[i]) || decides(b_ok, bv[i]) {
            (!is_and, true)
        } else if a_ok && b_ok {
            (is_and, true)
        } else {
            (false, false)
        };
        vals[i] = value;
        valid[i] = known;
    }
    Ok(Col::bool_col(vals, valid))
}

fn not_in_place(c: &mut Col, rows: &[u32]) -> R<()> {
    if let Data::Bool(v) = &mut c.data {
        for &r in rows {
            let i = r as usize;
            if c.valid[i] {
                v[i] = !v[i];
            }
        }
        Ok(())
    } else {
        Err(EvalError::internal("NOT of a column that is not boolean"))
    }
}

// ------------------------------------------------------------------------------------------------------------------------------ comparison

/// How two columns are compared, fixed once per operation from their types.
#[derive(Clone, Copy)]
enum Comparer {
    Int,
    /// Exact decimals (an integer meeting a decimal has scale 0).
    Dec(u8, u8),
    F32,
    F64,
    Bool,
    Str,
    Date,
}

fn scale_of(ty: SqlType) -> Option<u8> {
    match ty {
        SqlType::Decimal { scale, .. } => Some(scale),
        t if t.is_integer() => Some(0),
        _ => None,
    }
}

impl Comparer {
    fn new(l: &Col, r: &Col) -> R<Comparer> {
        Ok(match (&l.data, &r.data) {
            (Data::Int(_), Data::Int(_)) => Comparer::Int,
            (Data::Int(_) | Data::Dec(_), Data::Int(_) | Data::Dec(_)) => Comparer::Dec(
                scale_of(l.ty).ok_or_else(|| EvalError::internal("a decimal without a scale"))?,
                scale_of(r.ty).ok_or_else(|| EvalError::internal("a decimal without a scale"))?,
            ),
            (Data::F32(_), Data::F32(_)) => Comparer::F32,
            (Data::F64(_), Data::F64(_)) => Comparer::F64,
            (Data::Bool(_), Data::Bool(_)) => Comparer::Bool,
            (Data::Str(_), Data::Str(_)) => Comparer::Str,
            (Data::Date(_), Data::Date(_)) => Comparer::Date,
            _ => {
                return Err(EvalError::internal(format!(
                    "a comparison of {} with {} has no implementation",
                    l.ty, r.ty
                )));
            }
        })
    }

    /// The order of row `i` of `l` against row `i` of `r` (both valid).
    fn order(self, l: &Col, r: &Col, i: usize, _pos: Pos) -> R<Ordering> {
        let bad = || EvalError::internal("a column does not match its type");
        Ok(match (self, &l.data, &r.data) {
            (Comparer::Int, Data::Int(a), Data::Int(b)) => a[i].cmp(&b[i]),
            (Comparer::Dec(sa, sb), a, b) => {
                let x = dec_at(a, i).ok_or_else(bad)?;
                let y = dec_at(b, i).ok_or_else(bad)?;
                cmp_decimal(x, sa, y, sb)
            }
            (Comparer::F32, Data::F32(a), Data::F32(b)) => {
                cmp_float(f64::from(a[i]), f64::from(b[i]))
            }
            (Comparer::F64, Data::F64(a), Data::F64(b)) => cmp_float(a[i], b[i]),
            (Comparer::Bool, Data::Bool(a), Data::Bool(b)) => a[i].cmp(&b[i]),
            (Comparer::Str, Data::Str(a), Data::Str(b)) => {
                a.get(i).as_bytes().cmp(b.get(i).as_bytes())
            }
            (Comparer::Date, Data::Date(a), Data::Date(b)) => a[i].cmp(&b[i]),
            _ => return Err(bad()),
        })
    }
}

fn dec_at(d: &Data, i: usize) -> Option<i128> {
    match d {
        Data::Int(v) => Some(i128::from(*v.get(i)?)),
        Data::Dec(v) => v.get(i).copied(),
        _ => None,
    }
}

fn holds(op: CmpOp, o: Ordering) -> bool {
    match op {
        CmpOp::Eq => o == Ordering::Equal,
        CmpOp::Ne => o != Ordering::Equal,
        CmpOp::Lt => o == Ordering::Less,
        CmpOp::Le => o != Ordering::Greater,
        CmpOp::Gt => o == Ordering::Greater,
        CmpOp::Ge => o != Ordering::Less,
    }
}

fn compare(op: CmpOp, l: &Col, r: &Col, sel: &[u32], pos: Pos) -> R<Col> {
    let n = l.len();
    let cmp = Comparer::new(l, r)?;
    let mut vals = vec![false; n];
    let mut valid = vec![false; n];
    nata_kernels::decimal::compare(&l.valid, &r.valid, sel, &mut vals, &mut valid, |i| {
        cmp.order(l, r, i, pos).map(|o| holds(op, o))
    })?;
    Ok(Col::bool_col(vals, valid))
}

// ------------------------------------------------------------------------------------------------------------------------------ arithmetic

fn int_overflow(e: &Expr, what: &str) -> EvalError {
    EvalError::at(
        e.pos,
        format!(
            "integer overflow in {what}: the result does not fit {}. Cast an operand to a wider integer, or restrict the rows",
            e.ty
        ),
    )
}

fn dec_overflow(e: &Expr, what: &str) -> EvalError {
    EvalError::at(
        e.pos,
        format!(
            "decimal overflow in {what}: the result does not fit {}. Cast an operand to a wider decimal, or restrict the rows",
            e.ty
        ),
    )
}

fn div_zero(e: &Expr, op: ArithOp) -> EvalError {
    EvalError::at(
        e.pos,
        format!(
            "division by zero in {}. Guard it with CASE WHEN divisor <> 0 THEN ... END, or restrict the rows with WHERE",
            match op {
                ArithOp::Mod => "a remainder (%)",
                ArithOp::IntDiv => "an integer division (//)",
                _ => "a division (/)",
            }
        ),
    )
}

fn negate(c: &Col, e: &Expr, sel: &[u32]) -> R<Col> {
    let n = c.len();
    let mut out = Col::empty(e.ty, n);
    match (&c.data, &mut out.data) {
        (Data::Int(s), Data::Int(d)) => {
            let (lo, hi) = (
                e.ty.int_min().unwrap_or(i64::MIN),
                e.ty.int_max().unwrap_or(i64::MAX),
            );
            nata_kernels::decimal::unary(s, &c.valid, sel, d, &mut out.valid, |v| {
                v.checked_neg()
                    .filter(|v| *v >= lo && *v <= hi)
                    .ok_or_else(|| int_overflow(e, "a unary minus"))
            })?;
        }
        (Data::Dec(s), Data::Dec(d)) => {
            nata_kernels::decimal::unary(s, &c.valid, sel, d, &mut out.valid, |v| {
                v.checked_neg()
                    .ok_or_else(|| dec_overflow(e, "a unary minus"))
            })?;
        }
        _ => {
            return Err(EvalError::internal(
                "unary minus of a type without arithmetic",
            ));
        }
    }
    for &r in sel {
        out.valid[r as usize] = c.valid[r as usize];
    }
    Ok(out)
}

fn arith(op: ArithOp, l: &Col, r: &Col, e: &Expr, sel: &[u32]) -> R<Col> {
    let mut out = Col::empty(e.ty, l.len());
    arith_into(op, l, r, e, sel, &mut out)?;
    Ok(out)
}

pub(crate) fn arith_into(
    op: ArithOp,
    l: &Col,
    r: &Col,
    e: &Expr,
    sel: &[u32],
    out: &mut Col,
) -> R<()> {
    let name = format!("`{}`", op.symbol());
    match (&l.data, &r.data, &mut out.data) {
        (Data::Int(a), Data::Int(b), Data::Int(d)) => {
            let (lo, hi) = (
                e.ty.int_min().unwrap_or(i64::MIN),
                e.ty.int_max().unwrap_or(i64::MAX),
            );
            nata_kernels::decimal::binary(
                a,
                &l.valid,
                b,
                &r.valid,
                sel,
                d,
                &mut out.valid,
                |x, y| nata_kernels::decimal::integer(kernel_op(op), x, y, lo, hi),
            )
            .map_err(|f| match f {
                DecFail::Overflow => int_overflow(e, &name),
                DecFail::DivZero => div_zero(e, op),
            })?;
        }
        (Data::Dec(a), Data::Dec(b), Data::Dec(d)) => {
            let (
                SqlType::Decimal {
                    precision: p,
                    scale: s,
                },
                Some(sl),
                Some(sr),
            ) = (e.ty, scale_of(l.ty), scale_of(r.ty))
            else {
                return Err(EvalError::internal(
                    "decimal arithmetic without decimal types",
                ));
            };
            Decimal::new(kernel_op(op), sl, sr, p, s)
                .and_then(|k| k.column(a, &l.valid, b, &r.valid, sel, d, &mut out.valid))
                .map_err(|f| match f {
                    DecFail::Overflow => dec_overflow(e, &name),
                    DecFail::DivZero => div_zero(e, op),
                })?;
        }
        _ => {
            return Err(EvalError::internal(format!(
                "arithmetic on {} and {} has no implementation",
                l.ty, r.ty
            )));
        }
    }
    Ok(())
}

fn kernel_op(op: ArithOp) -> Op {
    match op {
        ArithOp::Add => Op::Add,
        ArithOp::Sub => Op::Sub,
        ArithOp::Mul => Op::Mul,
        ArithOp::Div => Op::Div,
        ArithOp::Mod => Op::Mod,
        ArithOp::IntDiv => Op::IntDiv,
    }
}

#[cfg(test)]
fn dec_arith(op: ArithOp, a: i128, sa: u8, b: i128, sb: u8, p: u8, s: u8) -> Result<i128, DecFail> {
    Decimal::new(kernel_op(op), sa, sb, p, s)?.scalar(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    const D152: (u8, u8) = (15, 2);

    fn dec(
        op: ArithOp,
        a: i128,
        sa: u8,
        b: i128,
        sb: u8,
        p: u8,
        s: u8,
    ) -> Result<i128, &'static str> {
        dec_arith(op, a, sa, b, sb, p, s).map_err(|f| match f {
            DecFail::Overflow => "overflow",
            DecFail::DivZero => "div0",
        })
    }

    #[test]
    fn decimal_arithmetic_follows_the_rules_page() {
        // (15,2) + (15,2) -> (16,2)
        assert_eq!(dec(ArithOp::Add, 12345, 2, 1, 2, 16, 2), Ok(12346));
        // 12345.67 * 0.05 -> (18,4)
        assert_eq!(dec(ArithOp::Mul, 1_234_567, 2, 5, 2, 18, 4), Ok(6_172_835));
        // 7 / 2 on (10,0),(10,0) -> (16,6): 3.500000
        assert_eq!(dec(ArithOp::Div, 7, 0, 2, 0, 16, 6), Ok(3_500_000));
        // division rounds half away from zero: 1 / 3 -> 0.333333, 2 / 3 -> 0.666667, -2 / 3 -> -0.666667
        assert_eq!(dec(ArithOp::Div, 1, 0, 3, 0, 16, 6), Ok(333_333));
        assert_eq!(dec(ArithOp::Div, 2, 0, 3, 0, 16, 6), Ok(666_667));
        assert_eq!(dec(ArithOp::Div, -2, 0, 3, 0, 16, 6), Ok(-666_667));
        assert_eq!(dec(ArithOp::Div, 1, 0, 0, 0, 16, 6), Err("div0"));
        // % takes the sign of the dividend
        assert_eq!(dec(ArithOp::Mod, -700, 2, 300, 2, 15, 2), Ok(-100));
        assert_eq!(dec(ArithOp::Mod, 700, 2, -300, 2, 15, 2), Ok(100));
        assert_eq!(dec(ArithOp::Mod, 700, 2, 0, 2, 15, 2), Err("div0"));
        let _ = D152;
    }

    #[test]
    fn decimal_overflow_is_checked_against_the_digits_of_the_result() {
        // (38,0) + (38,0) with the largest value overflows 38 digits
        let big = 10i128.pow(38) - 1;
        assert_eq!(dec(ArithOp::Add, big, 0, 1, 0, 38, 0), Err("overflow"));
        assert_eq!(dec(ArithOp::Add, big - 1, 0, 1, 0, 38, 0), Ok(big));
        // an operand that does not fit once rescaled is an overflow even if the sum would fit: (38,2) + (38,38) at scale 38
        assert_eq!(dec(ArithOp::Add, 1, 0, 0, 38, 38, 38), Err("overflow"));
        // ... also when the exact sum would fit: 1 + (-0.99999...) = 0.00000...1 (DuckDB errors too; decimal-rules section 3)
        assert_eq!(dec(ArithOp::Add, 1, 0, -(big), 38, 38, 38), Err("overflow"));
        assert_eq!(dec(ArithOp::Sub, 1, 0, big, 38, 38, 38), Err("overflow"));
        // a capped multiplication: (18,2) * (18,2) -> (18,4): 10^15 * 10^15 does not fit 18 digits
        assert_eq!(
            dec(ArithOp::Mul, 10i128.pow(17), 2, 10i128.pow(17), 2, 18, 4),
            Err("overflow")
        );
        // 128-bit product overflow is an error too
        assert_eq!(dec(ArithOp::Mul, big, 0, big, 0, 38, 0), Err("overflow"));
        assert_eq!(dec(ArithOp::Div, big, 0, 1, 0, 38, 0), Ok(big));
    }

    #[test]
    fn three_valued_and_or() {
        let mk = |v: &[Option<bool>]| {
            Col::bool_col(
                v.iter().map(|x| x.unwrap_or(false)).collect(),
                v.iter().map(Option::is_some).collect(),
            )
        };
        let vals = [Some(true), Some(false), None];
        let (mut a, mut b) = (vec![], vec![]);
        for x in vals {
            for y in vals {
                a.push(x);
                b.push(y);
            }
        }
        let rows: Vec<u32> = (0..9).collect();
        let and = three(&mk(&a), &mk(&b), &rows, true).expect("and");
        let or = three(&mk(&a), &mk(&b), &rows, false).expect("or");
        let get = |c: &Col, i: usize| c.valid[i].then(|| c.bools().expect("bool")[i]);
        let expect_and = [
            Some(true),
            Some(false),
            None,
            Some(false),
            Some(false),
            Some(false),
            None,
            Some(false),
            None,
        ];
        let expect_or = [
            Some(true),
            Some(true),
            Some(true),
            Some(true),
            Some(false),
            None,
            Some(true),
            None,
            None,
        ];
        for i in 0..9 {
            assert_eq!(get(&and, i), expect_and[i], "and row {i}");
            assert_eq!(get(&or, i), expect_or[i], "or row {i}");
        }
    }
}
