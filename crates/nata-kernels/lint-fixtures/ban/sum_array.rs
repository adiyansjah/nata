// expect: clippy::disallowed_methods sum_array
pub fn f(a: &arrow_array::Int64Array) { let _ = arrow_arith::aggregate::sum_array::<arrow_array::types::Int64Type, _>(a); }
