//! Arm A: the `sqlparser` crate, plus the thin conversion layer that (1) maps its AST onto the subset AST and (2) refuses everything else with
//! `not supported yet: <what>`. The layer is where the cost of reusing the crate is: every field of its `Select` and `Query` structs that could carry
//! something outside the subset must be checked (they are destructured exhaustively, so a new field in a later release is a compile error, not a silent accept).

use crate::ast::{
    self, BinOp, DataType as Ty, ErrorKind, ExplainMode, ExprKind, ParseError, SelectItem as Item,
    Span, UnOp,
};
use sqlparser::ast as sp;
use sqlparser::ast::Spanned;
use sqlparser::dialect::{Dialect, GenericDialect, PostgreSqlDialect};
use sqlparser::parser::{Parser, ParserError};
use sqlparser::tokenizer::Span as SpSpan;

pub use crate::hand::MAX_DEPTH;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dial {
    Generic,
    Postgres,
}

#[derive(Debug, Clone, Copy)]
pub struct Opts {
    pub dialect: Dial,
    /// sqlparser's own recursion counter (default 50). A nesting of 1,000 parentheses needs a larger one.
    pub recursion_limit: usize,
    /// sqlparser gives no position when the error is at the end of the input ("Expected: an expression, found: EOF"); the layer then reports the end of the text.
    pub eof_position_fix: bool,
    /// Refuse, before parsing, a query whose expression nesting (parentheses, CASE and operator chains) could exceed `MAX_DEPTH`: the crate itself has no bound on
    /// the length of an operator chain, and a very long one overflows the stack when the tree is dropped.
    pub depth_guard: bool,
}

impl Opts {
    pub fn new(dialect: Dial) -> Opts {
        Opts {
            dialect,
            recursion_limit: RECURSION_LIMIT,
            eof_position_fix: true,
            depth_guard: true,
        }
    }
}

/// Found by experiment (see the results file): enough for 1,000 nested parentheses.
pub const RECURSION_LIMIT: usize = 4000;

fn dialect(d: Dial) -> Box<dyn Dialect> {
    match d {
        Dial::Generic => Box::new(GenericDialect {}),
        Dial::Postgres => Box::new(PostgreSqlDialect {}),
    }
}

/// What the crate alone does (no conversion): used for the TPC-H check and for timing the parse apart from the conversion.
pub fn parse_raw(text: &str, o: Opts) -> Result<Vec<sp::Statement>, ParserError> {
    let d = dialect(o.dialect);
    Parser::new(d.as_ref())
        .with_recursion_limit(o.recursion_limit)
        .try_with_sql(text)?
        .parse_statements()
}

pub fn parse(text: &str) -> Result<ast::Query, ParseError> {
    parse_opts(text, Opts::new(Dial::Generic))
}

pub fn parse_opts(text: &str, o: Opts) -> Result<ast::Query, ParseError> {
    if o.depth_guard {
        guard(text, o.dialect)?;
    }
    let stmts = parse_raw(text, o).map_err(|e| map_error(text, e, o.eof_position_fix))?;
    convert_statements(text, &stmts)
}

// ---------------------------------------------------------------- errors

fn map_error(text: &str, e: ParserError, eof_fix: bool) -> ParseError {
    let (msg, limit) = match e {
        ParserError::TokenizerError(m) | ParserError::ParserError(m) => (m, false),
        ParserError::RecursionLimitExceeded => ("recursion limit exceeded".to_string(), true),
    };
    // sqlparser appends " at Line: L, Column: C" to the message when it knows the place
    let (message, line, col) = match msg.rfind(" at Line: ") {
        Some(i) => {
            let rest = &msg[i + " at Line: ".len()..];
            let mut it = rest.split(", Column: ");
            let l = it.next().and_then(|x| x.trim().parse::<usize>().ok());
            let c = it.next().and_then(|x| x.trim().parse::<usize>().ok());
            match (l, c) {
                (Some(l), Some(c)) => (msg[..i].to_string(), Some(l), Some(c)),
                _ => (msg.clone(), None, None),
            }
        }
        None => (msg.clone(), None, None),
    };
    let (mut line, mut col) = (line, col);
    if line.is_none()
        && eof_fix
        && (message.ends_with("found: EOF") || message.contains("Unexpected EOF"))
    {
        let (l, c) = ast::line_col(text, text.len());
        line = Some(l);
        col = Some(c);
    }
    let offset = match (line, col) {
        (Some(l), Some(c)) => ast::offset_of(text, l, c),
        _ => None,
    };
    ParseError {
        kind: if limit {
            ErrorKind::Limit
        } else {
            ErrorKind::Syntax
        },
        message,
        line,
        col,
        offset,
    }
}

