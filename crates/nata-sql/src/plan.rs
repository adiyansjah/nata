//! The logical plan of a query and the pushdown of rule R2 (card 0002):
//!
//! `Scan { files, columns, pushed } -> Filter { remaining } -> Project { exprs } -> Limit { n, offset }`
//!
//! The WHERE clause is split at its top-level ANDs. A conjunct is PUSHED into the scan only if it is `column <op> literal` (either order), `column IS
//! [NOT] NULL`, or a non-negated `column BETWEEN literal AND literal` (two conjuncts), on a column of the file, with a literal that the scan's own typing
//! accepts for that column's kind without conversion. A pushed conjunct is enforced by the scan's EXACT row filter (slice 3: the same SQL semantics) as well
//! as used for pruning, so it is not repeated in the `Filter`; pushdown can skip work and never change a result. Everything else (`OR`, `NOT`,
//! expressions, functions of columns, `IN`, comparisons between columns, casts) stays in the `Filter`.

use std::path::PathBuf;

use nata_parquet::ScanOptions;
use nata_parquet::civil;
use nata_parquet::filter::{Literal, Term, Test};
use nata_parquet::schema::Kind;
use nata_scan::CmpOp as ScanOp;

use crate::ast::ExplainMode;
use crate::bind::{Bound, digits};
use crate::error::Result;
use crate::keywords::is_reserved_word;
use crate::texpr::{CmpOp, Expr, ExprKind, Value};
use crate::types::SqlType;

/// A column the scan delivers to the operators above it.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanColumn {
    pub name: String,
    pub ty: SqlType,
    pub kind: Kind,
}

/// A conjunct handed to the scan: typed for the scan (`nata_parquet::filter::Term`), with the column it is about.
#[derive(Debug, Clone, PartialEq)]
pub struct PushedTerm {
    pub column: String,
    pub kind: Kind,
    pub term: Term,
}

#[derive(Debug, PartialEq)]
pub struct Scan {
    /// The path or glob as written.
    pub pattern: String,
    pub files: Vec<PathBuf>,
    /// The columns delivered, in this order; the column indices of the expressions above refer to this list.
    pub columns: Vec<ScanColumn>,
    pub pushed: Vec<PushedTerm>,
}

#[derive(Debug, PartialEq)]
pub struct Filter {
    /// What is left of the WHERE clause after the pushed conjuncts; a BOOLEAN expression over `Scan::columns`.
    pub predicate: Expr,
}

#[derive(Debug, PartialEq)]
pub struct ProjectExpr {
    pub name: String,
    pub expr: Expr,
}

