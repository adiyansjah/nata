// expect: clippy::disallowed_methods aggregate::sum`
pub fn f(a: &arrow_array::Int64Array) { let _ = arrow_arith::aggregate::sum(a); }
