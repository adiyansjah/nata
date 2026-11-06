//! Proof by induction that `wrapadd::add128::<true>` and `wrapadd::add64::<true>` (the step-6 sink of the NULL-path
//! spike) never wrap, so they equal the checked `+`. Each harness checks ONE induction step for ALL values (symbolic
//! `k`, partial sum and addend), so together with the base case the claim holds for every number of addends below 2^64.
//!
//! Invariant after `k` addends, each an `i64` value or 0:  -k * 2^63  <=  s  <=  k * (2^63 - 1).
//! For `k <= 2^64 - 1` both bounds fit in `i128` (|k * 2^63| <= 2^127 - 2^63), so the interval is inside `i128`.

#[path = "../../null-path/src/wrapadd.rs"]
pub mod wrapadd;

/// The invariant's bounds for `k` addends, computed exactly in `i128` (they never overflow for `k < 2^64`).
pub fn lo(k: u64) -> i128 { -((k as i128) << 63) }
pub fn hi(k: u64) -> i128 { ((k as i128) << 63) - (k as i128) }

#[cfg(kani)]
mod proofs {
    use super::*;

    /// Base case: zero addends, the sum is 0 and inside the bounds of k = 0.
    #[kani::proof]
    fn sum_base_case() {
        let s: i128 = 0;
        assert!(lo(0) <= s && s <= hi(0));
    }

    /// Step: from any sum inside the bounds of k (k < 2^64 - 1) and any addend that is an i64 value or 0, the wrapping
    /// add equals the exact (checked) add and the result is inside the bounds of k + 1.
    #[kani::proof]
    fn sum_step_never_wraps() {
        let k: u64 = kani::any();
        kani::assume(k < u64::MAX);
        let s: i128 = kani::any();
        kani::assume(lo(k) <= s && s <= hi(k));
        let v: i64 = kani::any();
        let keep: bool = kani::any();
        // the two addend shapes of wk::group_sums: `v * ok` and `v & -(bit)`; both are v or 0
        let x: i128 = if kani::any() { (v as i128) * (keep as i128) } else { (v as i128) & -(keep as i128) };
        let exact = s.checked_add(x);
        assert!(exact.is_some(), "the exact sum must not overflow");
        let mut w = s;
        wrapadd::add128::<true>(&mut w, x);
        assert_eq!(Some(w), exact, "wrapping add differs from the exact add");
        assert!(lo(k + 1) <= w && w <= hi(k + 1), "the invariant must hold after the step");
    }

    /// The bounds themselves stay inside i128 for every k (no overflow in the invariant's own arithmetic).
    #[kani::proof]
    fn bounds_fit_in_i128() {
        let k: u64 = kani::any();
        let l = ((k as i128) << 63).checked_neg();
        assert!(l.is_some());
        assert!(((k as i128) << 63).checked_sub(k as i128).is_some());
        assert!(lo(k) <= 0 && hi(k) >= 0);
    }

    /// Counters: an addend of 0 or 1 to a counter that is at most the number of rows processed, which is below
    /// u64::MAX, never wraps and equals the checked add.
    #[kani::proof]
    fn counter_step_never_wraps() {
        let c: u64 = kani::any();
        let rows_done: u64 = kani::any();
        kani::assume(rows_done < u64::MAX && c <= rows_done);
        let bit: u64 = kani::any::<u64>() & 1;
        let exact = c.checked_add(bit);
        let mut w = c;
        wrapadd::add64::<true>(&mut w, bit);
        assert_eq!(Some(w), exact);
        assert!(w <= rows_done + 1);
    }

    /// The checked variant (WRAP = false) agrees with the wrapping one wherever both are defined (same function file).
    #[kani::proof]
    fn checked_and_wrapping_agree() {
        let s: i128 = kani::any::<i64>() as i128;
        let x: i128 = kani::any::<i64>() as i128;
        let mut a = s;
        let mut b = s;
        wrapadd::add128::<true>(&mut a, x);
        wrapadd::add128::<false>(&mut b, x);
        assert_eq!(a, b);
    }

    /// Sanity (must FAIL, run by mutate.sh): with an addend that may be any i128, the step does wrap. If this ever
    /// passed, the harnesses above would be vacuous.
    #[kani::proof]
    #[cfg(feature = "negative")]
    fn negative_control_wide_addend_wraps() {
        let k: u64 = kani::any();
        kani::assume(k < u64::MAX);
        let s: i128 = kani::any();
        kani::assume(lo(k) <= s && s <= hi(k));
        let x: i128 = kani::any();
        assert!(s.checked_add(x).is_some());
    }
}
