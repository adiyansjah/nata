// expect: clippy::disallowed_methods ArrowNativeTypeOp::mod_wrapping
use arrow_array::ArrowNativeTypeOp;
pub fn f(x: i64) -> i64 { x.mod_wrapping(2) }