fn ns<T>(text: &str, span: Option<SpSpan>, what: &str) -> Result<T, ParseError> {
    let (line, col) = match span {
        Some(s) if s.start.line > 0 => (Some(s.start.line as usize), Some(s.start.column as usize)),
        _ => (None, None),
    };
    let offset = match (line, col) {
        (Some(l), Some(c)) => ast::offset_of(text, l, c),
        _ => None,
    };
    Err(ParseError {
        kind: ErrorKind::NotSupported,
        message: what.to_string(),
        line,
        col,
        offset,
    })
}

/// The name of an enum variant from its Debug output: `BinaryOp { .. }` -> `BinaryOp`.
fn variant<T: std::fmt::Debug>(x: &T) -> String {
    let d = format!("{x:?}");
    d.chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

// ---------------------------------------------------------------- statements

fn convert_statements(text: &str, stmts: &[sp::Statement]) -> Result<ast::Query, ParseError> {
    match stmts {
        [] => Err(ParseError {
            kind: ErrorKind::Syntax,
            message: "empty query".into(),
            line: Some(1),
            col: Some(1),
            offset: Some(0),
        }),
        [one] => convert_statement(text, one),
        [_, second, ..] => ns(text, Some(second.span()), "more than one statement"),
    }
}

fn convert_statement(text: &str, s: &sp::Statement) -> Result<ast::Query, ParseError> {
    match s {
        sp::Statement::Query(q) => convert_query(text, q, ExplainMode::No),
        sp::Statement::Explain {
            describe_alias,
            analyze,
            verbose,
            query_plan,
            estimate,
            statement,
            format,
            options,
        } => {
            if *describe_alias != sp::DescribeAlias::Explain {
                return ns(text, None, "DESCRIBE");
            }
            if *verbose || *query_plan || *estimate || format.is_some() || options.is_some() {
                return ns(text, None, "EXPLAIN options");
            }
            match statement.as_ref() {
                sp::Statement::Query(q) => convert_query(
                    text,
                    q,
                    if *analyze {
                        ExplainMode::Analyze
                    } else {
                        ExplainMode::Plan
                    },
                ),
                other => ns(text, None, &format!("EXPLAIN of {}", variant(other))),
            }
        }
        other => ns(text, None, &format!("{} statements", variant(other))),
    }
}

fn convert_query(
    text: &str,
    q: &sp::Query,
    explain: ExplainMode,
) -> Result<ast::Query, ParseError> {
    let sp::Query {
        with,
        body,
        order_by,
        limit_clause,
        fetch,
        locks,
        for_clause,
        settings,
        format_clause,
        pipe_operators,
    } = q;
    if let Some(w) = with {
        return ns(text, Some(w.span()), "WITH (common table expressions)");
    }
    if let Some(o) = order_by {
        return ns(text, Some(o.span()), "ORDER BY");
    }
    if let Some(f) = fetch {
        return ns(text, Some(f.span()), "FETCH");
    }
    if !locks.is_empty()
        || for_clause.is_some()
        || settings.is_some()
        || format_clause.is_some()
        || !pipe_operators.is_empty()
    {
        return ns(
            text,
            None,
            "query modifiers (FOR, SETTINGS, FORMAT, pipe operators)",
        );
    }
    let sel = match body.as_ref() {
        sp::SetExpr::Select(s) => s,
        other => {
            return ns(
                text,
                Some(other.span()),
                &format!("{} (set operations, VALUES, subqueries)", variant(other)),
            );
        }
    };
    let sp::Select {
        select_token,
        optimizer_hints,
        distinct,
        select_modifiers,
        top,
        top_before_distinct: _,
        projection,
        exclude,
        into,
        from,
        lateral_views,
        prewhere,
        selection,
        connect_by,
        group_by,
        cluster_by,
        distribute_by,
        sort_by,
        having,
        named_window,
        qualify,
        window_before_qualify: _,
        value_table_mode,
        flavor,
    } = sel.as_ref();
    let tok = Some(select_token.0.span);
    if projection.is_empty() {
        let st = select_token.0.span.end;
        return Err(ParseError {
            kind: ErrorKind::Syntax,
            message: "SELECT clause without selection list".into(),
            line: Some(st.line as usize),
            col: Some(st.column as usize),
            offset: ast::offset_of(text, st.line as usize, st.column as usize),
        });
    }
    if distinct.is_some() {
        return ns(text, tok, "SELECT DISTINCT");
    }
    if !optimizer_hints.is_empty()
        || select_modifiers.is_some()
        || top.is_some()
        || exclude.is_some()
        || into.is_some()
        || !lateral_views.is_empty()
        || prewhere.is_some()
        || !connect_by.is_empty()
        || !cluster_by.is_empty()
        || !distribute_by.is_empty()
        || !sort_by.is_empty()
        || value_table_mode.is_some()
    {
        return ns(text, tok, "SELECT modifiers");
    }
    match group_by {
        sp::GroupByExpr::Expressions(e, m) if e.is_empty() && m.is_empty() => {}
        _ => return ns(text, tok, "GROUP BY"),
    }
    if having.is_some() {
        return ns(text, tok, "HAVING");
    }
    if !named_window.is_empty() || qualify.is_some() {
        return ns(text, tok, "WINDOW and QUALIFY");
    }
    if *flavor != sp::SelectFlavor::Standard {
        return ns(text, tok, "FROM-first queries");
    }

    let items = projection
        .iter()
        .map(|i| convert_item(text, i))
        .collect::<Result<Vec<_>, _>>()?;
    let from = convert_from(text, from, tok)?;
    let where_ = selection
        .as_ref()
        .map(|e| convert_expr(text, e, 0))
        .transpose()?;
    let (limit, offset) = convert_limit(text, limit_clause.as_ref())?;
    Ok(ast::Query {
        explain,
        items,
        from,
        where_,
        limit,
        offset,
    })
}

fn convert_from(
    text: &str,
    from: &[sp::TableWithJoins],
    tok: Option<SpSpan>,
) -> Result<String, ParseError> {
    let twj = match from {
        [] => return ns(text, tok, "SELECT without FROM (write FROM 'path')"),
        [one] => one,
        [_, second, ..] => return ns(text, Some(second.span()), "more than one table (joins)"),
    };
    if !twj.joins.is_empty() {
        return ns(text, Some(twj.span()), "joins");
    }
    let sp::TableFactor::Table {
        name,
        alias,
        args,
        with_hints,
        version,
        with_ordinality,
        partitions,
        json_path,
        sample,
        index_hints,
    } = &twj.relation
    else {
        return ns(
            text,
            Some(twj.relation.span()),
            "a subquery or table function in FROM",
        );
    };
    if alias.is_some() {
        return ns(text, Some(twj.relation.span()), "table aliases");
    }
    if args.is_some()
        || !with_hints.is_empty()
        || version.is_some()
        || *with_ordinality
        || !partitions.is_empty()
        || json_path.is_some()
        || sample.is_some()
        || !index_hints.is_empty()
    {
        return ns(
            text,
            Some(twj.relation.span()),
            "table function arguments and table modifiers",
        );
    }
    match name.0.as_slice() {
        [sp::ObjectNamePart::Identifier(id)] if id.quote_style == Some('\'') => {
            Ok(id.value.clone())
        }
        _ => ns(
            text,
            Some(twj.relation.span()),
            "table names (only a quoted path or glob, FROM 'file.parquet')",
        ),
    }
}

fn whole_number(text: &str, e: &sp::Expr, what: &str) -> Result<u64, ParseError> {
    match e {
        sp::Expr::Value(v) => match &v.value {
            sp::Value::Number(s, false) if s.bytes().all(|c| c.is_ascii_digit()) => {
                s.parse::<u64>().map_err(|_| ParseError {
                    kind: ErrorKind::Syntax,
                    message: format!("{what} value {s} is larger than 18446744073709551615"),
                    line: Some(v.span.start.line as usize),
                    col: Some(v.span.start.column as usize),
                    offset: ast::offset_of(
                        text,
                        v.span.start.line as usize,
                        v.span.start.column as usize,
                    ),
                })
            }
            _ => ns(
                text,
                Some(e.span()),
                &format!("{what} with anything but a whole number written in digits"),
            ),
        },
        _ => ns(
            text,
            Some(e.span()),
            &format!("{what} with anything but a whole number written in digits"),
        ),
    }
}

fn convert_limit(
    text: &str,
    lc: Option<&sp::LimitClause>,
) -> Result<(Option<u64>, Option<u64>), ParseError> {
    match lc {
        None => Ok((None, None)),
        Some(sp::LimitClause::OffsetCommaLimit { offset, .. }) => ns(
            text,
            Some(offset.span()),
            "LIMIT offset, count (write LIMIT n OFFSET m)",
        ),
        Some(sp::LimitClause::LimitOffset {
            limit,
            offset,
            limit_by,
        }) => {
            if !limit_by.is_empty() {
                return ns(text, Some(limit_by[0].span()), "LIMIT ... BY");
            }
            let off_expr = offset.as_ref().map(|o| &o.value);
            match (limit, off_expr) {
                (None, None) => ns(text, None, "LIMIT ALL"),
                (None, Some(o)) => ns(
                    text,
                    Some(o.span()),
                    "OFFSET without LIMIT (write LIMIT n OFFSET m)",
                ),
                (Some(l), o) => {
                    if let Some(o) = o {
                        let (ls, os) = (l.span().start, o.span().start);
                        if (os.line, os.column) < (ls.line, ls.column) {
                            return ns(
                                text,
                                Some(o.span()),
                                "OFFSET before LIMIT (write LIMIT n OFFSET m)",
                            );
                        }
                    }
                    let lv = whole_number(text, l, "LIMIT")?;
                    let ov = o.map(|o| whole_number(text, o, "OFFSET")).transpose()?;
                    Ok((Some(lv), ov))
                }
            }
        }
    }
}

fn convert_item(text: &str, it: &sp::SelectItem) -> Result<Item, ParseError> {
    match it {
        sp::SelectItem::UnnamedExpr(e) => Ok(Item::Expr {
            expr: convert_expr(text, e, 0)?,
            alias: None,
        }),
        sp::SelectItem::ExprWithAlias { expr, alias } => {
            // `SELECT a b` and `SELECT a AS b` give the same node: look at the text before the alias for the AS
            let before = alias.span.start;
            let off =
                ast::offset_of(text, before.line as usize, before.column as usize).unwrap_or(0);
            let head = text[..off].trim_end();
            let has_as = head.len() >= 2
                && head.as_bytes()[head.len() - 2..].eq_ignore_ascii_case(b"as")
                && (head.len() == 2
                    || !(head.as_bytes()[head.len() - 3].is_ascii_alphanumeric()
                        || head.as_bytes()[head.len() - 3] == b'_'));
            if !has_as {
                // some aliases carry an empty span; the expression's is then the best place
                let at = if alias.span.start.line > 0 {
                    alias.span
                } else {
                    expr.span()
                };
                return ns(text, Some(at), "an alias without AS (write AS name)");
            }
            let e = convert_expr(text, expr, 0)?;
            Ok(Item::Expr {
                expr: e,
                alias: Some(convert_ident(text, alias)?),
            })
        }
        sp::SelectItem::Wildcard(o) => {
            let sp::WildcardAdditionalOptions {
                wildcard_token: _,
                opt_ilike,
                opt_exclude,
                opt_except,
                opt_replace,
                opt_rename,
                opt_alias,
            } = o;
            if opt_ilike.is_some()
                || opt_exclude.is_some()
                || opt_except.is_some()
                || opt_replace.is_some()
                || opt_rename.is_some()
                || opt_alias.is_some()
            {
                return ns(text, Some(o.wildcard_token.0.span), "modifiers on *");
            }
            Ok(Item::Star)
        }
        other => ns(
            text,
            Some(other.span()),
            &format!("{} in the select list", variant(other)),
        ),
    }
}

// ---------------------------------------------------------------- expressions

fn convert_ident(text: &str, id: &sp::Ident) -> Result<ast::Ident, ParseError> {
    match id.quote_style {
        Some('"') if id.value.is_empty() => Err(ParseError {
            kind: ErrorKind::Syntax,
            message: "zero-length quoted identifier".into(),
            line: Some(id.span.start.line as usize),
            col: Some(id.span.start.column as usize),
            offset: ast::offset_of(
                text,
                id.span.start.line as usize,
                id.span.start.column as usize,
            ),
        }),
        Some('"') => Ok(ast::Ident {
            name: id.value.clone(),
            quoted: true,
        }),
        Some(_) => ns(
            text,
            Some(id.span),
            "this identifier quoting (use double quotes)",
        ),
        None => {
            let ok = id.value.chars().enumerate().all(|(i, c)| {
                if i == 0 {
                    c.is_ascii_alphabetic() || c == '_' || !c.is_ascii()
                } else {
                    c.is_ascii_alphanumeric() || c == '_' || c == '$' || !c.is_ascii()
                }
            });
            if !ok {
                return ns(
                    text,
                    Some(id.span),
                    "identifier characters (letters, digits, _ and $)",
                );
            }
            if crate::hand::is_reserved_word(&id.value) {
                return Err(ParseError {
                    kind: ErrorKind::Syntax,
                    message: format!(
                        "{} is a reserved word; quote it to use it as a name",
                        id.value
                    ),
                    line: Some(id.span.start.line as usize),
                    col: Some(id.span.start.column as usize),
                    offset: ast::offset_of(
                        text,
                        id.span.start.line as usize,
                        id.span.start.column as usize,
                    ),
                });
            }
            Ok(ast::Ident {
                name: id.value.clone(),
                quoted: false,
            })
        }
    }
}

fn chain_error(text: &str, e: &sp::Expr) -> ParseError {
    let st = e.span().start;
    ParseError {
        kind: ErrorKind::Syntax,
        message: "chained comparisons (and chained BETWEEN / IN) are not supported, use AND or parentheses".into(),
        line: (st.line > 0).then_some(st.line as usize),
        col: (st.line > 0).then_some(st.column as usize),
        offset: ast::offset_of(text, st.line as usize, st.column as usize),
    }
}

fn bx(text: &str, e: &sp::Expr, depth: usize) -> Result<Box<ast::Expr>, ParseError> {
    Ok(Box::new(convert_expr(text, e, depth)?))
}

fn mk(kind: ExprKind) -> ast::Expr {
    ast::Expr::new(kind, Span::NONE)
}

fn convert_expr(text: &str, e: &sp::Expr, depth: usize) -> Result<ast::Expr, ParseError> {
    if depth > MAX_DEPTH {
        let sp = e.span();
        return Err(ParseError {
            kind: ErrorKind::Limit,
            message: format!("expression nested deeper than {MAX_DEPTH}"),
            line: (sp.start.line > 0).then_some(sp.start.line as usize),
            col: (sp.start.line > 0).then_some(sp.start.column as usize),
            offset: None,
        });
    }
    let d = depth + 1;
    let span = Some(e.span());
    Ok(mk(match e {
        sp::Expr::Identifier(id) => ExprKind::Ident(convert_ident(text, id)?),
        sp::Expr::CompoundIdentifier(_) => return ns(text, span, "qualified names (table.column)"),
        sp::Expr::Value(v) => match &v.value {
            sp::Value::Number(s, false) => ExprKind::Number(s.clone()),
            sp::Value::SingleQuotedString(s) => ExprKind::Str(s.clone()),
            sp::Value::Null => ExprKind::Null,
            sp::Value::Boolean(_) => return ns(text, span, "boolean literals"),
            other => {
                return ns(
                    text,
                    Some(v.span),
                    &format!("this kind of literal ({})", variant(other)),
                );
            }
        },
        sp::Expr::TypedString(ts) => match (&ts.data_type, &ts.value.value) {
            (sp::DataType::Date, sp::Value::SingleQuotedString(s)) => ExprKind::Date(s.clone()),
            _ => return ns(text, span, "typed literals (only DATE 'yyyy-mm-dd')"),
        },
        sp::Expr::UnaryOp { op, expr } => match op {
            sp::UnaryOperator::Minus => ExprKind::Unary {
                op: UnOp::Neg,
                expr: bx(text, expr, d)?,
            },
            sp::UnaryOperator::Not => ExprKind::Unary {
                op: UnOp::Not,
                expr: bx(text, expr, d)?,
            },
            other => return ns(text, span, &format!("the unary operator {other}")),
        },
        sp::Expr::BinaryOp { left, op, right } => {
            let op = match op {
                sp::BinaryOperator::Plus => BinOp::Add,
                sp::BinaryOperator::Minus => BinOp::Sub,
                sp::BinaryOperator::Multiply => BinOp::Mul,
                sp::BinaryOperator::Divide => BinOp::Div,
                sp::BinaryOperator::Modulo => BinOp::Mod,
                sp::BinaryOperator::Eq => BinOp::Eq,
                sp::BinaryOperator::NotEq => BinOp::NotEq,
                sp::BinaryOperator::Lt => BinOp::Lt,
                sp::BinaryOperator::LtEq => BinOp::LtEq,
                sp::BinaryOperator::Gt => BinOp::Gt,
                sp::BinaryOperator::GtEq => BinOp::GtEq,
                sp::BinaryOperator::And => BinOp::And,
                sp::BinaryOperator::Or => BinOp::Or,
                other => return ns(text, span, &format!("the operator {other}")),
            };
            let is_cmp = |b: &sp::BinaryOperator| {
                matches!(
                    b,
                    sp::BinaryOperator::Eq
                        | sp::BinaryOperator::NotEq
                        | sp::BinaryOperator::Lt
                        | sp::BinaryOperator::LtEq
                        | sp::BinaryOperator::Gt
                        | sp::BinaryOperator::GtEq
                )
            };
            let chained = |x: &sp::Expr| matches!(x, sp::Expr::BinaryOp { op, .. } if is_cmp(op));
            if matches!(
                op,
                BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq
            ) && (chained(left) || chained(right))
            {
                return Err(chain_error(text, e));
            }
            ExprKind::Binary {
                op,
                left: bx(text, left, d)?,
                right: bx(text, right, d)?,
            }
        }
        sp::Expr::Nested(inner) => return convert_expr(text, inner, d),
        sp::Expr::IsNull(x) => ExprKind::IsNull {
            expr: bx(text, x, d)?,
            negated: false,
        },
        sp::Expr::IsNotNull(x) => ExprKind::IsNull {
            expr: bx(text, x, d)?,
            negated: true,
        },
        sp::Expr::Between {
            expr,
            negated,
            low,
            high,
        } => {
            if matches!(
                expr.as_ref(),
                sp::Expr::Between { .. } | sp::Expr::InList { .. }
            ) {
                return Err(chain_error(text, e));
            }
            ExprKind::Between {
                expr: bx(text, expr, d)?,
                low: bx(text, low, d)?,
                high: bx(text, high, d)?,
                negated: *negated,
            }
        }
        sp::Expr::InList {
            expr,
            list,
            negated,
        } => {
            if matches!(
                expr.as_ref(),
                sp::Expr::Between { .. } | sp::Expr::InList { .. }
            ) {
                return Err(chain_error(text, e));
            }
            ExprKind::InList {
                expr: bx(text, expr, d)?,
                list: list
                    .iter()
                    .map(|x| convert_expr(text, x, d))
                    .collect::<Result<_, _>>()?,
                negated: *negated,
            }
        }
        sp::Expr::Case {
            case_token: _,
            end_token: _,
            operand,
            conditions,
            else_result,
        } => ExprKind::Case {
            operand: operand.as_ref().map(|o| bx(text, o, d)).transpose()?,
            whens: conditions
                .iter()
                .map(|w| {
                    Ok((
                        convert_expr(text, &w.condition, d)?,
                        convert_expr(text, &w.result, d)?,
                    ))
                })
                .collect::<Result<_, ParseError>>()?,
            else_: else_result.as_ref().map(|o| bx(text, o, d)).transpose()?,
        },
        sp::Expr::Cast {
            kind,
            expr,
            data_type,
            format,
        } => {
            match kind {
                sp::CastKind::Cast => {}
                sp::CastKind::DoubleColon => {
                    return ns(text, span, "the :: cast (write CAST(x AS type))");
                }
                other => return ns(text, span, &format!("{other:?}")),
            }
            if format.is_some() {
                return ns(text, span, "CAST ... FORMAT");
            }
            ExprKind::Cast {
                expr: bx(text, expr, d)?,
                ty: convert_type(text, data_type, span)?,
            }
        }
        sp::Expr::Function(f) => {
            return ns(text, span, &format!("function calls ({}(...))", f.name));
        }
        sp::Expr::Subquery(_) | sp::Expr::Exists { .. } | sp::Expr::InSubquery { .. } => {
            return ns(text, span, "subqueries");
        }
        sp::Expr::Like { .. } | sp::Expr::ILike { .. } | sp::Expr::SimilarTo { .. } => {
            return ns(text, span, "LIKE, ILIKE and SIMILAR TO");
        }
        other => return ns(text, span, &variant(other)),
    }))
}

fn convert_type(text: &str, t: &sp::DataType, span: Option<SpSpan>) -> Result<Ty, ParseError> {
    use sp::DataType as D;
    use sp::ExactNumberInfo as N;
    let dec = |n: &N| -> Result<Ty, ParseError> {
        let (p, s) = match n {
            N::None => (18, 3),
            N::Precision(p) => (*p, 0),
            N::PrecisionAndScale(p, s) => (*p, *s as u64),
        };
        match (u8::try_from(p), u8::try_from(s)) {
            (Ok(precision), Ok(scale)) => Ok(Ty::Decimal { precision, scale }),
            _ => ns(text, span, "DECIMAL precision and scale outside 0..255"),
        }
    };
    match t {
        D::Boolean | D::Bool => Ok(Ty::Boolean),
        D::TinyInt(None) => Ok(Ty::TinyInt),
        D::SmallInt(None) => Ok(Ty::SmallInt),
        D::Int(None) | D::Integer(None) => Ok(Ty::Integer),
        D::BigInt(None) => Ok(Ty::BigInt),
        D::DoublePrecision => Ok(Ty::Double),
        D::Double(N::None) => Ok(Ty::Double),
        D::Decimal(n) | D::Numeric(n) | D::Dec(n) => dec(n),
        D::Varchar(None) => Ok(Ty::Varchar),
        D::Date => Ok(Ty::Date),
        other => ns(text, span, &format!("the type {other}")),
    }
}

/// A token-level bound on expression depth: the sum, over the open parentheses and CASEs, of one plus the operators seen since the last comma or clause keyword.
/// It is an upper bound on the depth of the tree the crate will build (conservative: it also counts operators that do not deepen the tree).
fn guard(text: &str, d: Dial) -> Result<(), ParseError> {
    use sqlparser::keywords::Keyword;
    use sqlparser::tokenizer::{Token, Tokenizer};
    let dia = dialect(d);
    // a tokenizer error is left for the parser to report with its own message
    let Ok(toks) = Tokenizer::new(dia.as_ref(), text).tokenize_with_location() else {
        return Ok(());
    };
    let mut frames: Vec<usize> = vec![0];
    let mut total = 1usize;
    for t in &toks {
        let (push, pop, op, reset) = match &t.token {
            Token::LParen => (true, false, false, false),
            Token::RParen => (false, true, false, false),
            Token::Comma => (false, false, false, true),
            Token::Plus
            | Token::Minus
            | Token::Mul
            | Token::Div
            | Token::Mod
            | Token::Eq
            | Token::Neq
            | Token::Lt
            | Token::Gt
            | Token::LtEq
            | Token::GtEq => (false, false, true, false),
            Token::Word(w) => match w.keyword {
                Keyword::CASE => (true, false, false, false),
                Keyword::END => (false, true, false, false),
                Keyword::AND
                | Keyword::OR
                | Keyword::NOT
                | Keyword::BETWEEN
                | Keyword::IN
                | Keyword::IS => (false, false, true, false),
                Keyword::FROM | Keyword::WHERE | Keyword::LIMIT | Keyword::OFFSET => {
                    (false, false, false, true)
                }
                _ => (false, false, false, false),
            },
            _ => (false, false, false, false),
        };
        if push {
            frames.push(0);
            total += 1;
        } else if pop && frames.len() > 1 {
            total -= 1 + frames.pop().unwrap_or(0);
        } else if op {
            if let Some(f) = frames.last_mut() {
                *f += 1;
            }
            total += 1;
        } else if reset && let Some(f) = frames.last_mut() {
            total -= *f;
            *f = 0;
        }
        if total > MAX_DEPTH {
            let (l, c) = (t.span.start.line as usize, t.span.start.column as usize);
            return Err(ParseError {
                kind: ErrorKind::Limit,
                message: format!("expression nested deeper than {MAX_DEPTH}"),
                line: Some(l),
                col: Some(c),
                offset: ast::offset_of(text, l, c),
            });
        }
    }
    Ok(())
}
