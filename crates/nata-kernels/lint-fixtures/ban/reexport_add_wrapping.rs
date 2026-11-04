// expect: clippy::disallowed_methods add_wrapping
pub fn f(a: &arrow_array::Int64Array) { let _ = arrow::compute::kernels::numeric::add_wrapping(a, a); }
