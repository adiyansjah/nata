// expect: clippy::disallowed_methods sub_wrapping
pub fn f(a: &arrow_array::Int64Array) { let _ = arrow_arith::numeric::sub_wrapping(a, a); }
