// expect: clippy::disallowed_methods cast_with_options
pub fn f(a: &arrow_array::Int64Array) {
    let o = arrow_cast::cast::CastOptions { safe: false, ..Default::default() };
    let _ = arrow_cast::cast::cast_with_options(a, &arrow_schema::DataType::Int32, &o);
}
