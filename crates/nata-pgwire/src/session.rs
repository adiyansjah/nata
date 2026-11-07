//! Explicit compatibility surface. These commands never enter the lakehouse dialect.
use crate::server::WireError;
use nata_sql::types::SqlType;
use sqlparser::{
    dialect::PostgreSqlDialect,
    tokenizer::{Token, Tokenizer},
};
use std::collections::BTreeMap;
#[derive(Clone)]
pub(crate) struct State {
    pub transaction: u8,
    pub database: String,
    pub catalog: std::sync::Arc<crate::catalog::Snapshot>,
    settings: BTreeMap<String, String>,
}
pub(crate) struct Response {
    pub fields: Vec<(String, SqlType)>,
    pub rows: Vec<Vec<Option<String>>>,
    pub command: String,
    pub status: Vec<(String, String)>,
    pub deallocate: Option<Option<String>>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            transaction: b'I',
            database: "nata".into(),
            catalog: std::sync::Arc::default(),
            settings: [
                ("server_version", "16.0"),
                ("server_version_num", "160000"),
                ("server_encoding", "UTF8"),
                ("client_encoding", "UTF8"),
                ("datestyle", "ISO, MDY"),
                ("timezone", "UTC"),
                ("integer_datetimes", "on"),
                ("standard_conforming_strings", "on"),
                ("application_name", ""),
                ("extra_float_digits", "3"),
                ("search_path", "public"),
                ("statement_timeout", "0"),
                ("default_transaction_read_only", "off"),
                ("default_transaction_isolation", "read committed"),
                ("default_transaction_deferrable", "off"),
            ]
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect(),
        }
    }
}
impl State {
    pub fn timeout(&self) -> Option<std::time::Duration> {
        let ms = self.settings["statement_timeout"]
            .parse::<u64>()
            .expect("validated timeout");
        (ms != 0).then(|| std::time::Duration::from_millis(ms))
    }
    pub fn search_path(&self) -> Vec<String> {
        search_path_names(&self.settings["search_path"]).expect("validated search_path")
    }
    pub fn startup(&mut self, params: &[(String, String)]) -> Result<(), WireError> {
        for (k, v) in params {
            if matches!(k.as_str(), "user" | "database" | "replication") {
                if k == "database" {
                    self.database.clone_from(v);
                }
                if k == "replication" {
                    return Err(WireError::unsupported("replication"));
                }
                continue;
            }
            if k == "options" {
                return Err(WireError::unsupported("startup options; use explicit SET"));
            }
            self.set(&k.to_ascii_lowercase(), v)?;
        }
        Ok(())
    }
    pub fn set(&mut self, key: &str, value: &str) -> Result<Option<(String, String)>, WireError> {
        let l = value.to_ascii_lowercase();
        let allowed = match key {
            "application_name" | "timezone" => true,
            "search_path" => search_path_names(value).is_some(),
            "extra_float_digits" => matches!(l.as_str(), "1" | "2" | "3"),
            "datestyle" => matches!(
                l.as_str(),
                "iso" | "iso, mdy" | "iso , mdy" | "iso, dmy" | "iso , dmy"
            ),
            "client_encoding" => matches!(l.as_str(), "utf8" | "utf-8"),
            "statement_timeout" => timeout_ms(value).is_some(),
            "standard_conforming_strings" => l == "on",
            "default_transaction_read_only" | "default_transaction_deferrable" => l == "off",
            "default_transaction_isolation" => l == "read committed",
            _ => false,
        };
        if !allowed {
            return Err(WireError::unsupported("session parameter or value"));
        }
        let value: String = match key {
            "client_encoding" => "UTF8".into(),
            "datestyle" => "ISO, MDY".into(),
            "statement_timeout" => timeout_ms(value).expect("validated").to_string(),
            _ => value.into(),
        };
        self.settings.insert(key.into(), value.clone());
        if let Some(name) = status_name(key) {
            return Ok(Some((name.into(), value)));
        }
        Ok(None)
    }
    pub fn failed(&mut self) {
        if self.transaction == b'T' {
            self.transaction = b'E';
        }
    }
    pub fn evaluate(&mut self, sql: &str) -> Result<Option<Response>, WireError> {
        if self.transaction == b'E'
            && !matches!(
                sql.trim()
                    .trim_end_matches(';')
                    .trim()
                    .to_ascii_lowercase()
                    .as_str(),
                "rollback" | "commit" | "rollback work" | "commit work"
            )
        {
            return Err(WireError::new(
                "25P02",
                "current transaction is aborted; use ROLLBACK",
            ));
        }
        refuse_helpers(sql)?;
        if let Some(response) = bootstrap(sql, &self.catalog, &self.database)? {
            return Ok(Some(response));
        }
        let first = sql
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_end_matches(';')
            .to_ascii_lowercase();
        let session = matches!(
            first.as_str(),
            "deallocate" | "set" | "show" | "begin" | "commit" | "rollback"
        ) || (first == "select"
            && [
                "version(",
                "version (",
                "current_setting(",
                "current_setting (",
                "current_schema(",
                "current_schema (",
            ]
            .iter()
            .any(|p| sql.to_ascii_lowercase().contains(p))
            && !sql.to_ascii_lowercase().contains(" from "));
        let session = session
            || sql
                .trim()
                .trim_end_matches(';')
                .trim()
                .eq_ignore_ascii_case("SELECT 1");
        if !session {
            return Ok(None);
        }
        if sql.len() > 16_384 {
            return Err(WireError::unsupported("session command size"));
        }
        let tokens = Tokenizer::new(&PostgreSqlDialect {}, sql)
            .tokenize()
            .map_err(|_| WireError::new("42601", "invalid session command"))?;
        let tokens: Vec<_> = tokens
            .into_iter()
            .filter(|t| !matches!(t, Token::Whitespace(_)))
            .collect();
        let mut groups = tokens
            .split(|t| matches!(t, Token::SemiColon))
            .filter(|g| !g.is_empty());
        let group = groups.next().unwrap_or(&[]);
        // Driver startup can group SETs. Produce one response per command via the simple-query adapter instead.
        if groups.next().is_some() {
            return Err(WireError::unsupported(
                "multiple session commands; send separately",
            ));
        }
        let mut normalized = group.to_vec();
        // pgcli uses the SQL-standard TIME ZONE spelling.
        if normalized.len() >= 3
            && normalized[1].to_string().eq_ignore_ascii_case("time")
            && normalized[2].to_string().eq_ignore_ascii_case("zone")
        {
            normalized.splice(
                1..3,
                [Token::Word(sqlparser::tokenizer::Word {
                    value: "timezone".into(),
                    quote_style: None,
                    keyword: sqlparser::keywords::Keyword::NoKeyword,
                })],
            );
            if normalized[0].to_string().eq_ignore_ascii_case("set") {
                normalized.insert(2, Token::Eq);
            }
        }
        let group = normalized.as_slice();
        let words: Vec<String> = group
            .iter()
            .map(|t| match t {
                Token::SingleQuotedString(s) => s.clone(),
                Token::Word(w) => w.value.clone(),
                _ => t.to_string(),
            })
            .collect();
        let lower: Vec<_> = words.iter().map(|w| w.to_ascii_lowercase()).collect();
        let w: Vec<_> = lower.iter().map(String::as_str).collect();
        let mut out = Response {
            fields: vec![],
            rows: vec![],
            command: String::new(),
            status: vec![],
            deallocate: None,
        };
        match w.as_slice() {
            ["deallocate", _] | ["deallocate", "prepare", _] => {
                let Token::Word(word) = group.last().expect("target") else {
                    return Err(WireError::new("42601", "invalid prepared statement name"));
                };
                out.deallocate = Some(
                    if word.quote_style.is_none() && word.value.eq_ignore_ascii_case("all") {
                        None
                    } else {
                        Some(if word.quote_style.is_some() {
                            word.value.clone()
                        } else {
                            word.value.to_ascii_lowercase()
                        })
                    },
                );
                out.command = "DEALLOCATE".into();
            }
            ["begin"] | ["begin", "work"] | ["begin", "transaction"] => {
                self.transaction = b'T';
                out.command = "BEGIN".into();
            }
            ["commit"] | ["commit", "work"] | ["rollback"] | ["rollback", "work"] => {
                out.command = if w[0] == "rollback" || self.transaction == b'E' {
                    "ROLLBACK"
                } else {
                    "COMMIT"
                }
                .into();
                self.transaction = b'I';
            }
            ["set", key, "=" | "to", ..] if words.len() >= 4 => {
                let value = if *key == "search_path" {
                    match &group[3..] {
                        [Token::SingleQuotedString(s)] => s.clone(),
                        tokens => tokens
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(" "),
                    }
                } else {
                    words[3..].join(" ")
                };
                if let Some(status) = self.set(key, &value)? {
                    out.status.push(status);
                }
                out.command = "SET".into();
            }
            ["show", key] => {
                let value = self
                    .settings
                    .get(*key)
                    .ok_or_else(|| WireError::unsupported("SHOW parameter"))?
                    .clone();
                out.fields.push((words[1].clone(), SqlType::Varchar));
                out.rows.push(vec![Some(value)]);
                out.command = "SELECT 1".into();
            }
            ["select", "1"] => {
                out.fields.push(("?column?".into(), SqlType::Integer));
                out.rows.push(vec![Some("1".into())]);
                out.command = "SELECT 1".into();
            }
            ["select", "version", "(", ")"] => {
                out.fields.push(("version".into(), SqlType::Varchar));
                out.rows
                    .push(vec![Some("PostgreSQL 16.0 (Nata compatibility)".into())]);
                out.command = "SELECT 1".into();
            }
            ["select", "current_schema", "(", ")"] => {
                out.fields.push(("current_schema".into(), SqlType::Varchar));
                out.rows.push(vec![Some("public".into())]);
                out.command = "SELECT 1".into();
            }
            ["select", "current_setting", "(", key, ")"] => {
                if !matches!(group.get(3), Some(Token::SingleQuotedString(_))) {
                    return Err(WireError::unsupported(
                        "current_setting requires a string literal",
                    ));
                }
                let value = self
                    .settings
                    .get(*key)
                    .ok_or_else(|| WireError::unsupported("current_setting parameter"))?
                    .clone();
                out.fields
                    .push(("current_setting".into(), SqlType::Varchar));
                out.rows.push(vec![Some(value)]);
                out.command = "SELECT 1".into();
            }
            _ => return Err(WireError::unsupported("session statement")),
        }
        Ok(Some(out))
    }
}
// Captured helpers outside the bounded skeleton surface are explicit refusals.
fn refuse_helpers(sql: &str) -> Result<(), WireError> {
    const REFUSED: &[&str] = &[
        "_pg_expandarray",
        "array_agg",
        "array_to_string",
        "array_upper",
        "btrim",
        "current_catalog",
        "generate_series",
        "generate_subscripts",
        "pg_get_function_arguments",
        "pg_get_function_result",
        "pg_get_statisticsobjdef_columns",
        "pg_my_temp_schema",
        "pg_relation_is_publishable",
        "replace",
        "row_number",
        "string_agg",
        "substring",
        "unnest",
    ];
    if !REFUSED.iter().any(|n| sql.to_ascii_lowercase().contains(n)) {
        return Ok(());
    }
    let tokens = Tokenizer::new(&PostgreSqlDialect {}, sql)
        .tokenize()
        .map_err(|_| WireError::unsupported("metadata helper tokens"))?;
    let tokens: Vec<_> = tokens
        .iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect();
    if tokens.windows(2).any(|ts| {
        matches!(ts[0], Token::Word(w) if REFUSED.contains(&w.value.to_ascii_lowercase().as_str()))
            && *ts[1] == Token::LParen
    }) || tokens
        .iter()
        .any(|t| matches!(t, Token::Word(w) if w.quote_style.is_none() && w.value.eq_ignore_ascii_case("current_catalog")))
    {
        return Err(WireError::unsupported(
            "captured catalog helper outside the skeleton metadata contract",
        ));
    }
    Ok(())
}

