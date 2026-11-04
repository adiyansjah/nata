//! Wrapping arrow-rs kernels so they follow the selection rule (ADR 0003, decision 4).
//!
//! arrow-rs does not know about our selection vector: it computes on every slot that is valid in its own
//! null buffer, so a row the selection removed (`WHERE x <> 0 AND 1 / x > 1`) would still raise a division
//! error. A wrapper must either compact the input first or fold the selection into the null buffer.
use crate::kernels::{Col, Valid};
use arrow_arith::numeric::div;
use arrow_array::cast::AsArray;
use arrow_array::types::Int64Type;
use arrow_array::{Array, Int64Array, UInt32Array};
use arrow_buffer::{BooleanBuffer, NullBuffer, ScalarBuffer};
use arrow_schema::ArrowError;
use arrow_select::take::take;

/// Builds an arrow array from a kernel column (values and validity).
pub fn to_array(col: Col<i64>) -> Int64Array {
    let (v, valid) = col;
    let nulls = match valid {
        Valid::All => None,
        Valid::Bits(n) => Some(n.clone()),
        Valid::Bytes(b) => Some(NullBuffer::new(BooleanBuffer::from_iter(
            b.iter().map(|&x| x != 0),
        ))),
    };
    Int64Array::new(ScalarBuffer::from(v.to_vec()), nulls)
}

/// The WRONG way: arrow's checked `div` over every slot, ignoring the selection.
/// Kept to show why the rule exists; nothing in the engine may call it.
pub fn div_uncompacted(a: &Int64Array, b: &Int64Array) -> Result<Int64Array, ArrowError> {
    Ok(div(a, b)?.as_primitive::<Int64Type>().clone())
}

/// Compacts both inputs to the selected rows, then divides. The result has one slot per selected row,
/// in selection order.
pub fn div_compacted(
    a: &Int64Array,
    b: &Int64Array,
    sel: Option<&[u32]>,
) -> Result<Int64Array, ArrowError> {
    let Some(sel) = sel else {
        return div_uncompacted(a, b);
    };
    let idx = UInt32Array::from(sel.to_vec());
    let a = take(a, &idx, None)?;
    let b = take(b, &idx, None)?;
    Ok(div(&a, &b)?.as_primitive::<Int64Type>().clone())
}

/// Folds the selection into the null buffers (unselected rows become NULL), then divides. The result is
/// full length, NULL on every unselected row. Valid only if arrow skips NULL slots; a test checks that.
pub fn div_folded(
    a: &Int64Array,
    b: &Int64Array,
    sel: Option<&[u32]>,
) -> Result<Int64Array, ArrowError> {
    let Some(sel) = sel else {
        return div_uncompacted(a, b);
    };
    let mut keep = vec![false; a.len()];
    for &i in sel {
        if let Some(k) = keep.get_mut(i as usize) {
            *k = true;
        }
    }
    let keep = NullBuffer::new(BooleanBuffer::from(keep));
    let fold = |x: &Int64Array| {
        Int64Array::new(
            x.values().clone(),
            NullBuffer::union(x.nulls(), Some(&keep)),
        )
    };
    Ok(div(&fold(a), &fold(b))?.as_primitive::<Int64Type>().clone())
}
