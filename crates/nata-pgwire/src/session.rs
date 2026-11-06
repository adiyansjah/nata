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
    pub fn startup(&mut self, params: &[(String, String)]) -> Result<(), WireError> {
        for (k, v) in params {
            if matches!(k.as_str(), "user" | "database" | "replication") {
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
            "application_name" | "search_path" | "timezone" => true,
            "extra_float_digits" => matches!(l.as_str(), "-15" | "0" | "1" | "2" | "3"),
            "datestyle" => matches!(
                l.as_str(),
                "iso" | "iso, mdy" | "iso , mdy" | "iso, dmy" | "iso , dmy"
            ),
            "client_encoding" => matches!(l.as_str(), "utf8" | "utf-8"),
            "statement_timeout" => l == "0", // A nonzero timeout cannot be a truthful no-op.
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
                let value = words[3..].join(" ");
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
fn status_name(key: &str) -> Option<&str> {
    match key {
        "datestyle" => Some("DateStyle"),
        "timezone" => Some("TimeZone"),
        "client_encoding" | "application_name" | "standard_conforming_strings" => Some(key),
        _ => None,
    }
}
