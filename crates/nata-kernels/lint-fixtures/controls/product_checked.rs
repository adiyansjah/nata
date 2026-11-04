// expect: pass
pub fn f(a: &arrow_array::Int64Array) -> Result<Option<i64>, arrow_schema::ArrowError> { arrow_arith::aggregate::product_checked(a) }
