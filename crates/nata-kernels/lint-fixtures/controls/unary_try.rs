// expect: pass
pub fn f(a: &arrow_array::Int64Array) -> Result<arrow_array::Int64Array, arrow_schema::ArrowError> { a.try_unary(|x| x.checked_add(1).ok_or_else(|| arrow_schema::ArrowError::ComputeError("overflow".into()))) }
