//! The syntax tree of the subset of `docs/dialect.md` sections 1 to 7. Both the `sqlparser` conversion (`parse.rs`) and the test-only hand-written
//! oracle parser (`oracle/`) produce it, and the tests compare them: equality ignores positions and compares STRUCTURE and literal TEXT.
//! Numbers keep their source text (`Number("0.10")` is not `Number("0.1")`), so a decimal never passes through a float.

use std::fmt;

use crate::error::Pos;

/// The position of the first character of a node. Two spans are ALWAYS equal, so `#[derive(PartialEq)]` on the tree compares structure only.
#[derive(Debug, Clone, Copy)]
pub struct Span(pub Pos);

impl Span {
    pub const NONE: Span = Span(Pos::NONE);
}

impl PartialEq for Span {
    fn eq(&self, _: &Span) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExplainMode {
    No,
    Plan,
    Analyze,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub explain: ExplainMode,
    pub items: Vec<SelectItem>,
    /// The path or glob, unescaped.
    pub from: String,
    pub from_pos: Span,
    pub where_: Option<Expr>,
    pub limit: Option<u64>,
    pub limit_pos: Span,
    pub offset: Option<u64>,
    pub offset_pos: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SelectItem {
    Star(Span),
    Expr { expr: Expr, alias: Option<Ident> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ident {
    pub name: String,
    pub quoted: bool,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    /// `//`, integer division.
    IntDiv,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
}

/// The target types of `CAST` (dialect section 6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataType {
    Boolean,
    Integer,
    BigInt,
    Double,
    Decimal { precision: u8, scale: u8 },
    Varchar,
    Date,
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

impl PartialEq for Expr {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    Ident(Ident),
    /// The exact source text of the number: digits, optional `.digits`, optional exponent. The sign is a `Unary(Neg)` above it.
    Number(String),
    Str(String),
    Date(String),
    Bool(bool),
    Null,
    Unary {
        op: UnOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Between {
        expr: Box<Expr>,
        low: Box<Expr>,
        high: Box<Expr>,
        negated: bool,
    },
    InList {
        expr: Box<Expr>,
        list: Vec<Expr>,
        negated: bool,
    },
    IsNull {
        expr: Box<Expr>,
        negated: bool,
    },
    Case {
        operand: Option<Box<Expr>>,
        whens: Vec<(Expr, Expr)>,
        else_: Option<Box<Expr>>,
    },
    Cast {
        expr: Box<Expr>,
        ty: DataType,
    },
}

impl Expr {
    pub fn new(kind: ExprKind, span: Span) -> Expr {
        Expr { kind, span }
    }

    pub fn pos(&self) -> Pos {
        self.span.0
    }

    /// A canonical one-line form (an S-expression) for tests and for diffs between the two parsers.
    pub fn sexp(&self) -> String {
        let mut s = String::new();
        self.write_sexp(&mut s);
        s
    }

    fn write_sexp(&self, s: &mut String) {
        use std::fmt::Write;
        match &self.kind {
            ExprKind::Ident(i) => {
                let _ = write!(s, "(id{} {})", if i.quoted { "q" } else { "" }, i.name);
            }
            ExprKind::Number(t) => {
                let _ = write!(s, "(num {t})");
            }
            ExprKind::Str(t) => {
                let _ = write!(s, "(str {t:?})");
            }
            ExprKind::Date(t) => {
                let _ = write!(s, "(date {t:?})");
            }
            ExprKind::Bool(b) => {
                let _ = write!(s, "(bool {b})");
            }
            ExprKind::Null => s.push_str("(null)"),
            ExprKind::Unary { op, expr } => {
                let _ = write!(s, "({} ", if *op == UnOp::Neg { "neg" } else { "not" });
                expr.write_sexp(s);
                s.push(')');
            }
            ExprKind::Binary { op, left, right } => {
                let _ = write!(s, "({} ", op.name());
                left.write_sexp(s);
                s.push(' ');
                right.write_sexp(s);
                s.push(')');
            }
            ExprKind::Between {
                expr,
                low,
                high,
                negated,
            } => {
                s.push_str(if *negated {
                    "(not-between "
                } else {
                    "(between "
                });
                expr.write_sexp(s);
                s.push(' ');
                low.write_sexp(s);
                s.push(' ');
                high.write_sexp(s);
                s.push(')');
            }
            ExprKind::InList {
                expr,
                list,
                negated,
            } => {
                s.push_str(if *negated { "(not-in " } else { "(in " });
                expr.write_sexp(s);
                for e in list {
                    s.push(' ');
                    e.write_sexp(s);
                }
                s.push(')');
            }
            ExprKind::IsNull { expr, negated } => {
                s.push_str(if *negated {
                    "(is-not-null "
                } else {
                    "(is-null "
                });
                expr.write_sexp(s);
                s.push(')');
            }
            ExprKind::Case {
                operand,
                whens,
                else_,
            } => {
                s.push_str("(case");
                if let Some(o) = operand {
                    s.push(' ');
                    o.write_sexp(s);
                }
                for (c, v) in whens {
                    s.push_str(" (when ");
                    c.write_sexp(s);
                    s.push(' ');
                    v.write_sexp(s);
                    s.push(')');
                }
                if let Some(e) = else_ {
                    s.push_str(" (else ");
                    e.write_sexp(s);
                    s.push(')');
                }
                s.push(')');
            }
            ExprKind::Cast { expr, ty } => {
                let _ = write!(s, "(cast {ty} ");
                expr.write_sexp(s);
                s.push(')');
            }
        }
    }
}

impl BinOp {
    pub fn name(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::IntDiv => "//",
            BinOp::Eq => "=",
            BinOp::NotEq => "<>",
            BinOp::Lt => "<",
            BinOp::LtEq => "<=",
            BinOp::Gt => ">",
            BinOp::GtEq => ">=",
            BinOp::And => "and",
            BinOp::Or => "or",
        }
    }
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataType::Boolean => f.write_str("BOOLEAN"),
            DataType::Integer => f.write_str("INTEGER"),
            DataType::BigInt => f.write_str("BIGINT"),
            DataType::Double => f.write_str("DOUBLE"),
            DataType::Decimal { precision, scale } => write!(f, "DECIMAL({precision},{scale})"),
            DataType::Varchar => f.write_str("VARCHAR"),
            DataType::Date => f.write_str("DATE"),
        }
    }
}

impl Query {
    /// The whole query as one canonical line.
    pub fn sexp(&self) -> String {
        let mut s = String::new();
        s.push_str(match self.explain {
            ExplainMode::No => "(query",
            ExplainMode::Plan => "(explain",
            ExplainMode::Analyze => "(explain-analyze",
        });
        for it in &self.items {
            match it {
                SelectItem::Star(_) => s.push_str(" *"),
                SelectItem::Expr { expr, alias } => {
                    s.push_str(" [");
                    s.push_str(&expr.sexp());
                    if let Some(a) = alias {
                        s.push_str(&format!(
                            " as {}{}",
                            if a.quoted { "q:" } else { "" },
                            a.name
                        ));
                    }
                    s.push(']');
                }
            }
        }
        s.push_str(&format!(" from {:?}", self.from));
        if let Some(w) = &self.where_ {
            s.push_str(" where ");
            s.push_str(&w.sexp());
        }
        if let Some(l) = self.limit {
            s.push_str(&format!(" limit {l}"));
        }
        if let Some(o) = self.offset {
            s.push_str(&format!(" offset {o}"));
        }
        s.push(')');
        s
    }
}
