//! The footer-statistics ADAPTER: from the column metadata of one row group (the `parquet` crate's types) to the statistics types of `nata-scan`.
//!
//! The rule of this file: a statistic is handed over only if it is SOUND, and everything else is left out (`None`), which `nata-scan` reads as
//! "keep the row group". In detail:
//!
//! 1. Only `min_value` / `max_value`. The deprecated `min` / `max` fields are ordered SIGNED by old writers (wrong for byte arrays and unsigned types);
//!    the `parquet` crate reports "these came from the deprecated fields" as `is_min_max_deprecated()`, and then NO bound is used (the null count still is).
//! 2. Only with the column order `TYPE_DEFINED_ORDER` whose sort order is the one this file expects for the type (signed for numbers, dates, timestamps and
//!    decimals; unsigned bytewise for text and binary; `false < true` for booleans). `UNDEFINED` (no `column_orders` in the footer: the spec says the
//!    bounds are then undefined), `IEEE_754_TOTAL_ORDER` (what `parquet-rs` 60 writes for float columns), `UNKNOWN` and anything else: no bounds.
//! 3. A bound is EXACT only if the footer says so (`is_min_value_exact` / `is_max_value_exact`); a missing flag is "not exact". (The `parquet` crate
//!    cannot tell a missing flag from `false`; so for numbers written by a writer without the flags, `<>` is never pruned on a constant row group.)
//! 4. `nan_count` stays `None`: Parquet statistics have none, so float `>`, `>=` and `<>` never prune (rule 10 of `nata-scan`).
//! 5. Types that cannot be mapped soundly have no bounds: INT96, unsigned integers, FLOAT16, nested columns, time zones, nanoseconds, decimals above 38 digits,
//!    a text bound that is not valid UTF-8, a decimal bound whose bytes do not fit 128 bits, a timestamp in milliseconds that overflows microseconds.
//! 6. Statistics that contradict the row group (a negative count) are an error of the file, not a statistic: the caller names the file and row group.
//!
//! Timestamps in milliseconds are converted to microseconds with checked arithmetic (exact; an overflow drops that bound).

use nata_scan::{Bound, ColumnStats, ColumnType, RowGroupStats, Value};
use parquet::basic::{ColumnOrder, SortOrder};
use parquet::file::metadata::{ColumnChunkMetaData, FileMetaData, RowGroupMetaData};
use parquet::file::statistics::Statistics;

use crate::schema::{Kind, Leaf, Unit};

/// The sort order the footer must declare for the bounds of a column of this kind to be usable.
fn expected_sort_order(kind: Kind) -> SortOrder {
    match kind {
        Kind::Bool | Kind::Utf8 | Kind::Binary => SortOrder::UNSIGNED,
        Kind::Int { signed: true, .. }
        | Kind::Float32
        | Kind::Float64
        | Kind::Decimal { .. }
        | Kind::Date
        | Kind::Timestamp(_) => SortOrder::SIGNED,
        Kind::Int { signed: false, .. } => SortOrder::UNDEFINED,
    }
}

/// True if the bounds of a column with this declared order are in the order the scan assumes.
pub fn order_is_usable(order: ColumnOrder, kind: Kind) -> bool {
    match order {
        ColumnOrder::TYPE_DEFINED_ORDER(so) => {
            let expected = expected_sort_order(kind);
            so == expected && expected != SortOrder::UNDEFINED
        }
        _ => false,
    }
}

/// The column order of leaf `index`, `UNDEFINED` when the footer has none (also for a footer whose list is too short).
pub fn column_order(file: &FileMetaData, index: usize) -> ColumnOrder {
    file.column_orders()
        .and_then(|orders| orders.get(index))
        .copied()
        .unwrap_or(ColumnOrder::UNDEFINED)
}

/// A big-endian two's complement integer of any length that fits 128 bits.
pub fn be_twos_complement(bytes: &[u8]) -> Option<i128> {
    let first = *bytes.first()?;
    let negative = first & 0x80 != 0;
    let pad = if negative { 0xFFu8 } else { 0u8 };
    let mut buf = [pad; 16];
    if bytes.len() <= 16 {
        let start = 16usize.checked_sub(bytes.len())?;
        buf.get_mut(start..)?.copy_from_slice(bytes);
    } else {
        let split = bytes.len().checked_sub(16)?;
        let (head, tail) = bytes.split_at(split);
        if head.iter().any(|b| *b != pad) {
            return None;
        }
        // the sign of the value is that of the first byte, so the first byte of the tail must agree
        if (tail.first()? & 0x80 != 0) != negative {
            return None;
        }
        buf.copy_from_slice(tail);
    }
    Some(i128::from_be_bytes(buf))
}

