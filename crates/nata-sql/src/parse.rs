//! Parse: the `sqlparser` crate (PostgreSQL dialect) behind our own AST (decisions 4b of card 0002).
//!
//! 1. A **token guard** runs before the parser: the text size, the nesting depth (parentheses, `CASE`, runs of prefix operators), the length of a chain
//!    of one operator (dialect section 8), and the constructs the crate's tree would HIDE (`LIMIT ALL`, a stray `;`). It also turns the two characters
//!    `//` (integer division, which the crate tokenizes only for DuckDB's dialect) into one `/` token whose place is remembered.
//! 2. The parser runs on a thread with a **64 MiB stack** (including instrumentation headroom; see R15 and `WORKER_STACK_BYTES`).
//! 3. A **conversion** maps the crate's tree to [`crate::ast`] and refuses everything outside the subset as `not supported yet: <what>` at its position.
//!    Every field of the crate's `Query` and `Select` is destructured, so a new field in a later release is a compile error and not a silent accept.
//!    Long chains of one binary operator are walked in a loop (no recursion), so a chain of 10,000 terms costs no stack.

use sqlparser::ast as sp;
use sqlparser::ast::Spanned;
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::keywords::Keyword;
use sqlparser::parser::{Parser, ParserError};
use sqlparser::tokenizer::{Span as SpSpan, Token, TokenWithSpan, Tokenizer};

type Span2 = SpSpan;

use crate::ast::{
    self, BinOp, DataType as Ty, ExplainMode, ExprKind, Ident, SelectItem as Item, Span, UnOp,
};
use crate::error::{LIMIT_PREFIX, Pos, Result, SqlError};
use crate::keywords::is_reserved_word;

/// Longest query text, in bytes (1 MiB).
pub const MAX_TEXT_BYTES: usize = 1 << 20;
/// Deepest nesting of parentheses, `CASE` and prefix operators.
pub const MAX_NESTING: usize = 1_000;
/// Most terms in one chain of the same binary operator.
pub const MAX_CHAIN_TERMS: usize = 10_000;
/// The stack of the thread that parses (and binds and plans).
// Local x86-64 ASan/build-std: 16 MiB overflows in sqlparser's recursive
// parse_prefix -> parse_subexpr -> parse_expr path; 32 MiB passes every deep
// construct at the unchanged 1,000-level limit. Reserve twice that measured
// passing envelope for compiler/instrumentation variation. This is virtual
// stack reservation, not precommitted RSS. Server admission bounds concurrency.
// Evidence: bench/results/20261107-pgwire-server.md (R15).
pub const WORKER_STACK_BYTES: usize = 64 << 20;
/// The crate's own recursion counter (default 50): a backstop under the guard. 1,000 nested parentheses use about 4,000 of it.
pub const RECURSION_LIMIT: usize = 6_000;

/// Run `f` on a thread with the budgeted stack and wait for it. A panic of `f` becomes an error (the front end promises none; the fuzz test asserts it).
pub(crate) fn on_big_stack<T: Send>(f: impl FnOnce() -> T + Send) -> Result<T> {
    std::thread::scope(|s| {
        let h = std::thread::Builder::new()
            .name("nata-sql".into())
            .stack_size(WORKER_STACK_BYTES)
            .spawn_scoped(s, f)
            .map_err(|e| internal(&format!("cannot start the parse thread: {e}")))?;
        h.join().map_err(|_| internal("a panic in the front end"))
    })
}

fn internal(what: &str) -> SqlError {
    SqlError::new(
        crate::error::ErrorClass::Execution,
        Pos::START,
        format!("internal error: {what}"),
    )
}

/// Parse one statement on a thread of its own (budgeted stack).
pub fn parse_query(text: &str) -> Result<ast::Query> {
    on_big_stack(|| parse_query_here(text))?
}

/// As [`parse_query`], on the CURRENT thread, which must have a stack of at least [`WORKER_STACK_BYTES`].
pub fn parse_query_here(text: &str) -> Result<ast::Query> {
    parse_query_params_here(text, &[])
}

/// Protocol adapter: substitute literal expression nodes, never SQL text.
pub fn parse_query_params_here(text: &str, params: &[ast::Expr]) -> Result<ast::Query> {
    let prepared = prepare(text)?;
    let dialect = PostgreSqlDialect {};
    let parsed = Parser::new(&dialect)
        .with_recursion_limit(RECURSION_LIMIT)
        .with_tokens_with_locations(prepared.tokens.clone())
        .parse_statements();
    let converted = match parsed {
        Err(e) => Err(map_error(text, e)),
        Ok(stmts) => {
            let conv = Conv {
                params,
                toks: &prepared.tokens,
                int_div_at: &prepared.int_div_at,
                end: Pos::end_of(text),
            };
            conv.statements(&stmts)
        }
    };
    // a construct the crate's tree hides is reported only if nothing is wrong before it in the text
    match (converted, prepared.deferred) {
        (Ok(_), Some(d)) => Err(d),
        (Err(e), Some(d)) if e.pos >= d.pos => Err(d),
        (r, _) => r,
    }
}

// ---------------------------------------------------------------------------------------------------------------------------------- the guard

struct Prepared {
    tokens: Vec<TokenWithSpan>,
    /// The places of the `//` operators, in text order (each is ONE `/` token of the list above, two characters wide).
    int_div_at: Vec<Pos>,
    /// The first (lowest position) construct that the crate's tree hides or that the subset refuses at the token level.
    deferred: Option<SqlError>,
}

fn pos_of(l: sqlparser::tokenizer::Location) -> Pos {
    Pos {
        line: u32::try_from(l.line).unwrap_or(u32::MAX),
        col: u32::try_from(l.column).unwrap_or(u32::MAX),
    }
}

fn limit_error(pos: Pos, what: &str) -> SqlError {
    SqlError::syntax(pos, format!("{LIMIT_PREFIX} {what}"))
}

/// Operator kinds counted per chain (see [`MAX_CHAIN_TERMS`]).
const N_OPS: usize = 12;
const OP_ADD: usize = 0;
const OP_SUB: usize = 1;
const OP_MUL: usize = 2;
const OP_DIV: usize = 3;
const OP_MOD: usize = 4;
const OP_INTDIV: usize = 5;
const OP_CMP: usize = 6;
const OP_AND: usize = 7;
const OP_OR: usize = 8;
const OP_CAST: usize = 9;
const OP_IS: usize = 10;
const OP_BETWEEN_IN: usize = 11;

