// expect: clippy::disallowed_methods ArrowNativeTypeOp::mul_wrapping
use arrow_array::ArrowNativeTypeOp;
pub fn f(x: i64) -> i64 { x.mul_wrapping(2) }
