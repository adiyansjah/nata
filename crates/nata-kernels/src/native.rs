//! Typed buffer kernels. Masks contain selection AND validity, in ascending
//! row order; dispatch and Arrow bitmap offsets belong at the batch boundary.
use crate::decimal::{Fail, Op};

/// Visit bitmap words, with a contiguous fast path for a fully active word.
/// The last word's padding is ignored. No values under inactive bits are read.
pub fn selected<E>(
    mask: &[u64],
    len: usize,
    mut visit: impl FnMut(usize) -> Result<(), E>,
) -> Result<(), E> {
    for (word, &bits) in mask.iter().enumerate() {
        let base = word.checked_mul(64).expect("mask length fits usize");
        let end = len.min(base.checked_add(64).expect("batch length fits usize"));
        if bits == u64::MAX {
            for row in base..end {
                visit(row)?;
            }
        } else {
            let mut bits = bits;
            while bits != 0 {
                let bit = bits.trailing_zeros();
                let row = base
                    .checked_add(bit as usize)
                    .expect("batch length fits usize");
                if row < len {
                    visit(row)?;
                }
                bits &= !(1u64 << bit);
            }
        }
    }
    Ok(())
}

/// Native decimal arithmetic only when the result and alignment factors fit
/// Decimal64. Checked i64 overflow implies decimal precision overflow; add/sub
/// still check each aligned operand before cancellation (decimal-rules §3).
#[derive(Clone, Copy)]
pub struct Decimal64 {
    op: Op,
    left: i64,
    right: i64,
    limit: u64,
}
impl Decimal64 {
    pub fn new(op: Op, sa: u8, sb: u8, p: u8, s: u8) -> Option<Self> {
        if !(1..=18).contains(&p) || s > p {
            return None;
        }
        if !matches!(op, Op::Add | Op::Sub | Op::Mul) {
            return None;
        }
        if op == Op::Mul && sa.checked_add(sb)? != s {
            return None;
        }
        if op != Op::Mul && (sa > s || sb > s) {
            return None;
        }
        Some(Self {
            op,
            left: 10i64.checked_pow(u32::from(s.saturating_sub(sa)))?,
            right: 10i64.checked_pow(u32::from(s.saturating_sub(sb)))?,
            limit: 10u64.checked_pow(u32::from(p))?,
        })
    }
    fn check(self, value: Option<i64>) -> Result<i64, Fail> {
        value
            .filter(|v| v.unsigned_abs() < self.limit)
            .ok_or(Fail::Overflow)
    }
    /// Arithmetic leaf used by the typed column loop. Precision is checked in
    /// addition to machine overflow; no validity or input representation here.
    #[inline(always)]
    pub fn product(self, x: i64, y: i64) -> Result<i64, Fail> {
        self.check(x.checked_mul(y))
    }
    pub fn column(self, a: &[i64], b: &[i64], mask: &[u64], out: &mut [i64]) -> Result<(), Fail> {
        match self.op {
            Op::Mul => selected(mask, out.len(), |i| {
                out[i] = self.product(a[i], b[i])?;
                Ok(())
            }),
            Op::Add => selected(mask, out.len(), |i| {
                let x = self.check(a[i].checked_mul(self.left))?;
                let y = self.check(b[i].checked_mul(self.right))?;
                out[i] = self.check(x.checked_add(y))?;
                Ok(())
            }),
            Op::Sub => selected(mask, out.len(), |i| {
                let x = self.check(a[i].checked_mul(self.left))?;
                let y = self.check(b[i].checked_mul(self.right))?;
                out[i] = self.check(x.checked_sub(y))?;
                Ok(())
            }),
            _ => unreachable!("constructor restricts operations"),
        }
    }
}

