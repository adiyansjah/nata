// expect: clippy::disallowed_methods unary_opt
pub fn f(a: &arrow_array::Int64Array) { let _ = a.unary_opt::<_, arrow_array::types::Int64Type>(|x| x.checked_add(1)); }