struct Frame {
    counts: [u32; N_OPS],
    /// Prefix operators that were open when this frame was opened (they stay open until the frame closes).
    unary_before: usize,
}

fn is_prefix_op_token(t: &Token) -> bool {
    matches!(
        t,
        Token::Plus
            | Token::Minus
            | Token::Tilde
            | Token::ExclamationMark
            | Token::DoubleExclamationMark
            | Token::AtSign
            | Token::PGSquareRoot
            | Token::PGCubeRoot
    )
}

/// Words that cannot end an operand: after one of them an operator is a PREFIX operator.
fn is_operator_keyword(k: Keyword) -> bool {
    matches!(
        k,
        Keyword::SELECT
            | Keyword::FROM
            | Keyword::WHERE
            | Keyword::AND
            | Keyword::OR
            | Keyword::NOT
            | Keyword::WHEN
            | Keyword::THEN
            | Keyword::ELSE
            | Keyword::CASE
            | Keyword::BETWEEN
            | Keyword::IN
            | Keyword::IS
            | Keyword::LIMIT
            | Keyword::OFFSET
            | Keyword::AS
            | Keyword::EXPLAIN
            | Keyword::ANALYZE
            | Keyword::DISTINCT
            | Keyword::ALL
    )
}

/// Tokenize, enforce the limits of dialect section 8, find what the crate would hide, and rewrite `//`.
fn prepare(text: &str) -> Result<Prepared> {
    if text.len() > MAX_TEXT_BYTES {
        // the position of the first character beyond the limit
        let mut cut = MAX_TEXT_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        let pos = Pos::end_of(&text[..cut]);
        return Err(limit_error(
            pos,
            &format!("the text is longer than {MAX_TEXT_BYTES} bytes (1 MB)"),
        ));
    }
    let dialect = PostgreSqlDialect {};
    let raw = Tokenizer::new(&dialect, text)
        .tokenize_with_location()
        .map_err(|e| {
            let mut pos = pos_of(e.location);
            if pos.is_none() {
                pos = Pos::end_of(text);
            }
            SqlError::syntax(pos, e.message)
        })?;

    let mut frames: Vec<Frame> = vec![Frame {
        counts: [0; N_OPS],
        unary_before: 0,
    }];
    // depth = open parentheses and CASEs (plus the prefix operators open when they were opened) plus the current run of prefix operators
    let mut open_depth = 0usize;
    let mut run = 0usize;
    let mut prev_value = false;
    let mut prev_is = false;
    let mut deferred: Option<SqlError> = None;
    let mut defer = |e: SqlError| {
        if deferred.as_ref().is_none_or(|d| e.pos < d.pos) {
            deferred = Some(e);
        }
    };
    let mut seen_significant = false;
    let mut semicolon: Option<Pos> = None;
    let mut out: Vec<TokenWithSpan> = Vec::with_capacity(raw.len());
    let mut int_div_at: Vec<Pos> = Vec::new();
    let mut it = raw.into_iter().peekable();
    // the previous significant token was the word LIMIT
    let mut prev_limit = false;
    while let Some(tok) = it.next() {
        if matches!(tok.token, Token::Whitespace(_)) {
            out.push(tok);
            continue;
        }
        let at = pos_of(tok.span.start);
        // `//` is two adjacent `/` tokens in this dialect: one token, which the parser reads as an operator of the multiplication level
        let mut tok = tok;
        if tok.token == Token::Div
            && let Some(next) = it.peek()
            && next.token == Token::Div
            && let Some(second) = it.next()
        {
            int_div_at.push(at);
            tok = TokenWithSpan {
                token: Token::Div,
                span: SpSpan {
                    start: tok.span.start,
                    end: second.span.end,
                },
            };
        }
        if let Some(sc) = semicolon {
            // anything after the statement's semicolon
            if tok.token == Token::SemiColon {
                defer(SqlError::syntax(
                    at,
                    "unexpected `;`: an empty statement (one trailing `;` is allowed)",
                ));
            } else {
                defer(SqlError::not_supported(at, "more than one statement"));
            }
            let _ = sc;
        }
        if tok.token == Token::SemiColon {
            if !seen_significant {
                defer(SqlError::syntax(
                    at,
                    "unexpected `;`: an empty statement (write a SELECT)",
                ));
            }
            semicolon.get_or_insert(at);
        }
        seen_significant = true;
        // limits
        let mut kind_op: Option<usize> = None;
        let mut reset = false;
        let mut value_now = false;
        let mut is_prefix = false;
        match &tok.token {
            Token::LParen => {
                open_depth += 1 + run;
                frames.push(Frame {
                    counts: [0; N_OPS],
                    unary_before: run,
                });
                run = 0;
            }
            Token::RParen => {
                if frames.len() > 1
                    && let Some(f) = frames.pop()
                {
                    open_depth = open_depth.saturating_sub(1 + f.unary_before);
                }
                run = 0;
                value_now = true;
            }
            Token::Comma => reset = true,
            Token::Plus | Token::Minus if prev_value => {
                kind_op = Some(if tok.token == Token::Plus {
                    OP_ADD
                } else {
                    OP_SUB
                });
            }
            Token::Mul => kind_op = Some(OP_MUL),
            Token::Div => {
                kind_op = Some(if int_div_at.last() == Some(&at) {
                    OP_INTDIV
                } else {
                    OP_DIV
                })
            }
            Token::Mod => kind_op = Some(OP_MOD),
            Token::Eq | Token::Neq | Token::Lt | Token::Gt | Token::LtEq | Token::GtEq => {
                kind_op = Some(OP_CMP);
            }
            Token::DoubleColon => kind_op = Some(OP_CAST),
            Token::Number(..)
            | Token::SingleQuotedString(_)
            | Token::DoubleQuotedString(_)
            | Token::NationalStringLiteral(_)
            | Token::EscapedStringLiteral(_)
            | Token::HexStringLiteral(_)
            | Token::DollarQuotedString(_) => value_now = true,
            Token::Word(w) => {
                if w.quote_style.is_some() {
                    value_now = true;
                } else {
                    match w.keyword {
                        Keyword::CASE => {
                            open_depth += 1 + run;
                            frames.push(Frame {
                                counts: [0; N_OPS],
                                unary_before: run,
                            });
                            run = 0;
                        }
                        Keyword::END => {
                            if frames.len() > 1
                                && let Some(f) = frames.pop()
                            {
                                open_depth = open_depth.saturating_sub(1 + f.unary_before);
                            }
                            run = 0;
                            value_now = true;
                        }
                        Keyword::AND => kind_op = Some(OP_AND),
                        Keyword::OR => kind_op = Some(OP_OR),
                        Keyword::IS => {
                            kind_op = Some(OP_IS);
                        }
                        Keyword::BETWEEN | Keyword::IN => kind_op = Some(OP_BETWEEN_IN),
                        Keyword::NOT => {
                            // `a NOT BETWEEN`, `a NOT IN`: after a value; `IS NOT NULL`: after IS
                            if !prev_value && !prev_is {
                                is_prefix = true;
                            }
                        }
                        Keyword::LIMIT => reset = true,
                        Keyword::SELECT
                        | Keyword::FROM
                        | Keyword::WHERE
                        | Keyword::OFFSET
                        | Keyword::WHEN
                        | Keyword::THEN
                        | Keyword::ELSE
                        | Keyword::AS => reset = true,
                        Keyword::ALL if prev_limit => {
                            defer(SqlError::not_supported(
                                at,
                                "LIMIT ALL (write LIMIT n, or leave LIMIT out)",
                            ));
                        }
                        k if is_operator_keyword(k) => {}
                        _ => value_now = true,
                    }
                }
            }
            t if is_prefix_op_token(t) && !prev_value => is_prefix = true,
            _ => {}
        }
        prev_limit = matches!(&tok.token, Token::Word(w) if w.keyword == Keyword::LIMIT && w.quote_style.is_none());
        if is_prefix {
            run += 1;
        }
        if value_now {
            run = 0;
        }
        if reset && let Some(f) = frames.last_mut() {
            f.counts = [0; N_OPS];
        }
        if let Some(k) = kind_op
            && let Some(f) = frames.last_mut()
        {
            f.counts[k] += 1;
            if f.counts[k] as usize >= MAX_CHAIN_TERMS {
                return Err(limit_error(
                    at,
                    &format!("a chain of more than {MAX_CHAIN_TERMS} terms of one operator"),
                ));
            }
        }
        if open_depth + run > MAX_NESTING {
            return Err(limit_error(
                at,
                &format!("nesting (parentheses, CASE, prefix operators) deeper than {MAX_NESTING}"),
            ));
        }
        prev_is = matches!(&tok.token, Token::Word(w) if w.keyword == Keyword::IS && w.quote_style.is_none());
        prev_value = value_now;
        out.push(tok);
    }
    Ok(Prepared {
        tokens: out,
        int_div_at,
        deferred,
    })
}

