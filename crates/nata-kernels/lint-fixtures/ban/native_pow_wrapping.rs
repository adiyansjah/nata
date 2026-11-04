// expect: clippy::disallowed_methods ArrowNativeTypeOp::pow_wrapping
use arrow_array::ArrowNativeTypeOp;
pub fn f(x: i64) -> i64 { x.pow_wrapping(2) }
