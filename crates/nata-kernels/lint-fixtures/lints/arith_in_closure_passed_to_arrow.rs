// expect: clippy::arithmetic_side_effects
pub fn f(a: &arrow_array::Int64Array) -> arrow_array::Int64Array { a.unary(|x| x + 1) }
