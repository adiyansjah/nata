// expect: clippy::disallowed_methods i32::wrapping_mul
pub fn f(x: i32, y: i32) -> i32 { x.wrapping_mul(y) }
