//! Frozen read-only COPY STDOUT forms; data uses the ordinary bounded bridge.
use crate::server::WireError;
use sqlparser::{
    dialect::PostgreSqlDialect,
    tokenizer::{Token, Tokenizer},
};
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Format {
    Binary,
    Text,
    Csv,
}
pub(crate) struct Copy<'a> {
    pub sql: Cow<'a, str>,
    pub format: Format,
    pub header: bool,
    pub offset: Option<usize>,
}
pub(crate) fn query(sql: &str) -> Result<Option<Copy<'_>>, WireError> {
    if !sql
        .trim_start()
        .get(..4)
        .is_some_and(|s| s.eq_ignore_ascii_case("copy"))
    {
        return Ok(None);
    }
    let unsupported = || {
        WireError::unsupported(
            "COPY; supported forms are read-only query/table TO STDOUT with FORMAT binary/text/csv and CSV HEADER",
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
    if !word(0, "copy") {
        return Err(unsupported());
    }
    let byte_offset = |line: u64, column: u64| -> usize {
        let prefix: usize = sql
            .split_inclusive('\n')
            .take(line.saturating_sub(1) as usize)
            .map(str::len)
            .sum();
        prefix
            + sql[prefix..]
                .char_indices()
                .nth(column.saturating_sub(1) as usize)
                .map_or(sql.len() - prefix, |(i, _)| i)
    };
    let (inner, end, offset) = if ts.get(1).is_some_and(|t| t.token == Token::LParen) {
        if !word(2, "select") {
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
        let first = ts.get(2).ok_or_else(unsupported)?.span.start;
        let last = ts.get(end).ok_or_else(unsupported)?.span.start;
        let start = byte_offset(first.line, first.column);
        (
            Cow::Borrowed(&sql[start..byte_offset(last.line, last.column)]),
            end + 1,
            Some(start),
        )
    } else {
        // Only registered identifier sources, never a file, PROGRAM or an arbitrary query.
        let mut end = 1;
        let mut name = Vec::new();
        loop {
            let Some(t) = ts.get(end) else {
                return Err(unsupported());
            };
            let Token::Word(w) = &t.token else {
                return Err(unsupported());
            };
            if !matches!(w.quote_style, None | Some('"')) {
                return Err(unsupported());
            }
            name.push(t.token.to_string());
            end += 1;
            if ts.get(end).is_none_or(|t| t.token != Token::Period) {
                break;
            }
            end += 1;
        }
        if name.len() > 3 {
            return Err(unsupported());
        }
        (
            Cow::Owned(format!("SELECT * FROM {}", name.join("."))),
            end,
            None,
        )
    };
    if !word(end, "to") || !word(end + 1, "stdout") {
        return Err(unsupported());
    }
    let start = end + 2 + usize::from(word(end + 2, "with"));
    let mut format = Format::Text;
    let mut header = false;
    if start < ts.len() {
        if ts[start].token != Token::LParen || ts.last().is_none_or(|t| t.token != Token::RParen) {
            return Err(unsupported());
        }
        let mut seen_format = false;
        let mut seen_header = false;
        for option in ts[start + 1..ts.len() - 1].split(|t| t.token == Token::Comma) {
            if option
                .first()
                .is_none_or(|t| !matches!(&t.token, Token::Word(w) if w.quote_style.is_none()))
            {
                return Err(unsupported());
            }
            let values: Vec<_> = option
                .iter()
                .map(|t| match &t.token {
                    Token::Word(w) if w.quote_style.is_none() => w.value.to_ascii_lowercase(),
                    Token::SingleQuotedString(s) => s.clone(),
                    _ => String::new(),
                })
                .collect();
            match values
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .as_slice()
            {
                ["format", f] if !seen_format => {
                    format = match *f {
                        "binary" => Format::Binary,
                        "text" => Format::Text,
                        "csv" => Format::Csv,
                        _ => return Err(unsupported()),
                    };
                    seen_format = true;
                }
                ["header"] | ["header", "true"] if !seen_header => {
                    header = true;
                    seen_header = true;
                }
                ["header", "false"] if !seen_header => {
                    seen_header = true;
                }
                _ => return Err(unsupported()),
            }
        }
        if seen_header && format != Format::Csv {
            return Err(unsupported());
        }
    }
    Ok(Some(Copy {
        sql: inner,
        format,
        header,
        offset,
    }))
}

pub(crate) fn record(values: &[Option<String>], format: Format) -> Result<Vec<u8>, WireError> {
    if values.len() > i16::MAX as usize {
        return Err(WireError::new("54000", "COPY column count exceeds limit"));
    }
    let mut out = Vec::new();
    for (i, value) in values.iter().enumerate() {
        if i > 0 {
            out.push(if format == Format::Csv { b',' } else { b'\t' });
        }
        let Some(s) = value else {
            if format == Format::Text {
                out.extend_from_slice(b"\\N");
            }
            continue;
        };
        if s.contains('\0') {
            return Err(WireError::new(
                "22000",
                "value cannot be represented in Postgres text",
            ));
        }
        let quote = format == Format::Csv
            && (s.is_empty()
                || s.contains([',', '"', '\n', '\r'])
                || (values.len() == 1 && s == "\\."));
        let bytes = if format == Format::Csv {
            s.len()
                .saturating_add(s.bytes().filter(|b| *b == b'"').count())
                .saturating_add(usize::from(quote) * 2)
        } else {
            s.bytes().fold(0usize, |n, b| {
                n.saturating_add(
                    if matches!(b, b'\\' | b'\t' | b'\n' | b'\r' | 8 | 12 | 11) {
                        2
                    } else {
                        1
                    },
                )
            })
        };
        if out.len().saturating_add(bytes).saturating_add(5) > crate::codec::MAX_FRAME {
            return Err(WireError::new("54000", "COPY row exceeds frame limit"));
        }
        if format == Format::Csv {
            if quote {
                out.push(b'"');
            }
            for b in s.bytes() {
                out.push(b);
                if b == b'"' {
                    out.push(b'"');
                }
            }
            if quote {
                out.push(b'"');
            }
        } else {
            for b in s.bytes() {
                match b {
                    b'\\' => out.extend_from_slice(b"\\\\"),
                    b'\t' => out.extend_from_slice(b"\\t"),
                    b'\n' => out.extend_from_slice(b"\\n"),
                    b'\r' => out.extend_from_slice(b"\\r"),
                    8 => out.extend_from_slice(b"\\b"),
                    12 => out.extend_from_slice(b"\\f"),
                    11 => out.extend_from_slice(b"\\v"),
                    _ => out.push(b),
                }
            }
        }
        if out.len() > crate::codec::MAX_FRAME {
            return Err(WireError::new("54000", "COPY row exceeds frame limit"));
        }
    }
    out.push(b'\n');
    Ok(out)
}
pub(crate) const HEADER: &[u8] = b"PGCOPY\n\xff\r\n\0\0\0\0\0\0\0\0\0";
pub(crate) const TRAILER: &[u8] = &[0xff, 0xff];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn seeded_copy_fuzz() {
        let cases = std::env::var("NATA_COPY_CASES")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(200);
        let pieces = [
            "COPY ",
            "( ",
            ") ",
            "SELECT ",
            "* ",
            "FROM ",
            "public.",
            "example ",
            "TO ",
            "STDOUT ",
            "STDIN ",
            "WITH ",
            "FORMAT ",
            "csv ",
            "binary ",
            "HEADER ",
            "true ",
            "; ",
            "'é' ",
            "\"Mixed Table\" ",
            "--x\n",
            ", ",
        ];
        let mut seed = 20261107u64;
        for _ in 0..cases {
            let mut sql = String::from("COPY ");
            for _ in 0..32 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                sql.push_str(pieces[(seed >> 32) as usize % pieces.len()]);
            }
            let _ = query(&sql);
        }
    }
    #[test]
    fn text_csv_escapes_match_postgres_and_reject_zero() {
        let values = [
            None,
            Some(String::new()),
            Some("\\N\t\n\r\u{8}\u{b}\u{c}\\.\"é".into()),
        ];
        assert_eq!(
            record(&values, Format::Text).unwrap_or_else(|_| panic!("text")),
            "\\N\t\t\\\\N\\t\\n\\r\\b\\v\\f\\\\.\"é\n".as_bytes()
        );
        assert_eq!(
            record(&values, Format::Csv).unwrap_or_else(|_| panic!("csv")),
            ",\"\",\"\\N\t\n\r\u{8}\u{b}\u{c}\\.\"\"é\"\n".as_bytes()
        );
        assert_eq!(
            record(&[Some("\\.".into())], Format::Csv).unwrap_or_else(|_| panic!("terminator")),
            b"\"\\.\"\n"
        );
        assert!(record(&[Some("\0".into())], Format::Text).is_err());
        assert!(record(&[Some("\0".into())], Format::Csv).is_err());
        let escaped = "\\".repeat(crate::codec::MAX_FRAME / 2 + 1);
        assert!(record(&[Some(escaped)], Format::Text).is_err());
        let quoted = "\"".repeat(crate::codec::MAX_FRAME / 2 + 1);
        assert!(record(&[Some(quoted)], Format::Csv).is_err());
    }
    #[test]
    fn copy_table_identifiers_are_preserved_and_destinations_are_bounded() {
        let c = query("COPY \"Quoted Schema\".\"Mixed Table\" TO STDOUT (FORMAT csv, HEADER)")
            .unwrap_or_else(|_| panic!("parse"))
            .expect("copy");
        assert_eq!(c.sql, "SELECT * FROM \"Quoted Schema\".\"Mixed Table\"");
        assert_eq!(c.format, Format::Csv);
        assert!(c.header);
        for sql in [
            "COPY 'file.parquet' TO STDOUT",
            "COPY t TO PROGRAM 'x'",
            "COPY t TO 'file'",
            "COPY t FROM STDIN",
            "COPY t TO STDOUT (FORMAT binary, HEADER false)",
            "COPY t TO STDOUT; SELECT 1",
            "COPY t TO STDOUT ('format' csv)",
            "COPY t TO STDOUT (FORMAT csv, 'header')",
        ] {
            assert!(query(sql).is_err(), "{sql}");
        }
    }
}