// ---------------------------------------------------------------------------------------------------------------------------------- parser errors

fn map_error(text: &str, e: ParserError) -> SqlError {
    let msg = match e {
        ParserError::TokenizerError(m) | ParserError::ParserError(m) => m,
        ParserError::RecursionLimitExceeded => {
            return limit_error(
                Pos::START,
                &format!("nesting deeper than the parser's recursion limit ({RECURSION_LIMIT})"),
            );
        }
    };
    // the crate appends " at Line: L, Column: C" when it knows the place
    if let Some(i) = msg.rfind(" at Line: ") {
        let rest = &msg[i + " at Line: ".len()..];
        let mut parts = rest.split(", Column: ");
        let line = parts.next().and_then(|x| x.trim().parse::<u32>().ok());
        let col = parts.next().and_then(|x| x.trim().parse::<u32>().ok());
        if let (Some(line), Some(col)) = (line, col) {
            return SqlError::syntax(Pos { line, col }, &msg[..i]);
        }
    }
    // an error at the end of the input has no position in the crate: the end of the text is the place
    SqlError::syntax(Pos::end_of(text), msg)
}

// ---------------------------------------------------------------------------------------------------------------------------------- conversion

fn sp_pos(s: SpSpan) -> Pos {
    pos_of(s.start)
}

fn ns<T>(span: Option<SpSpan>, at_least: Pos, what: &str) -> Result<T> {
    let pos = match span {
        Some(s) if s.start.line > 0 => sp_pos(s),
        _ => at_least,
    };
    Err(SqlError::not_supported(pos, what))
}

/// The name of an enum variant from its Debug output (`BinaryOp { .. }` gives `BinaryOp`) without formatting the whole value.
fn variant<T: std::fmt::Debug>(x: &T) -> String {
    use std::fmt::Write;
    struct Head(String);
    impl Write for Head {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            for c in s.chars() {
                if c.is_alphanumeric() || c == '_' {
                    self.0.push(c);
                } else {
                    return Err(std::fmt::Error);
                }
            }
            Ok(())
        }
    }
    let mut h = Head(String::new());
    let _ = write!(h, "{x:?}");
    h.0
}

struct Conv<'a> {
    params: &'a [ast::Expr],
    toks: &'a [TokenWithSpan],
    int_div_at: &'a [Pos],
    /// The end of the text, computed once (it is the place of an error "at the end of the input").
    end: Pos,
}

