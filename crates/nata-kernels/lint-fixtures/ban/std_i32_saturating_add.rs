// expect: clippy::disallowed_methods i32::saturating_add
pub fn f(x: i32, y: i32) -> i32 { x.saturating_add(y) }
