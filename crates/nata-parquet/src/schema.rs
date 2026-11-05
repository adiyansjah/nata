//! What a Parquet leaf column is, as far as the scan is concerned: the types that are read, filtered and pruned, and why anything else is not.
//!
//! One decision table, used by the reader (which Arrow type to ask for), the statistics adapter (how to decode a footer bound) and the filter parser
//! (how to type a literal). A column that is not in the table is "not supported yet" and is an error only if the scan touches it (projects or filters it).

use arrow_schema::{DataType, TimeUnit};
use nata_scan::ColumnType;
use parquet::basic::{ConvertedType, LogicalType, TimeUnit as PqUnit, Type as Physical};
use parquet::schema::types::{ColumnDescriptor, SchemaDescriptor};

/// The unit of a timestamp column that the scan supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Millis,
    Micros,
}

/// The logical kind of a supported flat column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bool,
    /// `bits` is 8, 16, 32 or 64.
    Int {
        bits: u8,
        signed: bool,
    },
    Float32,
    Float64,
    Decimal {
        precision: u8,
        scale: u8,
    },
    Date,
    /// A timestamp WITHOUT time zone (`isAdjustedToUTC = false`).
    Timestamp(Unit),
    Utf8,
    Binary,
}

impl Kind {
    /// The Arrow type the reader asks for (ADR 0003: strings stay dictionary arrays, decimals up to 18 digits are `Decimal64`).
    pub fn arrow_type(self) -> DataType {
        match self {
            Kind::Bool => DataType::Boolean,
            Kind::Int {
                bits: 8,
                signed: true,
            } => DataType::Int8,
            Kind::Int {
                bits: 16,
                signed: true,
            } => DataType::Int16,
            Kind::Int {
                bits: 32,
                signed: true,
            } => DataType::Int32,
            Kind::Int { signed: true, .. } => DataType::Int64,
            Kind::Int {
                bits: 8,
                signed: false,
            } => DataType::UInt8,
            Kind::Int {
                bits: 16,
                signed: false,
            } => DataType::UInt16,
            Kind::Int {
                bits: 32,
                signed: false,
            } => DataType::UInt32,
            Kind::Int { signed: false, .. } => DataType::UInt64,
            Kind::Float32 => DataType::Float32,
            Kind::Float64 => DataType::Float64,
            Kind::Decimal { precision, scale } => {
                let scale = i8::try_from(scale).unwrap_or(0);
                if precision <= 18 {
                    DataType::Decimal64(precision, scale)
                } else {
                    DataType::Decimal128(precision, scale)
                }
            }
            Kind::Date => DataType::Date32,
            Kind::Timestamp(Unit::Millis) => DataType::Timestamp(TimeUnit::Millisecond, None),
            Kind::Timestamp(Unit::Micros) => DataType::Timestamp(TimeUnit::Microsecond, None),
            Kind::Utf8 => DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            Kind::Binary => DataType::Binary,
        }
    }

    /// The type as the pruning crate knows it. Unsigned integers have no pruning type: `Unsupported` means "never prune on the values".
    pub fn scan_type(self) -> ColumnType {
        match self {
            Kind::Bool => ColumnType::Bool,
            Kind::Int {
                bits: 64,
                signed: true,
            } => ColumnType::Int64,
            Kind::Int { signed: true, .. } => ColumnType::Int32,
            Kind::Int { signed: false, .. } => ColumnType::Unsupported,
            Kind::Float32 => ColumnType::Float32,
            Kind::Float64 => ColumnType::Float64,
            Kind::Decimal { precision, scale } => ColumnType::Decimal { precision, scale },
            Kind::Date => ColumnType::Date32,
            Kind::Timestamp(_) => ColumnType::TimestampMicros,
            Kind::Utf8 => ColumnType::Utf8,
            Kind::Binary => ColumnType::Binary,
        }
    }

    /// The SQL name used in messages.
    pub fn sql_name(self) -> String {
        match self {
            Kind::Bool => "BOOLEAN".into(),
            Kind::Int { bits, signed: true } => match bits {
                8 => "TINYINT".into(),
                16 => "SMALLINT".into(),
                32 => "INTEGER".into(),
                _ => "BIGINT".into(),
            },
            Kind::Int {
                bits,
                signed: false,
            } => match bits {
                8 => "UTINYINT".into(),
                16 => "USMALLINT".into(),
                32 => "UINTEGER".into(),
                _ => "UBIGINT".into(),
            },
            Kind::Float32 => "FLOAT".into(),
            Kind::Float64 => "DOUBLE".into(),
            Kind::Decimal { precision, scale } => format!("DECIMAL({precision},{scale})"),
            Kind::Date => "DATE".into(),
            Kind::Timestamp(Unit::Millis) => "TIMESTAMP_MS".into(),
            Kind::Timestamp(Unit::Micros) => "TIMESTAMP".into(),
            Kind::Utf8 => "VARCHAR".into(),
            Kind::Binary => "BLOB".into(),
        }
    }
}

