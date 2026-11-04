// expect: clippy::arithmetic_side_effects
pub fn f(a: &mut i64, b: i64) { *a += b; }
