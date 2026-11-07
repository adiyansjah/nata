//! Columns as the evaluator sees them: one typed vector and one validity vector per column, converted from Arrow on first use and converted back for
//! the output. Every decimal is an `i128` here (Arrow's `Decimal64` of the scan is widened; the result is narrowed again by precision), every integer an
//! `i64` (the range of the SQL type is enforced by the operations), strings are a dictionary (the scan's) or owned text (computed).

use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::{
    Date32Type, Decimal64Type, Decimal128Type, Float32Type, Float64Type, Int8Type, Int16Type,
    Int32Type, Int64Type,
};
use arrow_array::{
    Array, ArrayRef, BooleanArray, Date32Array, Decimal64Array, Decimal128Array, Float32Array,
    Float64Array, Int8Array, Int16Array, Int32Array, Int64Array, StringArray,
};
use arrow_buffer::NullBuffer;
use arrow_schema::DataType;
use nata_sql::types::SqlType;

use crate::error::EvalError;

/// Text values of a column: the dictionary of the scan with one key per row, or text built by the evaluator.
#[derive(Clone, Debug)]
pub enum StrData {
    Dict {
        values: Arc<StringArray>,
        keys: Vec<u32>,
    },
    Owned(Vec<String>),
}

impl StrData {
    /// The text of row `i`; only meaningful when the row is valid.
    pub fn get(&self, i: usize) -> &str {
        match self {
            StrData::Dict { values, keys } => {
                let k = keys.get(i).copied().unwrap_or(0) as usize;
                if k < values.len() {
                    values.value(k)
                } else {
                    ""
                }
            }
            StrData::Owned(v) => v.get(i).map_or("", String::as_str),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            StrData::Dict { keys, .. } => keys.len(),
            StrData::Owned(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, Debug)]
pub enum Data {
    Bool(Vec<bool>),
    /// TINYINT, SMALLINT, INTEGER, BIGINT.
    Int(Vec<i64>),
    F32(Vec<f32>),
    F64(Vec<f64>),
    /// DECIMAL(p, s): the unscaled values.
    Dec(Vec<i128>),
    Str(StrData),
    /// Days since 1970-01-01.
    Date(Vec<i32>),
}

/// A column of `valid.len()` rows. The value of a row that is not valid is unspecified (and never looked at).
#[derive(Clone, Debug)]
pub struct Col {
    pub ty: SqlType,
    pub data: Data,
    pub valid: Vec<bool>,
}

impl Col {
    pub fn len(&self) -> usize {
        self.valid.len()
    }

    pub fn is_empty(&self) -> bool {
        self.valid.is_empty()
    }

    /// A column of `n` rows, all NULL, whose values are placeholders: the starting point of an operation that fills the rows it computes.
    pub fn empty(ty: SqlType, n: usize) -> Col {
        let data = match ty {
            SqlType::Boolean | SqlType::Null => Data::Bool(vec![false; n]),
            SqlType::TinyInt | SqlType::SmallInt | SqlType::Integer | SqlType::BigInt => {
                Data::Int(vec![0; n])
            }
            SqlType::Float => Data::F32(vec![0.0; n]),
            SqlType::Double => Data::F64(vec![0.0; n]),
            SqlType::Decimal { .. } => Data::Dec(vec![0; n]),
            SqlType::Varchar => Data::Str(StrData::Owned(vec![String::new(); n])),
            SqlType::Date => Data::Date(vec![0; n]),
        };
        Col {
            ty,
            data,
            valid: vec![false; n],
        }
    }

    /// Reuse an admitted fixed-width buffer; old selection bits cannot leak.
    pub(crate) fn reset(&mut self, n: usize) {
        self.valid.resize(n, false);
        self.valid.fill(false);
        match &mut self.data {
            Data::Bool(v) => v.resize(n, false),
            Data::Int(v) => v.resize(n, 0),
            Data::Dec(v) => v.resize(n, 0),
            Data::F32(v) => v.resize(n, 0.0),
            Data::F64(v) => v.resize(n, 0.0),
            Data::Date(v) => v.resize(n, 0),
            Data::Str(_) => {}
        }
    }

    /// Widen a scan lane once into reusable storage. Uncommon Arrow layouts
    /// retain the sanctioned conversion and dictionary NULL validation.
    pub(crate) fn read_arrow(&mut self, array: &ArrayRef) -> Result<(), EvalError> {
        macro_rules! copy {
            ($values:expr, $native:ty, $convert:expr) => {{
                let a = array
                    .as_primitive_opt::<$native>()
                    .ok_or_else(|| EvalError::internal("scan lane type"))?;
                $values.clear();
                $values.extend(a.values().iter().copied().map($convert));
            }};
        }
        match (&mut self.data, array.data_type()) {
            (Data::Dec(v), DataType::Decimal64(..)) => copy!(v, Decimal64Type, i128::from),
            (Data::Dec(v), DataType::Decimal128(..)) => {
                copy!(v, Decimal128Type, std::convert::identity)
            }
            (Data::Int(v), DataType::Int8) => copy!(v, Int8Type, i64::from),
            (Data::Int(v), DataType::Int16) => copy!(v, Int16Type, i64::from),
            (Data::Int(v), DataType::Int32) => copy!(v, Int32Type, i64::from),
            (Data::Int(v), DataType::Int64) => copy!(v, Int64Type, std::convert::identity),
            (Data::Date(v), DataType::Date32) => copy!(v, Date32Type, std::convert::identity),
            _ => {
                *self = Self::from_arrow(array, self.ty)?;
                return Ok(());
            }
        }
        self.valid.clear();
        self.valid
            .extend((0..array.len()).map(|i| array.is_valid(i)));
        Ok(())
    }

    pub fn bool_col(values: Vec<bool>, valid: Vec<bool>) -> Col {
        Col {
            ty: SqlType::Boolean,
            data: Data::Bool(values),
            valid,
        }
    }

    pub fn bools(&self) -> Result<&[bool], EvalError> {
        match &self.data {
            Data::Bool(v) => Ok(v),
            _ => Err(EvalError::internal("a BOOLEAN column was expected")),
        }
    }

    /// Copy row `i` of `src` into row `i` of `self` for every `i` in `rows` (the types are the same: CASE results are of one type).
    pub fn assign(&mut self, src: &Col, rows: &[u32]) -> Result<(), EvalError> {
        let mismatch = || EvalError::internal("a CASE branch has another type than the CASE");
        for &r in rows {
            let i = r as usize;
            let v = src.valid.get(i).copied().unwrap_or(false);
            if let Some(slot) = self.valid.get_mut(i) {
                *slot = v;
            }
            if !v {
                continue;
            }
            match (&mut self.data, &src.data) {
                (Data::Bool(d), Data::Bool(s)) => d[i] = s[i],
                (Data::Int(d), Data::Int(s)) => d[i] = s[i],
                (Data::F32(d), Data::F32(s)) => d[i] = s[i],
                (Data::F64(d), Data::F64(s)) => d[i] = s[i],
                (Data::Dec(d), Data::Dec(s)) => d[i] = s[i],
                (Data::Date(d), Data::Date(s)) => d[i] = s[i],
                (Data::Str(StrData::Owned(d)), Data::Str(s)) => d[i] = s.get(i).to_string(),
                _ => return Err(mismatch()),
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------------------------------------------------------ Arrow in

    /// The column for an Arrow array the scan produced, typed as the plan says.
    pub fn from_arrow(array: &ArrayRef, ty: SqlType) -> Result<Col, EvalError> {
        let n = array.len();
        let valid: Vec<bool> = (0..n).map(|i| array.is_valid(i)).collect();
        let bad = || {
            EvalError::internal(
                "the scan returned a column of another Arrow type than its SQL type",
            )
        };
        let data = match array.data_type() {
            DataType::Boolean => {
                let a = array.as_boolean_opt().ok_or_else(bad)?;
                Data::Bool((0..n).map(|i| a.value(i)).collect())
            }
            DataType::Int8 => Data::Int(
                array
                    .as_primitive_opt::<Int8Type>()
                    .ok_or_else(bad)?
                    .values()
                    .iter()
                    .map(|v| i64::from(*v))
                    .collect(),
            ),
            DataType::Int16 => Data::Int(
                array
                    .as_primitive_opt::<Int16Type>()
                    .ok_or_else(bad)?
                    .values()
                    .iter()
                    .map(|v| i64::from(*v))
                    .collect(),
            ),
            DataType::Int32 => Data::Int(
                array
                    .as_primitive_opt::<Int32Type>()
                    .ok_or_else(bad)?
                    .values()
                    .iter()
                    .map(|v| i64::from(*v))
                    .collect(),
            ),
            DataType::Int64 => Data::Int(
                array
                    .as_primitive_opt::<Int64Type>()
                    .ok_or_else(bad)?
                    .values()
                    .to_vec(),
            ),
            DataType::Float32 => Data::F32(
                array
                    .as_primitive_opt::<Float32Type>()
                    .ok_or_else(bad)?
                    .values()
                    .to_vec(),
            ),
            DataType::Float64 => Data::F64(
                array
                    .as_primitive_opt::<Float64Type>()
                    .ok_or_else(bad)?
                    .values()
                    .to_vec(),
            ),
            DataType::Decimal64(..) => Data::Dec(
                array
                    .as_primitive_opt::<Decimal64Type>()
                    .ok_or_else(bad)?
                    .values()
                    .iter()
                    .map(|v| i128::from(*v))
                    .collect(),
            ),
            DataType::Decimal128(..) => Data::Dec(
                array
                    .as_primitive_opt::<Decimal128Type>()
                    .ok_or_else(bad)?
                    .values()
                    .to_vec(),
            ),
            DataType::Date32 => Data::Date(
                array
                    .as_primitive_opt::<Date32Type>()
                    .ok_or_else(bad)?
                    .values()
                    .to_vec(),
            ),
            DataType::Utf8 => {
                let a = array.as_string_opt::<i32>().ok_or_else(bad)?;
                let keys = (0..n)
                    .map(|i| u32::try_from(i).map_err(|_| bad()))
                    .collect::<Result<Vec<u32>, _>>()?;
                Data::Str(StrData::Dict {
                    values: Arc::new(a.clone()),
                    keys,
                })
            }
            DataType::Dictionary(k, v) if **k == DataType::Int32 && **v == DataType::Utf8 => {
                let d = array.as_dictionary_opt::<Int32Type>().ok_or_else(bad)?;
                let values = d.values().as_string_opt::<i32>().ok_or_else(bad)?;
                let keys = d
                    .keys()
                    .values()
                    .iter()
                    .map(|k| u32::try_from(*k).unwrap_or(u32::MAX))
                    .collect();
                Data::Str(StrData::Dict {
                    values: Arc::new(values.clone()),
                    keys,
                })
            }
            // anything else (a dictionary of another shape, a large string) is unpacked to plain text by the one sanctioned strict cast
            DataType::LargeUtf8 | DataType::Dictionary(..) => {
                let plain = nata_kernels::strict_cast::strict_cast(array.as_ref(), &DataType::Utf8)
                    .map_err(|_| bad())?;
                return Col::from_arrow(&plain, ty);
            }
            _ => return Err(bad()),
        };
        let mut col = Col { ty, data, valid };
        // a dictionary key that points outside the dictionary is a NULL, never a read outside it
        if let Data::Str(StrData::Dict { values, keys }) = &col.data {
            for (i, k) in keys.iter().enumerate() {
                if col.valid[i] && (*k as usize >= values.len() || values.is_null(*k as usize)) {
                    col.valid[i] = false;
                }
            }
        }
        Ok(col)
    }

    // ------------------------------------------------------------------------------------------------------------------ Arrow out

    /// The Arrow array of this column, of the type the CSV writer and the scan's other consumers know for the SQL type.
    pub fn to_arrow(&self) -> Result<ArrayRef, EvalError> {
        let bad = || EvalError::internal("a column does not match its type");
        let nulls = if self.valid.iter().all(|v| *v) {
            None
        } else {
            Some(NullBuffer::from(self.valid.clone()))
        };
        let arr: ArrayRef = match (&self.ty, &self.data) {
            (SqlType::Boolean, Data::Bool(v)) => Arc::new(BooleanArray::new(
                arrow_buffer::BooleanBuffer::from(v.clone()),
                nulls,
            )),
            (SqlType::TinyInt, Data::Int(v)) => Arc::new(Int8Array::new(
                v.iter()
                    .map(|x| i8::try_from(*x).unwrap_or(0))
                    .collect::<Vec<_>>()
                    .into(),
                nulls,
            )),
            (SqlType::SmallInt, Data::Int(v)) => Arc::new(Int16Array::new(
                v.iter()
                    .map(|x| i16::try_from(*x).unwrap_or(0))
                    .collect::<Vec<_>>()
                    .into(),
                nulls,
            )),
            (SqlType::Integer, Data::Int(v)) => Arc::new(Int32Array::new(
                v.iter()
                    .map(|x| i32::try_from(*x).unwrap_or(0))
                    .collect::<Vec<_>>()
                    .into(),
                nulls,
            )),
            (SqlType::BigInt, Data::Int(v)) => Arc::new(Int64Array::new(v.clone().into(), nulls)),
            (SqlType::Float, Data::F32(v)) => Arc::new(Float32Array::new(v.clone().into(), nulls)),
            (SqlType::Double, Data::F64(v)) => Arc::new(Float64Array::new(v.clone().into(), nulls)),
            (SqlType::Decimal { precision, scale }, Data::Dec(v)) => {
                let (p, s) = (*precision, i8::try_from(*scale).map_err(|_| bad())?);
                if *precision <= 18 {
                    let vals: Vec<i64> = v.iter().map(|x| i64::try_from(*x).unwrap_or(0)).collect();
                    Arc::new(
                        Decimal64Array::new(vals.into(), nulls)
                            .with_precision_and_scale(p, s)
                            .map_err(|_| bad())?,
                    )
                } else {
                    Arc::new(
                        Decimal128Array::new(v.clone().into(), nulls)
                            .with_precision_and_scale(p, s)
                            .map_err(|_| bad())?,
                    )
                }
            }
            (SqlType::Varchar, Data::Str(s)) => {
                let it = (0..self.len()).map(|i| self.valid[i].then(|| s.get(i)));
                Arc::new(it.collect::<StringArray>())
            }
            (SqlType::Date, Data::Date(v)) => Arc::new(Date32Array::new(v.clone().into(), nulls)),
            _ => return Err(bad()),
        };
        Ok(arr)
    }
}
