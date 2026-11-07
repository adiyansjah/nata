//! Frozen client SQL shapes lowered to snapshot operations, never to user-table SQL.
//! A shape matches every token except explicitly named predicate parameters.
use crate::{
    catalog::{Snapshot, Table},
    server::WireError,
    session::Response,
};
use nata_sql::{
    ast::{Expr, ExprKind},
    types::SqlType,
};
use sqlparser::{
    dialect::PostgreSqlDialect,
    tokenizer::{Token, Tokenizer},
};
use std::sync::OnceLock;
struct Shape {
    id: usize,
    sql: &'static str,
    fields: &'static [(&'static str, i32, i16, i32)],
}
include!("browse_shapes.rs");
type Match<'a> = (&'a Shape, Vec<Option<String>>);
fn tokens(sql: &str) -> Result<Vec<Token>, WireError> {
    let mut ts = Tokenizer::new(&PostgreSqlDialect {}, sql)
        .tokenize()
        .map_err(|_| WireError::unsupported("catalog query tokens"))?
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
fn value(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Cast { expr, .. } => value(expr),
        ExprKind::Str(s) | ExprKind::Number(s) => Some(s.clone()),
        ExprKind::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}
fn matched<'a>(
    ts: &[Token],
    params: &[Expr],
    shape: &'a Shape,
    pattern: &[Token],
) -> Option<Match<'a>> {
    if ts.len() != pattern.len() {
        return None;
    }
    let mut args = vec![None; 2];
    let mut assigned = [false; 2];
    for (a, b) in ts.iter().zip(pattern) {
        if let Token::Placeholder(p) = b {
            let i = p[1..].parse::<usize>().ok()?.checked_sub(1)?;
            let v = match a {
                Token::Placeholder(n) => params
                    .get(n[1..].parse::<usize>().ok()?.checked_sub(1)?)
                    .and_then(value),
                Token::SingleQuotedString(s) | Token::Number(s, false) => Some(s.clone()),
                Token::Word(w) if w.quote_style.is_none() && w.value == "null" => None,
                _ => return None,
            };
            if assigned[i] && args[i] != v {
                return None;
            }
            args[i] = v;
            assigned[i] = true;
        } else if a != b {
            return None;
        }
    }
    Some((shape, args))
}
pub(crate) fn recognizes(sql: &str) -> bool {
    identify(sql, &[]).ok().flatten().is_some()
}
fn identify(sql: &str, params: &[Expr]) -> Result<Option<Match<'static>>, WireError> {
    if sql.len() > 16384 {
        return Ok(None);
    }
    let lower = sql.to_ascii_lowercase();
    if ![
        "pg_catalog",
        "information_schema",
        "current_schemas",
        "current_catalog",
    ]
    .iter()
    .any(|s| lower.contains(s))
    {
        return Ok(None);
    }
    let Ok(ts) = tokens(sql) else {
        return Ok(None);
    };
    static PATTERNS: OnceLock<Vec<Vec<Token>>> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        SHAPES
            .iter()
            .map(|s| tokens(s.sql).unwrap_or_else(|_| panic!("frozen tokens")))
            .collect()
    });
    Ok(SHAPES
        .iter()
        .zip(patterns)
        .find_map(|(s, p)| matched(&ts, params, s, p)))
}
#[derive(Default)]
struct Rows {
    rows: Vec<Vec<Option<String>>>,
    bytes: usize,
    exceeded: bool,
}
impl Rows {
    fn push(&mut self, row: Vec<Option<String>>) {
        let charge = 4 * std::mem::size_of_val(&row)
            + row
                .iter()
                .map(|c| std::mem::size_of_val(c) + c.as_ref().map_or(0, String::capacity))
                .sum::<usize>();
        self.bytes = self.bytes.saturating_add(charge);
        if self.bytes > crate::catalog::MAX_SNAPSHOT_BYTES {
            self.exceeded = true;
        } else {
            self.rows.push(row);
        }
    }
}
fn s(v: impl ToString) -> Option<String> {
    Some(v.to_string())
}
fn boolean(v: bool) -> Option<String> {
    s(if v { "t" } else { "f" })
}
fn err() -> WireError {
    WireError::unsupported("catalog predicate outside the frozen client surface")
}
// PostgreSQL SQL LIKE, with backslash escapes, bounded by the wire SQL budget.
fn like(text: &str, pattern: &str) -> bool {
    let t: Vec<_> = text.chars().collect();
    let p: Vec<_> = pattern.chars().collect();
    let mut prev = vec![false; t.len() + 1];
    prev[0] = true;
    let mut i = 0;
    while i < p.len() {
        let c = p[i];
        let escaped = c == '\\' && i + 1 < p.len();
        if escaped {
            i += 1;
        }
        let c = if escaped { p[i] } else { c };
        let mut next = vec![false; t.len() + 1];
        for j in 0..=t.len() {
            next[j] = if !escaped && c == '%' {
                prev[j] || (j > 0 && next[j - 1])
            } else {
                j > 0 && prev[j - 1] && ((!escaped && c == '_') || c == t[j - 1])
            };
        }
        prev = next;
        i += 1;
    }
    prev[t.len()]
}
fn regex_name(text: &str, pattern: &str) -> Result<bool, WireError> {
    let Some(inner) = pattern
        .strip_prefix("^(")
        .and_then(|p| p.strip_suffix(")$"))
    else {
        return Err(err());
    };
    // psql identifier patterns: quoted names are literal; unquoted * and ? are emitted as .* and . .
    let mut p = String::new();
    let mut cs = inner.chars().peekable();
    while let Some(c) = cs.next() {
        match c {
            '\\' => {
                p.push('\\');
                p.push(cs.next().ok_or_else(err)?);
            }
            '.' => {
                if cs.peek() == Some(&'*') {
                    cs.next();
                    p.push('%');
                } else {
                    p.push('_');
                }
            }
            '*' | '+' | '[' | ']' | '(' | ')' | '|' | '^' | '$' => return Err(err()),
            '%' | '_' => {
                p.push('\\');
                p.push(c);
            }
            _ => p.push(c),
        }
    }
    Ok(like(text, &p))
}
fn effective_schemas(snapshot: &Snapshot, database: &str, path: &[String]) -> Vec<String> {
    let mut names = Vec::new();
    if !path.iter().any(|n| n == "pg_catalog") {
        names.push("pg_catalog".into());
    }
    for name in path {
        if (matches!(name.as_str(), "pg_catalog" | "information_schema")
            || snapshot
                .schemas
                .iter()
                .any(|s| s.catalog == database && s.name == *name))
            && !names.contains(name)
        {
            names.push(name.clone());
        }
    }
    names
}
pub(crate) fn array_oid(oid: i32) -> i32 {
    match oid {
        16 => 1000,
        20 => 1016,
        21 => 1005,
        23 => 1007,
        25 => 1009,
        700 => 1021,
        701 => 1022,
        1043 => 1015,
        1082 => 1182,
        1700 => 1231,
        _ => 0,
    }
}
// Only the scalar relation-kind arrays sent by the frozen clients. Reject
// ambiguous array syntax rather than treating a comma inside an element as a separator.
fn table_kinds(value: Option<&str>) -> Result<bool, WireError> {
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return Ok(false);
    };
    let body = value
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .ok_or_else(err)?;
    if body.is_empty() {
        return Ok(false);
    }
    let mut found = false;
    for element in body.split(',') {
        let element = element.trim();
        if element == "NULL" {
            continue;
        }
        let bytes = element.as_bytes();
        let kind = match bytes {
            [c] if c.is_ascii() && !matches!(*c, b'"' | b'\\' | b'{' | b'}') => *c,
            [b'"', c, b'"'] if c.is_ascii() && !matches!(*c, b'"' | b'\\') => *c,
            _ => return Err(err()),
        };
        found |= kind == b'r';
    }
    Ok(found)
}
fn type_data(
    ty: SqlType,
) -> (
    i32,
    i16,
    i32,
    &'static str,
    &'static str,
    Option<i32>,
    Option<i32>,
) {
    let (oid, len, modif) = crate::codec::type_info(ty);
    let (name, display, p, scale) = match ty {
        SqlType::Boolean => ("bool", "boolean", None, None),
        SqlType::SmallInt => ("int2", "smallint", Some(16), Some(0)),
        SqlType::Integer => ("int4", "integer", Some(32), Some(0)),
        SqlType::BigInt => ("int8", "bigint", Some(64), Some(0)),
        SqlType::Float => ("float4", "real", Some(24), None),
        SqlType::Double => ("float8", "double precision", Some(53), None),
        SqlType::Decimal { precision, scale } => (
            "numeric",
            "numeric",
            Some(i32::from(precision)),
            Some(i32::from(scale)),
        ),
        SqlType::Date => ("date", "date", None, None),
        _ => ("text", "text", None, None),
    };
    (oid, len, modif, name, display, p, scale)
}
fn column_type(t: &Table, i: usize) -> Result<SqlType, WireError> {
    t.source.columns[i].ty.as_ref().copied().map_err(|_| err())
}
fn regclass(
    snapshot: &Snapshot,
    database: &str,
    search_path: &[String],
    name: &str,
) -> Option<i32> {
    use sqlparser::tokenizer::Whitespace;
    let ts = Tokenizer::new(&PostgreSqlDialect {}, name)
        .tokenize()
        .ok()?
        .into_iter()
        .filter(|t| {
            !matches!(
                t,
                Token::Whitespace(Whitespace::Space | Whitespace::Newline | Whitespace::Tab)
            )
        })
        .collect::<Vec<_>>();
    let ident = |w: &sqlparser::tokenizer::Word| {
        if w.quote_style.is_some() {
            w.value.clone()
        } else {
            w.value.to_ascii_lowercase()
        }
    };
    let parts = match ts.as_slice() {
        [Token::Word(w)] => vec![ident(w)],
        [Token::Word(ns), Token::Period, Token::Word(n)] => vec![ident(ns), ident(n)],
        _ => return None,
    };
    snapshot
        .tables
        .iter()
        .find(|t| {
            t.catalog == database
                && match parts.as_slice() {
                    [n] => {
                        t.name == *n
                            && search_path
                                .iter()
                                .find(|ns| {
                                    snapshot.tables.iter().any(|x| {
                                        x.catalog == database && x.schema == **ns && x.name == *n
                                    })
                                })
                                .is_some_and(|ns| *ns == t.schema)
                    }
                    [ns, n] => t.schema == *ns && t.name == *n,
                    _ => false,
                }
        })
        .map(|t| t.oid)
}
pub(crate) fn query(
    snapshot: &Snapshot,
    database: &str,
    search_path: &[String],
    sql: &str,
    params: &[Expr],
) -> Result<Option<Response>, WireError> {
    fn parameter_bytes(e: &Expr) -> usize {
        match &e.kind {
            ExprKind::Cast { expr, .. } => parameter_bytes(expr),
            ExprKind::Str(s) | ExprKind::Number(s) => s.len(),
            _ => 8,
        }
    }
    if params.iter().map(parameter_bytes).sum::<usize>() > 16_384 && recognizes(sql) {
        return Err(WireError::new(
            "54000",
            "metadata predicate budget exceeded",
        ));
    }
    let Some((shape, args)) = identify(sql, params)? else {
        return Ok(None);
    };
    let arg = |i: usize| args.get(i).and_then(|a| a.as_deref());
    if matches!(shape.id, 5..=11 | 31 | 32)
        && let Some(v) = arg(0).filter(|v| !v.is_empty())
        && v.parse::<u32>().is_err()
    {
        return Err(WireError::new(
            if v.bytes().all(|c| c.is_ascii_digit()) {
                "22003"
            } else {
                "22P02"
            },
            "invalid catalog OID predicate",
        ));
    }

    if shape.id == 62
        && arg(0).is_some_and(|n| {
            !n.is_empty() && regclass(snapshot, database, search_path, n).is_none()
        })
    {
        return Err(WireError::new("42P01", "catalog relation does not exist"));
    }
    let has_tables = match shape.id {
        47 | 48 => table_kinds(arg(0))?,
        59 => table_kinds(arg(1))?,
        _ => false,
    };
    let tables: Vec<_> = snapshot
        .tables
        .iter()
        .filter(|t| t.catalog == database)
        .collect();
    // Reserve query scratch for reference vectors, predicate matching and one row.
    let mut rows = Rows {
        bytes: 1024 * 1024 + tables.len().saturating_mul(32),
        ..Rows::default()
    };
    let visible = |t: &Table| {
        search_path
            .iter()
            .find(|ns| tables.iter().any(|x| x.schema == **ns && x.name == t.name))
            .is_some_and(|ns| *ns == t.schema)
    };
    match shape.id {
        0..=2 | 4 | 28 | 47 | 59 => {
            for t in &tables {
                let take = match shape.id {
                    0 => visible(t) && !t.schema.starts_with("pg_toast"),
                    2 => arg(0)
                        .map(|p| regex_name(&t.schema, p))
                        .transpose()?
                        .unwrap_or(false),
                    4 => {
                        arg(0)
                            .map(|p| regex_name(&t.name, p))
                            .transpose()?
                            .unwrap_or(false)
                            && arg(1)
                                .map(|p| regex_name(&t.schema, p))
                                .transpose()?
                                .unwrap_or(false)
                    }
                    28 => arg(0).is_some_and(|p| like(&t.name, p)),
                    47 => has_tables,
                    59 => arg(0) == Some(t.schema.as_str()) && has_tables,
                    _ => true,
                };
                if !take {
                    continue;
                }
                rows.push(match shape.id {
                    4 => vec![s(t.oid), s(&t.schema), s(&t.name)],
                    28 => vec![
                        s(database),
                        s(&t.schema),
                        s(&t.name),
                        s(if t.schema == "pg_toast" {
                            "SYSTEM TOAST TABLE"
                        } else if t.schema.starts_with("pg_") {
                            "TEMPORARY TABLE"
                        } else {
                            "TABLE"
                        }),
                        None,
                        s(""),
                        s(""),
                        s(""),
                        s(""),
                        s(""),
                    ],
                    47 => vec![s(&t.schema), s(&t.name)],
                    59 => vec![s(&t.name), s("table")],
                    _ => vec![s(&t.schema), s(&t.name), s("table"), s("nata")],
                });
            }
            let keys = if shape.id == 4 {
                vec![1, 2]
            } else if shape.id == 28 {
                vec![3, 1, 2]
            } else {
                vec![0, 1]
            };
            if shape.id != 59 {
                rows.rows.sort_by(|a, b| {
                    keys.iter()
                        .map(|i| a[*i].cmp(&b[*i]))
                        .find(|c| !c.is_eq())
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
        }
        5 => {
            for t in &tables {
                if arg(0).and_then(|a| a.parse().ok()) == Some(t.oid) {
                    rows.push(vec![
                        s(0),
                        s("r"),
                        boolean(false),
                        boolean(false),
                        boolean(false),
                        boolean(false),
                        boolean(false),
                        boolean(false),
                        boolean(false),
                        s(""),
                        s(0),
                        s(""),
                        s("p"),
                        s("n"),
                        None,
                    ]);
                }
            }
        }
        6 | 30 | 48 | 60 | 62 => {
            let mut ordered = tables.clone();
            if shape.id == 30 || shape.id == 48 {
                ordered.sort_by_key(|t| (&t.schema, &t.name));
            }
            for t in ordered {
                let take = match shape.id {
                    6 => arg(0).and_then(|a| a.parse().ok()) == Some(t.oid),
                    30 => arg(0).is_some_and(|p| like(&t.name, p)),
                    48 => has_tables,
                    60 => {
                        arg(0).is_some_and(|p| like(&t.schema, p))
                            && arg(1).is_some_and(|p| like(&t.name, p))
                    }
                    62 => {
                        arg(0).and_then(|n| regclass(snapshot, database, search_path, n))
                            == Some(t.oid)
                    }
                    _ => false,
                };
                if !take {
                    continue;
                }
                for (i, c) in t.source.columns.iter().enumerate() {
                    if shape.id == 30 && !arg(1).is_some_and(|p| like(&c.name, p)) {
                        continue;
                    }
                    let ty = column_type(t, i)?;
                    let (oid, len, modif, name, display, _, _) = type_data(ty);
                    let formatted = if let SqlType::Decimal { precision, scale } = ty {
                        format!("numeric({precision},{scale})")
                    } else {
                        display.to_string()
                    };
                    rows.push(match shape.id {
                        6 => vec![
                            s(&c.name),
                            s(formatted),
                            None,
                            boolean(false),
                            None,
                            s(""),
                            s(""),
                        ],
                        30 => vec![
                            s(database),
                            s(&t.schema),
                            s(&t.name),
                            s(&c.name),
                            s(oid),
                            boolean(false),
                            s(modif),
                            s(len),
                            s(-1),
                            s(i + 1),
                            None,
                            None,
                            None,
                            None,
                            s(0),
                            s("b"),
                        ],
                        48 => vec![
                            s(&t.schema),
                            s(&t.name),
                            s(&c.name),
                            s(display),
                            boolean(false),
                            None,
                        ],
                        60 => vec![s(&c.name), s(i + 1), None, s(name)],
                        62 => vec![s(&c.name), s(oid)],
                        _ => unreachable!(),
                    });
                }
            }
        }
        20 | 27 | 46 | 58 => {
            let mut names = snapshot
                .schemas
                .iter()
                .filter(|n| n.catalog == database)
                .map(|n| n.name.clone())
                .collect::<Vec<_>>();
            if shape.id == 27 || shape.id == 46 {
                names.extend(["pg_catalog".into(), "information_schema".into()]);
            }
            names.retain(|n| match shape.id {
                20 | 58 => !n.starts_with("pg_"),
                27 => {
                    let schemas = effective_schemas(snapshot, database, search_path);
                    let first = schemas.first().map_or("", String::as_str);
                    n != "pg_toast"
                        && (!n.starts_with("pg_temp_") || n == first)
                        && (!n.starts_with("pg_toast_temp_")
                            || *n == first.replace("pg_temp_", "pg_toast_temp_"))
                }
                _ => true,
            });
            names.sort();
            names.dedup();
            for n in names {
                rows.push(match shape.id {
                    20 => vec![s(n), s("nata")],
                    27 => vec![s(n), s(database)],
                    _ => vec![s(n)],
                });
            }
        }
        22 | 26 | 51 | 57 | 65 => {
            let mut names = snapshot
                .catalogs
                .iter()
                .map(|c| &c.name)
                .collect::<Vec<_>>();
            if shape.id != 57 {
                names.sort();
            }
            for n in names {
                rows.push(if shape.id == 65 {
                    vec![
                        s(n),
                        s("nata"),
                        s("UTF8"),
                        None,
                        s("C"),
                        s("C"),
                        None,
                        None,
                        None,
                    ]
                } else if shape.id == 22 {
                    vec![s(n), s("nata"), s("UTF8"), s("C"), s("C"), None, None, None]
                } else {
                    vec![s(n)]
                });
            }
        }
        29 => rows.push(vec![s(database)]),
        45 => {
            for name in effective_schemas(snapshot, database, search_path) {
                rows.push(vec![s(name)]);
            }
        }
        31 | 32 | 35 | 36 | 42 => {
            // The ten supported scalar wire types, not PostgreSQL's unrelated storage types.
            let mut types = vec![
                (16, "bool"),
                (20, "int8"),
                (21, "int2"),
                (23, "int4"),
                (25, "text"),
                (700, "float4"),
                (701, "float8"),
                (1043, "varchar"),
                (1082, "date"),
                (1700, "numeric"),
            ];
            if shape.id == 32 || shape.id == 36 {
                types.sort_by_key(|(oid, _)| std::cmp::Reverse(*oid));
            }
            for (oid, name) in types {
                if (shape.id == 31 || shape.id == 32)
                    && arg(0).and_then(|n| n.parse::<i32>().ok()) != Some(oid)
                {
                    continue;
                }
                rows.push(match shape.id {
                    31 => vec![boolean(true), s("pg_catalog"), s(name)],
                    32 | 36 => vec![boolean(false), s("b"), s(name), s(oid)],
                    35 => vec![s(name), s(oid)],
                    _ => vec![s("pg_catalog"), s(name), s(oid), s(-1)],
                });
            }
        }
        40 | 41 => {
            for t in &tables {
                if arg(0) != Some(t.schema.as_str()) {
                    continue;
                }
                if shape.id == 40 {
                    rows.push(vec![
                        s(database),
                        s(&t.schema),
                        s(&t.name),
                        s("BASE TABLE"),
                        None,
                        None,
                        None,
                        None,
                        None,
                        s("NO"),
                        s("NO"),
                        None,
                    ]);
                } else {
                    for (i, c) in t.source.columns.iter().enumerate() {
                        let ty = column_type(t, i)?;
                        let (_, _, _, name, display, p, scale) = type_data(ty);
                        let mut row = vec![None; 44];
                        for (j, v) in [
                            (0, s(database)),
                            (1, s(&t.schema)),
                            (2, s(&t.name)),
                            (3, s(&c.name)),
                            (4, s(i + 1)),
                            (6, s("YES")),
                            (7, s(display)),
                            (
                                9,
                                if name == "text" {
                                    s(1_073_741_824)
                                } else {
                                    None
                                },
                            ),
                            (10, p.map(|p| p.to_string())),
                            (
                                11,
                                p.map(|_| {
                                    if matches!(ty, SqlType::Decimal { .. }) {
                                        "10"
                                    } else {
                                        "2"
                                    }
                                    .into()
                                }),
                            ),
                            (12, scale.map(|n| n.to_string())),
                            (13, (ty == SqlType::Date).then(|| "0".into())),
                            (25, s(database)),
                            (26, s("pg_catalog")),
                            (27, s(name)),
                            (32, s(i + 1)),
                            (33, s("NO")),
                            (34, s("NO")),
                            (40, s("NO")),
                            (41, s("NEVER")),
                            (43, s("NO")),
                        ] {
                            row[j] = v;
                        }
                        rows.push(row);
                    }
                }
            }
        }
        // These exact shapes read capabilities absent from the immutable registry.
        // Do not implement their general array/window/subquery constructs or infer emptiness for other SQL.
        7..=11 | 21 | 33 | 34 | 37 | 49 | 50 | 52 | 61 | 66 => {}
        _ => return Err(err()),
    }
    let fields = shape
        .fields
        .iter()
        .map(|(n, oid, _, _)| {
            (
                (*n).into(),
                match oid {
                    16 => SqlType::Boolean,
                    20 => SqlType::BigInt,
                    21 => SqlType::SmallInt,
                    23 | 26 => SqlType::Integer,
                    _ => SqlType::Varchar,
                },
            )
        })
        .collect();
    let wire_types = shape
        .fields
        .iter()
        .map(|(_, o, l, m)| (*o, *l, *m))
        .collect();
    if rows.exceeded {
        return Err(WireError::new("54000", "metadata result budget exceeded"));
    }
    let rows = rows.rows;
    if rows.iter().any(|r| r.len() != shape.fields.len()) {
        return Err(WireError::new(
            "XX000",
            "catalog result descriptor mismatch",
        ));
    }
    Ok(Some(Response {
        fields,
        wire_types,
        command: format!("SELECT {}", rows.len()),
        rows,
        status: vec![],
        deallocate: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> Snapshot {
        use nata_sql::source::{SourceColumn, SourceInfo};
        let mut snapshot = Snapshot::default();
        snapshot.catalogs.push(crate::catalog::Catalog {
            name: "nata".into(),
            oid: 16384,
        });
        for (ns, oid) in [
            ("public", 16385),
            ("other", 16386),
            ("empty", 16387),
            ("Quoted Schema", 16388),
        ] {
            snapshot.schemas.push(crate::catalog::Schema {
                catalog: "nata".into(),
                name: ns.into(),
                oid,
            });
        }
        for (ns, name, oid) in [
            ("public", "all_types", 16389),
            ("other", "all_types", 16390),
            ("Quoted Schema", "Mixed Table", 16391),
        ] {
            snapshot.tables.push(Table {
                catalog: "nata".into(),
                schema: ns.into(),
                name: name.into(),
                oid,
                schema_oid: 16385,
                source: SourceInfo {
                    pattern: String::new(),
                    files: vec![],
                    columns: [
                        ("id", SqlType::Integer),
                        (
                            "Mixed Column",
                            SqlType::Decimal {
                                precision: 10,
                                scale: 2,
                            },
                        ),
                    ]
                    .into_iter()
                    .map(|(n, t)| SourceColumn {
                        name: n.into(),
                        kind: Ok(nata_parquet::schema::Kind::Utf8),
                        ty: Ok(t),
                    })
                    .collect(),
                },
            });
        }
        snapshot
    }
    fn run(id: usize, params: &[Expr]) -> Response {
        let snap = snapshot();
        query(
            &snap,
            "nata",
            &["public".into()],
            SHAPES.iter().find(|s| s.id == id).expect("shape").sql,
            params,
        )
        .unwrap_or_else(|_| panic!("query {id}"))
        .expect("response")
    }
    fn param(v: &str) -> Expr {
        Expr::new(ExprKind::Str(v.into()), nata_sql::ast::Span::NONE)
    }
    #[test]
    fn captured_shapes_are_total_and_keep_descriptors() {
        for shape in SHAPES {
            let r = query(
                &snapshot(),
                "nata",
                &["public".into()],
                shape.sql,
                &[
                    param(if matches!(shape.id, 2 | 4) {
                        "^(public)$"
                    } else {
                        ""
                    }),
                    param(if shape.id == 4 { "^(public)$" } else { "" }),
                ],
            )
            .unwrap_or_else(|_| panic!("shape {}", shape.id))
            .expect("shape response");
            assert_eq!(r.fields.len(), shape.fields.len());
            assert!(r.rows.iter().all(|r| r.len() == shape.fields.len()));
            assert_eq!(r.description().expect("description")[0], b'T');
        }
    }
    #[test]
    fn snapshot_filters_identity_types_and_ordering() {
        let r = run(0, &[]);
        assert_eq!(
            r.rows,
            vec![vec![s("public"), s("all_types"), s("table"), s("nata")]]
        );
        assert!(run(2, &[param("^(empty)$")]).rows.is_empty());
        assert_eq!(
            run(4, &[param("^(Mixed Table)$"), param("^(Quoted Schema)$")]).rows,
            vec![vec![s(16391), s("Quoted Schema"), s("Mixed Table")]]
        );
        let r = run(6, &[param("16389")]);
        assert_eq!(r.rows[0][1], s("integer"));
        assert_eq!(r.rows[1][1], s("numeric(10,2)"));
        let r = run(30, &[param("%"), param("%")]);
        assert_eq!(r.rows[0][1], s("Quoted Schema"));
        assert_eq!(r.rows[0][4], s(23));
        assert_eq!(r.rows[1][4], s(1700));
        assert_eq!(r.rows[1][6], s(655366));
        let r = run(41, &[param("public")]);
        assert_eq!(r.rows.len(), 2);
        assert_eq!(r.rows[0][3], s("id"));
        assert_eq!(r.rows[1][3], s("Mixed Column"));
        assert_eq!(r.rows[1][10], s(10));
        assert_eq!(r.rows[1][12], s(2));
        assert_eq!(
            run(62, &[param("\"Quoted Schema\".\"Mixed Table\"")]).rows,
            vec![vec![s("id"), s(23)], vec![s("Mixed Column"), s(1700)]]
        );
        assert!(
            query(
                &snapshot(),
                "nata",
                &["public".into()],
                SHAPES.iter().find(|s| s.id == 62).expect("shape").sql,
                &[param("public.missing")]
            )
            .is_err()
        );
        let r = run(42, &[]);
        assert_eq!(
            r.rows.iter().find(|r| r[1] == s("int4")).expect("int4")[2],
            s(23)
        );
        assert_eq!(
            run(45, &[]).rows,
            vec![vec![s("pg_catalog")], vec![s("public")]]
        );
    }
    #[test]
    fn shape_matching_cannot_change_semantics_or_reach_user_tables() {
        let shape = SHAPES.iter().find(|s| s.id == 0).expect("shape");
        for sql in [
            shape.sql.replace("'table'", "'invented'"),
            shape.sql.replace("ORDER BY 1,2", "ORDER BY 2,1"),
            shape.sql.replace("pg_catalog.pg_class", "public.pg_class"),
            format!("{} SELECT 1", shape.sql),
        ] {
            assert!(!recognizes(&sql), "{sql}");
        }
        assert!(run(40, &[param("public' OR true --")]).rows.is_empty());
        assert!(
            query(
                &snapshot(),
                "nata",
                &[],
                "SELECT * FROM public.all_types",
                &[]
            )
            .unwrap_or_else(|_| panic!("not catalog"))
            .is_none()
        );
        assert!(!like("axb", "a\\_b"));
        assert!(like("a_b", "a\\_b"));
        assert!(like("日本語", "日%"));
        assert!(table_kinds(Some("{r,p}")).unwrap_or_else(|_| panic!("kinds")));
        assert!(!table_kinds(Some("{v,m}")).unwrap_or_else(|_| panic!("kinds")));
        assert!(table_kinds(Some(r#"{"r,x"}"#)).is_err());
    }
}
