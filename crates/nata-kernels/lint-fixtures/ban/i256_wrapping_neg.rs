// expect: clippy::disallowed_methods i256::wrapping_neg
pub fn f(x: arrow_buffer::i256) -> arrow_buffer::i256 { x.wrapping_neg() }
