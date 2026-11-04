// expect: clippy::disallowed_methods i128::saturating_sub
pub fn f(x: i128, y: i128) -> i128 { x.saturating_sub(y) }
