// expect: clippy::disallowed_methods cast_with_options
pub fn f(a: &arrow_array::Int64Array) {
    // the lenient default, spelled out: a lint cannot see `safe: true`, but it can see the function
    let o = arrow_cast::cast::CastOptions { safe: true, ..Default::default() };
    let _ = arrow_cast::cast::cast_with_options(a, &arrow_schema::DataType::Int32, &o);
}
