//! Checked hand codec derived from the slice-4b bake-off. No allocation precedes the frame cap.
use nata_sql::types::SqlType;

pub const MAX_STARTUP: usize = 10_000;
pub const MAX_FRAME: usize = 1_114_112;
pub const MAX_SQL: usize = 1 << 20;

#[derive(Debug, PartialEq, Eq)]
pub enum Message {
    Startup(Vec<(String, String)>),
    Encryption,
    Cancel(i32, i32),
    Query(String),
    Parse {
        name: String,
        sql: String,
        types: Vec<i32>,
    },
    Bind {
        portal: String,
        statement: String,
        formats: Vec<i16>,
        params: Vec<Option<String>>,
        results: Vec<i16>,
    },
    Describe(u8, String),
    Close(u8, String),
    Execute(String, i32),
    Sync,
    Flush,
    Terminate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invalid;
impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("malformed protocol message")
    }
}
impl std::error::Error for Invalid {}
type Result<T> = std::result::Result<T, Invalid>;

/// Validate the fixed header BEFORE allocating or reading a body.
pub fn length(bytes: [u8; 4], startup: bool) -> Result<usize> {
    let n = i32::from_be_bytes(bytes);
    let min = if startup { 8 } else { 4 };
    let max = if startup { MAX_STARTUP } else { MAX_FRAME };
    if n < min || n as usize > max {
        return Err(Invalid);
    }
    Ok(n as usize)
}
struct Cursor<'a>(&'a [u8]);
impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.0.len() {
            return Err(Invalid);
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_be_bytes(
            self.take(2)?.try_into().map_err(|_| Invalid)?,
        ))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(
            self.take(4)?.try_into().map_err(|_| Invalid)?,
        ))
    }
    fn string(&mut self) -> Result<String> {
        let n = self.0.iter().position(|b| *b == 0).ok_or(Invalid)?;
        let s = std::str::from_utf8(self.take(n)?)
            .map_err(|_| Invalid)?
            .to_owned();
        self.take(1)?;
        Ok(s)
    }
    fn sql(&mut self) -> Result<String> {
        let n = self.0.iter().position(|b| *b == 0).ok_or(Invalid)?;
        if n > MAX_SQL {
            return Err(Invalid);
        }
        self.string()
    }
    fn count(&mut self, width: usize) -> Result<usize> {
        let n = self.i16()?;
        if n < 0 || n as usize > self.0.len() / width {
            return Err(Invalid);
        }
        Ok(n as usize)
    }
    fn formats(&mut self) -> Result<Vec<i16>> {
        let n = self.count(2)?;
        (0..n).map(|_| self.i16()).collect()
    }
}
/// Decode exactly one complete frame. Unsupported features remain represented for the session layer to reject.
pub fn decode(bytes: &[u8], startup: bool) -> Result<Message> {
    let off = usize::from(!startup);
    let h = bytes.get(off..off + 4).ok_or(Invalid)?;
    let n = length(h.try_into().map_err(|_| Invalid)?, startup)?;
    if bytes.len() != n + off {
        return Err(Invalid);
    }
    let mut c = Cursor(&bytes[off + 4..]);
    let msg = if startup {
        match c.i32()? {
            196608 => {
                let mut params = Vec::new();
                while c.0.first().is_some_and(|b| *b != 0) {
                    params.push((c.string()?, c.string()?));
                }
                c.take(1)?;
                Message::Startup(params)
            }
            80877103 | 80877104 => Message::Encryption,
            80877102 => Message::Cancel(c.i32()?, c.i32()?),
            _ => return Err(Invalid),
        }
    } else {
        match bytes[0] {
            b'Q' => Message::Query(c.sql()?),
            b'P' => {
                let name = c.string()?;
                let sql = c.sql()?;
                let n = c.count(4)?;
                let types = (0..n).map(|_| c.i32()).collect::<Result<_>>()?;
                Message::Parse { name, sql, types }
            }
            b'B' => {
                let portal = c.string()?;
                let statement = c.string()?;
                let formats = c.formats()?;
                let n = c.count(4)?;
                let params = (0..n)
                    .map(|i| match c.i32()? {
                        -1 => Ok(None),
                        n if n >= 0 => {
                            let bytes = c.take(n as usize)?;
                            let format = if formats.len() == 1 {
                                formats[0]
                            } else {
                                formats.get(i).copied().unwrap_or(0)
                            };
                            // Binary payload is opaque; the session rejects its format with 0A000.
                            Ok(Some(if format != 0 {
                                String::new()
                            } else {
                                std::str::from_utf8(bytes).map_err(|_| Invalid)?.to_owned()
                            }))
                        }
                        _ => Err(Invalid),
                    })
                    .collect::<Result<_>>()?;
                let results = c.formats()?;
                Message::Bind {
                    portal,
                    statement,
                    formats,
                    params,
                    results,
                }
            }
            b'D' | b'C' => {
                let target = c.take(1)?[0];
                if !matches!(target, b'S' | b'P') {
                    return Err(Invalid);
                }
                let name = c.string()?;
                if bytes[0] == b'D' {
                    Message::Describe(target, name)
                } else {
                    Message::Close(target, name)
                }
            }
            b'E' => {
                let portal = c.string()?;
                let n = c.i32()?;
                if n < 0 {
                    return Err(Invalid);
                }
                Message::Execute(portal, n)
            }
            b'S' => Message::Sync,
            b'H' => Message::Flush,
            b'X' => Message::Terminate,
            _ => return Err(Invalid),
        }
    };
    if !c.0.is_empty() {
        return Err(Invalid);
    }
    Ok(msg)
}

