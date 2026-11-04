// expect: clippy::cast_possible_truncation
pub fn f(a: f64) -> i64 { a as i64 }
