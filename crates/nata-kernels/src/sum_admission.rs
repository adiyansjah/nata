//! Conservative batch envelope: round the selected count upward to a power
//! of two, then use checked unsigned addition. No symbolic wide multiply is
//! needed by the admission proof. Rejection always means use checked promotion.
pub fn batch_envelope(current: u128, n: u64, bound: u128) -> bool {
    if n == 0 {
        return current < (1u128 << 127);
    }
    let shift = 64u32.saturating_sub(n.saturating_sub(1).leading_zeros());
    if bound.leading_zeros() < shift {
        return false;
    }
    let contribution = bound << shift;
    current.checked_add(contribution).is_some_and(|v| v < (1u128 << 127))
}
