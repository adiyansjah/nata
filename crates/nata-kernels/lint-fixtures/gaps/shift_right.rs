// expect: pass
// KNOWN GAP: `>>` is not flagged either (see gaps/shift_left.rs).
pub fn f(a: i64, n: u32) -> i64 {
    a >> n
}
