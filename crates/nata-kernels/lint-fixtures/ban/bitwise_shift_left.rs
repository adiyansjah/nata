// expect: clippy::disallowed_methods bitwise_shift_left
pub fn f(a: &arrow_array::Int64Array) { let _ = arrow_arith::bitwise::bitwise_shift_left(a, a); }
