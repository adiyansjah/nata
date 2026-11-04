// expect: pass
#[allow(clippy::arithmetic_side_effects, reason = "a + b cannot overflow here: both are below 2^31")]
pub fn f(a: u8, b: u8) -> u32 { u32::from(a) + u32::from(b) }
