// expect: pass
// KNOWN GAP: `abs` (MIN has no positive) and `pow` can overflow but are method calls the arithmetic lint
// does not look at. Only overflow-checks = true makes them panic (see overflow/abs_i32_min, overflow/pow_i32).
pub fn f(a: i64) -> (i64, i64) {
    (a.abs(), a.pow(3))
}
