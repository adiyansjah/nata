//! Calendar arithmetic for DATE and TIMESTAMP values: days since 1970-01-01 to and from year, month and day (proleptic Gregorian, like DuckDB),
//! and the text forms. Checked arithmetic throughout.

/// Days since 1970-01-01 of a calendar date, or `None` if the date does not exist or its year is outside 1 to 9999.
pub fn days_from_civil(year: i64, month: i64, day: i64) -> Option<i32> {
    if !(1..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
    {
        return None;
    }
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i32::try_from(era * 146_097 + doe - 719_468).ok()
}

pub fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

pub fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap(year) => 29,
        _ => 28,
    }
}

/// (year, month, day) of a number of days since 1970-01-01. Years of 0 and below are astronomical years (year 0 is 1 BC).
pub fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// DuckDB's text of a date: `YYYY-MM-DD` (at least four digits), years before 1 as `YYYY-MM-DD (BC)`, and `infinity` / `-infinity`
/// for its two sentinels.
pub fn format_date(days: i32) -> String {
    if days == i32::MAX {
        return "infinity".into();
    }
    if days == -i32::MAX {
        return "-infinity".into();
    }
    let (y, m, d) = civil_from_days(i64::from(days));
    if y >= 1 {
        format!("{y:04}-{m:02}-{d:02}")
    } else {
        format!("{:04}-{m:02}-{d:02} (BC)", 1 - y)
    }
}

const MICROS_PER_DAY: i64 = 86_400_000_000;

