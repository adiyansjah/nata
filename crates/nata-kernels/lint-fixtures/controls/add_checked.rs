// expect: pass
pub fn f(a: &arrow_array::Int64Array) -> Result<arrow_array::ArrayRef, arrow_schema::ArrowError> { arrow_arith::numeric::add(a, a) }
