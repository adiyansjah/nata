// expect: clippy::disallowed_methods ArrowNativeTypeOp::add_wrapping
use arrow_array::ArrowNativeTypeOp;
pub fn f(x: i64) -> i64 { x.add_wrapping(2) }
