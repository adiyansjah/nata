//! Scalar reference: row by row through arrow's own typed accessors (`is_null`, `value`), sharing nothing with the
//! passes in `kernels.rs` except the pure per-value functions (which define what is computed).

use arrow::array::{Array, ArrayRef, AsArray};
use arrow::datatypes::{DataType, Decimal64Type, Decimal128Type, Int32Type, Int64Type};

use crate::kernels::{combine, g_bytes_dyn, g_i64_dyn, g_i128_dyn};

/// The logical value of one row.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Lv {
    Null,
    Int(i128),
    Bytes(Vec<u8>),
}

pub fn logical(a: &dyn Array, i: usize) -> Lv {
    if a.is_null(i) {
        return Lv::Null;
    }
    match a.data_type() {
        DataType::Int64 => Lv::Int(i128::from(a.as_primitive::<Int64Type>().value(i))),
        DataType::Decimal64(..) => Lv::Int(i128::from(a.as_primitive::<Decimal64Type>().value(i))),
        DataType::Decimal128(..) => Lv::Int(a.as_primitive::<Decimal128Type>().value(i)),
        DataType::Utf8 => Lv::Bytes(a.as_string::<i32>().value(i).as_bytes().to_vec()),
        DataType::Dictionary(..) => {
            let d = a.as_dictionary::<Int32Type>();
            let key = usize::try_from(d.keys().value(i)).expect("non-negative key");
            Lv::Bytes(d.values().as_string::<i32>().value(key).as_bytes().to_vec())
        }
        t => panic!("unsupported type {t:?}"),
    }
}

/// `combine` of functions `0..nf` over `rows` of `col`.
pub fn reference(col: &ArrayRef, rows: &[usize], nf: usize) -> u64 {
    combine((0..nf).map(|f| {
        rows.iter().fold(0u64, |acc, &i| match logical(col.as_ref(), i) {
            Lv::Null => acc,
            Lv::Int(x) => {
                // Int64 and Decimal64 hold an i64; Decimal128 an i128.
                let g = if matches!(col.data_type(), DataType::Decimal128(..)) { g_i128_dyn(f, x) } else { g_i64_dyn(f, i64::try_from(x).expect("i64 storage")) };
                acc.wrapping_add(g)
            }
            Lv::Bytes(b) => acc.wrapping_add(g_bytes_dyn(f, &b)),
        })
    }))
}
