// expect: clippy::disallowed_methods div_rem_wrapping
use arrow_array::ArrowNativeTypeOp;
pub fn f(x: i64) -> (i64, i64) { x.div_rem_wrapping(2) }
