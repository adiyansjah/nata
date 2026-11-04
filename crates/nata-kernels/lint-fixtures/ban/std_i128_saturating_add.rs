// expect: clippy::disallowed_methods i128::saturating_add
pub fn f(x: i128, y: i128) -> i128 { x.saturating_add(y) }
