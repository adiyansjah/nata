//! Bounded metadata projection adapter. Unsupported syntax/objects fail before producing rows.
use crate::{catalog::Snapshot, server::WireError, session::Response};
use nata_sql::types::SqlType;
use sqlparser::{
    dialect::PostgreSqlDialect,
    tokenizer::{Token, Tokenizer},
};
type Relation = (Vec<(String, SqlType)>, Vec<Vec<Option<String>>>);
#[derive(Default)]
struct BoundedRows {
    rows: Vec<Vec<Option<String>>>,
    bytes: usize,
}
impl BoundedRows {
    fn push(&mut self, row: Vec<Option<String>>) -> Result<(), WireError> {
        let bytes = std::mem::size_of::<Vec<Option<String>>>()
            + row
                .iter()
                .map(|v| std::mem::size_of::<Option<String>>() + v.as_ref().map_or(0, String::len))
                .sum::<usize>();
        self.bytes = self.bytes.saturating_add(bytes);
        if self.bytes > crate::catalog::MAX_SNAPSHOT_BYTES {
            return Err(WireError::new("54000", "metadata result budget exceeded"));
        }
        self.rows.push(row);
        Ok(())
    }
}
pub(crate) fn relation(
    snapshot: &Snapshot,
    database: &str,
    name: &str,
) -> Result<Relation, WireError> {
    let mut rows = BoundedRows::default();
    let columns: &[(&str, SqlType)] = match name {
        "information_schema.information_schema_catalog_name" => {
            for catalog in snapshot.catalogs.iter().filter(|c| c.name == database) {
                rows.push(vec![Some(catalog.name.clone())])?;
            }
            &[("catalog_name", SqlType::Varchar)]
        }
        "information_schema.schemata" => {
            for s in snapshot.schemas.iter().filter(|s| s.catalog == database) {
                rows.push(vec![Some(s.catalog.clone()), Some(s.name.clone())])?;
            }
            &[
                ("catalog_name", SqlType::Varchar),
                ("schema_name", SqlType::Varchar),
            ]
        }
        "information_schema.tables" => {
            for t in snapshot.tables.iter().filter(|t| t.catalog == database) {
                rows.push(
                    [
                        t.catalog.clone(),
                        t.schema.clone(),
                        t.name.clone(),
                        "BASE TABLE".into(),
                        "NO".into(),
                    ]
                    .into_iter()
                    .map(Some)
                    .collect(),
                )?;
            }
            &[
                ("table_catalog", SqlType::Varchar),
                ("table_schema", SqlType::Varchar),
                ("table_name", SqlType::Varchar),
                ("table_type", SqlType::Varchar),
                ("is_insertable_into", SqlType::Varchar),
            ]
        }
        "information_schema.columns" => {
            for t in snapshot.tables.iter().filter(|t| t.catalog == database) {
                for (i, c) in t.source.columns.iter().enumerate() {
                    let ty =
                        c.ty.as_ref()
                            .map_err(|_| WireError::unsupported("catalog column type"))?;
                    let (precision, scale) = match ty {
                        SqlType::Decimal { precision, scale } => {
                            (Some(precision.to_string()), Some(scale.to_string()))
                        }
                        SqlType::SmallInt => (Some("16".into()), Some("0".into())),
                        SqlType::Integer => (Some("32".into()), Some("0".into())),
                        SqlType::BigInt => (Some("64".into()), Some("0".into())),
                        SqlType::Float => (Some("24".into()), None),
                        SqlType::Double => (Some("53".into()), None),
                        _ => (None, None),
                    };
                    rows.push(vec![
                        Some(t.catalog.clone()),
                        Some(t.schema.clone()),
                        Some(t.name.clone()),
                        Some(c.name.clone()),
                        Some((i + 1).to_string()),
                        Some("YES".into()),
                        Some(
                            match ty {
                                SqlType::Boolean => "boolean",
                                SqlType::TinyInt => "text",
                                SqlType::SmallInt => "smallint",
                                SqlType::Integer => "integer",
                                SqlType::BigInt => "bigint",
                                SqlType::Float => "real",
                                SqlType::Double => "double precision",
                                SqlType::Decimal { .. } => "numeric",
                                SqlType::Varchar => "text",
                                SqlType::Date => "date",
                                SqlType::Null => {
                                    return Err(WireError::unsupported("untyped catalog column"));
                                }
                            }
                            .into(),
                        ),
                        precision,
                        scale,
                        Some("NO".into()),
                    ])?;
                }
            }
            &[
                ("table_catalog", SqlType::Varchar),
                ("table_schema", SqlType::Varchar),
                ("table_name", SqlType::Varchar),
                ("column_name", SqlType::Varchar),
                ("ordinal_position", SqlType::Integer),
                ("is_nullable", SqlType::Varchar),
                ("data_type", SqlType::Varchar),
                ("numeric_precision", SqlType::Integer),
                ("numeric_scale", SqlType::Integer),
                ("is_updatable", SqlType::Varchar),
            ]
        }
        _ => {
            return Err(WireError::unsupported(
                "catalog object or column outside the skeleton metadata contract",
            ));
        }
    };
    Ok((
        columns.iter().map(|(n, t)| ((*n).into(), *t)).collect(),
        rows.rows,
    ))
}
pub(crate) fn query(
    snapshot: &Snapshot,
    database: &str,
    sql: &str,
) -> Result<Option<Response>, WireError> {
    if !sql.to_ascii_lowercase().contains("information_schema") {
        return Ok(None);
    }
    if sql.len() > 16_384 {
        return Err(WireError::unsupported("metadata query size"));
    }
    let mut tokens: Vec<_> = Tokenizer::new(&PostgreSqlDialect {}, sql)
        .tokenize()
        .map_err(|_| WireError::unsupported("metadata tokens"))?
        .into_iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect();
    if tokens.last() == Some(&Token::SemiColon) {
        tokens.pop();
    }
    // String literals containing a catalog name are ordinary user SQL.
    if !tokens.windows(2).any(|ts| matches!((&ts[0], &ts[1]), (Token::Word(w), Token::Period) if w.value.eq_ignore_ascii_case("information_schema"))) { return Ok(None); }
    let err = || {
        WireError::unsupported(
            "metadata projection; joins, expressions, filters and aggregates are not supported in this skeleton",
        )
    };
    let word = |t: &Token| match t {
        Token::Word(w) => Some(if w.quote_style.is_some() {
            w.value.clone()
        } else {
            w.value.to_ascii_lowercase()
        }),
        _ => None,
    };
    if tokens.first().and_then(word).as_deref() != Some("select") {
        return Err(err());
    }
    let from = tokens
        .iter()
        .position(|t| word(t).as_deref() == Some("from"))
        .ok_or_else(err)?;
    let mut projection = Vec::new();
    for ts in tokens[1..from].split(|t| *t == Token::Comma) {
        match ts {
            [Token::Mul] => projection.push("*".into()),
            [t] => projection.push(word(t).ok_or_else(err)?),
            _ => return Err(err()),
        }
    }
    let rest = &tokens[from + 1..];
    if rest.len() < 3 || rest[1] != Token::Period {
        return Err(err());
    }
    let name = format!(
        "{}.{}",
        word(&rest[0]).ok_or_else(err)?,
        word(&rest[2]).ok_or_else(err)?
    );
    // A star would promise the complete standard view, including columns this
    // skeleton refuses. Never silently narrow it to the supported projection.
    if projection == ["*"] && name != "information_schema.information_schema_catalog_name" {
        return Err(WireError::unsupported(
            "wildcard of an incomplete catalog view; request explicit supported columns",
        ));
    }
    let (fields, mut rows) = relation(snapshot, database, &name)?;
    let index = |s: &str| {
        fields.iter().position(|(n, _)| n == s).ok_or_else(|| {
            WireError::unsupported(
                "catalog object or column outside the skeleton metadata contract",
            )
        })
    };
    if rest.len() > 3 {
        if rest.get(3).and_then(word).as_deref() != Some("order")
            || rest.get(4).and_then(word).as_deref() != Some("by")
        {
            return Err(err());
        }
        let keys = rest[5..]
            .split(|t| *t == Token::Comma)
            .map(|ts| match ts {
                [t] => index(&word(t).ok_or_else(err)?),
                _ => Err(err()),
            })
            .collect::<Result<Vec<_>, _>>()?;
        rows.sort_by(|a, b| {
            for i in &keys {
                let order = if a[*i].is_none() || b[*i].is_none() {
                    a[*i].is_none().cmp(&b[*i].is_none())
                } else if fields[*i].1 == SqlType::Integer {
                    a[*i]
                        .as_ref()
                        .and_then(|v| v.parse::<i32>().ok())
                        .cmp(&b[*i].as_ref().and_then(|v| v.parse::<i32>().ok()))
                } else {
                    a[*i].cmp(&b[*i])
                };
                if !order.is_eq() {
                    return order;
                }
            }
            std::cmp::Ordering::Equal
        });
    }
    let selected = if projection == ["*"] {
        (0..fields.len()).collect::<Vec<_>>()
    } else {
        projection
            .iter()
            .map(|n| index(n))
            .collect::<Result<Vec<_>, _>>()?
    };
    let fields = selected.iter().map(|i| fields[*i].clone()).collect();
    let rows: Vec<_> = rows
        .into_iter()
        .map(|r| selected.iter().map(|i| r[*i].clone()).collect())
        .collect();
    Ok(Some(Response {
        fields,
        wire_types: vec![],
        command: format!("SELECT {}", rows.len()),
        rows,
        status: vec![],
        deallocate: None,
    }))
}

