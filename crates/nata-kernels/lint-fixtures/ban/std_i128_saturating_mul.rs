// expect: clippy::disallowed_methods i128::saturating_mul
pub fn f(x: i128, y: i128) -> i128 { x.saturating_mul(y) }
