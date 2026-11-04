// expect: clippy::disallowed_methods i64::wrapping_sub
pub fn f(x: i64, y: i64) -> i64 { x.wrapping_sub(y) }
