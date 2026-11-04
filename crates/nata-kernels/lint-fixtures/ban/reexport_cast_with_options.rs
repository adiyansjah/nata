// expect: clippy::disallowed_methods cast_with_options
pub fn f(a: &arrow_array::Int64Array) { let _ = arrow::compute::cast_with_options(a, &arrow_schema::DataType::Int32, &Default::default()); }