impl Conv<'_> {
    fn end(&self) -> Pos {
        self.end
    }

    /// The place of the first unquoted keyword `k` in the text (for errors about a clause: its keyword is the place).
    fn kw_pos(&self, k: Keyword) -> Option<Span2> {
        self.toks
            .iter()
            .find(
                |t| matches!(&t.token, Token::Word(w) if w.keyword == k && w.quote_style.is_none()),
            )
            .map(|t| t.span)
    }

    /// The index of the token that starts at `p`.
    fn tok_at(&self, p: Pos) -> Option<usize> {
        let i = self.toks.partition_point(|t| pos_of(t.span.start) < p);
        (i < self.toks.len() && pos_of(self.toks[i].span.start) == p).then_some(i)
    }

    /// The first token before index `i` that is not white space and not an opening parenthesis.
    fn back_over_parens(&self, i: usize) -> Option<&TokenWithSpan> {
        self.toks[..i]
            .iter()
            .rev()
            .find(|t| !matches!(t.token, Token::Whitespace(_) | Token::LParen))
    }

    fn statements(&self, stmts: &[sp::Statement]) -> Result<ast::Query> {
        match stmts {
            [] => Err(SqlError::syntax(self.end(), "empty query (write a SELECT)")),
            [one] => self.statement(one),
            [_, second, ..] => ns(Some(second.span()), self.end(), "more than one statement"),
        }
    }

    fn statement(&self, s: &sp::Statement) -> Result<ast::Query> {
        match s {
            sp::Statement::Query(q) => self.query(q, ExplainMode::No),
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
                    return ns(Some(s.span()), self.end(), "DESCRIBE");
                }
                if *verbose || *query_plan || *estimate || format.is_some() || options.is_some() {
                    return ns(Some(s.span()), self.end(), "EXPLAIN options");
                }
                match statement.as_ref() {
                    sp::Statement::Query(q) => self.query(
                        q,
                        if *analyze {
                            ExplainMode::Analyze
                        } else {
                            ExplainMode::Plan
                        },
                    ),
                    other => ns(
                        Some(other.span()),
                        self.end(),
                        &format!("EXPLAIN of {}", variant(other)),
                    ),
                }
            }
            other => ns(
                Some(other.span()),
                self.end(),
                &format!("{} statements", variant(other)),
            ),
        }
    }

    fn query(&self, q: &sp::Query, explain: ExplainMode) -> Result<ast::Query> {
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
        let end = self.end();
        if let Some(with) = with {
            return ns(
                self.kw_pos(Keyword::WITH).or(Some(with.span())),
                end,
                "WITH (common table expressions)",
            );
        }
        if let Some(o) = order_by {
            return ns(
                self.kw_pos(Keyword::ORDER).or(Some(o.span())),
                end,
                "ORDER BY",
            );
        }
        if let Some(f) = fetch {
            return ns(self.kw_pos(Keyword::FETCH).or(Some(f.span())), end, "FETCH");
        }
        if !locks.is_empty()
            || for_clause.is_some()
            || settings.is_some()
            || format_clause.is_some()
            || !pipe_operators.is_empty()
        {
            return ns(
                None,
                end,
                "query modifiers (FOR, SETTINGS, FORMAT, pipe operators)",
            );
        }
        let sel = match body.as_ref() {
            sp::SetExpr::Select(s) => s,
            sp::SetExpr::SetOperation { .. } => {
                let at = self
                    .kw_pos(Keyword::UNION)
                    .or(self.kw_pos(Keyword::INTERSECT))
                    .or(self.kw_pos(Keyword::EXCEPT));
                return ns(at, end, "UNION, INTERSECT and EXCEPT (set operations)");
            }
            sp::SetExpr::Values(v) => return ns(Some(v.span()), end, "VALUES"),
            other => return ns(Some(other.span()), end, "subqueries"),
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
            return Err(SqlError::syntax(
                pos_of(st),
                "SELECT clause without a selection list",
            ));
        }
        if distinct.is_some() {
            return ns(
                self.kw_pos(Keyword::DISTINCT).or(tok),
                end,
                "SELECT DISTINCT",
            );
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
            return ns(tok, end, "SELECT modifiers");
        }
        let group_by = match group_by {
            sp::GroupByExpr::Expressions(e, m) if m.is_empty() => e
                .iter()
                .map(|e| {
                    let key = self.expr(e, 0)?;
                    let ordinal = match &key.kind {
                        ExprKind::Number(n) => n.bytes().all(|c| c.is_ascii_digit()),
                        ExprKind::Unary { op: UnOp::Neg, expr } => matches!(&expr.kind, ExprKind::Number(n) if n.bytes().all(|c| c.is_ascii_digit())),
                        _ => false,
                    };
                    if ordinal { return ns(Some(e.span()), end, "GROUP BY ordinals"); }
                    if matches!(key.kind, ExprKind::Null) {
                        return ns(Some(e.span()), end, "GROUP BY NULL without a type (write CAST(NULL AS type))");
                    }
                    Ok(key)
                })
                .collect::<Result<Vec<_>>>()?,
            _ => {
                return ns(
                    self.kw_pos(Keyword::GROUP).or(tok),
                    end,
                    "GROUP BY extensions",
                );
            }
        };
        let having = having.as_ref().map(|e| self.expr(e, 0)).transpose()?;
        if !named_window.is_empty() || qualify.is_some() {
            return ns(
                self.kw_pos(Keyword::QUALIFY)
                    .or(self.kw_pos(Keyword::WINDOW))
                    .or(tok),
                end,
                "WINDOW and QUALIFY",
            );
        }
        if *flavor != sp::SelectFlavor::Standard {
            return ns(tok, end, "FROM-first queries");
        }

        let items = projection
            .iter()
            .map(|i| self.item(i))
            .collect::<Result<Vec<_>>>()?;
        let (from, from_pos) = self.from(from, tok)?;
        let where_ = selection.as_ref().map(|e| self.expr(e, 0)).transpose()?;
        let (limit, limit_pos, offset, offset_pos) = self.limit(limit_clause.as_ref())?;
        Ok(ast::Query {
            explain,
            items,
            from,
            from_pos,
            where_,
            group_by,
            having,
            limit,
            limit_pos,
            offset,
            offset_pos,
        })
    }

    fn from(&self, from: &[sp::TableWithJoins], tok: Option<SpSpan>) -> Result<(String, Span)> {
        let end = self.end();
        let twj = match from {
            [] => return ns(tok, end, "SELECT without FROM (write FROM 'path')"),
            [one] => one,
            [_, ..] => {
                // the comma that follows the first table
                let comma = self
                    .kw_pos(Keyword::FROM)
                    .and_then(|f| self.tok_at(sp_pos(f)))
                    .and_then(|i| {
                        self.toks[i..]
                            .iter()
                            .find(|t| t.token == Token::Comma)
                            .map(|t| t.span)
                    });
                return ns(comma, end, "more than one table (joins)");
            }
        };
        if !twj.joins.is_empty() {
            return ns(
                self.kw_pos(Keyword::JOIN).or(Some(twj.span())),
                end,
                "joins",
            );
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
                Some(twj.relation.span()),
                end,
                "a subquery or table function in FROM",
            );
        };
        if alias.is_some() {
            return ns(Some(twj.relation.span()), end, "table aliases");
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
                Some(twj.relation.span()),
                end,
                "table function arguments and table modifiers",
            );
        }
        match name.0.as_slice() {
            [sp::ObjectNamePart::Identifier(id)] if id.quote_style == Some('\'') => {
                // the crate gives a quoted table name no span: the string is the token after the word FROM
                let pos = if id.span.start.line > 0 {
                    sp_pos(id.span)
                } else {
                    self.kw_pos(Keyword::FROM)
                        .and_then(|f| self.tok_at(sp_pos(f)))
                        .and_then(|i| {
                            self.toks[i + 1..]
                                .iter()
                                .find(|t| !matches!(t.token, Token::Whitespace(_)))
                        })
                        .map_or(self.end(), |t| sp_pos(t.span))
                };
                Ok((id.value.clone(), Span(pos)))
            }
            _ => ns(
                Some(twj.relation.span()),
                end,
                "table names (only a quoted path or glob, FROM 'file.parquet')",
            ),
        }
    }

    /// A LIMIT or OFFSET value: a whole number written in digits.
    fn whole_number(&self, e: &sp::Expr, what: &str) -> Result<(u64, Span)> {
        let end = self.end();
        match e {
            sp::Expr::Value(v) => match &v.value {
                sp::Value::Placeholder(name) if !self.params.is_empty() => {
                    let i = name
                        .strip_prefix('$')
                        .and_then(|n| n.parse::<usize>().ok())
                        .filter(|i| *i > 0)
                        .ok_or_else(|| {
                            SqlError::syntax(sp_pos(v.span), "invalid parameter index")
                        })?;
                    let mut e = self
                        .params
                        .get(i - 1)
                        .ok_or_else(|| SqlError::bind(sp_pos(v.span), "missing parameter"))?;
                    while let ast::ExprKind::Cast { expr, .. } = &e.kind {
                        e = expr;
                    }
                    if let ast::ExprKind::Number(n) = &e.kind
                        && n.bytes().all(|c| c.is_ascii_digit())
                    {
                        return n
                            .parse::<u64>()
                            .map(|n| (n, Span(sp_pos(v.span))))
                            .map_err(|_| {
                                SqlError::bind(
                                    sp_pos(v.span),
                                    "LIMIT/OFFSET parameter out of range",
                                )
                            });
                    }
                    Err(SqlError::bind(
                        sp_pos(v.span),
                        "LIMIT/OFFSET parameter must be a nonnegative whole number",
                    ))
                }
                sp::Value::Number(s, false) if s.bytes().all(|c| c.is_ascii_digit()) => s
                    .parse::<u64>()
                    .map(|n| (n, Span(sp_pos(v.span))))
                    .map_err(|_| {
                        SqlError::bind(
                            sp_pos(v.span),
                            format!("{what} value is larger than 18446744073709551615"),
                        )
                    }),
                sp::Value::Number(..) => Err(SqlError::bind(
                    sp_pos(v.span),
                    format!("{what} needs a whole number written in digits"),
                )),
                _ => ns(
                    Some(v.span),
                    end,
                    &format!("{what} with anything but a whole number written in digits"),
                ),
            },
            sp::Expr::UnaryOp {
                op: sp::UnaryOperator::Minus,
                expr,
            } if matches!(expr.as_ref(), sp::Expr::Value(v) if matches!(v.value, sp::Value::Number(..))) => {
                Err(SqlError::bind(
                    sp_pos(expr.span()),
                    format!("{what} must not be negative"),
                ))
            }
            _ => ns(
                Some(e.span()),
                end,
                &format!("{what} with anything but a whole number written in digits"),
            ),
        }
    }

    #[allow(clippy::type_complexity)]
    fn limit(
        &self,
        lc: Option<&sp::LimitClause>,
    ) -> Result<(Option<u64>, Span, Option<u64>, Span)> {
        let end = self.end();
        match lc {
            None => Ok((None, Span::NONE, None, Span::NONE)),
            Some(sp::LimitClause::OffsetCommaLimit { offset, .. }) => ns(
                Some(offset.span()),
                end,
                "LIMIT offset, count (write LIMIT n OFFSET m)",
            ),
            Some(sp::LimitClause::LimitOffset {
                limit,
                offset,
                limit_by,
            }) => {
                if !limit_by.is_empty() {
                    return ns(Some(limit_by[0].span()), end, "LIMIT ... BY");
                }
                if let Some(o) = offset
                    && o.rows != sp::OffsetRows::None
                {
                    return ns(
                        Some(o.value.span()),
                        end,
                        "OFFSET ... ROWS (write OFFSET m)",
                    );
                }
                let off_expr = offset.as_ref().map(|o| &o.value);
                match (limit, off_expr) {
                    (None, None) => ns(None, end, "LIMIT ALL"),
                    (None, Some(o)) => ns(
                        Some(o.span()),
                        end,
                        "OFFSET without LIMIT (write LIMIT n OFFSET m)",
                    ),
                    (Some(l), o) => {
                        if let Some(o) = o {
                            let (ls, os) = (l.span().start, o.span().start);
                            if (os.line, os.column) < (ls.line, ls.column) {
                                return ns(
                                    Some(o.span()),
                                    end,
                                    "OFFSET before LIMIT (write LIMIT n OFFSET m)",
                                );
                            }
                        }
                        let (lv, lp) = self.whole_number(l, "LIMIT")?;
                        let off = o.map(|o| self.whole_number(o, "OFFSET")).transpose()?;
                        Ok((
                            Some(lv),
                            lp,
                            off.map(|x| x.0),
                            off.map_or(Span::NONE, |x| x.1),
                        ))
                    }
                }
            }
        }
    }

    fn item(&self, it: &sp::SelectItem) -> Result<Item> {
        let end = self.end();
        match it {
            sp::SelectItem::UnnamedExpr(e) => Ok(Item::Expr {
                expr: self.expr(e, 0)?,
                alias: None,
            }),
            sp::SelectItem::ExprWithAlias { expr, alias } => {
                // the expression first: `SELECT b + FROM 'f'` is a reserved word used as a name, not an alias
                let e = self.expr(expr, 0)?;
                // `SELECT a b` and `SELECT a AS b` give the same node: look at the token before the alias for the AS
                let has_as = alias.span.start.line > 0
                    && self.tok_at(sp_pos(alias.span)).is_some_and(|i| {
                        self.toks[..i]
                            .iter()
                            .rev()
                            .find(|t| !matches!(t.token, Token::Whitespace(_)))
                            .is_some_and(|t| {
                                matches!(&t.token, Token::Word(w) if w.keyword == Keyword::AS && w.quote_style.is_none())
                            })
                    });
                if !has_as {
                    // some aliases carry an empty span; the expression's is then the best place
                    let at = if alias.span.start.line > 0 {
                        alias.span
                    } else {
                        expr.span()
                    };
                    return ns(Some(at), end, "an alias without AS (write AS name)");
                }
                Ok(Item::Expr {
                    expr: e,
                    alias: Some(self.ident(alias)?),
                })
            }
            sp::SelectItem::Wildcard(o) => {
                let sp::WildcardAdditionalOptions {
                    wildcard_token,
                    opt_ilike,
                    opt_exclude,
                    opt_except,
                    opt_replace,
                    opt_rename,
                    opt_alias,
                } = o;
                let at = wildcard_token.0.span;
                if opt_ilike.is_some()
                    || opt_exclude.is_some()
                    || opt_except.is_some()
                    || opt_replace.is_some()
                    || opt_rename.is_some()
                    || opt_alias.is_some()
                {
                    return ns(Some(at), end, "modifiers on *");
                }
                Ok(Item::Star(Span(sp_pos(at))))
            }
            other => ns(
                Some(other.span()),
                end,
                &format!("{} in the select list", variant(other)),
            ),
        }
    }

    fn ident(&self, id: &sp::Ident) -> Result<Ident> {
        let at = sp_pos(id.span);
        let end = self.end();
        match id.quote_style {
            Some('"') if id.value.is_empty() => {
                Err(SqlError::syntax(at, "zero-length quoted identifier"))
            }
            Some('"') => Ok(Ident {
                name: id.value.clone(),
                quoted: true,
                span: Span(at),
            }),
            Some(_) => ns(
                Some(id.span),
                end,
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
                        Some(id.span),
                        end,
                        "identifier characters (letters, digits, _ and $)",
                    );
                }
                if is_reserved_word(&id.value) {
                    return Err(SqlError::syntax(
                        at,
                        format!(
                            "{} is a reserved word; quote it to use it as a name",
                            id.value
                        ),
                    ));
                }
                Ok(Ident {
                    name: id.value.clone(),
                    quoted: false,
                    span: Span(at),
                })
            }
        }
    }

    fn chain_error(&self, e: &sp::Expr) -> SqlError {
        SqlError::syntax(
            sp_pos(e.span()),
            "chained comparisons (and chained BETWEEN / IN) are not supported, use AND or parentheses",
        )
    }

    fn bx(&self, e: &sp::Expr, depth: usize) -> Result<Box<ast::Expr>> {
        Ok(Box::new(self.expr(e, depth)?))
    }

    /// Everything that takes an operand on its LEFT and so nests to the left in the crate's tree (`a OP b OP c`, `a::t::u`, `a IS NULL IS NULL`,
    /// `a BETWEEN .. AND ..`, `a IN (..)`) is walked in a loop: a chain costs no stack, however long (the guard bounds it at 10,000 terms).
    fn spine(&self, top: &sp::Expr, depth: usize) -> Result<ast::Expr> {
        enum Step<'x> {
            Bin(&'x sp::Expr, &'x sp::BinaryOperator, &'x sp::Expr),
            Colon(&'x sp::DataType, &'x sp::Expr),
            IsNull(bool),
            Between(&'x sp::Expr, &'x sp::Expr, &'x sp::Expr, bool),
            In(&'x sp::Expr, &'x [sp::Expr], bool),
        }
        let mut spine: Vec<Step<'_>> = Vec::new();
        let mut cur = top;
        loop {
            match cur {
                sp::Expr::BinaryOp { left, op, right } => {
                    spine.push(Step::Bin(cur, op, right));
                    cur = left;
                }
                sp::Expr::IsNull(x) => {
                    spine.push(Step::IsNull(false));
                    cur = x;
                }
                sp::Expr::IsNotNull(x) => {
                    spine.push(Step::IsNull(true));
                    cur = x;
                }
                sp::Expr::Cast {
                    kind: sp::CastKind::DoubleColon,
                    expr,
                    data_type,
                    format: None,
                } => {
                    spine.push(Step::Colon(data_type, cur));
                    cur = expr;
                }
                sp::Expr::Between {
                    expr,
                    negated,
                    low,
                    high,
                } => {
                    spine.push(Step::Between(cur, low, high, *negated));
                    cur = expr;
                }
                sp::Expr::InList {
                    expr,
                    list,
                    negated,
                } => {
                    spine.push(Step::In(cur, list, *negated));
                    cur = expr;
                }
                _ => break,
            }
        }
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
        // the node that is the LEFT operand of spine[i] is spine[i + 1] (a parenthesised one is a `Nested` node and ends the spine)
        let left_of = |i: usize| -> Option<&Step<'_>> { spine.get(i + 1) };
        let mut acc = self.expr(cur, depth + 1)?;
        for i in (0..spine.len()).rev() {
            let span = acc.span;
            let kind = match &spine[i] {
                Step::Bin(node, op, right) => {
                    let bop = match op {
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
                        other => {
                            return ns(
                                Some(node.span()),
                                self.end(),
                                &format!("the operator {other}"),
                            );
                        }
                    };
                    if is_cmp(op) {
                        let left_cmp = matches!(left_of(i), Some(Step::Bin(_, o, _)) if is_cmp(o));
                        let right_cmp =
                            matches!(right, sp::Expr::BinaryOp { op, .. } if is_cmp(op));
                        if left_cmp || right_cmp {
                            return Err(self.chain_error(node));
                        }
                    }
                    let r = self.expr(right, depth + 1)?;
                    // `a // b` and `a / b` are both `Divide` in the crate's tree: the operator token is the one before the right operand
                    let bop = if bop == BinOp::Div
                        && self
                            .operator_before(r.pos())
                            .is_some_and(|p| self.int_div_at.binary_search(&p).is_ok())
                    {
                        BinOp::IntDiv
                    } else {
                        bop
                    };
                    ExprKind::Binary {
                        op: bop,
                        left: Box::new(acc),
                        right: Box::new(r),
                    }
                }
                Step::Colon(dt, whole) => ExprKind::Cast {
                    expr: Box::new(acc),
                    ty: self.data_type(dt, whole)?,
                },
                Step::IsNull(negated) => ExprKind::IsNull {
                    expr: Box::new(acc),
                    negated: *negated,
                },
                Step::Between(node, low, high, negated) => {
                    if matches!(left_of(i), Some(Step::Between(..) | Step::In(..))) {
                        return Err(self.chain_error(node));
                    }
                    ExprKind::Between {
                        expr: Box::new(acc),
                        low: self.bx(low, depth + 1)?,
                        high: self.bx(high, depth + 1)?,
                        negated: *negated,
                    }
                }
                Step::In(node, list, negated) => {
                    if matches!(left_of(i), Some(Step::Between(..) | Step::In(..))) {
                        return Err(self.chain_error(node));
                    }
                    ExprKind::InList {
                        expr: Box::new(acc),
                        list: list
                            .iter()
                            .map(|v| self.expr(v, depth + 1))
                            .collect::<Result<_>>()?,
                        negated: *negated,
                    }
                }
            };
            acc = ast::Expr::new(kind, span);
        }
        Ok(acc)
    }

    fn expr(&self, e: &sp::Expr, depth: usize) -> Result<ast::Expr> {
        let end = self.end();
        if depth > 3 * MAX_NESTING {
            return Err(limit_error(
                sp_pos(e.span()),
                "expression nested too deeply",
            ));
        }
        let d = depth + 1;
        let mk = |kind: ExprKind, pos: Pos| ast::Expr::new(kind, Span(pos));
        Ok(match e {
            sp::Expr::BinaryOp { .. }
            | sp::Expr::IsNull(_)
            | sp::Expr::IsNotNull(_)
            | sp::Expr::Between { .. }
            | sp::Expr::InList { .. }
            | sp::Expr::Cast {
                kind: sp::CastKind::DoubleColon,
                format: None,
                ..
            } => return self.spine(e, depth),
            sp::Expr::Identifier(id) => {
                let id = self.ident(id)?;
                let pos = id.span.0;
                mk(ExprKind::Ident(id), pos)
            }
            sp::Expr::CompoundIdentifier(_) => {
                return ns(Some(e.span()), end, "qualified names (table.column)");
            }
            sp::Expr::Value(v) => {
                let pos = sp_pos(v.span);
                match &v.value {
                    sp::Value::Number(s, false) => {
                        check_number_text(s, pos)?;
                        mk(ExprKind::Number(s.clone()), pos)
                    }
                    sp::Value::Placeholder(name) if !self.params.is_empty() => {
                        let i = name
                            .strip_prefix('$')
                            .and_then(|n| n.parse::<usize>().ok())
                            .filter(|i| *i > 0)
                            .ok_or_else(|| SqlError::syntax(pos, "invalid parameter index"))?;
                        let mut value = self
                            .params
                            .get(i - 1)
                            .cloned()
                            .ok_or_else(|| SqlError::bind(pos, "missing parameter"))?;
                        // Parameter trees contain only a literal and at most a cast/sign.
                        fn locate(e: &mut ast::Expr, pos: Pos) {
                            e.span = ast::Span(pos);
                            match &mut e.kind {
                                ast::ExprKind::Cast { expr, .. }
                                | ast::ExprKind::Unary { expr, .. } => locate(expr, pos),
                                _ => {}
                            }
                        }
                        locate(&mut value, pos);
                        value
                    }
                    sp::Value::SingleQuotedString(s) => mk(ExprKind::Str(s.clone()), pos),
                    sp::Value::Null => mk(ExprKind::Null, pos),
                    sp::Value::Boolean(b) => mk(ExprKind::Bool(*b), pos),
                    other => {
                        return ns(
                            Some(v.span),
                            end,
                            &format!("this kind of literal ({})", variant(other)),
                        );
                    }
                }
            }
            sp::Expr::TypedString(ts) => match (&ts.data_type, &ts.value.value) {
                (sp::DataType::Date, sp::Value::SingleQuotedString(s)) => mk(
                    ExprKind::Date(s.clone()),
                    self.date_pos(sp_pos(ts.value.span)),
                ),
                _ => {
                    return ns(
                        Some(e.span()),
                        end,
                        "typed literals (only DATE 'yyyy-mm-dd')",
                    );
                }
            },
            sp::Expr::UnaryOp { op, expr } => {
                let inner = self.expr(expr, d)?;
                let pos = self.unary_pos(inner.pos());
                let uop = match op {
                    sp::UnaryOperator::Minus => UnOp::Neg,
                    sp::UnaryOperator::Not => UnOp::Not,
                    other => {
                        return ns(
                            Some(expr.span()),
                            pos,
                            &format!("the unary operator {other}"),
                        )
                        .map_err(|mut x: SqlError| {
                            x.pos = pos;
                            x
                        });
                    }
                };
                mk(
                    ExprKind::Unary {
                        op: uop,
                        expr: Box::new(inner),
                    },
                    pos,
                )
            }
            sp::Expr::Nested(inner) => {
                // the place of a parenthesised expression is its opening parenthesis
                let mut x = self.expr(inner, d)?;
                if let Some(i) = self.tok_at(x.pos())
                    && let Some(t) = self.toks[..i]
                        .iter()
                        .rev()
                        .find(|t| !matches!(t.token, Token::Whitespace(_)))
                    && t.token == Token::LParen
                {
                    x.span = Span(sp_pos(t.span));
                }
                return Ok(x);
            }
            sp::Expr::Case {
                case_token,
                end_token: _,
                operand,
                conditions,
                else_result,
            } => mk(
                ExprKind::Case {
                    operand: operand.as_ref().map(|o| self.bx(o, d)).transpose()?,
                    whens: conditions
                        .iter()
                        .map(|w| Ok((self.expr(&w.condition, d)?, self.expr(&w.result, d)?)))
                        .collect::<Result<_>>()?,
                    else_: else_result.as_ref().map(|o| self.bx(o, d)).transpose()?,
                },
                sp_pos(case_token.0.span),
            ),
            sp::Expr::Cast {
                kind,
                expr,
                data_type,
                format,
            } => {
                let colon = match kind {
                    sp::CastKind::Cast => false,
                    sp::CastKind::DoubleColon => true,
                    other => {
                        return ns(Some(e.span()), end, &format!("{other:?}"));
                    }
                };
                if format.is_some() {
                    return ns(Some(e.span()), end, "CAST ... FORMAT");
                }
                let inner = self.bx(expr, d)?;
                // `CAST(x AS t)` starts at the word CAST, `x::t` at x
                let pos = if colon {
                    inner.pos()
                } else {
                    self.cast_pos(inner.pos())
                };
                let ty = self.data_type(data_type, e)?;
                mk(ExprKind::Cast { expr: inner, ty }, pos)
            }
            sp::Expr::Function(f) => {
                let name = f.name.to_string().to_lowercase();
                if !["count", "sum", "avg", "min", "max"].contains(&name.as_str()) {
                    return ns(
                        Some(e.span()),
                        end,
                        &format!("function calls ({}(...))", f.name),
                    );
                }
                if f.over.is_some() {
                    return ns(Some(e.span()), end, "window functions");
                }
                if f.filter.is_some() {
                    return ns(Some(e.span()), end, "aggregate FILTER");
                }
                if !f.within_group.is_empty()
                    || f.null_treatment.is_some()
                    || f.uses_odbc_syntax
                    || !matches!(f.parameters, sp::FunctionArguments::None)
                {
                    return ns(Some(e.span()), end, "aggregate modifiers");
                }
                let sp::FunctionArguments::List(args) = &f.args else {
                    return ns(Some(e.span()), end, "aggregate arguments");
                };
                if args.duplicate_treatment == Some(sp::DuplicateTreatment::Distinct) {
                    return ns(Some(e.span()), end, "DISTINCT aggregates");
                }
                if !args.clauses.is_empty() {
                    return ns(Some(e.span()), end, "aggregate ORDER BY or modifiers");
                }
                let arg = match args.args.as_slice() {
                    [sp::FunctionArg::Unnamed(sp::FunctionArgExpr::Wildcard)]
                        if name == "count" =>
                    {
                        None
                    }
                    [sp::FunctionArg::Unnamed(sp::FunctionArgExpr::Expr(arg))] => {
                        Some(self.bx(arg, d)?)
                    }
                    _ => return ns(Some(e.span()), end, "aggregate arguments"),
                };
                mk(ExprKind::Aggregate { name, arg }, sp_pos(e.span()))
            }
            sp::Expr::Subquery(_) | sp::Expr::Exists { .. } | sp::Expr::InSubquery { .. } => {
                return ns(Some(e.span()), end, "subqueries");
            }
            sp::Expr::Like { .. } | sp::Expr::ILike { .. } | sp::Expr::SimilarTo { .. } => {
                return ns(Some(e.span()), end, "LIKE, ILIKE and SIMILAR TO");
            }
            other => return ns(Some(other.span()), end, &variant(other)),
        })
    }

    /// The place of the token in front of an operand that starts at `operand` (past white space and opening parentheses).
    fn operator_before(&self, operand: Pos) -> Option<Pos> {
        self.tok_at(operand)
            .and_then(|i| self.back_over_parens(i))
            .map(|t| sp_pos(t.span))
    }

    /// The place of the prefix operator in front of an operand that starts at `operand`.
    fn unary_pos(&self, operand: Pos) -> Pos {
        self.tok_at(operand)
            .and_then(|i| self.back_over_parens(i))
            .map_or(operand, |t| sp_pos(t.span))
    }

    /// `DATE '...'` starts at the word DATE, which is the token before the string.
    fn date_pos(&self, string_pos: Pos) -> Pos {
        self.tok_at(string_pos)
            .and_then(|i| {
                self.toks[..i]
                    .iter()
                    .rev()
                    .find(|t| !matches!(t.token, Token::Whitespace(_)))
            })
            .filter(|t| matches!(&t.token, Token::Word(w) if w.keyword == Keyword::DATE))
            .map_or(string_pos, |t| sp_pos(t.span))
    }

    /// The place of the word CAST in front of the operand that starts at `operand`.
    fn cast_pos(&self, operand: Pos) -> Pos {
        self.tok_at(operand)
            .and_then(|i| self.back_over_parens(i))
            .filter(|t| matches!(&t.token, Token::Word(w) if w.keyword == Keyword::CAST))
            .map_or(operand, |t| sp_pos(t.span))
    }

    fn data_type(&self, t: &sp::DataType, whole: &sp::Expr) -> Result<Ty> {
        use sp::DataType as D;
        use sp::ExactNumberInfo as N;
        let end = self.end();
        let at = Some(whole.span());
        let dec = |n: &N| -> Result<Ty> {
            let (p, s) = match n {
                N::None => (18, 3),
                N::Precision(p) => (*p, 0),
                N::PrecisionAndScale(p, s) => (*p, *s),
            };
            match (u8::try_from(p), u8::try_from(s)) {
                (Ok(precision), Ok(scale)) => Ok(Ty::Decimal { precision, scale }),
                _ => ns(at, end, "DECIMAL precision and scale outside 0..255"),
            }
        };
        match t {
            D::Boolean | D::Bool => Ok(Ty::Boolean),
            D::Int(None) | D::Integer(None) => Ok(Ty::Integer),
            D::BigInt(None) => Ok(Ty::BigInt),
            D::DoublePrecision => Ok(Ty::Double),
            D::Double(N::None) => Ok(Ty::Double),
            D::Decimal(n) | D::Numeric(n) | D::Dec(n) => dec(n),
            D::Varchar(None) => Ok(Ty::Varchar),
            D::Date => Ok(Ty::Date),
            other => ns(at, end, &format!("the type {other}")),
        }
    }
}

/// Digits, one optional point, an optional exponent: what the binder reads exactly. (The crate's tokenizer lets other characters through in some
/// dialects, `1_000` for one.)
fn check_number_text(s: &str, pos: Pos) -> Result<()> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut digits = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
    }
    let mantissa_ok = digits > 0;
    let mut exp_ok = true;
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        exp_ok = i > start;
    }
    if mantissa_ok && exp_ok && i == b.len() {
        Ok(())
    } else {
        Err(SqlError::syntax(
            pos,
            "a number is digits with an optional point and exponent (no separators or suffixes)",
        ))
    }
}