enum Side {
    Min,
    Max,
}

/// One bound of a column chunk as a typed value, or `None` if it is missing or not usable.
fn bound(kind: Kind, type_length: i32, stats: &Statistics, side: Side) -> Option<Bound> {
    let exact = match side {
        Side::Min => stats.min_is_exact(),
        Side::Max => stats.max_is_exact(),
    };
    let make = |value: Value| Some(Bound { value, exact });
    match (stats, kind) {
        (Statistics::Boolean(s), Kind::Bool) => {
            let v = match side {
                Side::Min => s.min_opt(),
                Side::Max => s.max_opt(),
            };
            make(Value::Bool(*v?))
        }
        (
            Statistics::Int32(s),
            Kind::Int {
                bits: 8 | 16 | 32,
                signed: true,
            },
        ) => make(Value::Int32(*pick(s.min_opt(), s.max_opt(), &side)?)),
        (Statistics::Int32(s), Kind::Date) => {
            let d = *pick(s.min_opt(), s.max_opt(), &side)?;
            // i32::MAX and -i32::MAX are DuckDB's infinity sentinels for dates: not numbers of days.
            if d == i32::MAX || d <= -i32::MAX {
                return None;
            }
            make(Value::Date32(d))
        }
        (Statistics::Int32(s), Kind::Decimal { precision, scale }) => make(Value::Decimal {
            unscaled: i128::from(*pick(s.min_opt(), s.max_opt(), &side)?),
            precision,
            scale,
        }),
        (
            Statistics::Int64(s),
            Kind::Int {
                bits: 64,
                signed: true,
            },
        ) => make(Value::Int64(*pick(s.min_opt(), s.max_opt(), &side)?)),
        (Statistics::Int64(s), Kind::Timestamp(unit)) => {
            let t = *pick(s.min_opt(), s.max_opt(), &side)?;
            match unit {
                Unit::Micros => make(Value::TimestampMicros(t)),
                Unit::Millis => make(Value::TimestampMicros(t.checked_mul(1000)?)),
            }
        }
        (Statistics::Int64(s), Kind::Decimal { precision, scale }) => make(Value::Decimal {
            unscaled: i128::from(*pick(s.min_opt(), s.max_opt(), &side)?),
            precision,
            scale,
        }),
        (Statistics::Float(s), Kind::Float32) => {
            make(Value::Float32(*pick(s.min_opt(), s.max_opt(), &side)?))
        }
        (Statistics::Double(s), Kind::Float64) => {
            make(Value::Float64(*pick(s.min_opt(), s.max_opt(), &side)?))
        }
        (Statistics::ByteArray(s), Kind::Utf8) => {
            let bytes = match side {
                Side::Min => s.min_bytes_opt(),
                Side::Max => s.max_bytes_opt(),
            }?;
            Bound::utf8_from_bytes(bytes, exact)
        }
        (Statistics::ByteArray(s), Kind::Binary) => {
            let bytes = match side {
                Side::Min => s.min_bytes_opt(),
                Side::Max => s.max_bytes_opt(),
            }?;
            make(Value::Binary(bytes.to_vec()))
        }
        (Statistics::ByteArray(s), Kind::Decimal { precision, scale }) => {
            let bytes = match side {
                Side::Min => s.min_bytes_opt(),
                Side::Max => s.max_bytes_opt(),
            }?;
            make(Value::Decimal {
                unscaled: be_twos_complement(bytes)?,
                precision,
                scale,
            })
        }
        (Statistics::FixedLenByteArray(s), Kind::Decimal { precision, scale }) => {
            let bytes = match side {
                Side::Min => s.min_bytes_opt(),
                Side::Max => s.max_bytes_opt(),
            }?;
            if i32::try_from(bytes.len()).ok() != Some(type_length) {
                return None;
            }
            make(Value::Decimal {
                unscaled: be_twos_complement(bytes)?,
                precision,
                scale,
            })
        }
        _ => None,
    }
}

