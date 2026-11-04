// expect: clippy::disallowed_methods multiply_fixed_point`
pub fn f(a: &arrow_array::Decimal128Array) { let _ = arrow_arith::arithmetic::multiply_fixed_point(a, a, 0); }
