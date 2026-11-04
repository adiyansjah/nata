// expect: clippy::disallowed_methods aggregate::product`
pub fn f(a: &arrow_array::Int64Array) { let _ = arrow_arith::aggregate::product(a); }
