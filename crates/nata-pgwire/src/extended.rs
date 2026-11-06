//! Bounded unnamed statement/portal state. Values enter the planner as literal nodes.
use crate::{codec, server::WireError};
use nata_sql::ast::{DataType, Expr, ExprKind, Span, UnOp};
use nata_sql::{Pos, types::SqlType};
use sqlparser::{
    dialect::PostgreSqlDialect,
    tokenizer::{Token, Tokenizer},
};

#[derive(Clone)]
pub(crate) struct Statement {
    pub sql: String,
    pub types: Vec<i32>,
    pub description: Vec<u8>,
}
#[derive(Clone)]
pub(crate) struct Portal {
    pub statement: Statement,
    pub params: Vec<Expr>,
}
impl Statement {
    pub fn new(name: &str, sql: String, mut types: Vec<i32>) -> Result<Self, WireError> {
        unnamed(name)?;
        let tokens = Tokenizer::new(&PostgreSqlDialect {}, &sql)
            .tokenize()
            .map_err(|_| WireError::new("42601", "invalid query tokens"))?;
        let mut count = 0;
        for t in tokens {
            if let Token::Placeholder(s) = t {
                let n = s
                    .strip_prefix('$')
                    .and_then(|s| s.parse::<usize>().ok())
                    .filter(|n| (1..=1024).contains(n))
                    .ok_or_else(|| WireError::unsupported("parameter index outside 1..=1024"))?;
                count = count.max(n);
            }
        }
        if types.len() > count {
            return Err(WireError::new("08P01", "too many parameter types"));
        }
        types.resize(count, 0);
        for t in &mut types {
            if *t == 0 {
                *t = 25;
            }
            literal(*t, None)?;
        }
        Ok(Self {
            sql,
            types,
            description: Vec::new(),
        })
    }
    pub fn bind(&self, params: &[Option<String>]) -> Result<Portal, WireError> {
        if params.len() != self.types.len() {
            return Err(WireError::new("08P01", "parameter count mismatch"));
        }
        // Repeated references are AST clones; cap their aggregate source budget as well.
        let expansion = params
            .iter()
            .flatten()
            .map(String::len)
            .sum::<usize>()
            .saturating_mul(self.sql.matches('$').count().max(1));
        if expansion.saturating_add(self.sql.len()) > codec::MAX_SQL {
            return Err(WireError::unsupported(
                "parameter expansion exceeds SQL limit",
            ));
        }
        Ok(Portal {
            statement: self.clone(),
            params: self
                .types
                .iter()
                .zip(params)
                .map(|(t, p)| literal(*t, p.as_deref()))
                .collect::<Result<_, _>>()?,
        })
    }
    pub fn dummy(&self) -> Result<Vec<Expr>, WireError> {
        self.types
            .iter()
            .map(|t| {
                literal(
                    *t,
                    Some(match t {
                        16 => "false",
                        25 | 1043 => "",
                        1082 => "2000-01-01",
                        _ => "0",
                    }),
                )
            })
            .collect()
    }
}
pub(crate) fn unnamed(name: &str) -> Result<(), WireError> {
    if name.is_empty() {
        Ok(())
    } else {
        Err(WireError::unsupported(
            "named statements/portals; pgjdbc: prepareThreshold=0; psycopg: prepare_threshold=None",
        ))
    }
}
pub(crate) fn formats(formats: &[i16], count: usize) -> Result<(), WireError> {
    if formats.iter().any(|f| *f != 0) {
        return Err(WireError::unsupported(
            "binary parameters/results; use text format",
        ));
    }
    if !formats.is_empty() && formats.len() != 1 && formats.len() != count {
        return Err(WireError::new("08P01", "format count mismatch"));
    }
    Ok(())
}
fn literal(oid: i32, value: Option<&str>) -> Result<Expr, WireError> {
    let target = match oid {
        0 => None,
        16 => Some(DataType::Boolean),
        20 => Some(DataType::BigInt),
        21 | 23 => Some(DataType::Integer),
        700 | 701 => Some(DataType::Double),
        25 | 1043 => Some(DataType::Varchar),
        1082 => Some(DataType::Date),
        1700 => None,
        _ => return Err(WireError::unsupported("parameter type OID")),
    };
    let mk = |k| Expr::new(k, Span(Pos::START));
    let Some(v) = value else {
        return Ok(if let Some(ty) = target {
            mk(ExprKind::Cast {
                expr: Box::new(mk(ExprKind::Null)),
                ty,
            })
        } else {
            mk(ExprKind::Null)
        });
    };
    if v.contains('\0') {
        return Err(WireError::new("22P02", "invalid text parameter"));
    }
    if matches!(oid, 20 | 21 | 23) {
        let n = v.parse::<i64>().map_err(|_| {
            WireError::new(
                if v.trim_start_matches('-')
                    .bytes()
                    .all(|c| c.is_ascii_digit())
                {
                    "22003"
                } else {
                    "22P02"
                },
                "invalid integer parameter",
            )
        })?;
        if (oid == 21 && i16::try_from(n).is_err()) || (oid == 23 && i32::try_from(n).is_err()) {
            return Err(WireError::new("22003", "integer parameter out of range"));
        }
    }
    let node = match oid {
        25 | 1043 => mk(ExprKind::Str(v.into())),
        1082 => mk(ExprKind::Cast {
            expr: Box::new(mk(ExprKind::Str(v.into()))),
            ty: DataType::Date,
        }),
        16 => match v.to_ascii_lowercase().as_str() {
            "true" | "t" | "1" => mk(ExprKind::Bool(true)),
            "false" | "f" | "0" => mk(ExprKind::Bool(false)),
            _ => return Err(WireError::new("22P02", "invalid boolean parameter")),
        },
        _ => {
            let tokens = Tokenizer::new(&PostgreSqlDialect {}, v)
                .tokenize()
                .map_err(|_| WireError::new("22P02", "invalid numeric parameter"))?;
            let tokens: Vec<_> = tokens
                .into_iter()
                .filter(|t| !matches!(t, Token::Whitespace(_)))
                .collect();
            let (negative, number) = match tokens.as_slice() {
                [Token::Number(n, false)] => (false, n),
                [Token::Minus, Token::Number(n, false)] => (true, n),
                _ => {
                    return Err(WireError::new(
                        "22P02",
                        "parameter must be a numeric dialect literal",
                    ));
                }
            };
            let n = mk(ExprKind::Number(number.clone()));
            if negative {
                mk(ExprKind::Unary {
                    op: UnOp::Neg,
                    expr: Box::new(n),
                })
            } else {
                n
            }
        }
    };
    Ok(if let Some(ty) = target {
        mk(ExprKind::Cast {
            expr: Box::new(node),
            ty,
        })
    } else {
        node
    })
}
pub(crate) fn fields(p: &nata_sql::Planned) -> Vec<(String, SqlType)> {
    if p.mode != nata_sql::ast::ExplainMode::No {
        vec![("QUERY PLAN".into(), SqlType::Varchar)]
    } else {
        p.output.iter().map(|c| (c.name.clone(), c.ty)).collect()
    }
}

/// Ignore labels, compare every wire type/size/typmod/format before delivering rows.
pub(crate) fn same_types(a: &[u8], b: &[u8]) -> bool {
    fn columns(mut b: &[u8]) -> Option<Vec<&[u8]>> {
        b = b.get(5..)?;
        let n = i16::from_be_bytes(b.get(..2)?.try_into().ok()?);
        b = &b[2..];
        let mut out = Vec::new();
        for _ in 0..n {
            let at = b.iter().position(|c| *c == 0)? + 1;
            b = b.get(at..)?;
            out.push(b.get(..18)?);
            b = b.get(18..)?;
        }
        Some(out)
    }
    columns(a) == columns(b)
}
