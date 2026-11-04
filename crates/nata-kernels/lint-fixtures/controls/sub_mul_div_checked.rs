// expect: pass
pub fn f(a: &arrow_array::Int64Array) -> Result<(), arrow_schema::ArrowError> {
    arrow_arith::numeric::sub(a, a)?;
    arrow_arith::numeric::mul(a, a)?;
    arrow_arith::numeric::div(a, a)?;
    Ok(())
}
