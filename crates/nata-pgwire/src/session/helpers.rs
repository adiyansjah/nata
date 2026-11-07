//! Card 0007 utility grammar. No general SQL evaluation or invented catalog rows.
use super::*;

fn response(names: &[(&str, SqlType)], rows: Vec<Vec<Option<String>>>, command: &str) -> Response {
    Response {
        fields: names.iter().map(|(n, t)| ((*n).into(), *t)).collect(),
        wire_types: vec![],
        rows,
        command: command.into(),
        status: vec![],
        deallocate: None,
    }
}
fn ident(t: &Token) -> Option<String> {
    match t {
        Token::Word(w) if w.quote_style == Some('"') => Some(w.value.clone()),
        Token::Word(w) if w.quote_style.is_none() => Some(w.value.to_ascii_lowercase()),
        _ => None,
    }
}
fn path(ts: &[Token]) -> Option<Vec<String>> {
    if ts.is_empty() || ts.len().is_multiple_of(2) {
        return None;
    }
    ts.iter()
        .enumerate()
        .filter_map(|(i, t)| {
            if i % 2 == 0 {
                Some(ident(t))
            } else if *t == Token::Period {
                None
            } else {
                Some(None)
            }
        })
        .collect()
}
fn literal(t: &Token) -> Option<&str> {
    if let Token::SingleQuotedString(s) = t {
        Some(s)
    } else {
        None
    }
}
fn word(t: &Token, s: &str) -> bool {
    ident(t).as_deref() == Some(s)
}

