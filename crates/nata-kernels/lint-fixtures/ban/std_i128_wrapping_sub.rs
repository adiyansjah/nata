// expect: clippy::disallowed_methods i128::wrapping_sub
pub fn f(x: i128, y: i128) -> i128 { x.wrapping_sub(y) }
