// expect: pass
pub fn f(a: i32, b: u8, c: i64) -> (i64, u32, Option<i32>) { (i64::from(a), u32::from(b), i32::try_from(c).ok()) }