pub(crate) fn query_with(
    snapshot: &Snapshot,
    database: &str,
    search_path: &[String],
    sql: &str,
    params: &[nata_sql::ast::Expr],
) -> Result<Option<Response>, WireError> {
    if let Some(r) = crate::browse::query(snapshot, database, search_path, sql, params)? {
        return Ok(Some(r));
    }
    query(snapshot, database, sql)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_columns_preserve_every_field_in_order() {
        use nata_sql::source::{SourceColumn, SourceInfo};
        let columns = [
            ("id", SqlType::Integer),
            (
                "Mixed Column",
                SqlType::Decimal {
                    precision: 10,
                    scale: 2,
                },
            ),
            ("txt", SqlType::Varchar),
        ]
        .into_iter()
        .map(|(name, ty)| SourceColumn {
            name: name.into(),
            kind: Ok(nata_parquet::schema::Kind::Utf8),
            ty: Ok(ty),
        })
        .collect();
        let snapshot = Snapshot {
            tables: vec![crate::catalog::Table {
                catalog: "nata".into(),
                schema: "public".into(),
                name: "example".into(),
                oid: 16384,
                schema_oid: 16385,
                source: SourceInfo {
                    pattern: String::new(),
                    files: vec![],
                    columns,
                },
            }],
            ..Snapshot::default()
        };
        let r = query(&snapshot, "nata", "SELECT column_name,ordinal_position,numeric_precision,numeric_scale FROM information_schema.columns ORDER BY ordinal_position")
            .unwrap_or_else(|_| panic!("metadata")).expect("response");
        assert_eq!(
            r.rows,
            vec![
                vec![
                    Some("id".into()),
                    Some("1".into()),
                    Some("32".into()),
                    Some("0".into())
                ],
                vec![
                    Some("Mixed Column".into()),
                    Some("2".into()),
                    Some("10".into()),
                    Some("2".into())
                ],
                vec![Some("txt".into()), Some("3".into()), None, None],
            ]
        );
        assert_eq!(r.command, "SELECT 3");
        let other = query(
            &snapshot,
            "empty",
            "SELECT column_name FROM information_schema.columns",
        )
        .unwrap_or_else(|_| panic!("metadata"))
        .expect("response");
        assert!(other.rows.is_empty());
    }
    #[test]
    fn seeded_metadata_fuzz() {
        let cases = std::env::var("NATA_METADATA_CASES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(200);
        let mut seed = 20261107u64;
        let pieces = [
            "SELECT ",
            "SHOW ",
            "DESCRIBE ",
            "RESET ",
            "SET ",
            "ALL ",
            "current_setting ",
            "set_config ",
            "current_schemas ",
            "pg_settings ",
            "name ",
            "= ",
            "true ",
            "false ",
            "'statement_timeout' ",
            "FROM ",
            "information_schema.",
            "tables ",
            "columns ",
            "pg_policy ",
            "* ",
            "table_name ",
            "WHERE ",
            "ORDER BY ",
            "UNION ",
            "'pg_catalog' ",
            "( ",
            ") ",
            "; ",
            "\"x\" ",
            "--x\n",
            ", ",
        ];
        for _ in 0..cases {
            let mut sql = String::new();
            for _ in 0..32 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                sql.push_str(pieces[(seed >> 32) as usize % pieces.len()]);
            }
            let _ = query_with(&Snapshot::default(), "nata", &["public".into()], &sql, &[]);
            let _ = crate::session::State::default().evaluate(&sql);
        }
    }
}
