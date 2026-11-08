//! Finite PostgreSQL dates. Date32 is wider and has no infinity metadata.
use crate::server::WireError;
use nata_parquet::civil::civil_from_days;
const EPOCH: i64 = 10_957; // 1970 -> 2000
// PostgreSQL datetime.h: Julian day 0 inclusive, DATE_END_JULIAN exclusive.
const MIN: i64 = -2_451_545;
const END: i64 = 2_145_031_949;
fn range() -> WireError {
    WireError::new(
        "22008",
        "date outside PostgreSQL range (4713 BC to 5874897 AD)",
    )
}
fn check(days: i64) -> Result<i32, WireError> {
    if !(MIN..END).contains(&days) {
        return Err(range());
    }
    i32::try_from(days).map_err(|_| range())
}
pub(crate) fn text(days: i32) -> Result<String, WireError> {
    check(i64::from(days) - EPOCH)?;
    let (y, m, d) = civil_from_days(i64::from(days));
    Ok(if y <= 0 {
        format!("{:04}-{m:02}-{d:02} BC", 1 - y)
    } else {
        format!("{y:04}-{m:02}-{d:02}")
    })
}
pub(crate) fn binary_days(days: i32) -> Result<[u8; 4], WireError> {
    Ok(check(i64::from(days) - EPOCH)?.to_be_bytes())
}
pub(crate) fn binary(s: &str) -> Result<[u8; 4], WireError> {
    let (date, bc) = s.strip_suffix(" BC").map_or((s, false), |s| (s, true));
    let mut parts = date.split('-');
    let mut number = || {
        parts
            .next()
            .and_then(|s| s.parse::<i64>().ok())
            .ok_or_else(range)
    };
    let mut y = number()?;
    let m = number()?;
    let d = number()?;
    if parts.next().is_some() || !(1..=5_874_897).contains(&y) || !(1..=12).contains(&m) {
        return Err(range());
    }
    if bc {
        y = 1 - y;
    }
    if !(1..=nata_parquet::civil::days_in_month(y, m)).contains(&d) {
        return Err(range());
    }
    let y = y - i64::from(m <= 2);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let era = y.div_euclid(400);
    let yo = y.rem_euclid(400);
    let days = era * 146_097 + yo * 365 + yo / 4 - yo / 100 + (153 * mp + 2) / 5 + d - 1 - 719_468;
    Ok(check(days - EPOCH)?.to_be_bytes())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exceptional_dates_and_postgres_endpoints() {
        for (days, s) in [
            (-735_160, "0044-03-15 BC"),
            (-719_528, "0001-01-01 BC"),
            (2_932_897, "10000-01-01"),
            (-2_440_588, "4714-11-24 BC"),
            (2_145_042_905, "5874897-12-31"),
        ] {
            assert_eq!(text(days).ok().expect("finite"), s);
            assert_eq!(
                binary(s).ok().expect("binary"),
                (days - 10_957).to_be_bytes()
            );
        }
        for days in [i32::MIN, -i32::MAX, -2_440_589, 2_145_042_906, i32::MAX] {
            assert!(text(days).is_err());
        }
        for s in ["infinity", "-infinity", "5874898-01-01", "4714-11-23 BC"] {
            assert!(binary(s).is_err());
        }
    }
}
