// expect: clippy::disallowed_methods i64::wrapping_mul
pub fn f(x: i64, y: i64) -> i64 { x.wrapping_mul(y) }