fn search_path_names(value: &str) -> Option<Vec<String>> {
    let tokens = Tokenizer::new(&PostgreSqlDialect {}, value)
        .tokenize()
        .ok()?;
    let tokens: Vec<_> = tokens
        .into_iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect();
    if tokens.is_empty() {
        return Some(vec![]);
    }
    if tokens.len().is_multiple_of(2) {
        return None;
    }
    let mut names = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        if i % 2 == 1 {
            if *t != Token::Comma {
                return None;
            }
            continue;
        }
        let Token::Word(w) = t else {
            return None;
        };
        if w.value == "$user" || !matches!(w.quote_style, None | Some('"')) {
            return None;
        }
        names.push(if w.quote_style.is_some() {
            w.value.clone()
        } else {
            w.value.to_ascii_lowercase()
        });
    }
    Some(names)
}
fn timeout_ms(value: &str) -> Option<u64> {
    let value = value.trim();
    let end = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    let n = value[..end].parse::<u64>().ok()?;
    let multiplier = match value[end..].trim() {
        "" | "ms" => 1,
        "s" => 1000,
        "min" => 60_000,
        _ => return None,
    };
    n.checked_mul(multiplier).filter(|n| *n <= i32::MAX as u64)
}
fn status_name(key: &str) -> Option<&str> {
    match key {
        "datestyle" => Some("DateStyle"),
        "timezone" => Some("TimeZone"),
        "client_encoding" | "application_name" | "standard_conforming_strings" => Some(key),
        _ => None,
    }
}

