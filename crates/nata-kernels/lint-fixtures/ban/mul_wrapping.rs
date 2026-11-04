// expect: clippy::disallowed_methods mul_wrapping
pub fn f(a: &arrow_array::Int64Array) { let _ = arrow_arith::numeric::mul_wrapping(a, a); }