#[derive(Debug, PartialEq)]
pub struct Project {
    pub exprs: Vec<ProjectExpr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limit {
    pub n: u64,
    pub offset: u64,
}

#[derive(Debug, PartialEq)]
pub struct LogicalPlan {
    pub scan: Scan,
    pub filter: Option<Filter>,
    pub project: Project,
    pub limit: Option<Limit>,
}

/// The planned statement: what to print for `EXPLAIN` / `EXPLAIN ANALYZE`, or to run.
#[derive(Debug, PartialEq)]
pub struct Planned {
    pub mode: ExplainMode,
    pub plan: LogicalPlan,
    /// The result columns (name and type), in order.
    pub output: Vec<OutputColumn>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OutputColumn {
    pub name: String,
    pub ty: SqlType,
}

// ---------------------------------------------------------------------------------------------------------------------------------- planning

pub fn plan(b: Bound) -> Result<Planned> {
    let Bound {
        explain,
        source,
        items,
        where_,
        limit,
        offset,
    } = b;
    // 1. split the WHERE clause and push what can be pushed
    let mut pushed: Vec<PushedTerm> = Vec::new();
    let mut remaining: Vec<Expr> = Vec::new();
    if let Some(mut w) = where_ {
        let conjuncts = match w.kind {
            ExprKind::And(_) => {
                match std::mem::replace(&mut w.kind, ExprKind::Literal(Value::Null)) {
                    ExprKind::And(v) => v,
                    _ => Vec::new(),
                }
            }
            _ => vec![w],
        };
        for c in conjuncts {
            match try_push(&c, &source.columns) {
                Some(terms) => pushed.extend(terms),
                None => remaining.push(c),
            }
        }
    }
    // 2. the columns the operators above the scan read: projected first (in order of first use), then those only the remaining filter needs
    let mut used: Vec<usize> = Vec::new();
    for it in &items {
        for i in it.expr.columns() {
            if !used.contains(&i) {
                used.push(i);
            }
        }
    }
    for r in &remaining {
        for i in r.columns() {
            if !used.contains(&i) {
                used.push(i);
            }
        }
    }
    let mut position = vec![usize::MAX; source.columns.len()];
    for (pos, &src) in used.iter().enumerate() {
        position[src] = pos;
    }
    let columns: Vec<ScanColumn> = used
        .iter()
        .filter_map(|&i| {
            let c = &source.columns[i];
            match (&c.ty, &c.kind) {
                (Ok(ty), Ok(kind)) => Some(ScanColumn {
                    name: c.name.clone(),
                    ty: *ty,
                    kind: *kind,
                }),
                _ => None,
            }
        })
        .collect();
    let remap = |i: usize| position.get(i).copied().unwrap_or(usize::MAX);
    // 3. the operators
    let predicate = match remaining.len() {
        0 => None,
        1 => remaining.pop(),
        _ => {
            let pos = remaining[0].pos;
            Some(Expr::new(ExprKind::And(remaining), SqlType::Boolean, pos))
        }
    }
    .map(|mut p| {
        p.remap_columns(remap);
        Filter { predicate: p }
    });
    let mut exprs = Vec::with_capacity(items.len());
    let mut output = Vec::with_capacity(items.len());
    for it in items {
        let mut e = it.expr;
        e.remap_columns(remap);
        output.push(OutputColumn {
            name: it.name.clone(),
            ty: e.ty,
        });
        exprs.push(ProjectExpr {
            name: it.name,
            expr: e,
        });
    }
    Ok(Planned {
        mode: explain,
        plan: LogicalPlan {
            scan: Scan {
                pattern: source.pattern,
                files: source.files,
                columns,
                pushed,
            },
            filter: predicate,
            project: Project { exprs },
            limit: limit.map(|n| Limit {
                n,
                offset: offset.unwrap_or(0),
            }),
        },
        output,
    })
}

fn scan_op(op: CmpOp) -> ScanOp {
    match op {
        CmpOp::Eq => ScanOp::Eq,
        CmpOp::Ne => ScanOp::Ne,
        CmpOp::Lt => ScanOp::Lt,
        CmpOp::Le => ScanOp::Le,
        CmpOp::Gt => ScanOp::Gt,
        CmpOp::Ge => ScanOp::Ge,
    }
}

/// The literal as the scan types it for a column of `kind`, or `None` if the scan would have to convert it (then the conjunct is not pushed).
fn scan_literal(kind: Kind, e: &Expr) -> Option<Literal> {
    let ExprKind::Literal(v) = &e.kind else {
        return None;
    };
    match (kind, v) {
        (Kind::Bool, Value::Bool(b)) => Some(Literal::Bool(*b)),
        (Kind::Int { signed: true, .. }, Value::Int(i)) if e.ty.is_integer() => {
            Some(Literal::Int(i128::from(*i)))
        }
        (Kind::Int { signed: true, .. }, Value::Decimal(u))
            if matches!(e.ty, SqlType::Decimal { scale: 0, .. }) =>
        {
            Some(Literal::Int(*u))
        }
        (Kind::Decimal { .. }, Value::Int(i)) if e.ty.is_integer() => {
            let u = i128::from(*i);
            Some(Literal::Decimal {
                unscaled: u,
                precision: u8::try_from(digits(u).max(1)).ok()?,
                scale: 0,
            })
        }
        (Kind::Decimal { .. }, Value::Decimal(u)) => {
            let SqlType::Decimal { scale, .. } = e.ty else {
                return None;
            };
            Some(Literal::Decimal {
                unscaled: *u,
                precision: u8::try_from(digits(*u).max(u32::from(scale)).max(1)).ok()?,
                scale,
            })
        }
        (Kind::Date, Value::Date(d)) => Some(Literal::Date(*d)),
        (Kind::Utf8, Value::Str(s)) => Some(Literal::Text(s.clone())),
        _ => None,
    }
}

fn bare_column(e: &Expr, cols: &[crate::source::SourceColumn]) -> Option<(String, Kind)> {
    let ExprKind::Column(i) = e.kind else {
        return None;
    };
    let c = cols.get(i)?;
    Some((c.name.clone(), *c.kind.as_ref().ok()?))
}

fn cmp_term(column: String, kind: Kind, op: CmpOp, lit: &Expr) -> Option<PushedTerm> {
    let l = scan_literal(kind, lit)?;
    Some(PushedTerm {
        term: Term {
            column: column.clone(),
            test: Test::Cmp(scan_op(op), l),
        },
        column,
        kind,
    })
}

/// The pushed form of a conjunct, or `None` (rule R2).
fn try_push(c: &Expr, cols: &[crate::source::SourceColumn]) -> Option<Vec<PushedTerm>> {
    match &c.kind {
        ExprKind::Cmp { op, left, right } => {
            if let Some((name, kind)) = bare_column(left, cols) {
                return cmp_term(name, kind, *op, right).map(|t| vec![t]);
            }
            if let Some((name, kind)) = bare_column(right, cols) {
                return cmp_term(name, kind, op.flip(), left).map(|t| vec![t]);
            }
            None
        }
        ExprKind::IsNull { expr, negated } => {
            let (name, kind) = bare_column(expr, cols)?;
            Some(vec![PushedTerm {
                term: Term {
                    column: name.clone(),
                    test: if *negated {
                        Test::IsNotNull
                    } else {
                        Test::IsNull
                    },
                },
                column: name,
                kind,
            }])
        }
        ExprKind::Between {
            expr,
            low,
            high,
            negated: false,
        } => {
            let (name, kind) = bare_column(expr, cols)?;
            let lo = cmp_term(name.clone(), kind, CmpOp::Ge, low)?;
            let hi = cmp_term(name, kind, CmpOp::Le, high)?;
            Some(vec![lo, hi])
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------------------------------------------- the scan's text form

fn quote_ident(n: &str) -> String {
    let simple = !n.is_empty()
        && n.chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && n.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '$')
        && !is_reserved_word(n);
    if simple {
        n.to_string()
    } else {
        format!("\"{}\"", n.replace('"', "\"\""))
    }
}

fn decimal_text(unscaled: i128, scale: u8) -> String {
    let neg = unscaled < 0;
    let mut d = unscaled.unsigned_abs().to_string();
    let scale = usize::from(scale);
    if scale > 0 {
        while d.len() <= scale {
            d.insert(0, '0');
        }
        d.insert(scale_point(d.len(), scale), '.');
    }
    if neg { format!("-{d}") } else { d }
}

fn scale_point(len: usize, scale: usize) -> usize {
    len - scale
}

fn op_symbol(op: ScanOp) -> &'static str {
    match op {
        ScanOp::Lt => "<",
        ScanOp::Le => "<=",
        ScanOp::Gt => ">",
        ScanOp::Ge => ">=",
        ScanOp::Eq => "=",
        ScanOp::Ne => "<>",
    }
}

impl PushedTerm {
    fn literal_text(l: &Literal, sql: bool) -> String {
        match l {
            Literal::Bool(b) => {
                if sql {
                    b.to_string().to_uppercase()
                } else {
                    b.to_string()
                }
            }
            Literal::Int(i) => i.to_string(),
            Literal::Decimal {
                unscaled, scale, ..
            } => decimal_text(*unscaled, *scale),
            Literal::Date(d) => {
                let t = civil::format_date(*d);
                if sql { format!("DATE '{t}'") } else { t }
            }
            Literal::Text(s) => format!("'{}'", s.replace('\'', "''")),
            Literal::F32(f) => f.to_string(),
            Literal::F64(f) => f.to_string(),
            Literal::TimestampMicros(t) => t.to_string(),
            Literal::Bytes(b) => format!(
                "X'{}'",
                b.iter().map(|x| format!("{x:02X}")).collect::<String>()
            ),
        }
    }

    fn render(&self, sql: bool) -> String {
        let col = quote_ident(&self.column);
        match &self.term.test {
            Test::IsNull => format!("{col} IS NULL"),
            Test::IsNotNull => format!("{col} IS NOT NULL"),
            Test::Cmp(op, lit) => {
                format!("{col} {} {}", op_symbol(*op), Self::literal_text(lit, sql))
            }
        }
    }

    /// The conjunct in SQL, for the plan text: `l_shipdate >= DATE '1994-01-01'`.
    pub fn sql(&self) -> String {
        self.render(true)
    }

    /// The conjunct in the text form that `nata_parquet::filter` parses (`ScanOptions::filters`): `"l_shipdate" >= 1994-01-01`. Parsing it with the
    /// column's kind gives back `self.term` (a unit test holds that).
    pub fn filter_text(&self) -> String {
        let col = format!("\"{}\"", self.column.replace('"', "\"\""));
        match &self.term.test {
            Test::IsNull => format!("{col} IS NULL"),
            Test::IsNotNull => format!("{col} IS NOT NULL"),
            Test::Cmp(op, lit) => format!(
                "{col} {} {}",
                op_symbol(*op),
                Self::literal_text(lit, false)
            ),
        }
    }
}

impl Scan {
    /// The options for `nata_parquet::ScanPlan::open`: the columns delivered, the pushed conjuncts as filters (pruning on). With no column delivered
    /// (`SELECT 1 FROM 'f'`) the list is empty, which the scan refuses today: step 3 must read a column for the row count.
    pub fn scan_options(&self) -> ScanOptions {
        ScanOptions {
            columns: Some(self.columns.iter().map(|c| c.name.clone()).collect()),
            filters: self.pushed.iter().map(PushedTerm::filter_text).collect(),
            prune: true,
            late_materialization: false,
            // the binder read only the first footer: the scan checks the whole schema of every other file when it reaches it
            strict_schema: true,
        }
    }

    /// The columns the scan reads: those it delivers, then those only a pushed conjunct needs.
    pub fn columns_read(&self) -> Vec<String> {
        let mut v: Vec<String> = self.columns.iter().map(|c| c.name.clone()).collect();
        for p in &self.pushed {
            if !v.contains(&p.column) {
                v.push(p.column.clone());
            }
        }
        v
    }
}

// ---------------------------------------------------------------------------------------------------------------------------------- printing

fn literal_sql(v: &Value, ty: SqlType) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        Value::Int(i) => i.to_string(),
        Value::Decimal(u) => match ty {
            SqlType::Decimal { scale, .. } => decimal_text(*u, scale),
            _ => u.to_string(),
        },
        Value::Str(s) => format!("'{}'", s.replace('\'', "''")),
        Value::Date(d) => format!("DATE '{}'", civil::format_date(*d)),
    }
}

enum W<'a> {
    E(&'a Expr),
    T(String),
    S(&'static str),
}

/// An expression as SQL, every operator parenthesised, columns by name. Written with a work list, not recursion: a chain of 10,000 terms is a tree
/// 10,000 deep.
pub fn render_expr(root: &Expr, names: &[String]) -> String {
    let mut out = String::new();
    let mut stack: Vec<W<'_>> = vec![W::E(root)];
    // pieces are pushed in reverse order
    while let Some(w) = stack.pop() {
        let e = match w {
            W::T(t) => {
                out.push_str(&t);
                continue;
            }
            W::S(t) => {
                out.push_str(t);
                continue;
            }
            W::E(e) => e,
        };
        match &e.kind {
            ExprKind::Column(i) => {
                out.push_str(
                    &names
                        .get(*i)
                        .map_or_else(|| format!("#{i}"), |n| quote_ident(n)),
                );
            }
            ExprKind::Literal(v) => out.push_str(&literal_sql(v, e.ty)),
            ExprKind::Neg(x) => stack.extend([W::S(")"), W::E(x), W::S("(-")]),
            ExprKind::Not(x) => stack.extend([W::S(")"), W::E(x), W::S("(NOT ")]),
            ExprKind::Arith { op, left, right } => stack.extend([
                W::S(")"),
                W::E(right),
                W::T(format!(" {} ", op.symbol())),
                W::E(left),
                W::S("("),
            ]),
            ExprKind::Cmp { op, left, right } => stack.extend([
                W::S(")"),
                W::E(right),
                W::T(format!(" {} ", op.symbol())),
                W::E(left),
                W::S("("),
            ]),
            ExprKind::And(v) | ExprKind::Or(v) => {
                let sep = if matches!(e.kind, ExprKind::And(_)) {
                    " AND "
                } else {
                    " OR "
                };
                stack.push(W::S(")"));
                for (k, x) in v.iter().enumerate().rev() {
                    stack.push(W::E(x));
                    if k > 0 {
                        stack.push(W::S(sep));
                    }
                }
                stack.push(W::S("("));
            }
            ExprKind::IsNull { expr, negated } => stack.extend([
                W::S(if *negated {
                    " IS NOT NULL)"
                } else {
                    " IS NULL)"
                }),
                W::E(expr),
                W::S("("),
            ]),
            ExprKind::Between {
                expr,
                low,
                high,
                negated,
            } => stack.extend([
                W::S(")"),
                W::E(high),
                W::S(" AND "),
                W::E(low),
                W::S(if *negated {
                    " NOT BETWEEN "
                } else {
                    " BETWEEN "
                }),
                W::E(expr),
                W::S("("),
            ]),
            ExprKind::InList {
                expr,
                list,
                negated,
            } => {
                stack.push(W::S("))"));
                for (k, x) in list.iter().enumerate().rev() {
                    stack.push(W::E(x));
                    if k > 0 {
                        stack.push(W::S(", "));
                    }
                }
                stack.extend([
                    W::S(if *negated { " NOT IN (" } else { " IN (" }),
                    W::E(expr),
                    W::S("("),
                ]);
            }
            ExprKind::Case {
                operand,
                whens,
                else_,
            } => {
                stack.extend([W::S(" END"), W::E(else_), W::S(" ELSE ")]);
                for (c, v) in whens.iter().rev() {
                    stack.extend([W::E(v), W::S(" THEN "), W::E(c), W::S(" WHEN ")]);
                }
                if let Some(o) = operand {
                    stack.extend([W::E(o), W::S(" ")]);
                }
                stack.push(W::S("CASE"));
            }
            ExprKind::Cast { expr, implicit } => {
                if *implicit {
                    stack.extend([W::T(format!("::{}", e.ty)), W::E(expr)]);
                } else {
                    stack.extend([W::T(format!(" AS {})", e.ty)), W::E(expr), W::S("CAST(")]);
                }
            }
        }
    }
    out
}

impl LogicalPlan {
    /// The plan as text, the operators from the last (printed first) to the scan; this is what `EXPLAIN` shows. Pushed conjuncts are listed on the scan,
    /// the filter holds what is left.
    pub fn explain_text(&self) -> String {
        match crate::parse::on_big_stack(|| self.render()) {
            Ok(s) => s,
            Err(_) => "(the plan could not be printed)".to_string(),
        }
    }

    fn render(&self) -> String {
        let names: Vec<String> = self.scan.columns.iter().map(|c| c.name.clone()).collect();
        let mut out = String::new();
        let mut depth = 0usize;
        let mut line = |depth: usize, s: String| {
            out.push_str(&"  ".repeat(depth));
            out.push_str(&s);
            out.push('\n');
        };
        if let Some(l) = &self.limit {
            line(depth, format!("Limit {} offset {}", l.n, l.offset));
            depth += 1;
        }
        let exprs: Vec<String> = self
            .project
            .exprs
            .iter()
            .map(|p| {
                let text = render_expr(&p.expr, &names);
                let shown = if text == quote_ident(&p.name) {
                    text
                } else {
                    format!("{text} AS {}", quote_ident(&p.name))
                };
                format!("{shown}: {}", p.expr.ty)
            })
            .collect();
        line(depth, format!("Project {}", exprs.join(", ")));
        depth += 1;
        if let Some(f) = &self.filter {
            line(
                depth,
                format!("Filter {}", render_expr(&f.predicate, &names)),
            );
            depth += 1;
        }
        let files = self.scan.files.len();
        line(
            depth,
            format!(
                "Scan '{}' ({files} file{})",
                self.scan.pattern.replace('\'', "''"),
                if files == 1 { "" } else { "s" }
            ),
        );
        depth += 1;
        let cols = if names.is_empty() {
            "(none)".to_string()
        } else {
            names
                .iter()
                .map(|n| quote_ident(n))
                .collect::<Vec<_>>()
                .join(", ")
        };
        line(depth, format!("columns: {cols}"));
        if self.scan.pushed.is_empty() {
            line(depth, "pushed: (none)".to_string());
        } else {
            line(
                depth,
                format!(
                    "pushed: {}",
                    self.scan
                        .pushed
                        .iter()
                        .map(PushedTerm::sql)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
            let extra: Vec<String> = self
                .scan
                .columns_read()
                .into_iter()
                .skip(self.scan.columns.len())
                .map(|n| quote_ident(&n))
                .collect();
            if !extra.is_empty() {
                line(
                    depth,
                    format!("also read for the pushed filter: {}", extra.join(", ")),
                );
            }
        }
        out
    }
}
