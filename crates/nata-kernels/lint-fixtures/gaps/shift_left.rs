// expect: pass
// KNOWN GAP: clippy::arithmetic_side_effects does not flag `<<`. If a newer clippy starts to, this fixture
// fails and tells us to move it to lints/. overflow-checks catches a shift amount >= the bit width (see
// overflow/shl_amount) but NOT bits shifted out (overflow/shl_bits_lost).
pub fn f(a: i64, n: u32) -> i64 {
    a << n
}
