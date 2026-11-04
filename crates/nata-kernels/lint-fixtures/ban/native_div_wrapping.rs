// expect: clippy::disallowed_methods ArrowNativeTypeOp::div_wrapping
use arrow_array::ArrowNativeTypeOp;
pub fn f(x: i64) -> i64 { x.div_wrapping(2) }
