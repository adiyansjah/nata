// expect: clippy::cast_possible_truncation
pub fn f(a: i128) -> i64 { a as i64 }
