//! PostgreSQL binary scalars at the wire boundary. Query evaluation still uses typed literals.
use crate::{codec, server::WireError};
use chrono::NaiveDate;
fn invalid() -> WireError {
    WireError::new("22P03", "invalid binary parameter")
}
fn epoch() -> NaiveDate {
    NaiveDate::from_ymd_opt(2000, 1, 1).expect("epoch")
}
fn format(fs: &[i16], i: usize) -> i16 {
    if fs.len() == 1 {
        fs[0]
    } else {
        fs.get(i).copied().unwrap_or(0)
    }
}
pub(crate) fn parameters(
    types: &[i32],
    fs: &[i16],
    ps: &[Option<Vec<u8>>],
) -> Result<Vec<Option<String>>, WireError> {
    if types.len() != ps.len() {
        return Err(WireError::new("08P01", "parameter count mismatch"));
    }
    types
        .iter()
        .zip(ps)
        .enumerate()
        .map(|(i, (ty, p))| {
            p.as_ref()
                .map(|b| {
                    if format(fs, i) == 0 {
                        return String::from_utf8(b.clone())
                            .map_err(|_| WireError::new("22P02", "invalid text parameter"));
                    }
                    Ok(match ty {
                        16 => match b.as_slice() {
                            [0] => "false".into(),
                            [1] => "true".into(),
                            _ => return Err(invalid()),
                        },
                        21 => i16::from_be_bytes(b.as_slice().try_into().map_err(|_| invalid())?)
                            .to_string(),
                        23 => i32::from_be_bytes(b.as_slice().try_into().map_err(|_| invalid())?)
                            .to_string(),
                        20 => i64::from_be_bytes(b.as_slice().try_into().map_err(|_| invalid())?)
                            .to_string(),
                        700 => f32::from_be_bytes(b.as_slice().try_into().map_err(|_| invalid())?)
                            .to_string(),
                        701 => f64::from_be_bytes(b.as_slice().try_into().map_err(|_| invalid())?)
                            .to_string(),
                        25 | 1043 => String::from_utf8(b.clone()).map_err(|_| invalid())?,
                        1082 => {
                            let days =
                                i32::from_be_bytes(b.as_slice().try_into().map_err(|_| invalid())?);
                            epoch()
                                .checked_add_signed(chrono::Duration::days(i64::from(days)))
                                .ok_or_else(invalid)?
                                .to_string()
                        }
                        _ => return Err(WireError::unsupported("binary parameter type OID")),
                    })
                })
                .transpose()
        })
        .collect()
}
fn encode(ty: i32, s: &str) -> Result<Vec<u8>, WireError> {
    Ok(match ty {
        16 => vec![u8::from(s == "t")],
        21 => s
            .parse::<i16>()
            .map_err(|_| invalid())?
            .to_be_bytes()
            .to_vec(),
        24 => {
            let oid: i32 = match s {
                "boolrecv" => 2436,
                "int8recv" => 2408,
                "int2recv" => 2404,
                "int4recv" => 2406,
                "textrecv" => 2414,
                "float4recv" => 2424,
                "float8recv" => 2426,
                "varcharrecv" => 2432,
                "date_recv" => 2468,
                "numeric_recv" => 2460,
                _ => return Err(invalid()),
            };
            oid.to_be_bytes().to_vec()
        }
        // Metadata-only name[] from current_schemas. This does not add array SQL.
        1003 => encode_names(s)?,
        23 | 26 => s
            .parse::<i32>()
            .map_err(|_| invalid())?
            .to_be_bytes()
            .to_vec(),
        20 => s
            .parse::<i64>()
            .map_err(|_| invalid())?
            .to_be_bytes()
            .to_vec(),
        700 => s
            .parse::<f32>()
            .map_err(|_| invalid())?
            .to_be_bytes()
            .to_vec(),
        701 => s
            .parse::<f64>()
            .map_err(|_| invalid())?
            .to_be_bytes()
            .to_vec(),
        18 => match s.as_bytes() {
            [] => vec![0],
            [c] => vec![*c],
            _ => return Err(invalid()),
        },
        19 | 25 | 1043 => s.as_bytes().to_vec(),
        1082 => crate::date::binary(s)?.to_vec(),
        1700 => {
            let negative = s.starts_with('-');
            let s = s.trim_start_matches('-');
            let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
            let digits = format!(
                "{}{}{}{}",
                "0".repeat((4 - whole.len() % 4) % 4),
                whole,
                frac,
                "0".repeat((4 - frac.len() % 4) % 4)
            );
            let mut weight = (whole.len().div_ceil(4) as i16) - 1;
            let mut groups = digits
                .as_bytes()
                .chunks(4)
                .map(|d| {
                    std::str::from_utf8(d)
                        .map_err(|_| invalid())?
                        .parse::<i16>()
                        .map_err(|_| invalid())
                })
                .collect::<Result<Vec<_>, _>>()?;
            while groups.first() == Some(&0) {
                groups.remove(0);
                weight -= 1;
            }
            while groups.last() == Some(&0) {
                groups.pop();
            }
            if groups.is_empty() {
                weight = 0;
            }
            let mut out = Vec::new();
            for n in [
                groups.len() as i16,
                weight,
                if negative && !groups.is_empty() {
                    0x4000
                } else {
                    0
                },
                frac.len() as i16,
            ] {
                out.extend(n.to_be_bytes());
            }
            for d in groups {
                out.extend(d.to_be_bytes());
            }
            out
        }
        _ => return Err(WireError::unsupported("binary result type OID")),
    })
}
// Parse the server-generated PostgreSQL array spelling, preserving quoted names,
// escaped quotes/backslashes and empty arrays. No NULL elements are generated.
fn encode_names(s: &str) -> Result<Vec<u8>, WireError> {
    let inner = s
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .ok_or_else(invalid)?;
    let mut names = vec![];
    let mut chars = inner.chars().peekable();
    while chars.peek().is_some() {
        let mut name = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            loop {
                match chars.next() {
                    Some('"') => break,
                    Some('\\') => name.push(chars.next().ok_or_else(invalid)?),
                    Some(c) => name.push(c),
                    None => return Err(invalid()),
                }
            }
        } else {
            while chars.peek().is_some_and(|c| *c != ',') {
                name.push(chars.next().expect("peek"));
            }
            if name.is_empty() || name.eq_ignore_ascii_case("null") {
                return Err(invalid());
            }
        }
        names.push(name);
        match chars.next() {
            None => break,
            Some(',') if chars.peek().is_some() => {}
            _ => return Err(invalid()),
        }
    }
    let mut out = vec![];
    let dims = if names.is_empty() { 0i32 } else { 1 };
    for n in [dims, 0, 19] {
        out.extend(n.to_be_bytes());
    }
    if !names.is_empty() {
        out.extend(
            i32::try_from(names.len())
                .map_err(|_| invalid())?
                .to_be_bytes(),
        );
        out.extend(1i32.to_be_bytes());
        for name in names {
            out.extend(
                i32::try_from(name.len())
                    .map_err(|_| invalid())?
                    .to_be_bytes(),
            );
            out.extend(name.as_bytes());
        }
    }
    Ok(out)
}
// Only server-generated, checked packets enter this conversion.
pub(crate) fn packet(mut p: Vec<u8>, desc: &[u8], fs: &[i16]) -> Result<Vec<u8>, WireError> {
    if fs.iter().all(|f| *f == 0) {
        return Ok(p);
    }
    if p[0] != b'D' && p[0] != b'T' {
        return Ok(p);
    }
    let actual = if p[0] == b'T' {
        p.clone()
    } else {
        desc.to_vec()
    };
    let desc = actual.as_slice();
    let mut at = 7;
    let mut types = Vec::new();
    for i in 0..super::server::column_count(desc) {
        at += desc[at..]
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(invalid)?
            + 1;
        let ty = i32::from_be_bytes(desc[at + 6..at + 10].try_into().map_err(|_| invalid())?);
        if p[0] == b'T' {
            p[at + 16..at + 18].copy_from_slice(&format(fs, i).to_be_bytes());
        }
        types.push(ty);
        at += 18;
    }
    if p[0] == b'T' {
        return Ok(p);
    }
    let mut out = p[5..7].to_vec();
    at = 7;
    for (i, ty) in types.into_iter().enumerate() {
        let n = i32::from_be_bytes(p[at..at + 4].try_into().map_err(|_| invalid())?);
        at += 4;
        if n < 0 {
            out.extend(n.to_be_bytes());
            continue;
        }
        let value = &p[at..at + n as usize];
        at += n as usize;
        let value = if format(fs, i) == 1 {
            encode(ty, std::str::from_utf8(value).map_err(|_| invalid())?)?
        } else {
            value.to_vec()
        };
        out.extend((value.len() as i32).to_be_bytes());
        out.extend(value);
    }
    codec::packet(b'D', &out).map_err(|_| invalid())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helpers_name_arrays_match_pg17_send_bytes() {
        // PostgreSQL 17.11: SELECT encode(array_send(current_schemas(...)),'hex')
        // public/default, quoted schema, and empty search_path, respectively.
        for (text, expected) in [
            (
                "{pg_catalog,public}",
                "00000001000000000000001300000002000000010000000a70675f636174616c6f67000000067075626c6963",
            ),
            (
                "{\"Quoted Schema\"}",
                "00000001000000000000001300000001000000010000000d51756f74656420536368656d61",
            ),
            ("{}", "000000000000000000000013"),
        ] {
            let bytes = encode(1003, text).unwrap_or_else(|_| panic!("metadata names"));
            let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(hex, expected);
        }
    }
}