/// A top-level leaf column of a file: its position among the leaves, its physical type, and either its [`Kind`] or the reason it is not supported.
#[derive(Clone, Debug, PartialEq)]
pub struct Leaf {
    pub name: String,
    pub index: usize,
    pub physical: Physical,
    pub kind: Result<Kind, String>,
}

/// The top-level columns of a file, in file order. A nested column (a struct, list or map) is ONE entry whose kind is an `Err`; its leaves are not
/// listed separately. Two top-level columns with the same name are an error (the scan finds columns by name).
pub fn leaves(descr: &SchemaDescriptor) -> Result<Vec<Leaf>, String> {
    let mut out: Vec<Leaf> = Vec::new();
    for (index, c) in descr.columns().iter().enumerate() {
        let parts = c.path().parts();
        let name = parts.first().cloned().unwrap_or_default();
        let nested = parts.len() != 1;
        if let Some(existing) = out.iter().find(|l| l.name == name) {
            if nested && existing.kind.is_err() {
                continue; // another leaf of the same nested column
            }
            return Err(format!("two columns are named {name:?}"));
        }
        out.push(Leaf {
            name,
            index,
            physical: c.physical_type(),
            kind: classify(c),
        });
    }
    Ok(out)
}

/// Decide what a column is. `Err(reason)` is the `<what>` of `not supported yet: <what>`.
pub fn classify(d: &ColumnDescriptor) -> Result<Kind, String> {
    if d.path().parts().len() != 1 || d.max_rep_level() != 0 || d.max_def_level() > 1 {
        return Err("a nested or repeated column (structs, lists and maps)".into());
    }
    let physical = d.physical_type();
    let converted = d.converted_type();
    let logical = d.logical_type_ref();
    // Logical annotations first (the newer form); the legacy converted type only when there is no logical one.
    match logical {
        Some(LogicalType::String) => only(
            physical,
            Physical::BYTE_ARRAY,
            Kind::Utf8,
            "a STRING annotation",
        ),
        Some(LogicalType::Integer(i)) => int(physical, i.bit_width, i.is_signed),
        Some(LogicalType::Decimal(dt)) => decimal(d, dt.precision, dt.scale),
        Some(LogicalType::Date) => only(physical, Physical::INT32, Kind::Date, "a DATE annotation"),
        Some(LogicalType::Timestamp(t)) => timestamp(physical, t.is_adjusted_to_u_t_c, &t.unit),
        Some(LogicalType::Float16) => Err("FLOAT16".into()),
        Some(LogicalType::Time(_)) => Err("TIME".into()),
        Some(LogicalType::Uuid) => Err("UUID".into()),
        Some(LogicalType::Enum) => Err("ENUM".into()),
        Some(LogicalType::Json) => Err("JSON".into()),
        Some(LogicalType::Bson) => Err("BSON".into()),
        Some(other) => Err(format!("the logical type {}", logical_name(other))),
        None => match converted {
            ConvertedType::NONE => match physical {
                Physical::BOOLEAN => Ok(Kind::Bool),
                Physical::INT32 => Ok(Kind::Int {
                    bits: 32,
                    signed: true,
                }),
                Physical::INT64 => Ok(Kind::Int {
                    bits: 64,
                    signed: true,
                }),
                Physical::FLOAT => Ok(Kind::Float32),
                Physical::DOUBLE => Ok(Kind::Float64),
                Physical::BYTE_ARRAY => Ok(Kind::Binary),
                Physical::INT96 => Err("INT96 (the legacy timestamp)".into()),
                Physical::FIXED_LEN_BYTE_ARRAY => {
                    Err("FIXED_LEN_BYTE_ARRAY without an annotation".into())
                }
            },
            ConvertedType::UTF8 => only(physical, Physical::BYTE_ARRAY, Kind::Utf8, "UTF8"),
            ConvertedType::INT_8 => int(physical, 8, true),
            ConvertedType::INT_16 => int(physical, 16, true),
            ConvertedType::INT_32 => int(physical, 32, true),
            ConvertedType::INT_64 => int(physical, 64, true),
            ConvertedType::UINT_8 => int(physical, 8, false),
            ConvertedType::UINT_16 => int(physical, 16, false),
            ConvertedType::UINT_32 => int(physical, 32, false),
            ConvertedType::UINT_64 => int(physical, 64, false),
            ConvertedType::DECIMAL => decimal(d, d.type_precision(), d.type_scale()),
            ConvertedType::DATE => only(physical, Physical::INT32, Kind::Date, "DATE"),
            // The legacy TIMESTAMP_MILLIS / TIMESTAMP_MICROS mean "adjusted to UTC".
            ConvertedType::TIMESTAMP_MILLIS | ConvertedType::TIMESTAMP_MICROS => {
                Err("TIMESTAMP WITH TIME ZONE (a timestamp adjusted to UTC)".into())
            }
            other => Err(format!("the converted type {other}")),
        },
    }
}

