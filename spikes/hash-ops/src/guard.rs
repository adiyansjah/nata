//! Configuration guard: this crate's own arithmetic must stay overflow-checked (ADR 0003 decision 6).
//! `tests/overflow_config.rs` calls this and requires a panic. It is `inline(never)` and takes its operands
//! from the caller, so the compiler cannot fold the addition away.

#[inline(never)]
pub fn plain_add(a: i64, b: i64) -> i64 {
    a + b
}
