// expect: pass
#[allow(clippy::disallowed_methods, reason = "the single sanctioned caller; safe: false is set explicitly")]
pub fn strict(a: &arrow_array::Int64Array) -> Result<arrow_array::ArrayRef, arrow_schema::ArrowError> {
    let o = arrow_cast::cast::CastOptions { safe: false, ..Default::default() };
    arrow_cast::cast::cast_with_options(a, &arrow_schema::DataType::Int32, &o)
}