pub fn packet(tag: u8, body: &[u8]) -> Result<Vec<u8>> {
    let n = body
        .len()
        .checked_add(4)
        .and_then(|n| i32::try_from(n).ok())
        .ok_or(Invalid)?;
    let mut bytes = Vec::with_capacity(body.len() + 5);
    bytes.push(tag);
    bytes.extend(n.to_be_bytes());
    bytes.extend(body);
    Ok(bytes)
}
/// OID, fixed size and typmod from ADR 0008.
pub fn type_info(ty: SqlType) -> (i32, i16, i32) {
    use SqlType::*;
    match ty {
        Boolean => (16, 1, -1),
        TinyInt | Varchar | Null => (25, -1, -1),
        SmallInt => (21, 2, -1),
        Integer => (23, 4, -1),
        BigInt => (20, 8, -1),
        Float => (700, 4, -1),
        Double => (701, 8, -1),
        Date => (1082, 4, -1),
        Decimal { precision, scale } => (
            1700,
            -1,
            4 + (i32::from(precision) << 16) + i32::from(scale),
        ),
    }
}
pub fn description(fields: &[(String, SqlType)]) -> Result<Vec<u8>> {
    let mut b = i16::try_from(fields.len())
        .map_err(|_| Invalid)?
        .to_be_bytes()
        .to_vec();
    for (name, ty) in fields {
        if name.contains('\0') {
            return Err(Invalid);
        }
        b.extend(name.as_bytes());
        b.push(0);
        b.extend(0i32.to_be_bytes());
        b.extend(0i16.to_be_bytes());
        let (oid, size, modifier) = type_info(*ty);
        b.extend(oid.to_be_bytes());
        b.extend(size.to_be_bytes());
        b.extend(modifier.to_be_bytes());
        b.extend(0i16.to_be_bytes());
    }
    packet(b'T', &b)
}
pub fn row(values: &[Option<String>]) -> Result<Vec<u8>> {
    let mut b = i16::try_from(values.len())
        .map_err(|_| Invalid)?
        .to_be_bytes()
        .to_vec();
    for v in values {
        match v {
            None => b.extend((-1i32).to_be_bytes()),
            Some(s) => {
                if s.contains('\0') {
                    return Err(Invalid);
                }
                b.extend(i32::try_from(s.len()).map_err(|_| Invalid)?.to_be_bytes());
                b.extend(s.as_bytes());
            }
        }
    }
    packet(b'D', &b)
}
pub fn error(code: &str, message: &str, position: Option<usize>) -> Vec<u8> {
    let mut b = format!("SERROR\0VERROR\0C{code}\0M{message}\0").into_bytes();
    if let Some(p) = position {
        b.extend(format!("P{p}\0").as_bytes());
    }
    b.push(0);
    packet(b'E', &b).expect("bounded server error")
}
/// PostgreSQL positions count characters from the start of the whole query, including newlines.
pub fn position(sql: &str, pos: nata_sql::Pos) -> usize {
    let mut at = nata_sql::Pos::START;
    for (i, ch) in sql.chars().enumerate() {
        if at >= pos {
            return i + 1;
        }
        if ch == '\n' {
            at.line += 1;
            at.col = 1;
        } else {
            at.col += 1;
        }
    }
    sql.chars().count() + 1
}