/// Feed typed inputs into the exact accumulator consumer. `rows` is sorted,
/// unique and in bounds. The optional word mask is selection AND validity;
/// when present, its active rows must be a subset of `rows`. The callback gets
/// the selection ordinal (the index into the caller's admitted group IDs).
/// No-NULL inputs use the selection directly: no bitmap or row/ordinal merge.
#[inline(always)]
pub fn numbers<T: Copy + Into<i128>, E>(
    values: &[T],
    rows: &[u32],
    mask: Option<&[u64]>,
    mut add: impl FnMut(usize, i128) -> Result<(), E>,
) -> Result<(), E> {
    if let Some(mask) = mask {
        let mut ordinal = 0;
        selected(mask, values.len(), |row| {
            while rows[ordinal] as usize != row {
                ordinal = ordinal
                    .checked_add(1)
                    .expect("selection ordinal fits usize");
            }
            let current = ordinal;
            ordinal = ordinal
                .checked_add(1)
                .expect("selection ordinal fits usize");
            add(current, values[row].into())
        })
    } else {
        for (ordinal, &row) in rows.iter().enumerate() {
            add(ordinal, values[row as usize].into())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decimal::Decimal;
    #[test]
    fn native_matches_checked_wide_including_precision_and_machine_overflow() {
        for op in [Op::Add, Op::Sub, Op::Mul] {
            for (sa, sb, p, s) in [(0, 0, 18, 0), (0, 2, 18, 2), (2, 2, 18, 4)] {
                let Some(k) = Decimal64::new(op, sa, sb, p, s) else {
                    continue;
                };
                let wide = Decimal::new(op, sa, sb, p, s).expect("type");
                for a in [
                    i64::MIN,
                    -1_000_000_000_000_000_000,
                    -100,
                    -1,
                    0,
                    1,
                    100,
                    999_999_999_999_999_999,
                    i64::MAX,
                ] {
                    for b in [i64::MIN, -100, -1, 0, 1, 100, i64::MAX] {
                        let mut out = [0];
                        assert_eq!(
                            k.column(&[a], &[b], &[1], &mut out)
                                .map(|()| i128::from(out[0])),
                            wide.scalar(i128::from(a), i128::from(b)),
                            "{op:?} {a} {b}"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn word_boundaries_padding_and_masked_junk() {
        // A callback may update SUM/COUNT: every active row must be visited
        // exactly once. This assertion also makes a broken bit-clear terminate
        // the test before the arithmetic loop can spin on a repeated row.
        let mut seen = [false; 130];
        selected(&[1u64 | (1 << 63), 1, u64::MAX ^ 1], 130, |row| {
            assert!(!seen[row], "each active row is visited once");
            seen[row] = true;
            Ok::<_, Fail>(())
        })
        .expect("bitmap traversal");
        assert_eq!(seen.iter().filter(|&&v| v).count(), 4);
        for row in [0, 63, 64, 129] {
            assert!(seen[row]);
        }
        let mut a = [i64::MAX; 130];
        let b = [2; 130];
        let mut out = [0; 130];
        for i in [0, 63, 64, 129] {
            a[i] = 3;
        }
        let k = Decimal64::new(Op::Mul, 0, 0, 18, 0).expect("type");
        k.column(&a, &b, &[1 | (1 << 63), 1, u64::MAX ^ 1], &mut out)
            .expect("masked");
        for i in [0, 63, 64, 129] {
            assert_eq!(out[i], 6);
        }
        assert_eq!(out[65], 0);
        let mut visited = vec![];
        let values: [i128; 65] = std::array::from_fn(|i| i128::try_from(i).expect("bounded"));
        let rows: Vec<u32> = (0..65).collect();
        numbers(&values, &rows, None, |i, v| {
            visited.push((i, v));
            Ok::<_, Fail>(())
        })
        .expect("numbers");
        assert_eq!(visited.len(), 65);
        assert_eq!(visited[64], (64, 64));
    }
}

#[cfg(test)]
mod selection_offsets {
    use super::*;
    #[test]
    fn sparse_selection_ordinals_skip_nulls_without_losing_group_ids() {
        let values: [i128; 130] = std::array::from_fn(|i| i128::try_from(i).expect("bounded"));
        let rows = [0, 63, 64, 129];
        for (mask, expected) in [
            (None, vec![(0, 0), (1, 63), (2, 64), (3, 129)]),
            (Some(&[1u64, 1, 2][..]), vec![(0, 0), (2, 64), (3, 129)]),
        ] {
            let mut got = vec![];
            numbers(&values, &rows, mask, |offset, v| {
                got.push((offset, v));
                Ok::<_, Fail>(())
            })
            .expect("typed inputs");
            assert_eq!(got, expected);
        }
    }
}
