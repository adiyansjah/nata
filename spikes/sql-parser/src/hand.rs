//! Arm B: a hand-written recursive-descent / precedence-climbing parser for exactly the slice-4 subset. Byte spans on every expression node, line and
//! column on every error, `not supported yet: <what>` for everything recognisably outside the subset, a nesting-depth limit of `MAX_DEPTH`.
//! No dependencies, no `unsafe`.

use crate::ast::*;

/// The error is boxed so that every `Result` on the recursion path is as small as the value it carries.
type PResult<T> = Result<T, Box<ParseError>>;

/// Expression nesting deeper than this (1,000 plus a margin; parentheses, `NOT`, unary minus, and the length of a chain `a AND b AND c ...`) is an error, never a stack overflow.
pub const MAX_DEPTH: usize = 1024;

/// A deliberately wrong parser, to show that the checks can fail (spike only). `None` is the real parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mutant {
    None,
    /// `OR` binds tighter than `AND`.
    OrBeforeAnd,
    /// `NOT` is parsed and then dropped.
    LoseNot,
    /// A number's text goes through `f64` and back.
    NumberViaF64,
}

pub fn parse(text: &str) -> Result<Query, ParseError> {
    parse_with(text, Mutant::None)
}

pub fn parse_with(text: &str, mutant: Mutant) -> Result<Query, ParseError> {
    parse_inner(text, mutant).map_err(|e| *e)
}

