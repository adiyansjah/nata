// expect: clippy::cast_possible_truncation
pub fn f(a: usize) -> u32 { a as u32 }