/// DuckDB's text of a timestamp in microseconds: `YYYY-MM-DD HH:MM:SS` and the fraction without trailing zeros.
pub fn format_timestamp_micros(micros: i64) -> String {
    let days = micros.div_euclid(MICROS_PER_DAY);
    let rem = micros.rem_euclid(MICROS_PER_DAY);
    let date = match i32::try_from(days) {
        Ok(d) => format_date(d),
        Err(_) => "out-of-range".into(),
    };
    let (secs, frac) = (rem / 1_000_000, rem % 1_000_000);
    let (h, mi, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    let mut out = format!("{date} {h:02}:{mi:02}:{s:02}");
    if frac != 0 {
        let f = format!("{frac:06}");
        out.push('.');
        out.push_str(f.trim_end_matches('0'));
    }
    out
}

/// `YYYY-MM-DD`, exactly (no other layout).
pub fn parse_date(text: &str) -> Result<i32, String> {
    let b = text.as_bytes();
    let well_formed = b.len() == 10
        && b.get(4) == Some(&b'-')
        && b.get(7) == Some(&b'-')
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
    if !well_formed {
        return Err("a date is written YYYY-MM-DD".into());
    }
    let num = |s: &str| {
        s.parse::<i64>()
            .map_err(|_| "a date is written YYYY-MM-DD".to_string())
    };
    let (y, m, d) = (num(&text[0..4])?, num(&text[5..7])?, num(&text[8..10])?);
    days_from_civil(y, m, d)
        .ok_or_else(|| "this date does not exist (years 0001 to 9999 are accepted)".to_string())
}

/// `YYYY-MM-DD HH:MM:SS` or with `T`, and an optional fraction of 1 to 6 digits (more digits would need rounding, which is never done).
pub fn parse_timestamp_micros(text: &str) -> Result<i64, String> {
    let shape = "a timestamp is written YYYY-MM-DD HH:MM:SS[.ffffff]";
    if text.len() < 19 || !text.is_char_boundary(10) || !text.is_char_boundary(11) {
        return Err(shape.into());
    }
    let (date, rest) = text.split_at(10);
    let days = parse_date(date).map_err(|_| shape.to_string())?;
    let rest = rest
        .strip_prefix([' ', 'T'])
        .ok_or_else(|| shape.to_string())?;
    let (clock, fraction) = match rest.split_once('.') {
        Some((c, f)) => (c, Some(f)),
        None => (rest, None),
    };
    let cb = clock.as_bytes();
    let ok = cb.len() == 8
        && cb.get(2) == Some(&b':')
        && cb.get(5) == Some(&b':')
        && cb
            .iter()
            .enumerate()
            .all(|(i, c)| i == 2 || i == 5 || c.is_ascii_digit());
    if !ok {
        return Err(shape.into());
    }
    let num = |s: &str| s.parse::<i64>().map_err(|_| shape.to_string());
    let (h, m, s) = (num(&clock[0..2])?, num(&clock[3..5])?, num(&clock[6..8])?);
    if h > 23 || m > 59 || s > 59 {
        return Err("the time of day is out of range".into());
    }
    let mut micros = 0i64;
    if let Some(f) = fraction {
        if f.is_empty() || !f.bytes().all(|c| c.is_ascii_digit()) {
            return Err(shape.into());
        }
        if f.len() > 6 {
            return Err("more than 6 fraction digits: a timestamp has microsecond precision and the literal is never rounded".into());
        }
        let padded = format!("{f:0<6}");
        micros = num(&padded)?;
    }
    let secs = i64::from(days)
        .checked_mul(86_400)
        .and_then(|d| d.checked_add(h * 3600 + m * 60 + s))
        .ok_or("the timestamp is out of range")?;
    secs.checked_mul(1_000_000)
        .and_then(|v| v.checked_add(micros))
        .ok_or_else(|| "the timestamp is out of range".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_round_trip_over_the_whole_supported_range() {
        let first = days_from_civil(1, 1, 1).expect("first");
        let last = days_from_civil(9999, 12, 31).expect("last");
        for days in (first..=last).step_by(97).chain([first, last]) {
            let (y, m, d) = civil_from_days(i64::from(days));
            assert_eq!(days_from_civil(y, m, d), Some(days));
        }
        assert_eq!(days_from_civil(1970, 1, 1), Some(0));
        assert_eq!(days_from_civil(1994, 1, 1), Some(8766));
        assert_eq!(days_from_civil(2000, 2, 29), Some(11_016));
        assert_eq!(days_from_civil(1900, 2, 29), None);
        assert_eq!(days_from_civil(2023, 4, 31), None);
        assert_eq!(days_from_civil(10_000, 1, 1), None);
        assert_eq!(days_from_civil(0, 1, 1), None);
    }

    #[test]
    fn dates_and_timestamps_have_duckdbs_text() {
        assert_eq!(format_date(8766), "1994-01-01");
        assert_eq!(format_date(i32::MAX), "infinity");
        assert_eq!(format_date(-i32::MAX), "-infinity");
        assert_eq!(
            format_date(days_from_civil(1, 1, 1).expect("d")),
            "0001-01-01"
        );
        assert_eq!(
            format_date(days_from_civil(1, 1, 1).expect("d") - 1),
            "0001-12-31 (BC)"
        );
        assert_eq!(format_timestamp_micros(0), "1970-01-01 00:00:00");
        assert_eq!(format_timestamp_micros(500_000), "1970-01-01 00:00:00.5");
        assert_eq!(format_timestamp_micros(-1), "1969-12-31 23:59:59.999999");
        assert_eq!(parse_timestamp_micros("1970-01-01 00:00:00.5"), Ok(500_000));
        assert_eq!(parse_timestamp_micros("1969-12-31T23:59:59.999999"), Ok(-1));
        assert!(parse_timestamp_micros("1970-01-01 00:00:00.1234567").is_err());
        assert!(parse_timestamp_micros("1970-01-01 24:00:00").is_err());
        assert!(parse_timestamp_micros("1970-01-01").is_err());
        assert!(parse_date("1994-1-1").is_err());
        assert!(parse_date("1994-13-01").is_err());
        assert_eq!(parse_date("1994-01-01"), Ok(8766));
    }
}
