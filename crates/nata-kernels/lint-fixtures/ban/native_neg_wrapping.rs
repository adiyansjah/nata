// expect: clippy::disallowed_methods ArrowNativeTypeOp::neg_wrapping
use arrow_array::ArrowNativeTypeOp;
pub fn f(x: i64) -> i64 { x.neg_wrapping() }
