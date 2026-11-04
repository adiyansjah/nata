// expect: clippy::disallowed_methods cast::cast`
pub fn f(a: &arrow_array::Int64Array) { let _ = arrow::compute::cast(a, &arrow_schema::DataType::Int32); }
