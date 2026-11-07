//! The single read-only COPY query wrapper used by ADBC, not general COPY SQL.
use crate::server::WireError;
use sqlparser::{
    dialect::PostgreSqlDialect,
    tokenizer::{Token, Tokenizer},
};

pub(crate) fn query(sql: &str) -> Result<Option<&str>, WireError> {
    // Avoid tokenizing ordinary queries a second time.
    if !sql
        .trim_start()
        .get(..4)
        .is_some_and(|s| s.eq_ignore_ascii_case("copy"))
    {
        return Ok(None);
    }
    let unsupported = || {
        WireError::unsupported(
            "COPY; only COPY (SELECT ...) TO STDOUT [WITH] (FORMAT binary) is supported",
        )
    };
    let tokens = Tokenizer::new(&PostgreSqlDialect {}, sql)
        .tokenize_with_location()
        .map_err(|_| unsupported())?;
    let mut ts: Vec<_> = tokens
        .iter()
        .filter(|t| !matches!(t.token, Token::Whitespace(_)))
        .collect();
    if ts.last().is_some_and(|t| t.token == Token::SemiColon) {
        ts.pop();
    }
    let word = |i: usize, s: &str| {
        ts.get(i).is_some_and(|t| matches!(&t.token, Token::Word(w) if w.quote_style.is_none() && w.value.eq_ignore_ascii_case(s)))
    };
    if !word(0, "copy") || ts.get(1).is_none_or(|t| t.token != Token::LParen) || !word(2, "select")
    {
        return Err(unsupported());
    }
    let mut depth = 1;
    let mut end = 2;
    while end < ts.len() {
        match ts[end].token {
            Token::LParen => depth += 1,
            Token::RParen => depth -= 1,
            Token::SemiColon => return Err(unsupported()),
            _ => {}
        }
        if depth == 0 {
            break;
        }
        end += 1;
    }
    if !word(end + 1, "to") || !word(end + 2, "stdout") {
        return Err(unsupported());
    }
    let start = end + 3 + usize::from(word(end + 3, "with"));
    let options: Vec<_> = ts[start..].iter().map(|t| t.token.clone()).collect();
    // Driver 1.13.0 sends (FORMAT binary). PostgreSQL also accepts the quoted
    // option value; deliberately accept no other COPY options or destinations.
    let binary = options.get(2).is_some_and(|t| match t {
        Token::Word(w) => w.quote_style.is_none() && w.value.eq_ignore_ascii_case("binary"),
        Token::SingleQuotedString(s) => s == "binary",
        _ => false,
    });
    if options.len() != 4
        || options[0] != Token::LParen
        || !word(start + 1, "format")
        || !binary
        || options[3] != Token::RParen
    {
        return Err(unsupported());
    }
    let offset = |loc: sqlparser::tokenizer::Location| {
        let mut line = 1;
        let mut col = 1;
        for (i, c) in sql.char_indices() {
            if line == loc.line && col == loc.column {
                return i;
            }
            if c == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        sql.len()
    };
    Ok(Some(
        &sql[offset(ts[1].span.end)..offset(ts[end].span.start)],
    ))
}
pub(crate) const HEADER: &[u8] = b"PGCOPY\n\xff\r\n\0\0\0\0\0\0\0\0\0";
pub(crate) const TRAILER: &[u8] = &(-1i16).to_be_bytes();
