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
    current
        .checked_add(contribution)
        .is_some_and(|v| v < (1u128 << 127))
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    reason = "independent checked boundary-test reference"
)]
mod tests {
    use super::*;
    #[test]
    fn admission_never_allows_an_overflowing_prefix() {
        let ceiling = 1u128 << 127;
        for bound in [
            0,
            1,
            9,
            99,
            999,
            1_000_000_000_000_000_000,
            ceiling - 1,
            ceiling,
            u128::MAX,
        ] {
            for n in [0, 1, 2, 3, 4, 7, 8, 255, 256, u64::MAX] {
                for current in [0, 1, ceiling - 2, ceiling - 1, ceiling, u128::MAX] {
                    if batch_envelope(current, n, bound) {
                        let exact = bound
                            .checked_mul(u128::from(n))
                            .and_then(|v| v.checked_add(current));
                        assert!(
                            exact.is_some_and(|v| v < ceiling),
                            "current={current} n={n} bound={bound}"
                        );
                    }
                }
            }
        }
        assert!(batch_envelope(0, 1, ceiling - 1));
        assert!(batch_envelope(0, 0, u128::MAX));
        assert!(batch_envelope(0, 3, 9));
        assert!(!batch_envelope(ceiling - 1, 1, 1));
        assert!(!batch_envelope(ceiling, 0, 0));
        assert!(!batch_envelope(0, u64::MAX, u128::MAX));
    }
}
