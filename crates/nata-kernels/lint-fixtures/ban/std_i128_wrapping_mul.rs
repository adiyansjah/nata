// expect: clippy::disallowed_methods i128::wrapping_mul
pub fn f(x: i128, y: i128) -> i128 { x.wrapping_mul(y) }
