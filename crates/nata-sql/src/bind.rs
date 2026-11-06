//! Bind: resolve the names of a parsed query against the schema of its source and give every expression its exact type (dialect sections 2 to 7,
//! `decimal-rules.md`). The result is a [`Bound`] query whose expressions are typed trees over the source's columns; nothing is evaluated here.

use std::collections::{HashMap, HashSet};

use nata_parquet::civil;

use crate::ast::{self, BinOp, DataType, ExplainMode, ExprKind as A, Ident, SelectItem, UnOp};
use crate::error::{Pos, Result, SqlError};
use crate::source::{BindOptions, SourceInfo, SourceProvider, resolve_with};
use crate::texpr::{CmpOp, Expr, ExprKind, Value};
use crate::types::{
    ArithOp, SqlType, TypeError, arith_type, check_cast, check_comparable, neg_type, unify_results,
};

/// One output column of the query.
#[derive(Debug, PartialEq)]
pub struct OutItem {
    pub name: String,
    pub expr: Expr,
}

/// A bound query. Column references in `items` and `where_` index `source.columns` (file order).
#[derive(Debug, PartialEq)]
pub struct Bound {
    pub explain: ExplainMode,
    pub source: SourceInfo,
    pub items: Vec<OutItem>,
    pub where_: Option<Expr>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

pub fn bind(q: &ast::Query, provider: &dyn SourceProvider) -> Result<Bound> {
    bind_with(q, provider, BindOptions::default())
}

/// As [`bind`]; `options.all_footers` reads every footer of a glob at bind time (see [`BindOptions`]).
pub fn bind_with(
    q: &ast::Query,
    provider: &dyn SourceProvider,
    options: BindOptions,
) -> Result<Bound> {
    let source = resolve_with(provider, &q.from, q.from_pos.0, options)?;
    let b = Binder::new(&source, q);
    let mut items = Vec::new();
    for it in &q.items {
        match it {
            SelectItem::Star(span) => {
                for (i, c) in source.columns.iter().enumerate() {
                    let ty = c.ty.clone().map_err(|why| {
                        SqlError::not_supported(span.0, format!("column {:?}: {why}", c.name))
                    })?;
                    items.push(OutItem {
                        name: c.name.clone(),
                        expr: Expr::new(ExprKind::Column(i), ty, span.0),
                    });
                }
            }
            SelectItem::Expr { expr, alias } => {
                let e = b.expr(expr, None)?;
                if e.ty == SqlType::Null {
                    return Err(SqlError::not_supported(
                        expr.pos(),
                        "NULL without a type in the select list (write CAST(NULL AS type))",
                    ));
                }
                let name = match alias {
                    Some(a) => fold(a),
                    None => default_name(expr),
                };
                items.push(OutItem { name, expr: e });
            }
        }
    }
    let where_ = match &q.where_ {
        None => None,
        Some(w) => {
            let mut e = b.expr(w, Some(SqlType::Boolean))?;
            retype_null(&mut e, SqlType::Boolean);
            if e.ty != SqlType::Boolean {
                return Err(SqlError::bind(
                    e.pos,
                    format!("WHERE needs a BOOLEAN condition, found {}", e.ty),
                ));
            }
            Some(e)
        }
    };
    Ok(Bound {
        explain: q.explain,
        source,
        items,
        where_,
        limit: q.limit,
        offset: q.offset,
    })
}

/// The name of an identifier: folded to lower case unless quoted.
pub fn fold(id: &Ident) -> String {
    if id.quoted {
        id.name.clone()
    } else {
        id.name.to_lowercase()
    }
}

/// The name of an output column without an alias: a column keeps its name (also under a CAST), CASE is `case`, anything else `?column?` (as PostgreSQL).
fn default_name(e: &ast::Expr) -> String {
    match &e.kind {
        A::Ident(id) => fold(id),
        A::Cast { expr, .. } => default_name(expr),
        A::Case { .. } => "case".into(),
        _ => "?column?".into(),
    }
}

// ---------------------------------------------------------------------------------------------------------------------------------- helpers

fn te(e: TypeError, pos: Pos) -> SqlError {
    match e {
        TypeError::Bind(m) => SqlError::bind(pos, m),
        TypeError::NotSupported(m) => SqlError::not_supported(pos, m),
    }
}

fn take_kind(e: &mut Expr) -> ExprKind {
    std::mem::replace(&mut e.kind, ExprKind::Literal(Value::Null))
}

fn placeholder() -> Expr {
    Expr::literal(Value::Null, SqlType::Null, Pos::NONE)
}

fn retype_null(e: &mut Expr, ty: SqlType) {
    if e.is_null_literal() {
        e.ty = ty;
    }
}

fn pow10(n: u32) -> Option<i128> {
    10i128.checked_pow(n)
}

/// Number of decimal digits of |v| (0 has none).
pub fn digits(v: i128) -> u32 {
    let mut n = 0;
    let mut x = v.unsigned_abs();
    while x > 0 {
        n += 1;
        x /= 10;
    }
    n
}

/// The type of an integer by value: the smallest of INTEGER, BIGINT that holds it; larger is a decimal of its digits.
fn int_literal(v: i128, pos: Pos) -> Expr {
    if let Ok(i) = i32::try_from(v) {
        Expr::literal(Value::Int(i64::from(i)), SqlType::Integer, pos)
    } else if let Ok(i) = i64::try_from(v) {
        Expr::literal(Value::Int(i), SqlType::BigInt, pos)
    } else {
        let p = u8::try_from(digits(v).max(1)).unwrap_or(38);
        Expr::literal(
            Value::Decimal(v),
            SqlType::Decimal {
                precision: p,
                scale: 0,
            },
            pos,
        )
    }
}

fn decimal_literal(unscaled: i128, scale: u32, pos: Pos) -> Expr {
    let p = digits(unscaled).max(scale).max(1);
    Expr::literal(
        Value::Decimal(unscaled),
        SqlType::Decimal {
            precision: u8::try_from(p).unwrap_or(38),
            scale: u8::try_from(scale).unwrap_or(38),
        },
        pos,
    )
}

/// A number literal from its source text, exactly (dialect section 3): an integer by value, a decimal from its digits.
pub fn number_literal(text: &str, negative: bool, pos: Pos) -> std::result::Result<Expr, String> {
    let too_many = || format!("the number {text} does not fit 38 digits");
    let (mant, exp) = match text.find(['e', 'E']) {
        Some(i) => (&text[..i], Some(&text[i + 1..])),
        None => (text, None),
    };
    let exp: i64 = match exp {
        None => 0,
        Some(x) => x.parse::<i64>().map_err(|_| too_many())?,
    };
    if exp.abs() > 1_000 {
        return Err(too_many());
    }
    let (int_part, frac_part) = match mant.find('.') {
        Some(i) => (&mant[..i], &mant[i + 1..]),
        None => (mant, ""),
    };
    let plain_integer = exp_none(text) && !mant.contains('.');
    let all = format!("{int_part}{frac_part}");
    let trimmed = all.trim_start_matches('0');
    if trimmed.len() > 38 {
        return Err(too_many());
    }
    let mut unscaled: i128 = if trimmed.is_empty() {
        0
    } else {
        trimmed.parse().map_err(|_| too_many())?
    };
    if negative {
        unscaled = -unscaled;
    }
    if plain_integer {
        return Ok(int_literal(unscaled, pos));
    }
    // value = unscaled * 10^(point - len)
    let len = i64::try_from(all.len()).map_err(|_| too_many())?;
    let point = i64::try_from(int_part.len()).map_err(|_| too_many())? + exp;
    let scale = len - point;
    if scale < 0 {
        let up = u32::try_from(-scale).map_err(|_| too_many())?;
        if up > 38 {
            return Err(too_many());
        }
        unscaled = pow10(up)
            .and_then(|p| unscaled.checked_mul(p))
            .ok_or_else(too_many)?;
        if digits(unscaled) > 38 {
            return Err(too_many());
        }
        return Ok(decimal_literal(unscaled, 0, pos));
    }
    let scale = u32::try_from(scale).map_err(|_| too_many())?;
    if scale > 38 {
        return Err(format!(
            "the number {text} has more than 38 digits after the point"
        ));
    }
    Ok(decimal_literal(unscaled, scale, pos))
}

fn exp_none(text: &str) -> bool {
    !text.contains(['e', 'E'])
}

/// Widen `e` to `to` (a conversion that cannot fail and loses nothing): a literal is converted in place, anything else gets an implicit CAST node.
fn coerce(e: Expr, to: SqlType) -> Expr {
    if e.ty == to {
        return e;
    }
    if e.is_null_literal() {
        return Expr::literal(Value::Null, to, e.pos);
    }
    if let ExprKind::Literal(v) = &e.kind {
        let folded = match (v, e.ty, to) {
            (Value::Int(i), t, SqlType::Decimal { scale, .. }) if t.is_integer() => {
                pow10(u32::from(scale))
                    .and_then(|p| i128::from(*i).checked_mul(p))
                    .map(Value::Decimal)
            }
            (Value::Int(i), t, u) if t.is_integer() && u.is_integer() => Some(Value::Int(*i)),
            (
                Value::Decimal(u),
                SqlType::Decimal { scale: s1, .. },
                SqlType::Decimal { scale: s2, .. },
            ) if s2 >= s1 => pow10(u32::from(s2 - s1))
                .and_then(|p| u.checked_mul(p))
                .map(Value::Decimal),
            _ => None,
        };
        if let Some(v) = folded {
            return Expr::literal(v, to, e.pos);
        }
    }
    let pos = e.pos;
    Expr::new(
        ExprKind::Cast {
            expr: Box::new(e),
            implicit: true,
        },
        to,
        pos,
    )
}

fn coerce_in_place(x: &mut Expr, to: SqlType) {
    if x.ty != to {
        let e = std::mem::replace(x, placeholder());
        *x = coerce(e, to);
    }
}

fn fits(e: &Expr, t: SqlType) -> bool {
    match (&e.kind, t.int_min(), t.int_max()) {
        (ExprKind::Literal(Value::Int(v)), Some(lo), Some(hi)) => {
            e.ty.is_integer() && lo <= *v && *v <= hi
        }
        _ => false,
    }
}

/// Bring the operands of a comparison (or of the BETWEEN and IN groups) to types that the evaluator compares directly: all one integer type, or
/// decimals (an integer meeting a decimal becomes a decimal of scale 0). Integer LITERALS are retyped to the type of the other operands when they fit,
/// so that a column stays a bare column (a pushable one).
fn normalize(xs: &mut [Expr]) {
    if !xs.iter().all(|x| x.ty.is_exact_numeric()) {
        return;
    }
    if xs.iter().all(|x| x.ty.is_integer()) {
        let cols: Vec<SqlType> = xs
            .iter()
            .filter(|x| !x.is_literal())
            .map(|x| x.ty)
            .collect();
        let target = match cols.first() {
            Some(t)
                if cols.iter().all(|c| c == t)
                    && xs.iter().all(|x| !x.is_literal() || fits(x, *t)) =>
            {
                *t
            }
            _ => SqlType::int_of_rank(xs.iter().filter_map(|x| x.ty.int_rank()).max().unwrap_or(4)),
        };
        for x in xs.iter_mut() {
            coerce_in_place(x, target);
        }
        return;
    }
    for x in xs.iter_mut() {
        if x.ty.is_integer()
            && let Some((p, s)) = x.ty.as_decimal()
        {
            coerce_in_place(
                x,
                SqlType::Decimal {
                    precision: p,
                    scale: s,
                },
            );
        }
    }
}

/// Edit distance between two names (characters), or `None` if it exceeds `max`.
pub fn edit_distance(a: &str, b: &str, max: usize) -> Option<usize> {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > max {
        return None;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let sub = prev[j - 1] + usize::from(a[i - 1] != b[j - 1]);
            cur[j] = sub.min(prev[j] + 1).min(cur[j - 1] + 1);
        }
        prev = cur;
    }
    let d = prev[b.len()];
    (d <= max).then_some(d)
}