fn logical_name(l: &LogicalType) -> String {
    format!("{l:?}")
        .split(['(', ' ', '{'])
        .next()
        .unwrap_or("?")
        .to_string()
}

fn only(have: Physical, want: Physical, kind: Kind, what: &str) -> Result<Kind, String> {
    if have == want {
        Ok(kind)
    } else {
        Err(format!("{what} on the physical type {have}"))
    }
}

fn int(physical: Physical, bits: i8, signed: bool) -> Result<Kind, String> {
    let ok = matches!(
        (physical, bits),
        (Physical::INT32, 8 | 16 | 32) | (Physical::INT64, 64)
    );
    if !ok {
        return Err(format!(
            "an integer of {bits} bits on the physical type {physical}"
        ));
    }
    let bits = u8::try_from(bits).map_err(|_| "an integer width".to_string())?;
    Ok(Kind::Int { bits, signed })
}

fn decimal(d: &ColumnDescriptor, precision: i32, scale: i32) -> Result<Kind, String> {
    let p = u8::try_from(precision)
        .ok()
        .filter(|p| (1..=38).contains(p));
    let s = u8::try_from(scale).ok();
    let (Some(p), Some(s)) = (p, s) else {
        return Err(format!(
            "DECIMAL({precision},{scale}) (precision 1 to 38 and scale 0 to precision are supported)"
        ));
    };
    if s > p {
        return Err(format!(
            "DECIMAL({precision},{scale}) (the scale is above the precision)"
        ));
    }
    let fits = match d.physical_type() {
        Physical::INT32 => p <= 9,
        Physical::INT64 => p <= 18,
        Physical::BYTE_ARRAY => true,
        Physical::FIXED_LEN_BYTE_ARRAY => d.type_length() > 0,
        _ => false,
    };
    if !fits {
        return Err(format!(
            "DECIMAL({p},{s}) on the physical type {}",
            d.physical_type()
        ));
    }
    Ok(Kind::Decimal {
        precision: p,
        scale: s,
    })
}

fn timestamp(physical: Physical, adjusted: bool, unit: &PqUnit) -> Result<Kind, String> {
    if physical != Physical::INT64 {
        return Err(format!("a TIMESTAMP on the physical type {physical}"));
    }
    if adjusted {
        return Err("TIMESTAMP WITH TIME ZONE (a timestamp adjusted to UTC)".into());
    }
    match unit {
        PqUnit::MILLIS => Ok(Kind::Timestamp(Unit::Millis)),
        PqUnit::MICROS => Ok(Kind::Timestamp(Unit::Micros)),
        PqUnit::NANOS => Err("TIMESTAMP with nanosecond precision".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_types_follow_adr_0003() {
        assert_eq!(
            Kind::Decimal {
                precision: 15,
                scale: 2
            }
            .arrow_type(),
            DataType::Decimal64(15, 2)
        );
        assert_eq!(
            Kind::Decimal {
                precision: 18,
                scale: 0
            }
            .arrow_type(),
            DataType::Decimal64(18, 0)
        );
        assert_eq!(
            Kind::Decimal {
                precision: 19,
                scale: 2
            }
            .arrow_type(),
            DataType::Decimal128(19, 2)
        );
        assert_eq!(
            Kind::Utf8.arrow_type(),
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
        );
    }

    #[test]
    fn unsigned_integers_are_never_pruned() {
        assert_eq!(
            Kind::Int {
                bits: 32,
                signed: false
            }
            .scan_type(),
            ColumnType::Unsupported
        );
        assert_eq!(
            Kind::Int {
                bits: 16,
                signed: true
            }
            .scan_type(),
            ColumnType::Int32
        );
        assert_eq!(
            Kind::Int {
                bits: 64,
                signed: true
            }
            .scan_type(),
            ColumnType::Int64
        );
    }
}
