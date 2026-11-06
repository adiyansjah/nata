//! The typed expression tree that the binder produces and step 3 evaluates: every node has its exact SQL type and its position in the query (for the
//! `execution` errors of step 3), column references are resolved to indices, literals are values.
//!
//! Contract for the evaluator (dialect sections 4 to 6, `decimal-rules.md`):
//! - the operands of an `Arith` node are already in the types the kernel expects: both the same integer type, or both decimals (an integer meeting a
//!   decimal, and both sides of an integer `/`, are widened by an explicit [`ExprKind::Cast`] with `implicit: true`); the node's `ty` is the result type;
//! - the operands of a `Cmp`, the three operands of a `Between` and the list of an `InList` are likewise the same integer type, or decimals (of any
//!   scale: compare exactly), or the same non-numeric type;
//! - `And`, `Or` hold two or more operands, evaluated left to right with the selection of the earlier ones (errors of a masked row are not raised);
//! - `Case` always has an `else_` (`ELSE NULL` is a typed NULL literal); only the taken branch is evaluated for a row;
//! - a nested tree may be as deep as 10,000 (a chain of 10,000 terms): evaluate on a thread with a large stack or iterate.

use crate::error::Pos;
use crate::types::{ArithOp, SqlType};

/// A literal value; the node's `ty` says how to read it.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// TINYINT, SMALLINT, INTEGER, BIGINT.
    Int(i64),
    /// DECIMAL(p, s): the unscaled value (`1.50` of DECIMAL(3,2) is 150).
    Decimal(i128),
    Str(String),
    /// Days since 1970-01-01.
    Date(i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    pub fn symbol(self) -> &'static str {
        match self {
            CmpOp::Eq => "=",
            CmpOp::Ne => "<>",
            CmpOp::Lt => "<",
            CmpOp::Le => "<=",
            CmpOp::Gt => ">",
            CmpOp::Ge => ">=",
        }
    }

    /// The operator with its operands swapped: `5 < x` is `x > 5`.
    pub fn flip(self) -> CmpOp {
        match self {
            CmpOp::Eq => CmpOp::Eq,
            CmpOp::Ne => CmpOp::Ne,
            CmpOp::Lt => CmpOp::Gt,
            CmpOp::Le => CmpOp::Ge,
            CmpOp::Gt => CmpOp::Lt,
            CmpOp::Ge => CmpOp::Le,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: SqlType,
    pub pos: Pos,
}

#[derive(Debug, PartialEq)]
pub enum ExprKind {
    /// An index into the column list of the operator the expression belongs to (the scan's `columns` for a filter or a projection).
    Column(usize),
    Literal(Value),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Arith {
        op: ArithOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Cmp {
        op: CmpOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    And(Vec<Expr>),
    Or(Vec<Expr>),
    IsNull {
        expr: Box<Expr>,
        negated: bool,
    },
    /// `expr >= low AND expr <= high` (negated: NOT of it), without evaluating `expr` twice.
    Between {
        expr: Box<Expr>,
        low: Box<Expr>,
        high: Box<Expr>,
        negated: bool,
    },
    /// `expr = v1 OR expr = v2 ...` (negated: NOT of it); the list holds literals.
    InList {
        expr: Box<Expr>,
        list: Vec<Expr>,
        negated: bool,
    },
    /// With an `operand`: `CASE operand WHEN v THEN r`, the conditions being values to compare with it; without: conditions are booleans.
    Case {
        operand: Option<Box<Expr>>,
        whens: Vec<(Expr, Expr)>,
        else_: Box<Expr>,
    },
    /// Convert to this node's `ty`. `implicit` is a widening the binder inserted (it cannot fail); an explicit CAST can.
    Cast {
        expr: Box<Expr>,
        implicit: bool,
    },
}

impl Expr {
    pub fn new(kind: ExprKind, ty: SqlType, pos: Pos) -> Expr {
        Expr { kind, ty, pos }
    }

    pub fn literal(v: Value, ty: SqlType, pos: Pos) -> Expr {
        Expr::new(ExprKind::Literal(v), ty, pos)
    }

    pub fn is_literal(&self) -> bool {
        matches!(self.kind, ExprKind::Literal(_))
    }

    pub fn is_null_literal(&self) -> bool {
        matches!(self.kind, ExprKind::Literal(Value::Null))
    }

    /// Move the children out (leaving a NULL literal in `self`), so that a deep tree can be dropped or visited with a loop instead of recursion.
    fn take_children(&mut self, out: &mut Vec<Expr>) {
        let kind = std::mem::replace(&mut self.kind, ExprKind::Literal(Value::Null));
        match kind {
            ExprKind::Column(_) | ExprKind::Literal(_) => {}
            ExprKind::Neg(e) | ExprKind::Not(e) => out.push(*e),
            ExprKind::Arith { left, right, .. } | ExprKind::Cmp { left, right, .. } => {
                out.push(*left);
                out.push(*right);
            }
            ExprKind::And(v) | ExprKind::Or(v) => out.extend(v),
            ExprKind::IsNull { expr, .. } | ExprKind::Cast { expr, .. } => out.push(*expr),
            ExprKind::Between {
                expr, low, high, ..
            } => {
                out.push(*expr);
                out.push(*low);
                out.push(*high);
            }
            ExprKind::InList { expr, list, .. } => {
                out.push(*expr);
                out.extend(list);
            }
            ExprKind::Case {
                operand,
                whens,
                else_,
            } => {
                out.extend(operand.map(|b| *b));
                for (c, r) in whens {
                    out.push(c);
                    out.push(r);
                }
                out.push(*else_);
            }
        }
    }

    /// The direct children, in evaluation order.
    pub fn children(&self) -> Vec<&Expr> {
        match &self.kind {
            ExprKind::Column(_) | ExprKind::Literal(_) => vec![],
            ExprKind::Neg(e) | ExprKind::Not(e) => vec![e],
            ExprKind::Arith { left, right, .. } | ExprKind::Cmp { left, right, .. } => {
                vec![left, right]
            }
            ExprKind::And(v) | ExprKind::Or(v) => v.iter().collect(),
            ExprKind::IsNull { expr, .. } | ExprKind::Cast { expr, .. } => vec![expr],
            ExprKind::Between {
                expr, low, high, ..
            } => vec![expr, low, high],
            ExprKind::InList { expr, list, .. } => {
                std::iter::once(&**expr).chain(list.iter()).collect()
            }
            ExprKind::Case {
                operand,
                whens,
                else_,
            } => {
                let mut v: Vec<&Expr> = operand.iter().map(|b| &**b).collect();
                for (c, r) in whens {
                    v.push(c);
                    v.push(r);
                }
                v.push(else_);
                v
            }
        }
    }

    pub fn children_mut(&mut self) -> Vec<&mut Expr> {
        match &mut self.kind {
            ExprKind::Column(_) | ExprKind::Literal(_) => vec![],
            ExprKind::Neg(e) | ExprKind::Not(e) => vec![e],
            ExprKind::Arith { left, right, .. } | ExprKind::Cmp { left, right, .. } => {
                vec![left, right]
            }
            ExprKind::And(v) | ExprKind::Or(v) => v.iter_mut().collect(),
            ExprKind::IsNull { expr, .. } | ExprKind::Cast { expr, .. } => vec![expr],
            ExprKind::Between {
                expr, low, high, ..
            } => vec![expr, low, high],
            ExprKind::InList { expr, list, .. } => std::iter::once(&mut **expr)
                .chain(list.iter_mut())
                .collect(),
            ExprKind::Case {
                operand,
                whens,
                else_,
            } => {
                let mut v: Vec<&mut Expr> = operand.iter_mut().map(|b| &mut **b).collect();
                for (c, r) in whens {
                    v.push(c);
                    v.push(r);
                }
                v.push(else_);
                v
            }
        }
    }

    /// Visit every node (parents before children) with a loop: safe on a tree of any depth.
    pub fn for_each(&self, mut f: impl FnMut(&Expr)) {
        let mut stack = vec![self];
        while let Some(e) = stack.pop() {
            f(e);
            let mut kids = e.children();
            kids.reverse();
            stack.extend(kids);
        }
    }

    /// As [`Expr::for_each`], with mutable access.
    pub fn for_each_mut(&mut self, mut f: impl FnMut(&mut Expr)) {
        let mut stack = vec![self];
        while let Some(e) = stack.pop() {
            f(e);
            stack.extend(e.children_mut());
        }
    }

    /// Replace every column index `i` by `map(i)`.
    pub fn remap_columns(&mut self, map: impl Fn(usize) -> usize) {
        self.for_each_mut(|e| {
            if let ExprKind::Column(i) = &mut e.kind {
                *i = map(*i);
            }
        });
    }

    /// The column indices the expression reads, each once, in order of first use.
    pub fn columns(&self) -> Vec<usize> {
        let mut v = Vec::new();
        self.for_each(|e| {
            if let ExprKind::Column(i) = e.kind
                && !v.contains(&i)
            {
                v.push(i);
            }
        });
        v
    }
}

/// A deep tree (a chain of 10,000 terms is left-nested 10,000 deep) is dropped with a loop: the derived drop would recurse once per level.
impl Drop for Expr {
    fn drop(&mut self) {
        let mut stack: Vec<Expr> = Vec::new();
        self.take_children(&mut stack);
        while let Some(mut e) = stack.pop() {
            e.take_children(&mut stack);
        }
    }
}
