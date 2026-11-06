//! The two accumulations of the grouped-sum sink (`wk::group_sums`). With `WRAP = false` they are the checked `+`
//! (a panic on overflow, because `overflow-checks` is on); with `WRAP = true` (step 6) they are `wrapping_add`, which is
//! only correct if it can never wrap. The argument, proved by Kani in `spikes/kani-wrap-bound` on THIS file (included
//! there by `#[path]`):
//!
//! - every `i128` addend is an `i64` value or 0, and a sum of `k` such addends lies in `[-k * 2^63, k * (2^63 - 1)]`;
//!   for `k < 2^64` that interval is inside `i128`, so the wrapping and the checked add give the same result;
//! - every `u64` addend is 0 or 1 and a counter is at most the number of rows processed, so it stays below `u64::MAX`.

#[inline(always)]
pub fn add128<const WRAP: bool>(a: &mut i128, x: i128) {
    if WRAP { *a = a.wrapping_add(x) } else { *a += x }
}

#[inline(always)]
pub fn add64<const WRAP: bool>(a: &mut u64, x: u64) {
    if WRAP { *a = a.wrapping_add(x) } else { *a += x }
}