// Exact ADBC PostgreSQL 1.13.0 RebuildTypeResolver queries, not a SQL catalog.
// Token equality permits whitespace/case/one terminal semicolon only.
const ATTRIBUTES: &str =
    "SELECT attrelid, attname, atttypid FROM pg_catalog.pg_attribute ORDER BY attrelid, attnum";
const TYPES: &str = "SELECT oid, typname, typreceive, typbasetype, typrelid, typarray FROM pg_catalog.pg_type WHERE (typreceive != 0 OR typsend != 0) AND typtype != 'r' AND typreceive::TEXT != 'array_recv'";
fn bootstrap(
    sql: &str,
    snapshot: &crate::catalog::Snapshot,
    database: &str,
) -> Result<Option<Response>, WireError> {
    // Only tokenize potential catalog chatter, never every ordinary data query.
    if !sql.to_ascii_lowercase().contains("pg_catalog") {
        return Ok(None);
    }
    fn tokens(s: &str) -> Result<Vec<Token>, WireError> {
        let mut ts = Tokenizer::new(&PostgreSqlDialect {}, s)
            .tokenize()
            .map_err(|_| WireError::unsupported("catalog bootstrap query"))?
            .into_iter()
            .filter(|t| !matches!(t, Token::Whitespace(_)))
            .map(|t| match t {
                Token::Word(mut w) if w.quote_style.is_none() => {
                    w.value.make_ascii_lowercase();
                    Token::Word(w)
                }
                t => t,
            })
            .collect::<Vec<_>>();
        if ts.last() == Some(&Token::SemiColon) {
            ts.pop();
        }
        Ok(ts)
    }
    let Ok(ts) = tokens(sql) else { return Ok(None) };
    // A string literal or a file named pg_catalog is still ordinary user SQL.
    if !ts.windows(2).any(
        |w| matches!((&w[0], &w[1]), (Token::Word(n), Token::Period) if n.value == "pg_catalog"),
    ) {
        return Ok(None);
    }
    if sql.len() > 16_384 {
        return Err(WireError::unsupported("catalog bootstrap query size"));
    }
    let mut out = Response {
        fields: vec![],
        rows: vec![],
        command: String::new(),
        status: vec![],
        deallocate: None,
    };
    if ts == tokens(ATTRIBUTES)? {
        for t in snapshot.tables.iter().filter(|t| t.catalog == database) {
            for c in &t.source.columns {
                let ty = *c
                    .ty
                    .as_ref()
                    .map_err(|_| WireError::unsupported("catalog column type"))?;
                let (oid, _, _) = crate::codec::type_info(ty);
                out.rows.push(vec![
                    Some(t.oid.to_string()),
                    Some(c.name.clone()),
                    Some(oid.to_string()),
                ]);
            }
        }
        out.fields = vec![
            ("attrelid".into(), SqlType::Integer),
            ("attname".into(), SqlType::Varchar),
            ("atttypid".into(), SqlType::Integer),
        ];
    } else if ts == tokens(TYPES)? {
        out.fields = [
            ("oid", SqlType::Integer),
            ("typname", SqlType::Varchar),
            ("typreceive", SqlType::Varchar),
            ("typbasetype", SqlType::Integer),
            ("typrelid", SqlType::Integer),
            ("typarray", SqlType::Integer),
        ]
        .into_iter()
        .map(|(n, t)| (n.into(), t))
        .collect();
        for (oid, name, recv) in [
            (16, "bool", "boolrecv"),
            (20, "int8", "int8recv"),
            (21, "int2", "int2recv"),
            (23, "int4", "int4recv"),
            (25, "text", "textrecv"),
            (700, "float4", "float4recv"),
            (701, "float8", "float8recv"),
            (1043, "varchar", "varcharrecv"),
            (1082, "date", "date_recv"),
            (1700, "numeric", "numeric_recv"),
        ] {
            out.rows.push(
                [
                    oid.to_string(),
                    name.into(),
                    recv.into(),
                    "0".into(),
                    "0".into(),
                    "0".into(),
                ]
                .into_iter()
                .map(Some)
                .collect(),
            );
        }
    } else {
        return Err(WireError::unsupported(
            "catalog query; only exact ADBC type bootstrap is supported",
        ));
    }
    out.command = format!("SELECT {}", out.rows.len());
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoted_search_path_preserves_case_and_empty_path_is_valid() {
        let mut state = State::default();
        assert!(
            state
                .evaluate("SET search_path = \"Quoted Schema\", PUBLIC")
                .is_ok()
        );
        assert_eq!(state.search_path(), vec!["Quoted Schema", "public"]);
        assert!(state.evaluate("SET search_path = ''").is_ok());
        assert!(state.search_path().is_empty());
        assert!(state.set("search_path", "public; SELECT 1").is_err());
        assert!(state.set("search_path", "public,").is_err());
    }
}
