//! Column statistics of one row group, as a decoder of Parquet footers (or Iceberg metrics) would hand them over.
//!
//! The types say what is KNOWN. Every field that a writer may leave out is an `Option`; "absent" is never the same as zero.

/// The logical type of a column, as far as pruning is concerned. Anything else is `Unsupported` and is never pruned on
/// (except by the null counts, which do not depend on the type).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnType {
    Bool,
    Int32,
    Int64,
    Float32,
    Float64,
    /// `DECIMAL(precision, scale)` with 1 <= precision <= 38 and scale <= precision (`docs/decimal-rules.md`, section 2).
    Decimal {
        precision: u8,
        scale: u8,
    },
    /// Days since 1970-01-01.
    Date32,
    /// Microseconds since the epoch. Other units must be converted by the caller (or the column given as `Unsupported`).
    TimestampMicros,
    /// UTF-8 text, ordered bytewise (which is code point order for valid UTF-8; DuckDB's default collation).
    Utf8,
    /// Bytes, ordered bytewise as unsigned numbers.
    Binary,
    /// Nested types, unsigned integers, INT96, anything not listed: statistics are not used.
    Unsupported,
}

/// A typed value: a statistic bound, or the literal of a predicate.
///
/// A `Float32` value is compared as the `f64` it widens to exactly. A `Decimal` carries its own precision and scale, so a literal may
/// have more fraction digits than the column it is compared with; the comparison is exact and never rounds the literal
/// (`docs/decimal-rules.md`, section 5).
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int32(i32),
    Int64(i64),
    Float32(f32),
    Float64(f64),
    Decimal {
        unscaled: i128,
        precision: u8,
        scale: u8,
    },
    Date32(i32),
    TimestampMicros(i64),
    Utf8(String),
    Binary(Vec<u8>),
}

/// One bound of a column (a minimum or a maximum) and whether it is EXACT.
///
/// `exact: true`: the bound is a value that occurs in the row group. `exact: false`: the bound may have been truncated by the writer
/// (a string or binary prefix, with or without the increment the Parquet format asks for), or widened. A truncated minimum is used as a lower
/// bound only; a truncated maximum as an upper bound only, and, because a writer that forgot the increment leaves a maximum that is
/// BELOW the true maximum, it is only trusted up to its own increment (see the module docs of `prune`).
#[derive(Clone, Debug, PartialEq)]
pub struct Bound {
    pub value: Value,
    pub exact: bool,
}

impl Bound {
    pub fn exact(value: Value) -> Bound {
        Bound { value, exact: true }
    }

    pub fn truncated(value: Value) -> Bound {
        Bound {
            value,
            exact: false,
        }
    }

    /// A text bound from the bytes of a footer. `None` (so: no statistic) if the bytes are not valid UTF-8, for example a prefix
    /// that was cut inside a character: such a statistic comes from a faulty writer and is not trusted.
    pub fn utf8_from_bytes(bytes: &[u8], exact: bool) -> Option<Bound> {
        let s = std::str::from_utf8(bytes).ok()?;
        Some(Bound {
            value: Value::Utf8(s.to_string()),
            exact,
        })
    }
}

/// What the footer says about one column chunk of a row group.
///
/// `min` and `max` follow the Parquet rules for floating point: NaN is NOT part of them, so a group can hold NaN rows that they do not
/// show. `nan_count` is `Some` only for sources that count NaN (not Parquet's `Statistics` today; Iceberg metrics do); `Some(0)` proves that
/// there is no NaN row.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnStats {
    pub ty: ColumnType,
    pub null_count: Option<u64>,
    pub nan_count: Option<u64>,
    pub min: Option<Bound>,
    pub max: Option<Bound>,
}

impl ColumnStats {
    /// No statistics at all for a column of this type.
    pub fn none(ty: ColumnType) -> ColumnStats {
        ColumnStats {
            ty,
            null_count: None,
            nan_count: None,
            min: None,
            max: None,
        }
    }

    pub fn with_null_count(mut self, n: u64) -> ColumnStats {
        self.null_count = Some(n);
        self
    }

    pub fn with_nan_count(mut self, n: u64) -> ColumnStats {
        self.nan_count = Some(n);
        self
    }

    /// Exact minimum and maximum.
    pub fn with_range(mut self, min: Value, max: Value) -> ColumnStats {
        self.min = Some(Bound::exact(min));
        self.max = Some(Bound::exact(max));
        self
    }
}

/// The statistics of one row group: its row count and its columns (looked up by name, exact match).
#[derive(Clone, Debug, PartialEq)]
pub struct RowGroupStats {
    pub num_rows: u64,
    pub columns: Vec<(String, ColumnStats)>,
}

impl RowGroupStats {
    pub fn new(num_rows: u64) -> RowGroupStats {
        RowGroupStats {
            num_rows,
            columns: Vec::new(),
        }
    }

    pub fn with_column(mut self, name: &str, stats: ColumnStats) -> RowGroupStats {
        self.columns.push((name.to_string(), stats));
        self
    }

    pub fn column(&self, name: &str) -> Option<&ColumnStats> {
        self.columns.iter().find(|(n, _)| n == name).map(|(_, c)| c)
    }
}
