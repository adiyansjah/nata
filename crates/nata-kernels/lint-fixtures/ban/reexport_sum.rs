// expect: clippy::disallowed_methods aggregate::sum`
pub fn f(a: &arrow_array::Int64Array) { let _ = arrow::compute::sum(a); }