fn pick<'a, T>(min: Option<&'a T>, max: Option<&'a T>, side: &Side) -> Option<&'a T> {
    match side {
        Side::Min => min,
        Side::Max => max,
    }
}

/// The statistics of one column chunk. `kind` is `None` for a column the scan does not understand: only its null count is handed over (and `nata-scan`
/// does not use the null counts of a column of an unsupported type either, so such a column is never pruned).
pub fn column_stats(
    chunk: &ColumnChunkMetaData,
    order: ColumnOrder,
    kind: Option<Kind>,
) -> ColumnStats {
    let ty = kind.map_or(ColumnType::Unsupported, Kind::scan_type);
    let mut out = ColumnStats::none(ty);
    let Some(stats) = chunk.statistics() else {
        return out;
    };
    out.null_count = stats.null_count_opt();
    let Some(kind) = kind else { return out };
    if stats.is_min_max_deprecated() || !order_is_usable(order, kind) {
        return out;
    }
    let type_length = chunk.column_descr().type_length();
    out.min = bound(kind, type_length, stats, Side::Min);
    out.max = bound(kind, type_length, stats, Side::Max);
    out
}

/// The statistics of the columns of one row group that a predicate looks at. `columns` pairs a column name with its leaf. An unknown leaf
/// index in this row group (a footer that does not list it) simply has no statistics.
pub fn row_group_stats(
    rg: &RowGroupMetaData,
    file: &FileMetaData,
    columns: &[(&str, &Leaf)],
) -> Option<RowGroupStats> {
    let rows = u64::try_from(rg.num_rows()).ok()?;
    let mut out = RowGroupStats::new(rows);
    for (name, leaf) in columns {
        let stats = match rg.columns().get(leaf.index) {
            Some(chunk) => column_stats(
                chunk,
                column_order(file, leaf.index),
                leaf.kind.as_ref().ok().copied(),
            ),
            None => ColumnStats::none(ColumnType::Unsupported),
        };
        out = out.with_column(name, stats);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twos_complement_decoding() {
        assert_eq!(be_twos_complement(&[0x00]), Some(0));
        assert_eq!(be_twos_complement(&[0xFF]), Some(-1));
        assert_eq!(be_twos_complement(&[0x80]), Some(-128));
        assert_eq!(be_twos_complement(&[0x7F]), Some(127));
        assert_eq!(be_twos_complement(&[0x01, 0x00]), Some(256));
        assert_eq!(be_twos_complement(&[0xFF, 0x00]), Some(-256));
        assert_eq!(be_twos_complement(&[]), None);
        // 20 bytes holding -5: sign extension is accepted
        let mut b = vec![0xFF; 19];
        b.push(0xFB);
        assert_eq!(be_twos_complement(&b), Some(-5));
        // 20 bytes that do not fit 128 bits
        let mut b = vec![0x00; 19];
        b.push(0x01);
        b[0] = 0x01;
        assert_eq!(be_twos_complement(&b), None);
        // a sign byte that disagrees with the tail
        let mut b = vec![0x00; 4];
        b.extend([0x80; 16]);
        assert_eq!(be_twos_complement(&b), None);
    }

    #[test]
    fn only_the_expected_type_defined_order_is_usable() {
        let text = Kind::Utf8;
        assert!(order_is_usable(
            ColumnOrder::TYPE_DEFINED_ORDER(SortOrder::UNSIGNED),
            text
        ));
        assert!(!order_is_usable(
            ColumnOrder::TYPE_DEFINED_ORDER(SortOrder::SIGNED),
            text
        ));
        assert!(!order_is_usable(ColumnOrder::UNDEFINED, text));
        assert!(!order_is_usable(ColumnOrder::UNKNOWN, text));
        assert!(!order_is_usable(
            ColumnOrder::IEEE_754_TOTAL_ORDER,
            Kind::Float64
        ));
        assert!(order_is_usable(
            ColumnOrder::TYPE_DEFINED_ORDER(SortOrder::SIGNED),
            Kind::Float64
        ));
        let uint = Kind::Int {
            bits: 32,
            signed: false,
        };
        assert!(!order_is_usable(
            ColumnOrder::TYPE_DEFINED_ORDER(SortOrder::UNSIGNED),
            uint
        ));
    }
}
