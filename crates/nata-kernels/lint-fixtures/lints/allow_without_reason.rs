// expect: clippy::allow_attributes_without_reason
#[allow(clippy::arithmetic_side_effects)]
pub fn f(a: i64, b: i64) -> i64 { a + b }