fn parse_inner(text: &str, mutant: Mutant) -> PResult<Query> {
    let mut p = Parser {
        src: text,
        b: text.as_bytes(),
        pos: 0,
        cur: Token::EOF,
        ahead: None,
        depth: 0,
        last_end: 0,
        mutant,
    };
    p.cur = p.lex()?;
    p.query()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum K {
    Word,
    QIdent,
    Number,
    Str,
    LParen,
    RParen,
    Comma,
    Semi,
    Star,
    Plus,
    Minus,
    Slash,
    Percent,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Dot,
    ColonColon,
    Concat,
    Other,
    Eof,
}

#[derive(Debug, Clone, Copy)]
struct Token {
    k: K,
    s: usize,
    e: usize,
}

impl Token {
    const EOF: Token = Token {
        k: K::Eof,
        s: 0,
        e: 0,
    };
}

// Words that cannot be an unquoted column name or alias: DuckDB 2.0.0's `duckdb_keywords()` with keyword_category 'reserved' or 'type_function'
// (the words PostgreSQL's grammar also refuses as a ColId). Sorted.
const RESERVED: &[&str] = &[
    "ALL",
    "ANALYSE",
    "ANALYZE",
    "AND",
    "ANTI",
    "ANY",
    "ARRAY",
    "AS",
    "ASC",
    "ASOF",
    "ASYMMETRIC",
    "AT",
    "AUTHORIZATION",
    "BINARY",
    "BOTH",
    "BY",
    "CASE",
    "CAST",
    "CHECK",
    "COLLATE",
    "COLLATION",
    "COLUMN",
    "COLUMNS",
    "CONCURRENTLY",
    "CONSTRAINT",
    "CREATE",
    "CROSS",
    "DEFAULT",
    "DEFERRABLE",
    "DESC",
    "DESCRIBE",
    "DISTINCT",
    "DO",
    "ELSE",
    "END",
    "EXCEPT",
    "FALSE",
    "FETCH",
    "FOR",
    "FOREIGN",
    "FREEZE",
    "FROM",
    "FULL",
    "GENERATED",
    "GLOB",
    "GROUP",
    "HAVING",
    "ILIKE",
    "IN",
    "INITIALLY",
    "INNER",
    "INTERSECT",
    "INTO",
    "IS",
    "ISNULL",
    "JOIN",
    "LAMBDA",
    "LATERAL",
    "LEADING",
    "LEFT",
    "LIKE",
    "LIMIT",
    "MAP",
    "NATURAL",
    "NOT",
    "NOTNULL",
    "NULL",
    "OFFSET",
    "ON",
    "ONLY",
    "OR",
    "ORDER",
    "OUTER",
    "OVERLAPS",
    "PIVOT",
    "PIVOT_LONGER",
    "PIVOT_WIDER",
    "PLACING",
    "POSITIONAL",
    "PRIMARY",
    "QUALIFY",
    "REFERENCES",
    "RETURNING",
    "RIGHT",
    "SELECT",
    "SEMI",
    "SHOW",
    "SIMILAR",
    "SOME",
    "STRUCT",
    "SUMMARIZE",
    "SYMMETRIC",
    "TABLE",
    "TABLESAMPLE",
    "THEN",
    "TO",
    "TRAILING",
    "TRUE",
    "TRY_CAST",
    "TUPLE",
    "UNION",
    "UNIQUE",
    "UNPACK",
    "UNPIVOT",
    "USING",
    "VARIADIC",
    "VERBOSE",
    "WHEN",
    "WHERE",
    "WINDOW",
    "WITH",
];

enum Infix {
    End,
    Op(u8, Option<BinOp>),
    Reject(&'static str),
    RejectOp,
}

struct Parser<'a> {
    src: &'a str,
    b: &'a [u8],
    pos: usize,
    cur: Token,
    ahead: Option<Token>,
    depth: usize,
    last_end: usize,
    mutant: Mutant,
}

/// The non-ASCII characters DuckDB 2.0.0's lexer treats as white space (found by experiment, `bench/results/20261106-sql-parser-baseline.md`); every other
/// non-ASCII character is part of a name, as in DuckDB.
fn is_unicode_space(c: char) -> bool {
    matches!(
        c,
        '\u{a0}' | '\u{2000}'..='\u{200b}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}
fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}
fn is_ident_part(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$' || c >= 0x80
}

impl<'a> Parser<'a> {
    // ------------------------------------------------------------ errors

    #[cold]
    #[inline(never)]
    fn error(&self, kind: ErrorKind, offset: usize, message: String) -> Box<ParseError> {
        let (line, col) = line_col(self.src, offset);
        Box::new(ParseError {
            kind,
            message,
            line: Some(line),
            col: Some(col),
            offset: Some(offset),
        })
    }
    #[cold]
    #[inline(never)]
    fn syntax<T>(&self, offset: usize, message: String) -> PResult<T> {
        Err(self.error(ErrorKind::Syntax, offset, message))
    }
    #[cold]
    #[inline(never)]
    fn unsupported<T>(&self, offset: usize, what: &str) -> PResult<T> {
        Err(self.error(ErrorKind::NotSupported, offset, what.to_string()))
    }
    #[cold]
    #[inline(never)]
    fn describe(&self, t: Token) -> String {
        if t.k == K::Eof {
            return "end of query".to_string();
        }
        let s = &self.src[t.s..t.e];
        if s.chars().count() > 24 {
            format!("'{}...'", s.chars().take(24).collect::<String>())
        } else {
            format!("'{s}'")
        }
    }
    #[cold]
    #[inline(never)]
    fn expected<T>(&self, what: &str) -> PResult<T> {
        let t = self.cur;
        self.syntax(
            if t.k == K::Eof { self.src.len() } else { t.s },
            format!("expected {what}, found {}", self.describe(t)),
        )
    }

    // ------------------------------------------------------------ lexer

    fn lex(&mut self) -> PResult<Token> {
        loop {
            // whitespace and comments
            while let Some(&c) = self.b.get(self.pos) {
                if matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c) {
                    self.pos += 1;
                } else if c >= 0x80
                    && self.src[self.pos..]
                        .chars()
                        .next()
                        .is_some_and(is_unicode_space)
                {
                    self.pos += self.src[self.pos..]
                        .chars()
                        .next()
                        .map_or(1, char::len_utf8);
                } else {
                    break;
                }
            }
            if self.b[self.pos..].starts_with(b"--") {
                while self.pos < self.b.len() && self.b[self.pos] != b'\n' {
                    self.pos += 1;
                }
                continue;
            }
            if self.b[self.pos..].starts_with(b"/*") {
                let start = self.pos;
                let mut depth = 0usize;
                loop {
                    if self.pos >= self.b.len() {
                        return self.syntax(start, "unterminated /* comment".to_string());
                    }
                    if self.b[self.pos..].starts_with(b"/*") {
                        depth += 1;
                        self.pos += 2;
                    } else if self.b[self.pos..].starts_with(b"*/") {
                        depth -= 1;
                        self.pos += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        self.pos += 1;
                    }
                }
                continue;
            }
            break;
        }
        let s = self.pos;
        let Some(&c) = self.b.get(s) else {
            return Ok(Token {
                k: K::Eof,
                s: self.b.len(),
                e: self.b.len(),
            });
        };
        let simple = |p: &mut Self, k: K, n: usize| {
            p.pos += n;
            Ok(Token { k, s, e: p.pos })
        };
        let next = self.b.get(s + 1).copied();
        match c {
            b'(' => simple(self, K::LParen, 1),
            b')' => simple(self, K::RParen, 1),
            b',' => simple(self, K::Comma, 1),
            b';' => simple(self, K::Semi, 1),
            b'*' => simple(self, K::Star, 1),
            b'+' => simple(self, K::Plus, 1),
            b'-' => simple(self, K::Minus, 1),
            b'/' => simple(self, K::Slash, 1),
            b'%' => simple(self, K::Percent, 1),
            b'=' => simple(self, K::Eq, 1),
            b'<' => match next {
                Some(b'=') => simple(self, K::LtEq, 2),
                Some(b'>') => simple(self, K::NotEq, 2),
                _ => simple(self, K::Lt, 1),
            },
            b'>' => {
                if next == Some(b'=') {
                    simple(self, K::GtEq, 2)
                } else {
                    simple(self, K::Gt, 1)
                }
            }
            b'!' if next == Some(b'=') => simple(self, K::NotEq, 2),
            b':' if next == Some(b':') => simple(self, K::ColonColon, 2),
            b'|' if next == Some(b'|') => simple(self, K::Concat, 2),
            b'.' if !next.is_some_and(|d| d.is_ascii_digit()) => simple(self, K::Dot, 1),
            b'\'' => {
                self.pos += 1;
                loop {
                    match self.b.get(self.pos) {
                        None => return self.syntax(s, "unterminated string literal".to_string()),
                        Some(b'\'') if self.b.get(self.pos + 1) == Some(&b'\'') => self.pos += 2,
                        Some(b'\'') => {
                            self.pos += 1;
                            return Ok(Token {
                                k: K::Str,
                                s,
                                e: self.pos,
                            });
                        }
                        Some(_) => self.pos += 1,
                    }
                }
            }
            b'"' => {
                self.pos += 1;
                loop {
                    match self.b.get(self.pos) {
                        None => {
                            return self.syntax(s, "unterminated quoted identifier".to_string());
                        }
                        Some(b'"') if self.b.get(self.pos + 1) == Some(&b'"') => self.pos += 2,
                        Some(b'"') => {
                            self.pos += 1;
                            if self.pos - s == 2 {
                                return self.syntax(s, "zero-length quoted identifier".to_string());
                            }
                            return Ok(Token {
                                k: K::QIdent,
                                s,
                                e: self.pos,
                            });
                        }
                        Some(_) => self.pos += 1,
                    }
                }
            }
            b'0'..=b'9' | b'.' => {
                while self.b.get(self.pos).is_some_and(u8::is_ascii_digit) {
                    self.pos += 1;
                }
                if self.b.get(self.pos) == Some(&b'.') {
                    self.pos += 1;
                    while self.b.get(self.pos).is_some_and(u8::is_ascii_digit) {
                        self.pos += 1;
                    }
                }
                if matches!(self.b.get(self.pos), Some(b'e' | b'E')) {
                    let mut q = self.pos + 1;
                    if matches!(self.b.get(q), Some(b'+' | b'-')) {
                        q += 1;
                    }
                    if self.b.get(q).is_some_and(u8::is_ascii_digit) {
                        while self.b.get(q).is_some_and(u8::is_ascii_digit) {
                            q += 1;
                        }
                        self.pos = q;
                    }
                }
                if self.b.get(self.pos).is_some_and(|&c| is_ident_start(c))
                    && !self.src[self.pos..]
                        .chars()
                        .next()
                        .is_some_and(is_unicode_space)
                {
                    let next = self.src[self.pos..]
                        .chars()
                        .next()
                        .map_or(0, char::len_utf8);
                    return self.syntax(
                        s,
                        format!(
                            "trailing characters after numeric literal '{}'",
                            &self.src[s..self.pos + next]
                        ),
                    );
                }
                Ok(Token {
                    k: K::Number,
                    s,
                    e: self.pos,
                })
            }
            c if is_ident_start(c) => {
                loop {
                    match self.b.get(self.pos) {
                        Some(&c) if c >= 0x80 => {
                            let ch = self.src[self.pos..].chars().next().unwrap_or(' ');
                            if is_unicode_space(ch) {
                                break;
                            }
                            self.pos += ch.len_utf8();
                        }
                        Some(&c) if is_ident_part(c) => self.pos += 1,
                        _ => break,
                    }
                }
                Ok(Token {
                    k: K::Word,
                    s,
                    e: self.pos,
                })
            }
            _ => {
                // one whole character
                let ch = self.src[s..].chars().next().map_or(1, char::len_utf8);
                simple(self, K::Other, ch)
            }
        }
    }

    // ------------------------------------------------------------ token helpers

    fn bump(&mut self) -> PResult<Token> {
        let t = self.cur;
        self.last_end = t.e;
        self.cur = match self.ahead.take() {
            Some(a) => a,
            None => self.lex()?,
        };
        Ok(t)
    }
    fn peek2(&mut self) -> PResult<Token> {
        if self.ahead.is_none() {
            self.ahead = Some(self.lex()?);
        }
        Ok(self.ahead.unwrap_or(Token::EOF))
    }
    fn text(&self, t: Token) -> &'a str {
        &self.src[t.s..t.e]
    }
    fn is_kw(&self, t: Token, kw: &str) -> bool {
        t.k == K::Word && self.text(t).eq_ignore_ascii_case(kw)
    }
    fn at_kw(&self, kw: &str) -> bool {
        self.is_kw(self.cur, kw)
    }
    fn eat_kw(&mut self, kw: &str) -> PResult<bool> {
        if self.at_kw(kw) {
            self.bump()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn expect_kw(&mut self, kw: &str) -> PResult<()> {
        if self.at_kw(kw) {
            self.bump().map(|_| ())
        } else {
            self.expected(kw)
        }
    }
    fn expect(&mut self, k: K, what: &str) -> PResult<Token> {
        if self.cur.k == k {
            self.bump()
        } else {
            self.expected(what)
        }
    }
    fn is_reserved(&self, t: Token) -> bool {
        t.k == K::Word
            && RESERVED
                .iter()
                .any(|r| self.text(t).eq_ignore_ascii_case(r))
    }
    fn unquote(&self, t: Token) -> String {
        let inner = &self.src[t.s + 1..t.e - 1];
        let q = self.b[t.s] as char;
        inner.replace(&format!("{q}{q}"), &q.to_string())
    }
    fn enter(&mut self, extra: usize) -> PResult<()> {
        self.depth += extra;
        if self.depth > MAX_DEPTH {
            let at = if self.cur.k == K::Eof {
                self.src.len()
            } else {
                self.cur.s
            };
            return Err(self.error(
                ErrorKind::Limit,
                at,
                format!("expression nested deeper than {MAX_DEPTH}"),
            ));
        }
        Ok(())
    }

    // ------------------------------------------------------------ statement

    fn query(&mut self) -> PResult<Query> {
        let mut explain = ExplainMode::No;
        if self.at_kw("EXPLAIN") {
            self.bump()?;
            explain = ExplainMode::Plan;
            if self.at_kw("ANALYZE") || self.at_kw("ANALYSE") {
                self.bump()?;
                explain = ExplainMode::Analyze;
            } else if self.cur.k == K::LParen || self.at_kw("VERBOSE") || self.at_kw("FORMAT") {
                return self.unsupported(self.cur.s, "EXPLAIN options");
            }
        }
        if !self.at_kw("SELECT") {
            let t = self.cur;
            if t.k == K::Word {
                let w = self.text(t).to_ascii_uppercase();
                return match w.as_str() {
                    "WITH" => self.unsupported(t.s, "WITH (common table expressions)"),
                    "VALUES" => self.unsupported(t.s, "VALUES"),
                    "FROM" => self.unsupported(t.s, "FROM-first queries"),
                    "EXPLAIN" => self.unsupported(t.s, "nested EXPLAIN"),
                    "INSERT" | "UPDATE" | "DELETE" | "CREATE" | "DROP" | "ALTER" | "COPY"
                    | "SET" | "PRAGMA" | "DESCRIBE" | "SHOW" | "ATTACH" | "USE" | "CALL"
                    | "BEGIN" | "COMMIT" | "ROLLBACK" | "EXPORT" | "IMPORT" | "INSTALL"
                    | "LOAD" | "TABLE" | "SUMMARIZE" | "PREPARE" | "EXECUTE" | "TRUNCATE"
                    | "ANALYZE" | "VACUUM" | "CHECKPOINT" | "RESET" | "DETACH" | "MERGE"
                    | "FORCE" => self.unsupported(t.s, &format!("{w} statements")),
                    _ => self.expected("SELECT or EXPLAIN"),
                };
            }
            return if t.k == K::LParen {
                self.unsupported(t.s, "parenthesised queries")
            } else {
                self.expected("SELECT or EXPLAIN")
            };
        }
        self.bump()?;
        for (kw, what) in [
            ("DISTINCT", "SELECT DISTINCT"),
            ("ALL", "SELECT ALL"),
            ("TOP", "SELECT TOP"),
        ] {
            if self.at_kw(kw) {
                return self.unsupported(self.cur.s, what);
            }
        }
        let mut items = Vec::new();
        loop {
            items.push(self.item()?);
            if self.cur.k == K::Comma {
                self.bump()?;
                continue;
            }
            break;
        }
        if !self.at_kw("FROM") {
            let t = self.cur;
            let aliased = items
                .last()
                .is_some_and(|i| matches!(i, SelectItem::Expr { alias: Some(_), .. }));
            return match t.k {
                K::Eof | K::Semi => self.unsupported(
                    self.src.len().min(t.s),
                    "SELECT without FROM (write FROM 'path')",
                ),
                K::Word if self.is_kw(t, "WHERE") || self.is_kw(t, "LIMIT") => {
                    self.unsupported(t.s, "SELECT without FROM (write FROM 'path')")
                }
                K::Word if !self.is_reserved(t) && !aliased => {
                    self.unsupported(t.s, "an alias without AS (write AS name)")
                }
                _ => self.expected("FROM"),
            };
        }
        self.bump()?;
        let t = self.cur;
        let from = match t.k {
            K::Str => {
                self.bump()?;
                self.unquote(t)
            }
            K::Word | K::QIdent => {
                return self.unsupported(
                    t.s,
                    "table names (only a quoted path or glob, FROM 'file.parquet')",
                );
            }
            K::LParen => return self.unsupported(t.s, "a subquery or table function in FROM"),
            _ => return self.expected("a quoted file path after FROM"),
        };
        self.after_clause(true)?;
        let where_ = if self.at_kw("WHERE") {
            self.bump()?;
            Some(self.expr()?)
        } else {
            None
        };
        self.after_clause(false)?;
        let (mut limit, mut offset) = (None, None);
        if self.at_kw("LIMIT") {
            self.bump()?;
            limit = Some(self.whole_number("LIMIT")?);
            if self.at_kw("OFFSET") {
                self.bump()?;
                offset = Some(self.whole_number("OFFSET")?);
            }
        } else if self.at_kw("OFFSET") {
            return self.unsupported(self.cur.s, "OFFSET without LIMIT (write LIMIT n OFFSET m)");
        }
        self.after_clause(false)?;
        if self.cur.k == K::Semi {
            self.bump()?;
        }
        match self.cur.k {
            K::Eof => {}
            _ if self.cur.k == K::Semi || self.at_kw("SELECT") || self.at_kw("EXPLAIN") => {
                return self.unsupported(self.cur.s, "more than one statement");
            }
            _ => {
                return self.syntax(
                    self.cur.s,
                    format!(
                        "unexpected {} after the end of the query",
                        self.describe(self.cur)
                    ),
                );
            }
        }
        Ok(Query {
            explain,
            items,
            from,
            where_,
            limit,
            offset,
        })
    }

    /// Names the clause when the next token starts something outside the subset; returns when the next token may belong to the subset.
    fn after_clause(&mut self, after_from: bool) -> PResult<()> {
        let t = self.cur;
        if t.k == K::Comma {
            return self.unsupported(t.s, "more than one table (joins)");
        }
        if t.k != K::Word {
            return Ok(());
        }
        let w = self.text(t).to_ascii_uppercase();
        let what = match w.as_str() {
            "JOIN" | "INNER" | "LEFT" | "RIGHT" | "FULL" | "CROSS" | "NATURAL" | "ASOF"
            | "POSITIONAL" | "ANTI" | "SEMI" | "OUTER" => "joins",
            "GROUP" => "GROUP BY",
            "HAVING" => "HAVING",
            "ORDER" => "ORDER BY",
            "UNION" | "INTERSECT" | "EXCEPT" => "UNION, INTERSECT and EXCEPT",
            "QUALIFY" => "QUALIFY",
            "WINDOW" => "WINDOW",
            "SAMPLE" | "TABLESAMPLE" | "USING" => "sampling and USING",
            "FETCH" => "FETCH",
            "FOR" => "locking clauses",
            "AS" if after_from => "table aliases",
            "WHERE" if !after_from => return Ok(()),
            _ if after_from
                && !self.is_reserved(t)
                && w != "WHERE"
                && w != "LIMIT"
                && w != "OFFSET" =>
            {
                "table aliases"
            }
            _ => return Ok(()),
        };
        self.unsupported(t.s, what)
    }

    fn whole_number(&mut self, what: &str) -> PResult<u64> {
        let t = self.cur;
        if t.k != K::Number {
            return if t.k == K::Minus || t.k == K::Plus || t.k == K::LParen || t.k == K::Word {
                self.unsupported(
                    t.s,
                    &format!("{what} with anything but a whole number written in digits"),
                )
            } else {
                self.expected(&format!("a whole number after {what}"))
            };
        }
        let text = self.text(t);
        match text.parse::<u64>() {
            Ok(n) => {
                self.bump()?;
                Ok(n)
            }
            Err(_) if text.bytes().all(|c| c.is_ascii_digit()) => self.syntax(
                t.s,
                format!("{what} value {text} is larger than 18446744073709551615"),
            ),
            Err(_) => self.syntax(t.s, format!("{what} needs a whole number, found {text}")),
        }
    }

    fn item(&mut self) -> PResult<SelectItem> {
        if self.cur.k == K::Star {
            let star = self.bump()?;
            if self.cur.k == K::Dot
                || self.at_kw("EXCLUDE")
                || self.at_kw("REPLACE")
                || self.at_kw("RENAME")
                || self.at_kw("ILIKE")
            {
                return self.unsupported(self.cur.s.max(star.e), "modifiers on *");
            }
            return Ok(SelectItem::Star);
        }
        let expr = self.expr()?;
        let alias = if self.at_kw("AS") {
            self.bump()?;
            let t = self.cur;
            match t.k {
                K::QIdent => {
                    self.bump()?;
                    Some(Ident {
                        name: self.unquote(t),
                        quoted: true,
                    })
                }
                K::Word if !self.is_reserved(t) => {
                    self.bump()?;
                    Some(Ident {
                        name: self.text(t).to_string(),
                        quoted: false,
                    })
                }
                _ => return self.expected("an alias name after AS"),
            }
        } else {
            None
        };
        Ok(SelectItem::Expr { expr, alias })
    }

    // ------------------------------------------------------------ expressions

    pub fn expr(&mut self) -> PResult<Expr> {
        self.enter(1)?;
        let e = self.binary(1)?;
        self.depth -= 1;
        Ok(e)
    }

    fn mk(&self, kind: ExprKind, start: usize) -> Expr {
        Expr::new(
            kind,
            Span {
                start,
                end: self.last_end,
            },
        )
    }

    // Levels: 1 OR, 2 AND, (3 NOT prefix), 4 IS NULL, 5 comparison, 6 BETWEEN / IN, 7 + -, 8 * / %.
    // The recursion path of a deeply nested query goes binary -> unary -> primary -> expr -> binary: everything that is not on that path (errors, IS NULL,
    // BETWEEN, IN) lives in functions of its own, so that a nesting level costs as little stack as possible.
    fn binary(&mut self, min: u8) -> PResult<Expr> {
        let start = self.cur.s;
        let mut left = if min <= 3 && self.at_kw("NOT") {
            self.bump()?;
            self.enter(1)?;
            let operand = self.binary(3)?; // `NOT NOT a` nests; AND and OR (below NOT) end the operand
            self.depth -= 1;
            if self.mutant == Mutant::LoseNot {
                operand
            } else {
                self.mk(
                    ExprKind::Unary {
                        op: UnOp::Not,
                        expr: Box::new(operand),
                    },
                    start,
                )
            }
        } else {
            self.unary()?
        };
        let base = self.depth;
        let mut last_level = 0u8; // the level of the operator just applied in this loop, for the two non-associative levels
        loop {
            let t = self.cur;
            let (prec, op) = match self.infix(t) {
                Infix::End => break,
                Infix::Reject(what) => return self.unsupported(t.s, what),
                Infix::RejectOp => {
                    return self.unsupported(t.s, &format!("the operator {}", self.describe(t)));
                }
                Infix::Op(prec, op) => (prec, op),
            };
            if prec < min {
                break;
            }
            if prec == last_level && (prec == 5 || prec == 6) {
                return self.chain_error(t, prec);
            }
            last_level = prec;
            if let Some(op) = op {
                self.bump()?;
                self.enter(1)?; // the chain a OP b OP c ... is a tree as deep as it is long
                let right = self.binary(prec + 1)?;
                left = self.mk(
                    ExprKind::Binary {
                        op,
                        left: Box::new(left),
                        right: Box::new(right),
                    },
                    start,
                );
            } else if prec == 4 {
                left = self.is_null(left, start)?;
            } else {
                match self.between_in(left, start)? {
                    Ok(e) => left = e,
                    Err(e) => {
                        // `x NOT <something else>`: not ours; hand the expression back
                        left = e;
                        break;
                    }
                }
            }
        }
        // each operator of a chain added one level to `self.depth` while the loop ran; give them back
        self.depth = base;
        Ok(left)
    }

    /// What the current token is, as an infix operator.
    fn infix(&self, t: Token) -> Infix {
        let (or_prec, and_prec) = if self.mutant == Mutant::OrBeforeAnd {
            (2, 1)
        } else {
            (1, 2)
        };
        match t.k {
            K::Word => {
                let w = self.text(t);
                if w.eq_ignore_ascii_case("OR") {
                    Infix::Op(or_prec, Some(BinOp::Or))
                } else if w.eq_ignore_ascii_case("AND") {
                    Infix::Op(and_prec, Some(BinOp::And))
                } else if w.eq_ignore_ascii_case("IS") {
                    Infix::Op(4, None)
                } else if ["BETWEEN", "IN", "NOT"]
                    .iter()
                    .any(|k| w.eq_ignore_ascii_case(k))
                {
                    Infix::Op(6, None)
                } else if ["LIKE", "ILIKE", "SIMILAR", "GLOB"]
                    .iter()
                    .any(|k| w.eq_ignore_ascii_case(k))
                {
                    Infix::Reject("LIKE, ILIKE and SIMILAR TO")
                } else {
                    Infix::End
                }
            }
            K::Eq => Infix::Op(5, Some(BinOp::Eq)),
            K::NotEq => Infix::Op(5, Some(BinOp::NotEq)),
            K::Lt => Infix::Op(5, Some(BinOp::Lt)),
            K::LtEq => Infix::Op(5, Some(BinOp::LtEq)),
            K::Gt => Infix::Op(5, Some(BinOp::Gt)),
            K::GtEq => Infix::Op(5, Some(BinOp::GtEq)),
            K::Plus => Infix::Op(7, Some(BinOp::Add)),
            K::Minus => Infix::Op(7, Some(BinOp::Sub)),
            K::Star => Infix::Op(8, Some(BinOp::Mul)),
            K::Slash => Infix::Op(8, Some(BinOp::Div)),
            K::Percent => Infix::Op(8, Some(BinOp::Mod)),
            K::ColonColon => Infix::Reject("the :: cast (write CAST(x AS type))"),
            K::Concat => Infix::Reject("the || operator"),
            K::Other if matches!(self.b[t.s], b'^' | b'&' | b'|' | b'~' | b'#' | b'@' | b'!') => {
                Infix::RejectOp
            }
            _ => Infix::End,
        }
    }

    #[cold]
    #[inline(never)]
    fn chain_error<T>(&self, t: Token, prec: u8) -> PResult<T> {
        if prec == 5 {
            self.syntax(
                t.s,
                "chained comparisons are not supported, use AND to combine comparisons".to_string(),
            )
        } else {
            self.syntax(
                t.s,
                format!(
                    "unexpected {}: BETWEEN and IN cannot be chained, use parentheses",
                    self.describe(t)
                ),
            )
        }
    }

    /// `left IS [NOT] NULL`, the current token being IS.
    #[inline(never)]
    fn is_null(&mut self, left: Expr, start: usize) -> PResult<Expr> {
        self.bump()?;
        let negated = self.eat_kw("NOT")?;
        if !self.at_kw("NULL") {
            return self.is_error();
        }
        self.bump()?;
        self.enter(1)?;
        Ok(self.mk(
            ExprKind::IsNull {
                expr: Box::new(left),
                negated,
            },
            start,
        ))
    }

    #[cold]
    #[inline(never)]
    fn is_error<T>(&self) -> PResult<T> {
        let c = self.cur;
        if self.is_kw(c, "DISTINCT") {
            self.unsupported(c.s, "IS [NOT] DISTINCT FROM")
        } else if self.is_kw(c, "TRUE") || self.is_kw(c, "FALSE") || self.is_kw(c, "UNKNOWN") {
            self.unsupported(c.s, "IS [NOT] TRUE, FALSE and UNKNOWN")
        } else {
            self.expected("NULL after IS")
        }
    }

    /// `left [NOT] BETWEEN a AND b` or `left [NOT] IN (...)`, the current token being NOT, BETWEEN or IN. `Err(left)` (inside the `Ok`) when it was a NOT that
    /// starts nothing of ours, so that the caller can stop.
    #[inline(never)]
    fn between_in(&mut self, left: Expr, start: usize) -> PResult<Result<Expr, Expr>> {
        let mut negated = false;
        if self.at_kw("NOT") {
            let n = self.peek2()?;
            if self.is_kw(n, "BETWEEN") || self.is_kw(n, "IN") {
                self.bump()?;
                negated = true;
            } else if ["LIKE", "ILIKE", "SIMILAR", "GLOB"]
                .iter()
                .any(|k| self.is_kw(n, k))
            {
                return self.unsupported(n.s, "LIKE, ILIKE and SIMILAR TO");
            } else {
                return Ok(Err(left));
            }
        }
        if self.at_kw("BETWEEN") {
            self.bump()?;
            self.enter(1)?;
            let low = self.binary(7)?;
            self.expect_kw("AND")?;
            let high = self.binary(7)?;
            return Ok(Ok(self.mk(
                ExprKind::Between {
                    expr: Box::new(left),
                    low: Box::new(low),
                    high: Box::new(high),
                    negated,
                },
                start,
            )));
        }
        self.bump()?; // IN
        self.expect(K::LParen, "'(' after IN")?;
        if self.at_kw("SELECT") || self.at_kw("WITH") || self.at_kw("VALUES") {
            return self.unsupported(self.cur.s, "subqueries");
        }
        self.enter(1)?;
        let mut list = vec![self.expr()?];
        while self.cur.k == K::Comma {
            self.bump()?;
            list.push(self.expr()?);
        }
        self.expect(K::RParen, "',' or ')' in the IN list")?;
        Ok(Ok(self.mk(
            ExprKind::InList {
                expr: Box::new(left),
                list,
                negated,
            },
            start,
        )))
    }

    fn unary(&mut self) -> PResult<Expr> {
        let t = self.cur;
        match t.k {
            K::Minus => {
                self.bump()?;
                self.enter(1)?;
                let operand = self.unary()?;
                self.depth -= 1;
                Ok(self.mk(
                    ExprKind::Unary {
                        op: UnOp::Neg,
                        expr: Box::new(operand),
                    },
                    t.s,
                ))
            }
            K::Plus => self.unsupported(t.s, "unary + (just leave it out)"),
            _ => self.primary(),
        }
    }

    fn primary(&mut self) -> PResult<Expr> {
        let t = self.cur;
        match t.k {
            K::Number => {
                self.bump()?;
                let text = self.text(t);
                let text = if self.mutant == Mutant::NumberViaF64 {
                    text.parse::<f64>()
                        .map_or_else(|_| text.to_string(), |f| f.to_string())
                } else {
                    text.to_string()
                };
                Ok(self.mk(ExprKind::Number(text), t.s))
            }
            K::Str => {
                self.bump()?;
                Ok(self.mk(ExprKind::Str(self.unquote(t)), t.s))
            }
            K::QIdent => {
                self.bump()?;
                if self.cur.k == K::Dot {
                    return self.unsupported(self.cur.s, "qualified names (table.column)");
                }
                if self.cur.k == K::LParen {
                    return self.unsupported(self.cur.s, "function calls");
                }
                Ok(self.mk(
                    ExprKind::Ident(Ident {
                        name: self.unquote(t),
                        quoted: true,
                    }),
                    t.s,
                ))
            }
            K::LParen => {
                self.bump()?;
                if self.at_kw("SELECT") || self.at_kw("WITH") || self.at_kw("VALUES") {
                    return self.unsupported(self.cur.s, "subqueries");
                }
                let inner = self.expr()?;
                if self.cur.k == K::Comma {
                    return self.unsupported(self.cur.s, "row values and tuples");
                }
                self.expect(K::RParen, "')'")?;
                // parentheses leave no node; widen the span to include them
                Ok(Expr::new(
                    inner.kind,
                    Span {
                        start: t.s,
                        end: self.last_end,
                    },
                ))
            }
            K::Word => self.word(t),
            K::Other => match self.b[t.s] {
                b'$' | b'?' => {
                    self.unsupported(t.s, "parameters ($1, ?) and dollar-quoted strings")
                }
                b'[' | b'{' => self.unsupported(t.s, "list and struct literals"),
                _ => self.syntax(
                    t.s,
                    format!("expected an expression, found {}", self.describe(t)),
                ),
            },
            K::Star => self.syntax(
                t.s,
                "expected an expression, found '*' (a bare * is only allowed as a select item)"
                    .to_string(),
            ),
            _ => self.expected("an expression"),
        }
    }

    #[inline(never)]
    fn word(&mut self, t: Token) -> PResult<Expr> {
        let w = self.text(t).to_ascii_uppercase();
        // E'..', X'..', B'..', N'..' and U&'..' prefixed strings
        if matches!(w.as_str(), "E" | "X" | "B" | "N" | "U") {
            let n = self.peek2()?;
            if n.k == K::Str && n.s == t.e {
                return self.unsupported(t.s, "prefixed string literals (E'', X'', B'', N'')");
            }
        }
        match w.as_str() {
            "NULL" => {
                self.bump()?;
                Ok(self.mk(ExprKind::Null, t.s))
            }
            "TRUE" | "FALSE" => self.unsupported(t.s, "boolean literals"),
            "CASE" => self.case(t),
            "CAST" => {
                self.bump()?;
                self.expect(K::LParen, "'(' after CAST")?;
                let inner = self.expr()?;
                self.expect_kw("AS")?;
                let ty = self.data_type()?;
                self.expect(K::RParen, "')' after the type")?;
                Ok(self.mk(
                    ExprKind::Cast {
                        expr: Box::new(inner),
                        ty,
                    },
                    t.s,
                ))
            }
            "TRY_CAST" => self.unsupported(t.s, "TRY_CAST"),
            "DATE" | "TIME" | "TIMESTAMP" | "TIMESTAMPTZ" | "INTERVAL" => {
                let n = self.peek2()?;
                if n.k == K::Str {
                    if w != "DATE" {
                        return self
                            .unsupported(t.s, &format!("{w} literals (only DATE 'yyyy-mm-dd')"));
                    }
                    self.bump()?;
                    let s = self.bump()?;
                    return Ok(self.mk(ExprKind::Date(self.unquote(s)), t.s));
                }
                self.ident(t)
            }
            "EXISTS" | "ARRAY" | "ROW" | "STRUCT" | "MAP" | "LAMBDA" | "ANY" | "SOME" | "ALL" => {
                let n = self.peek2()?;
                if n.k == K::LParen || n.k == K::Other {
                    return self.unsupported(t.s, &w);
                }
                self.ident(t)
            }
            _ if self.is_reserved(t) => self.expected("an expression"),
            _ => self.ident(t),
        }
    }

    #[inline(never)]
    fn ident(&mut self, t: Token) -> PResult<Expr> {
        self.bump()?;
        match self.cur.k {
            K::Dot => self.unsupported(self.cur.s, "qualified names (table.column)"),
            K::LParen => self.unsupported(t.s, &format!("function calls ({}(...))", self.text(t))),
            _ => Ok(self.mk(
                ExprKind::Ident(Ident {
                    name: self.text(t).to_string(),
                    quoted: false,
                }),
                t.s,
            )),
        }
    }

    #[inline(never)]
    fn case(&mut self, t: Token) -> PResult<Expr> {
        self.bump()?;
        let operand = if self.at_kw("WHEN") {
            None
        } else {
            Some(Box::new(self.expr()?))
        };
        let mut whens = Vec::new();
        while self.at_kw("WHEN") {
            self.bump()?;
            let c = self.expr()?;
            self.expect_kw("THEN")?;
            let v = self.expr()?;
            whens.push((c, v));
        }
        if whens.is_empty() {
            return self.expected("WHEN");
        }
        let else_ = if self.eat_kw("ELSE")? {
            Some(Box::new(self.expr()?))
        } else {
            None
        };
        self.expect_kw("END")?;
        Ok(self.mk(
            ExprKind::Case {
                operand,
                whens,
                else_,
            },
            t.s,
        ))
    }

    #[inline(never)]
    fn data_type(&mut self) -> PResult<DataType> {
        let t = self.cur;
        if t.k != K::Word {
            return if t.k == K::QIdent {
                self.unsupported(t.s, "quoted type names")
            } else {
                self.expected("a type name")
            };
        }
        let w = self.text(t).to_ascii_uppercase();
        self.bump()?;
        let ty = match w.as_str() {
            "BOOLEAN" | "BOOL" => DataType::Boolean,
            "TINYINT" => DataType::TinyInt,
            "SMALLINT" => DataType::SmallInt,
            "INTEGER" | "INT" => DataType::Integer,
            "BIGINT" => DataType::BigInt,
            "DOUBLE" => {
                if self.at_kw("PRECISION") {
                    self.bump()?;
                }
                DataType::Double
            }
            "VARCHAR" => DataType::Varchar,
            "DATE" => DataType::Date,
            "DECIMAL" | "NUMERIC" | "DEC" => {
                let (mut precision, mut scale) = (18u8, 3u8); // DuckDB's default for a bare DECIMAL
                if self.cur.k == K::LParen {
                    self.bump()?;
                    precision = self.small_number("precision")?;
                    scale = 0;
                    if self.cur.k == K::Comma {
                        self.bump()?;
                        scale = self.small_number("scale")?;
                    }
                    self.expect(K::RParen, "')' after the precision and scale")?;
                }
                DataType::Decimal { precision, scale }
            }
            _ => return self.unsupported(t.s, &format!("the type {}", self.text(t))),
        };
        if self.cur.k == K::LParen {
            return self.unsupported(self.cur.s, "type parameters (only DECIMAL(p, s) has them)");
        }
        Ok(ty)
    }

    fn small_number(&mut self, what: &str) -> PResult<u8> {
        let t = self.cur;
        match (t.k, self.text(t).parse::<u8>()) {
            (K::Number, Ok(n)) => {
                self.bump()?;
                Ok(n)
            }
            _ => self.expected(&format!("the {what} (a whole number)")),
        }
    }
}

/// Whether `w` is a word that cannot be an unquoted name (shared with arm A's conversion layer so both arms enforce the same rule).
pub fn is_reserved_word(w: &str) -> bool {
    RESERVED.iter().any(|r| w.eq_ignore_ascii_case(r))
}
