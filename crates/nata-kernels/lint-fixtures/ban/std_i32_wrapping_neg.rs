// expect: clippy::disallowed_methods i32::wrapping_neg
pub fn f(x: i32) -> i32 { x.wrapping_neg() }
