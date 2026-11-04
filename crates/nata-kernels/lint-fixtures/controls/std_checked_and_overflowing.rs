// expect: pass
pub fn f(a: i64, b: i64) -> (Option<i64>, (i64, bool), Option<i64>) { (a.checked_add(b), a.overflowing_mul(b), a.checked_div(b)) }
