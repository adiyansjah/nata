// expect: clippy::disallowed_methods i64::wrapping_neg
pub fn f(x: i64) -> i64 { x.wrapping_neg() }
