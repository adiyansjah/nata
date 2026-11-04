// expect: clippy::disallowed_methods ArrowNativeTypeOp::sub_wrapping
use arrow_array::ArrowNativeTypeOp;
pub fn f(x: i64) -> i64 { x.sub_wrapping(2) }
