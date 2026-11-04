// expect: clippy::disallowed_methods i128::wrapping_neg
pub fn f(x: i128) -> i128 { x.wrapping_neg() }
