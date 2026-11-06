//! The small AST of the slice-4 subset, shared by both arms. Equality ignores spans: the two arms must agree on STRUCTURE and on literal TEXT.
//! Numbers keep their source text (`Number("0.10")` is not `Number("0.1")`), so a decimal can never have passed through a float.

use std::fmt;

/// Byte offsets into the query text. `NONE` for a node an arm cannot place.
#[derive(Debug, Clone, Copy)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub const NONE: Span = Span {
        start: usize::MAX,
        end: usize::MAX,
    };
    pub fn is_none(&self) -> bool {
        self.start == usize::MAX
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
    pub where_: Option<Expr>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SelectItem {
    Star,
    Expr { expr: Expr, alias: Option<Ident> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ident {
    pub name: String,
    pub quoted: bool,
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
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataType {
    Boolean,
    TinyInt,
    SmallInt,
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

    /// A canonical one-line form (an S-expression) for reports, for diffs between arms and for the comparison with DuckDB's own tree.
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
            DataType::TinyInt => f.write_str("TINYINT"),
            DataType::SmallInt => f.write_str("SMALLINT"),
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
                SelectItem::Star => s.push_str(" *"),
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

// ---------------------------------------------------------------- errors

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The text is not valid in the subset's grammar.
    Syntax,
    /// Valid SQL (or plausibly so) outside the subset: `not supported yet: <what>`.
    NotSupported,
    /// A limit of the parser (nesting depth, size).
    Limit,
}

/// A parse error. `line` and `col` are 1-based, the column counts characters. `offset` is the byte offset when the arm knows it.
#[derive(Debug, Clone)]
pub struct ParseError {
    pub kind: ErrorKind,
    pub message: String,
    pub line: Option<usize>,
    pub col: Option<usize>,
    pub offset: Option<usize>,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            ErrorKind::NotSupported => write!(f, "not supported yet: {}", self.message)?,
            ErrorKind::Syntax => write!(f, "syntax error: {}", self.message)?,
            ErrorKind::Limit => write!(f, "query too complex: {}", self.message)?,
        }
        if let (Some(l), Some(c)) = (self.line, self.col) {
            write!(f, " (line {l}, column {c})")?;
        }
        Ok(())
    }
}

impl std::error::Error for ParseError {}

/// Line and column (1-based; the column counts characters) of a byte offset. An offset past the end gives the position after the last character.
pub fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for (i, ch) in text.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// The byte offset of a (line, column) pair, the inverse of `line_col`; `None` when the position is outside the text.
pub fn offset_of(text: &str, line: usize, col: usize) -> Option<usize> {
    let mut l = 1;
    let mut c = 1;
    for (i, ch) in text.char_indices() {
        if l == line && c == col {
            return Some(i);
        }
        if ch == '\n' {
            l += 1;
            c = 1;
        } else {
            c += 1;
        }
    }
    if l == line && c == col {
        Some(text.len())
    } else {
        None
    }
}
