// expect: clippy::disallowed_methods i64::saturating_sub
pub fn f(x: i64, y: i64) -> i64 { x.saturating_sub(y) }