impl State {
    pub(super) fn display_setting(&self, key: &str) -> Result<String, WireError> {
        let value = self
            .settings
            .get(key)
            .ok_or_else(|| WireError::unsupported("session parameter"))?;
        if key == "statement_timeout" {
            let ms: u64 = value.parse().expect("validated timeout");
            for (unit, divisor) in [
                ("d", 86_400_000),
                ("h", 3_600_000),
                ("min", 60_000),
                ("s", 1000),
            ] {
                if ms != 0 && ms.is_multiple_of(divisor) {
                    return Ok(format!("{}{unit}", ms / divisor));
                }
            }
            return Ok(if ms == 0 {
                "0".into()
            } else {
                format!("{ms}ms")
            });
        }
        Ok(value.clone())
    }
    pub(super) fn current_schema(&self) -> Option<String> {
        self.search_path().into_iter().find(|n| {
            matches!(n.as_str(), "pg_catalog" | "information_schema")
                || self
                    .catalog
                    .schemas
                    .iter()
                    .any(|s| s.catalog == self.database && s.name == *n)
        })
    }
    fn reset(&mut self, key: &str) -> Result<Option<(String, String)>, WireError> {
        if is_internal(key) {
            return Err(WireError::unsupported("read-only session parameter"));
        }
        let defaults = State::default();
        let value = self
            .reset_settings
            .get(key)
            .or_else(|| defaults.settings.get(key))
            .ok_or_else(|| WireError::unsupported("RESET parameter"))?
            .clone();
        self.settings.insert(key.into(), value.clone());
        self.changed.remove(key);
        Ok(status_name(key).map(|n| (n.into(), value)))
    }
    pub(super) fn helpers(&mut self, sql: &str) -> Result<Option<Response>, WireError> {
        // Bound before tokenization. Ordinary data queries retain their normal path.
        let lower = sql.to_ascii_lowercase();
        let command = lower.split_whitespace().next().unwrap_or("");
        let utility = matches!(command, "show" | "describe" | "reset" | "set");
        let helper = [
            "current_setting",
            "set_config",
            "pg_settings",
            "current_schema",
        ]
        .iter()
        .any(|s| lower.contains(s));
        // Leading SQL comments are possible in client startup statements.
        let commented =
            lower.trim_start().starts_with("--") || lower.trim_start().starts_with("/*");
        if !utility && !helper && !commented {
            return Ok(None);
        }
        if sql.len() > 16_384 {
            return if utility || helper {
                Err(WireError::unsupported("session command size"))
            } else {
                Ok(None)
            };
        }
        let mut ts: Vec<_> = Tokenizer::new(&PostgreSqlDialect {}, sql)
            .tokenize()
            .map_err(|_| WireError::new("42601", "invalid session command"))?
            .into_iter()
            .filter(|t| !matches!(t, Token::Whitespace(_)))
            .collect();
        if ts.last() == Some(&Token::SemiColon) {
            ts.pop();
        }
        let Some(first) = ts.first().and_then(ident) else {
            return Ok(None);
        };
        let empty = || response(&[], vec![], "");
        if first == "reset" {
            let [_, k] = ts.as_slice() else {
                return Err(WireError::unsupported("RESET syntax"));
            };
            let key = ident(k)
                .ok_or_else(|| WireError::unsupported("RESET parameter"))?
                .to_ascii_lowercase();
            let keys = if key == "all" {
                self.settings
                    .keys()
                    .filter(|k| !is_internal(k))
                    .cloned()
                    .collect()
            } else {
                vec![key]
            };
            let mut out = empty();
            out.command = "RESET".into();
            for key in keys {
                if let Some(s) = self.reset(&key)? {
                    out.status.push(s);
                }
            }
            return Ok(Some(out));
        }
        if first == "set"
            && ts.len() == 4
            && word(&ts[3], "default")
            && (ts[2] == Token::Eq || word(&ts[2], "to"))
        {
            let key = ident(&ts[1])
                .ok_or_else(|| WireError::unsupported("SET parameter"))?
                .to_ascii_lowercase();
            let mut out = empty();
            out.command = "SET".into();
            if let Some(s) = self.reset(&key)? {
                out.status.push(s);
            }
            return Ok(Some(out));
        }
        if first == "show" && ts.len() == 2 && word(&ts[1], "all") {
            let rows = self
                .settings
                .keys()
                .map(|k| {
                    Ok(vec![
                        Some(status_name(k).unwrap_or(k).into()),
                        Some(self.display_setting(k)?),
                        Some(setting_description(k).into()),
                    ])
                })
                .collect::<Result<Vec<_>, WireError>>()?;
            return Ok(Some(response(
                &[
                    ("name", SqlType::Varchar),
                    ("setting", SqlType::Varchar),
                    ("description", SqlType::Varchar),
                ],
                rows,
                "SHOW",
            )));
        }
        if first == "describe"
            || (first == "show"
                && ts.get(1).and_then(ident).is_some_and(|s| {
                    matches!(s.as_str(), "catalogs" | "schemas" | "tables" | "columns")
                }))
        {
            return self.show_catalog(&ts, sql).map(Some);
        }
        if first == "select" {
            // Only a single literal helper call, optionally pg_catalog-qualified.
            let mut call = &ts[1..];
            if call.len() >= 2 && word(&call[0], "pg_catalog") && call[1] == Token::Period {
                call = &call[2..];
            }
            if let [f, Token::LParen, args @ .., Token::RParen] = call {
                if word(f, "current_setting") {
                    let (key, missing_ok) = match args {
                        [k] => (literal(k), false),
                        [k, Token::Comma, b] if word(b, "true") || word(b, "false") => {
                            (literal(k), word(b, "true"))
                        }
                        _ => {
                            return Err(WireError::unsupported(
                                "current_setting literal arguments",
                            ));
                        }
                    };
                    let key = key
                        .ok_or_else(|| WireError::unsupported("current_setting literal name"))?
                        .to_ascii_lowercase();
                    let value = if self.settings.contains_key(&key) {
                        Some(self.display_setting(&key)?)
                    } else if missing_ok {
                        None
                    } else {
                        return Err(WireError::unsupported("current_setting parameter"));
                    };
                    return Ok(Some(response(
                        &[("current_setting", SqlType::Varchar)],
                        vec![vec![value]],
                        "SELECT 1",
                    )));
                }
                if word(f, "set_config") {
                    let [k, Token::Comma, v, Token::Comma, b] = args else {
                        return Err(WireError::unsupported("set_config literal arguments"));
                    };
                    if !word(b, "false") {
                        return Err(WireError::unsupported("transaction-local set_config"));
                    }
                    let key = literal(k)
                        .ok_or_else(|| WireError::unsupported("set_config literal name"))?
                        .to_ascii_lowercase();
                    let value = literal(v)
                        .ok_or_else(|| WireError::unsupported("set_config literal value"))?;
                    let status = self.set(&key, value)?;
                    let mut out = response(
                        &[("set_config", SqlType::Varchar)],
                        vec![vec![Some(self.display_setting(&key)?)]],
                        "SELECT 1",
                    );
                    out.status.extend(status);
                    return Ok(Some(out));
                }
                if word(f, "current_schemas") {
                    let [b] = args else {
                        return Err(WireError::unsupported("current_schemas literal boolean"));
                    };
                    if !word(b, "true") && !word(b, "false") {
                        return Err(WireError::unsupported("current_schemas literal boolean"));
                    }
                    let mut names = vec![];
                    let path = self.search_path();
                    if word(b, "true") && !path.iter().any(|n| n == "pg_catalog") {
                        names.push("pg_catalog".to_string());
                    }
                    for n in path {
                        if (n == "pg_catalog"
                            || n == "information_schema"
                            || self
                                .catalog
                                .schemas
                                .iter()
                                .any(|s| s.catalog == self.database && s.name == n))
                            && !names.contains(&n)
                        {
                            names.push(n);
                        }
                    }
                    let value = format!(
                        "{{{}}}",
                        names
                            .iter()
                            .map(|n| {
                                if !n.is_empty()
                                    && !n.eq_ignore_ascii_case("null")
                                    && !n.chars().any(|c| {
                                        c.is_whitespace()
                                            || matches!(c, '{' | '}' | ',' | '"' | '\\')
                                    })
                                {
                                    n.clone()
                                } else {
                                    format!("\"{}\"", n.replace('\\', "\\\\").replace('"', "\\\""))
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(",")
                    );
                    let mut out = response(
                        &[("current_schemas", SqlType::Varchar)],
                        vec![vec![Some(value)]],
                        "SELECT 1",
                    );
                    out.wire_types = vec![(1003, -1, -1)];
                    return Ok(Some(out));
                }
                if word(f, "current_schema") && args.is_empty() {
                    let mut out = response(
                        &[("current_schema", SqlType::Varchar)],
                        vec![vec![self.current_schema()]],
                        "SELECT 1",
                    );
                    out.wire_types = vec![(19, 64, -1)];
                    return Ok(Some(out));
                }
            }
            if ts.iter().any(|t| word(t, "pg_settings")) {
                return self.pg_settings(&ts).map(Some);
            }
        }
        Ok(None)
    }

    fn show_catalog(&self, ts: &[Token], sql: &str) -> Result<Response, WireError> {
        let err = || WireError::unsupported("SHOW/DESCRIBE syntax");
        let describe = word(&ts[0], "describe");
        let kind = if describe {
            "columns".into()
        } else {
            ident(&ts[1]).ok_or_else(err)?
        };
        let target = if describe {
            &ts[1..]
        } else if ts.len() == 2 {
            &[]
        } else {
            if ts.len() == 3 || !word(&ts[2], "from") {
                return Err(err());
            }
            &ts[3..]
        };
        let name = if target.is_empty() {
            vec![]
        } else {
            path(target).ok_or_else(err)?
        };
        // Position the target's first token, including multiline and quoted names.
        let positioned = |code, message| {
            let index = ts.len() - target.len();
            let located = Tokenizer::new(&PostgreSqlDialect {}, sql)
                .tokenize_with_location()
                .ok();
            let pos = located
                .and_then(|v| {
                    v.into_iter()
                        .filter(|t| !matches!(t.token, Token::Whitespace(_)))
                        .nth(index)
                })
                .map(|t| nata_sql::Pos {
                    line: t.span.start.line as u32,
                    col: t.span.start.column as u32,
                })
                .unwrap_or(nata_sql::Pos::START);
            WireError::new(code, message).at(pos)
        };
        let database = match (kind.as_str(), name.as_slice()) {
            ("catalogs", []) => self.database.as_str(),
            ("schemas", []) | ("tables", []) | ("columns", _) => self.database.as_str(),
            ("schemas", [c]) | ("tables", [c, _]) => c,
            ("tables", [_]) => self.database.as_str(),
            _ => return Err(err()),
        };
        if kind == "catalogs" {
            let mut rows: Vec<_> = self
                .catalog
                .catalogs
                .iter()
                .map(|c| vec![Some(c.name.clone())])
                .collect();
            rows.sort();
            return Ok(response(
                &[("catalog_name", SqlType::Varchar)],
                rows,
                "SHOW",
            ));
        }
        if kind != "columns" && !self.catalog.catalogs.iter().any(|c| c.name == database) {
            return Err(positioned("3D000", "unknown registered catalog"));
        }
        if kind == "schemas" {
            let (fields, mut rows) =
                crate::metadata::relation(&self.catalog, database, "information_schema.schemata")?;
            rows.sort();
            let mut out = empty_response(fields, rows);
            out.command = "SHOW".into();
            return Ok(out);
        }
        if kind == "tables" {
            let default_schema = self.current_schema();
            let schema = name
                .last()
                .map(String::as_str)
                .or(default_schema.as_deref());
            if schema.is_some_and(|n| {
                !self
                    .catalog
                    .schemas
                    .iter()
                    .any(|s| s.catalog == database && s.name == n)
            }) {
                return Err(positioned("3F000", "unknown registered schema"));
            }
            let (mut fields, rows) =
                crate::metadata::relation(&self.catalog, database, "information_schema.tables")?;
            fields.truncate(4);
            let mut rows: Vec<_> = rows
                .into_iter()
                .filter(|r| r[1].as_deref() == schema && schema.is_some())
                .map(|mut r| {
                    r.truncate(4);
                    r
                })
                .collect();
            rows.sort();
            let mut out = empty_response(fields, rows);
            out.command = "SHOW".into();
            return Ok(out);
        }
        // Resolve using the same snapshot/search path as ordinary registered SQL.
        if name.is_empty() || name.len() > 3 {
            return Err(err());
        }
        let schemas = self.search_path();
        let mut table = None;
        for schema in &schemas {
            table = self.catalog.tables.iter().find(|t| match name.as_slice() {
                [n] => t.catalog == self.database && t.schema == *schema && t.name == *n,
                [s, n] => t.catalog == self.database && t.schema == *s && t.name == *n,
                [c, s, n] => t.catalog == *c && t.schema == *s && t.name == *n,
                _ => false,
            });
            if table.is_some() {
                break;
            }
        }
        // Qualified names do not depend on an empty search_path.
        if name.len() > 1 {
            table = self.catalog.tables.iter().find(|t| match name.as_slice() {
                [s, n] => t.catalog == self.database && t.schema == *s && t.name == *n,
                [c, s, n] => t.catalog == *c && t.schema == *s && t.name == *n,
                _ => false,
            });
        }
        let table = table.ok_or_else(|| positioned("42P01", "unknown registered table"))?;
        let (fields, rows) =
            crate::metadata::relation(&self.catalog, &table.catalog, "information_schema.columns")?;
        let selected = [3, 6, 5, 4, 7, 8];
        let fields = selected.iter().map(|i| fields[*i].clone()).collect();
        let rows = rows
            .into_iter()
            .filter(|r| {
                r[1].as_deref() == Some(&table.schema) && r[2].as_deref() == Some(&table.name)
            })
            .map(|r| selected.iter().map(|i| r[*i].clone()).collect())
            .collect();
        let mut out = empty_response(fields, rows);
        out.command = if describe { "DESCRIBE" } else { "SHOW" }.into();
        Ok(out)
    }

    fn pg_settings(&self, ts: &[Token]) -> Result<Response, WireError> {
        let err = || {
            WireError::unsupported(
                "pg_settings projection; explicit columns, optional WHERE name = literal, ORDER BY name only",
            )
        };
        let from = ts.iter().position(|t| word(t, "from")).ok_or_else(err)?;
        let mut tail = &ts[from + 1..];
        if tail.len() >= 2 && word(&tail[0], "pg_catalog") && tail[1] == Token::Period {
            tail = &tail[2..];
        }
        if tail.first().is_none_or(|t| !word(t, "pg_settings")) {
            return Err(err());
        }
        tail = &tail[1..];
        let filter = if let [w, n, Token::Eq, v, rest @ ..] = tail {
            if !word(w, "where") || !word(n, "name") {
                return Err(err());
            }
            tail = rest;
            Some(literal(v).ok_or_else(err)?)
        } else {
            None
        };
        if !tail.is_empty()
            && !(tail.len() == 3
                && word(&tail[0], "order")
                && word(&tail[1], "by")
                && word(&tail[2], "name"))
        {
            return Err(err());
        }
        let names = [
            "name",
            "setting",
            "unit",
            "category",
            "short_desc",
            "context",
            "vartype",
            "source",
            "min_val",
            "max_val",
            "enumvals",
            "boot_val",
            "reset_val",
            "pending_restart",
        ];
        let selected = ts[1..from]
            .split(|t| *t == Token::Comma)
            .map(|part| {
                let [t] = part else {
                    return Err(err());
                };
                let n = ident(t).ok_or_else(err)?;
                names.iter().position(|name| *name == n).ok_or_else(err)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let defaults = State::default();
        let mut rows = vec![];
        let mut entries: Vec<_> = self.settings.iter().collect();
        entries.sort_by_key(|(key, _)| status_name(key).unwrap_or(key));
        for (key, value) in entries {
            let name = status_name(key).unwrap_or(key);
            if filter.is_some_and(|f| f != name) {
                continue;
            }
            let internal = is_internal(key);
            let (vartype, min, max) = match key.as_str() {
                "statement_timeout" => ("integer", Some("0"), Some("2147483647")),
                "extra_float_digits" => ("integer", Some("1"), Some("3")),
                "server_version_num" => ("integer", Some("160000"), Some("160000")),
                "integer_datetimes"
                | "standard_conforming_strings"
                | "default_transaction_read_only"
                | "default_transaction_deferrable" => ("bool", None, None),
                _ => ("string", None, None),
            };
            let reset = self
                .reset_settings
                .get(key)
                .unwrap_or(&defaults.settings[key]);
            let row = vec![
                Some(name.into()),
                Some(value.clone()),
                (key == "statement_timeout").then(|| "ms".into()),
                Some("Nata compatibility".into()),
                Some(setting_description(key).into()),
                Some(if internal { "internal" } else { "user" }.into()),
                Some(vartype.into()),
                Some(
                    if self.changed.contains(key) {
                        "session"
                    } else if self.startup_keys.contains(key) {
                        "client"
                    } else {
                        "default"
                    }
                    .into(),
                ),
                min.map(String::from),
                max.map(String::from),
                None,
                Some(defaults.settings[key].clone()),
                Some(reset.clone()),
                Some("f".into()),
            ];
            rows.push(selected.iter().map(|i| row[*i].clone()).collect());
        }
        let fields = selected
            .iter()
            .map(|i| {
                (
                    names[*i].into(),
                    if *i == 13 {
                        SqlType::Boolean
                    } else {
                        SqlType::Varchar
                    },
                )
            })
            .collect();
        let mut out = empty_response(fields, rows);
        out.wire_types = selected
            .iter()
            .map(|i| {
                if *i == 10 {
                    (1009, -1, -1)
                } else if *i == 13 {
                    (16, 1, -1)
                } else {
                    (25, -1, -1)
                }
            })
            .collect();
        Ok(out)
    }
}
fn empty_response(fields: Vec<(String, SqlType)>, rows: Vec<Vec<Option<String>>>) -> Response {
    Response {
        fields,
        wire_types: vec![],
        command: format!("SELECT {}", rows.len()),
        rows,
        status: vec![],
        deallocate: None,
    }
}
fn is_internal(key: &str) -> bool {
    matches!(
        key,
        "server_version" | "server_version_num" | "server_encoding" | "integer_datetimes"
    )
}
fn setting_description(key: &str) -> &str {
    match key {
        "statement_timeout" => "Statement deadline through Sync, including suspended portals.",
        "search_path" => "Registered schema resolution order.",
        "timezone" => "Compatibility value; Date32 has no time zone.",
        "default_transaction_read_only"
        | "default_transaction_isolation"
        | "default_transaction_deferrable" => {
            "Compatibility value; no writes or transaction isolation guarantees."
        }
        _ => "Restricted Nata PostgreSQL compatibility setting; see serve.md.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn eval(s: &mut State, sql: &str) -> Response {
        s.evaluate(sql)
            .unwrap_or_else(|_| panic!("{sql}"))
            .expect("response")
    }
    #[test]
    fn settings_registry_reset_helpers_and_refusals() {
        let mut s = State::default();
        assert!(
            s.startup(&[("application_name".into(), "startup".into())])
                .is_ok()
        );
        eval(&mut s, "SET application_name='changed'");
        assert_eq!(
            eval(
                &mut s,
                "SELECT set_config('application_name','helper',false)"
            )
            .rows,
            vec![vec![Some("helper".into())]]
        );
        eval(&mut s, "RESET application_name");
        assert_eq!(
            eval(&mut s, "SHOW application_name").rows,
            vec![vec![Some("startup".into())]]
        );
        eval(&mut s, "SET statement_timeout='1s'");
        assert_eq!(
            eval(&mut s, "SELECT current_setting('statement_timeout', false)").rows,
            vec![vec![Some("1s".into())]]
        );
        assert_eq!(
            eval(&mut s, "SELECT current_setting('unknown',true)").rows,
            vec![vec![None]]
        );
        let r = eval(
            &mut s,
            "SELECT name,setting,unit,source,reset_val,pending_restart FROM pg_catalog.pg_settings WHERE name='statement_timeout' ORDER BY name",
        );
        assert_eq!(
            r.rows,
            vec![vec![
                Some("statement_timeout".into()),
                Some("1000".into()),
                Some("ms".into()),
                Some("session".into()),
                Some("0".into()),
                Some("f".into())
            ]]
        );
        eval(&mut s, "SET DateStyle='ISO, DMY'");
        assert_eq!(
            eval(&mut s, "SHOW DateStyle").rows,
            vec![vec![Some("ISO, DMY".into())]]
        );
        eval(&mut s, "SET statement_timeout=DEFAULT");
        assert!(s.timeout().is_none());
        eval(&mut s, "RESET ALL");
        assert_eq!(eval(&mut s, "SHOW ALL").rows.len(), 15);
        for sql in [
            "SET LOCAL application_name='x'",
            "SELECT set_config('application_name','x',true)",
            "SELECT set_config('work_mem','1MB',false)",
            "SELECT current_setting('missing')",
            "RESET work_mem",
            "SELECT * FROM pg_settings",
            "SELECT setting FROM pg_settings WHERE name LIKE '%'",
            "SELECT current_setting(application_name)",
            "SELECT current_setting('application_name'); SELECT 1",
        ] {
            assert!(s.evaluate(sql).is_err(), "must refuse {sql}");
        }
        assert_eq!(
            eval(&mut s, "SHOW application_name").rows,
            vec![vec![Some("startup".into())]]
        );
    }
    #[test]
    fn show_describe_snapshot_qualified_quoted_and_missing() {
        use crate::catalog::{Catalog, Schema, Snapshot, Table};
        use nata_sql::source::{SourceColumn, SourceInfo};
        let mut s = State {
            catalog: std::sync::Arc::new(Snapshot {
                catalogs: vec![Catalog {
                    name: "nata".into(),
                    oid: 16384,
                }],
                schemas: vec![Schema {
                    catalog: "nata".into(),
                    name: "Quoted Schema".into(),
                    oid: 16385,
                }],
                tables: vec![Table {
                    catalog: "nata".into(),
                    schema: "Quoted Schema".into(),
                    name: "Mixed Table".into(),
                    oid: 16386,
                    schema_oid: 16385,
                    source: SourceInfo {
                        pattern: String::new(),
                        files: vec![],
                        columns: vec![SourceColumn {
                            name: "Mixed Column".into(),
                            kind: Ok(nata_parquet::schema::Kind::Utf8),
                            ty: Ok(SqlType::Decimal {
                                precision: 10,
                                scale: 2,
                            }),
                        }],
                    },
                }],
            }),
            ..State::default()
        };
        eval(&mut s, "SET search_path=\"Quoted Schema\"");
        assert_eq!(
            eval(&mut s, "SHOW CATALOGS").rows,
            vec![vec![Some("nata".into())]]
        );
        assert_eq!(
            eval(&mut s, "SHOW SCHEMAS FROM nata").rows,
            vec![vec![Some("nata".into()), Some("Quoted Schema".into())]]
        );
        assert_eq!(eval(&mut s, "SHOW TABLES").rows.len(), 1);
        assert_eq!(
            eval(&mut s, "SELECT current_schema()").rows,
            vec![vec![Some("Quoted Schema".into())]]
        );
        let r = eval(&mut s, "DESCRIBE \"Mixed Table\"");
        assert_eq!(
            r.rows,
            vec![vec![
                Some("Mixed Column".into()),
                Some("numeric".into()),
                Some("YES".into()),
                Some("1".into()),
                Some("10".into()),
                Some("2".into())
            ]]
        );
        eval(&mut s, "SET search_path=''");
        assert_eq!(
            eval(
                &mut s,
                "SHOW COLUMNS FROM nata.\"Quoted Schema\".\"Mixed Table\""
            )
            .rows,
            r.rows
        );
        for sql in [
            "SHOW TABLES FROM missing",
            "SHOW SCHEMAS FROM missing",
            "DESCRIBE missing",
            "SHOW COLUMNS FROM missing",
            "SHOW TABLES FROM a.b.c",
            "DESCRIBE 'file.parquet'",
            "SHOW TABLES; SELECT 1",
        ] {
            assert!(s.evaluate(sql).is_err(), "{sql}");
        }
        assert_eq!(
            eval(&mut s, "SELECT current_schema()").rows,
            vec![vec![None]]
        );
    }
}
