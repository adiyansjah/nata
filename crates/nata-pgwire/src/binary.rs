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
        23 => s
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
        25 | 1043 => s.as_bytes().to_vec(),
        1082 => {
            let date = NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| invalid())?;
            (date.signed_duration_since(epoch()).num_days() as i32)
                .to_be_bytes()
                .to_vec()
        }
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
            let weight = (whole.len().div_ceil(4) as i16) - 1;
            let mut out = Vec::new();
            for n in [
                (digits.len() / 4) as i16,
                weight,
                if negative { 0x4000 } else { 0 },
                frac.len() as i16,
            ] {
                out.extend(n.to_be_bytes());
            }
            for d in digits.as_bytes().chunks(4) {
                out.extend(
                    std::str::from_utf8(d)
                        .map_err(|_| invalid())?
                        .parse::<i16>()
                        .map_err(|_| invalid())?
                        .to_be_bytes(),
                );
            }
            out
        }
        _ => return Err(WireError::unsupported("binary result type OID")),
    })
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
