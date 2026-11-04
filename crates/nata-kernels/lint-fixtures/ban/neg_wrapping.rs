// expect: clippy::disallowed_methods neg_wrapping
pub fn f(a: &arrow_array::Int64Array) { let _ = arrow_arith::numeric::neg_wrapping(a); }