// ---------------------------------------------------------------------------------------------------------------------------------- the binder

struct Binder<'a> {
    cols: &'a [crate::source::SourceColumn],
    by_name: HashMap<&'a str, usize>,
    aliases: HashSet<String>,
}

enum Step<'x> {
    Bin(BinOp, &'x ast::Expr),
    Cast(&'x ast::Expr, &'x DataType),
    IsNull(bool),
    Between(&'x ast::Expr, &'x ast::Expr, bool),
    In(&'x [ast::Expr], bool),
}

impl<'a> Binder<'a> {
    fn new(source: &'a SourceInfo, q: &ast::Query) -> Binder<'a> {
        let mut by_name = HashMap::new();
        for (i, c) in source.columns.iter().enumerate() {
            by_name.entry(c.name.as_str()).or_insert(i);
        }
        let aliases = q
            .items
            .iter()
            .filter_map(|i| match i {
                SelectItem::Expr { alias: Some(a), .. } => Some(fold(a)),
                _ => None,
            })
            .collect();
        Binder {
            cols: &source.columns,
            by_name,
            aliases,
        }
    }

    fn column(&self, id: &Ident) -> Result<Expr> {
        let name = fold(id);
        let pos = id.span.0;
        let Some(&i) = self.by_name.get(name.as_str()) else {
            return Err(self.unknown_column(&name, pos));
        };
        let col = &self.cols[i];
        let ty = col
            .ty
            .clone()
            .map_err(|why| SqlError::not_supported(pos, format!("column {:?}: {why}", col.name)))?;
        Ok(Expr::new(ExprKind::Column(i), ty, pos))
    }

    fn unknown_column(&self, name: &str, pos: Pos) -> SqlError {
        if self.aliases.contains(name) {
            return SqlError::bind(
                pos,
                format!(
                    "unknown column {name:?}: it is an alias of this SELECT, and an alias cannot be used inside the same SELECT or WHERE"
                ),
            );
        }
        // the closest column within an edit distance of 2 (a difference of case only counts as 0)
        let lower = name.to_lowercase();
        let mut best: Option<(usize, &str)> = None;
        for c in self.cols {
            let d = if c.name.to_lowercase() == lower && c.name != name {
                Some(0)
            } else {
                edit_distance(name, &c.name, 2)
            };
            if let Some(d) = d
                && best.is_none_or(|(bd, _)| d < bd)
            {
                best = Some((d, c.name.as_str()));
            }
        }
        let e = SqlError::bind(pos, format!("unknown column {name:?}"));
        match best {
            Some((_, n)) => e.with_suggestion(n),
            None => e,
        }
    }

    /// Bind an expression. `hint` is the type the context needs for a NULL literal; an expression that is not a NULL literal ignores it.
    fn expr(&self, e: &ast::Expr, hint: Option<SqlType>) -> Result<Expr> {
        let pos = e.pos();
        match &e.kind {
            A::Ident(id) => self.column(id),
            A::Number(t) => number_literal(t, false, pos).map_err(|m| SqlError::bind(pos, m)),
            A::Str(s) => Ok(Expr::literal(Value::Str(s.clone()), SqlType::Varchar, pos)),
            A::Bool(b) => Ok(Expr::literal(Value::Bool(*b), SqlType::Boolean, pos)),
            A::Date(t) => civil::parse_date(t)
                .map(|d| Expr::literal(Value::Date(d), SqlType::Date, pos))
                .map_err(|why| SqlError::bind(pos, format!("DATE '{t}': {why}"))),
            A::Null => Ok(Expr::literal(
                Value::Null,
                hint.unwrap_or(SqlType::Null),
                pos,
            )),
            A::Unary {
                op: UnOp::Not,
                expr,
            } => {
                let mut x = self.expr(expr, Some(SqlType::Boolean))?;
                retype_null(&mut x, SqlType::Boolean);
                self.need_boolean(&x, "NOT")?;
                Ok(Expr::new(ExprKind::Not(Box::new(x)), SqlType::Boolean, pos))
            }
            A::Unary {
                op: UnOp::Neg,
                expr,
            } => self.negate(expr, pos),
            A::Case {
                operand,
                whens,
                else_,
            } => self.case(operand.as_deref(), whens, else_.as_deref(), pos),
            A::Binary { .. }
            | A::Cast { .. }
            | A::IsNull { .. }
            | A::Between { .. }
            | A::InList { .. } => self.spine(e),
        }
    }

    fn need_boolean(&self, x: &Expr, what: &str) -> Result<()> {
        if x.ty == SqlType::Boolean {
            Ok(())
        } else {
            Err(SqlError::bind(
                x.pos,
                format!("the argument of {what} must be BOOLEAN, found {}", x.ty),
            ))
        }
    }

    /// `-x`: a number literal is folded (so `-2147483648` is an INTEGER), anything else is a negation node.
    fn negate(&self, operand: &ast::Expr, pos: Pos) -> Result<Expr> {
        if let A::Number(t) = &operand.kind {
            return number_literal(t, true, pos).map_err(|m| SqlError::bind(pos, m));
        }
        let x = self.expr(operand, None)?;
        if x.ty == SqlType::Null {
            return Err(SqlError::not_supported(
                pos,
                "unary minus on NULL (the type is unknown; write CAST(NULL AS type))",
            ));
        }
        let ty = neg_type(x.ty).map_err(|e| te(e, pos))?;
        if let ExprKind::Literal(v) = &x.kind {
            match v {
                Value::Int(i) => return Ok(int_literal(-i128::from(*i), pos)),
                Value::Decimal(u) if x.ty.is_decimal() => {
                    return Ok(Expr::literal(Value::Decimal(-*u), x.ty, pos));
                }
                _ => {}
            }
        }
        Ok(Expr::new(ExprKind::Neg(Box::new(x)), ty, pos))
    }

    /// Everything with an operand on its left (`a OP b OP c`, `a::t`, `a IS NULL`, `a BETWEEN`, `a IN`) is bound in a loop along the left spine.
    fn spine(&self, top: &ast::Expr) -> Result<Expr> {
        let mut steps: Vec<Step<'_>> = Vec::new();
        let mut cur = top;
        loop {
            match &cur.kind {
                A::Binary { op, left, right } => {
                    steps.push(Step::Bin(*op, right));
                    cur = left;
                }
                A::Cast { expr, ty } => {
                    steps.push(Step::Cast(cur, ty));
                    cur = expr;
                }
                A::IsNull { expr, negated } => {
                    steps.push(Step::IsNull(*negated));
                    cur = expr;
                }
                A::Between {
                    expr,
                    low,
                    high,
                    negated,
                } => {
                    steps.push(Step::Between(low, high, *negated));
                    cur = expr;
                }
                A::InList {
                    expr,
                    list,
                    negated,
                } => {
                    steps.push(Step::In(list, *negated));
                    cur = expr;
                }
                _ => break,
            }
        }
        let mut acc = self.expr(cur, None)?;
        for step in steps.into_iter().rev() {
            acc = self.apply(step, acc)?;
        }
        Ok(acc)
    }

    fn apply(&self, step: Step<'_>, acc: Expr) -> Result<Expr> {
        match step {
            Step::Bin(op, right) => {
                let pos = acc.pos;
                match op {
                    BinOp::And | BinOp::Or => self.logic(op == BinOp::And, acc, right),
                    BinOp::Eq
                    | BinOp::NotEq
                    | BinOp::Lt
                    | BinOp::LtEq
                    | BinOp::Gt
                    | BinOp::GtEq => {
                        let r = self.expr(right, None)?;
                        let cop = match op {
                            BinOp::Eq => CmpOp::Eq,
                            BinOp::NotEq => CmpOp::Ne,
                            BinOp::Lt => CmpOp::Lt,
                            BinOp::LtEq => CmpOp::Le,
                            BinOp::Gt => CmpOp::Gt,
                            _ => CmpOp::Ge,
                        };
                        self.compare(cop, acc, r, pos)
                    }
                    _ => {
                        let aop = match op {
                            BinOp::Add => ArithOp::Add,
                            BinOp::Sub => ArithOp::Sub,
                            BinOp::Mul => ArithOp::Mul,
                            BinOp::Div => ArithOp::Div,
                            BinOp::Mod => ArithOp::Mod,
                            _ => ArithOp::IntDiv,
                        };
                        let r = self.expr(right, None)?;
                        self.arith(aop, acc, r, pos)
                    }
                }
            }
            Step::Cast(node, dt) => self.cast(acc, dt, node.pos()),
            Step::IsNull(negated) => {
                if acc.ty == SqlType::Null {
                    return Err(SqlError::not_supported(
                        acc.pos,
                        "IS NULL on a NULL literal (the type is unknown; write CAST(NULL AS type))",
                    ));
                }
                let pos = acc.pos;
                Ok(Expr::new(
                    ExprKind::IsNull {
                        expr: Box::new(acc),
                        negated,
                    },
                    SqlType::Boolean,
                    pos,
                ))
            }
            Step::Between(low, high, negated) => {
                let (lo, hi) = (self.expr(low, None)?, self.expr(high, None)?);
                let pos = acc.pos;
                let mut g = vec![acc, lo, hi];
                self.comparable_group(&mut g)?;
                let hi = g.pop().unwrap_or_else(placeholder);
                let lo = g.pop().unwrap_or_else(placeholder);
                let x = g.pop().unwrap_or_else(placeholder);
                Ok(Expr::new(
                    ExprKind::Between {
                        expr: Box::new(x),
                        low: Box::new(lo),
                        high: Box::new(hi),
                        negated,
                    },
                    SqlType::Boolean,
                    pos,
                ))
            }
            Step::In(list, negated) => {
                let mut g = Vec::with_capacity(list.len() + 1);
                let pos = acc.pos;
                g.push(acc);
                for item in list {
                    let it = self.expr(item, None)?;
                    if !it.is_literal() {
                        return Err(SqlError::not_supported(
                            item.pos(),
                            "a value that is not a literal in an IN list (write the values, or use OR)",
                        ));
                    }
                    g.push(it);
                }
                self.comparable_group(&mut g)?;
                let mut it = g.into_iter();
                let x = it.next().unwrap_or_else(placeholder);
                Ok(Expr::new(
                    ExprKind::InList {
                        expr: Box::new(x),
                        list: it.collect(),
                        negated,
                    },
                    SqlType::Boolean,
                    pos,
                ))
            }
        }
    }

    /// AND / OR: both sides BOOLEAN; a chain is one n-ary node.
    fn logic(&self, is_and: bool, mut left: Expr, right: &ast::Expr) -> Result<Expr> {
        let what = if is_and { "AND" } else { "OR" };
        let mut r = self.expr(right, Some(SqlType::Boolean))?;
        retype_null(&mut left, SqlType::Boolean);
        retype_null(&mut r, SqlType::Boolean);
        self.need_boolean(&left, what)?;
        self.need_boolean(&r, what)?;
        let pos = left.pos;
        let same = |e: &Expr| {
            matches!(
                (&e.kind, is_and),
                (ExprKind::And(_), true) | (ExprKind::Or(_), false)
            )
        };
        // a chain `a AND b AND c ...` is one node: the left operand's list is extended, not copied
        let mut operands: Vec<Expr> = if same(&left) {
            match take_kind(&mut left) {
                ExprKind::And(v) | ExprKind::Or(v) => v,
                _ => Vec::new(),
            }
        } else {
            vec![left]
        };
        if same(&r) {
            match take_kind(&mut r) {
                ExprKind::And(v) | ExprKind::Or(v) => operands.extend(v),
                _ => {}
            }
        } else {
            operands.push(r);
        }
        let kind = if is_and {
            ExprKind::And(operands)
        } else {
            ExprKind::Or(operands)
        };
        Ok(Expr::new(kind, SqlType::Boolean, pos))
    }

    /// Give NULL literals the type of their neighbours, check that all can be compared, and normalize the numeric types. `g[0]` is the left operand.
    fn comparable_group(&self, g: &mut [Expr]) -> Result<()> {
        let known = g.iter().find(|x| x.ty != SqlType::Null).map(|x| x.ty);
        let Some(t) = known else {
            return Err(SqlError::not_supported(
                g[0].pos,
                "a comparison of NULLs only (the type is unknown; write CAST(NULL AS type))",
            ));
        };
        for x in g.iter_mut() {
            retype_null(x, t);
        }
        let x_ty = g[0].ty;
        let x_pos = g[0].pos;
        for other in &g[1..] {
            check_comparable(x_ty, other.ty).map_err(|e| te(e, x_pos))?;
        }
        normalize(g);
        Ok(())
    }

    fn compare(&self, op: CmpOp, l: Expr, r: Expr, pos: Pos) -> Result<Expr> {
        let mut g = [l, r];
        self.comparable_group(&mut g)?;
        let [l, r] = g;
        Ok(Expr::new(
            ExprKind::Cmp {
                op,
                left: Box::new(l),
                right: Box::new(r),
            },
            SqlType::Boolean,
            pos,
        ))
    }

    fn arith(&self, op: ArithOp, mut l: Expr, mut r: Expr, pos: Pos) -> Result<Expr> {
        match (l.ty == SqlType::Null, r.ty == SqlType::Null) {
            (true, true) => {
                return Err(SqlError::not_supported(
                    pos,
                    "arithmetic on NULLs only (the type is unknown; write CAST(NULL AS type))",
                ));
            }
            (true, false) => {
                let t = r.ty;
                retype_null(&mut l, t);
            }
            (false, true) => {
                let t = l.ty;
                retype_null(&mut r, t);
            }
            _ => {}
        }
        let ty = arith_type(op, l.ty, r.ty).map_err(|e| te(e, pos))?;
        // operands in the types the kernel takes: one integer type, or decimals
        if l.ty.is_integer() && r.ty.is_integer() && op != ArithOp::Div {
            coerce_in_place(&mut l, ty);
            coerce_in_place(&mut r, ty);
        } else {
            for x in [&mut l, &mut r] {
                if x.ty.is_integer()
                    && let Some((p, s)) = x.ty.as_decimal()
                {
                    coerce_in_place(
                        x,
                        SqlType::Decimal {
                            precision: p,
                            scale: s,
                        },
                    );
                }
            }
        }
        Ok(Expr::new(
            ExprKind::Arith {
                op,
                left: Box::new(l),
                right: Box::new(r),
            },
            ty,
            pos,
        ))
    }

    fn cast(&self, mut x: Expr, dt: &DataType, pos: Pos) -> Result<Expr> {
        let to = match dt {
            DataType::Boolean => SqlType::Boolean,
            DataType::Integer => SqlType::Integer,
            DataType::BigInt => SqlType::BigInt,
            DataType::Double => SqlType::Double,
            DataType::Varchar => SqlType::Varchar,
            DataType::Date => SqlType::Date,
            DataType::Decimal { precision, scale } => {
                if *precision == 0 || scale > precision {
                    return Err(SqlError::bind(
                        pos,
                        format!(
                            "DECIMAL({precision},{scale}) is not a decimal type: the precision is at least 1 and the scale at most the precision"
                        ),
                    ));
                }
                if *precision > 38 {
                    return Err(SqlError::not_supported(
                        pos,
                        format!("DECIMAL({precision},{scale}): at most 38 digits"),
                    ));
                }
                SqlType::Decimal {
                    precision: *precision,
                    scale: *scale,
                }
            }
        };
        if x.is_null_literal() {
            return Ok(Expr::literal(Value::Null, to, pos));
        }
        check_cast(x.ty, to).map_err(|e| te(e, pos))?;
        if x.ty == to {
            x.pos = pos;
            return Ok(x);
        }
        Ok(Expr::new(
            ExprKind::Cast {
                expr: Box::new(x),
                implicit: false,
            },
            to,
            pos,
        ))
    }

    fn case(
        &self,
        operand: Option<&ast::Expr>,
        whens: &[(ast::Expr, ast::Expr)],
        else_: Option<&ast::Expr>,
        pos: Pos,
    ) -> Result<Expr> {
        let mut results: Vec<Expr> = Vec::with_capacity(whens.len() + 1);
        for (_, r) in whens {
            results.push(self.expr(r, None)?);
        }
        results.push(match else_ {
            Some(e) => self.expr(e, None)?,
            None => Expr::literal(Value::Null, SqlType::Null, pos),
        });
        let types: Vec<SqlType> = results.iter().map(|r| r.ty).collect();
        let t = unify_results(&types).map_err(|e| te(e, pos))?;
        if t == SqlType::Null {
            return Err(SqlError::not_supported(
                pos,
                "a CASE whose results are all NULL (the type is unknown; write CAST(NULL AS type))",
            ));
        }
        for r in &mut results {
            retype_null(r, t);
            coerce_in_place(r, t);
        }
        let else_expr = results.pop().unwrap_or_else(placeholder);
        let mut conds: Vec<Expr> = Vec::with_capacity(whens.len());
        let mut op_expr = None;
        match operand {
            None => {
                for (c, _) in whens {
                    let mut x = self.expr(c, Some(SqlType::Boolean))?;
                    retype_null(&mut x, SqlType::Boolean);
                    self.need_boolean(&x, "WHEN")?;
                    conds.push(x);
                }
            }
            Some(o) => {
                let mut g = vec![self.expr(o, None)?];
                for (c, _) in whens {
                    g.push(self.expr(c, None)?);
                }
                self.comparable_group(&mut g)?;
                let mut it = g.into_iter();
                op_expr = it.next();
                conds.extend(it);
            }
        }
        Ok(Expr::new(
            ExprKind::Case {
                operand: op_expr.map(Box::new),
                whens: conds.into_iter().zip(results).collect(),
                else_: Box::new(else_expr),
            },
            t,
            pos,
        ))
    }
}
