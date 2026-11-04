//! Checked, NULL-aware, selection-aware kernels.
//!
//! Rules (ADR 0003, decision 4; docs/decimal-rules.md):
//! - an arithmetic overflow is an error, but only for a row that is selected AND valid;
//! - a NULL input gives a NULL output; a NULL in a filter column drops the row;
//! - the selection is a vector of row indexes (None = every row); kernels never touch other rows.
//!
//! Error checks are branch-free: every selected row computes, and an overflow flag is masked by validity.
use arrow_buffer::NullBuffer;

#[derive(Debug, Clone, PartialEq)]
pub struct EngineError(pub String);
pub type R<T> = Result<T, EngineError>;

pub fn overflow<T>(what: &str) -> R<T> {
    Err(EngineError(format!("overflow in {what}")))
}

#[derive(Clone, Copy)]
pub enum Valid<'a> {
    All,
    Bits(&'a NullBuffer),
    Bytes(&'a [u8]),
}

impl Valid<'_> {
    #[inline(always)]
    pub fn ok(&self, i: usize) -> bool {
        match *self {
            Valid::All => true,
            Valid::Bits(n) => n.is_valid(i),
            Valid::Bytes(b) => b[i] != 0,
        }
    }

    pub fn is_all(&self) -> bool {
        matches!(self, Valid::All)
    }
}

pub fn from_nulls(n: Option<&NullBuffer>) -> Valid<'_> {
    n.map_or(Valid::All, |n| {
        if n.null_count() == 0 {
            Valid::All
        } else {
            Valid::Bits(n)
        }
    })
}

pub type Col<'a, T> = (&'a [T], Valid<'a>);

/// A computed column: full length, only the selected positions are meaningful.
#[derive(Debug, Clone, PartialEq)]
pub struct Out {
    pub v: Vec<i64>,
    pub valid: Option<Vec<u8>>,
}

impl Out {
    pub fn col(&self) -> Col<'_, i64> {
        (
            &self.v,
            self.valid.as_deref().map_or(Valid::All, Valid::Bytes),
        )
    }
}

#[inline(always)]
pub fn each(rows: usize, sel: Option<&[u32]>, mut f: impl FnMut(usize)) {
    match sel {
        None => (0..rows).for_each(&mut f),
        Some(s) => s.iter().for_each(|&i| f(i as usize)),
    }
}

/// `c - a`
pub fn sub_const_left(c: i64, a: Col<i64>, rows: usize, sel: Option<&[u32]>) -> R<Out> {
    let (a, va) = a;
    let mut v = vec![0i64; rows];
    let mut bad = false;
    if va.is_all() {
        each(rows, sel, |i| {
            let (r, o) = c.overflowing_sub(a[i]);
            v[i] = r;
            bad |= o;
        });
        if bad {
            return overflow("subtraction");
        }
        return Ok(Out { v, valid: None });
    }
    let mut valid = vec![0u8; rows];
    each(rows, sel, |i| {
        let ok = va.ok(i);
        let (r, o) = c.overflowing_sub(a[i]);
        v[i] = r;
        valid[i] = ok as u8;
        bad |= o & ok;
    });
    if bad {
        return overflow("subtraction");
    }
    Ok(Out {
        v,
        valid: Some(valid),
    })
}

/// `a * b`
pub fn mul(a: Col<i64>, b: Col<i64>, rows: usize, sel: Option<&[u32]>) -> R<Out> {
    let ((a, va), (b, vb)) = (a, b);
    let mut v = vec![0i64; rows];
    let mut bad = false;
    if va.is_all() && vb.is_all() {
        each(rows, sel, |i| {
            let (r, o) = a[i].overflowing_mul(b[i]);
            v[i] = r;
            bad |= o;
        });
        if bad {
            return overflow("multiplication");
        }
        return Ok(Out { v, valid: None });
    }
    let mut valid = vec![0u8; rows];
    each(rows, sel, |i| {
        let ok = va.ok(i) & vb.ok(i);
        let (r, o) = a[i].overflowing_mul(b[i]);
        v[i] = r;
        valid[i] = ok as u8;
        bad |= o & ok;
    });
    if bad {
        return overflow("multiplication");
    }
    Ok(Out {
        v,
        valid: Some(valid),
    })
}

/// Why a division failed. Kept separate so the message names the cause.
#[derive(Clone, Copy, Default)]
struct DivFlags {
    zero: bool,
    overflow: bool,
}

impl DivFlags {
    fn into_result(self) -> R<()> {
        if self.zero {
            Err(EngineError("division by zero".to_string()))
        } else if self.overflow {
            overflow("division")
        } else {
            Ok(())
        }
    }
}

/// One checked division step. The quotient is 0 when the division fails; the flags say why, and the
/// caller masks them with validity.
#[inline(always)]
fn div_step(a: i64, b: i64) -> (i64, bool, bool) {
    // `checked_div` is None for a zero divisor and for `i64::MIN / -1`, the only two failures.
    let q = a.checked_div(b).unwrap_or(0);
    (q, b == 0, a == i64::MIN && b == -1)
}

/// `a / b` (integer division truncating toward zero). Zero divisor and `i64::MIN / -1` are errors.
pub fn div(a: Col<i64>, b: Col<i64>, rows: usize, sel: Option<&[u32]>) -> R<Out> {
    let ((a, va), (b, vb)) = (a, b);
    let mut v = vec![0i64; rows];
    let mut flags = DivFlags::default();
    if va.is_all() && vb.is_all() {
        each(rows, sel, |i| {
            let (q, z, o) = div_step(a[i], b[i]);
            v[i] = q;
            flags.zero |= z;
            flags.overflow |= o;
        });
        flags.into_result()?;
        return Ok(Out { v, valid: None });
    }
    let mut valid = vec![0u8; rows];
    each(rows, sel, |i| {
        let ok = va.ok(i) & vb.ok(i);
        let (q, z, o) = div_step(a[i], b[i]);
        v[i] = q;
        valid[i] = ok as u8;
        flags.zero |= z & ok;
        flags.overflow |= o & ok;
    });
    flags.into_result()?;
    Ok(Out {
        v,
        valid: Some(valid),
    })
}

/// `c / a`
pub fn div_const_left(c: i64, a: Col<i64>, rows: usize, sel: Option<&[u32]>) -> R<Out> {
    let (a, va) = a;
    let mut v = vec![0i64; rows];
    let mut flags = DivFlags::default();
    if va.is_all() {
        each(rows, sel, |i| {
            let (q, z, o) = div_step(c, a[i]);
            v[i] = q;
            flags.zero |= z;
            flags.overflow |= o;
        });
        flags.into_result()?;
        return Ok(Out { v, valid: None });
    }
    let mut valid = vec![0u8; rows];
    each(rows, sel, |i| {
        let ok = va.ok(i);
        let (q, z, o) = div_step(c, a[i]);
        v[i] = q;
        valid[i] = ok as u8;
        flags.zero |= z & ok;
        flags.overflow |= o & ok;
    });
    flags.into_result()?;
    Ok(Out {
        v,
        valid: Some(valid),
    })
}

/// Rows of the input selection where `col <> value` is TRUE. A NULL comparison is not TRUE, so the row is dropped.
pub fn filter_ne<T: Copy + PartialEq>(
    col: &[T],
    valid: Valid,
    value: T,
    rows: usize,
    sel: Option<&[u32]>,
) -> Vec<u32> {
    split_ne(col, valid, value, rows, sel).0
}

/// Splits the input selection by `col <> value`: (rows where it is TRUE, every other row of the selection,
/// including rows where the comparison is NULL). This is `CASE WHEN col <> value THEN .. ELSE .. END`:
/// each branch then runs only over its own rows, so an error in the other branch's rows cannot occur.
#[allow(
    clippy::cast_possible_truncation,
    reason = "row indexes are below rows, which the assert keeps under 2^32"
)]
pub fn split_ne<T: Copy + PartialEq>(
    col: &[T],
    valid: Valid,
    value: T,
    rows: usize,
    sel: Option<&[u32]>,
) -> (Vec<u32>, Vec<u32>) {
    assert!(
        u32::try_from(rows).is_ok(),
        "a batch has fewer than 2^32 rows (selection indexes are u32)"
    );
    let (mut yes, mut no) = (Vec::new(), Vec::new());
    each(rows, sel, |i| {
        if valid.ok(i) && col[i] != value {
            yes.push(i as u32);
        } else {
            no.push(i as u32);
        }
    });
    (yes, no)
}

/// Combines the results of `CASE` branches: each part is a selection and the column computed over it.
/// Rows in no part stay NULL. If parts overlap (CASE branches never do), the later part wins.
pub fn merge_branches(rows: usize, parts: &[(&[u32], &Out)]) -> Out {
    let mut v = vec![0i64; rows];
    let mut valid = vec![0u8; rows];
    for (sel, out) in parts {
        for &i in *sel {
            let i = i as usize;
            v[i] = out.v[i];
            valid[i] = out.valid.as_ref().map_or(1, |x| x[i]);
        }
    }
    Out {
        v,
        valid: Some(valid),
    }
}

/// A column holding the constant `c` on the rows of `sel` (the `ELSE 0` branch).
pub fn constant(c: i64, rows: usize) -> Out {
    Out {
        v: vec![c; rows],
        valid: None,
    }
}

/// Rows of the input selection whose value lies in `[lo, hi]` and is not NULL.
#[allow(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    reason = "k counts kept rows, so k <= n <= rows; the row index fits u32 by the assert"
)]
pub fn filter_range<T: Copy + PartialOrd>(
    col: &[T],
    valid: Valid,
    lo: T,
    hi: T,
    rows: usize,
    sel: Option<&[u32]>,
) -> Vec<u32> {
    assert!(
        u32::try_from(rows).is_ok(),
        "a batch has fewer than 2^32 rows (selection indexes are u32)"
    );
    let n = sel.map_or(rows, <[u32]>::len);
    let mut out = vec![0u32; n];
    let mut k = 0usize;
    if valid.is_all() {
        each(rows, sel, |i| {
            let x = col[i];
            out[k] = i as u32;
            k += ((x >= lo) & (x <= hi)) as usize;
        });
    } else {
        each(rows, sel, |i| {
            let x = col[i];
            out[k] = i as u32;
            k += ((x >= lo) & (x <= hi) & valid.ok(i)) as usize;
        });
    }
    out.truncate(k);
    out
}

/// Exact sum of the valid selected values, and how many there were. i128 cannot overflow here.
#[allow(
    clippy::arithmetic_side_effects,
    reason = "i128 holds the sum of fewer than 2^64 i64 values; the row count is a batch length"
)]
pub fn sum(col: Col<i64>, rows: usize, sel: Option<&[u32]>) -> (i128, u64) {
    let (v, valid) = col;
    let (mut s, mut n) = (0i128, 0u64);
    if valid.is_all() {
        each(rows, sel, |i| s += v[i] as i128);
        n = sel.map_or(rows, <[u32]>::len) as u64;
    } else {
        each(rows, sel, |i| {
            let ok = valid.ok(i);
            s += (v[i] as i128) * (ok as i128);
            n += ok as u64;
        });
    }
    (s, n)
}

/// Single pass over the selected rows for K SUM columns that have no NULLs: group id, count and all K sums.
#[allow(
    clippy::arithmetic_side_effects,
    reason = "per-group i128 sums and u64 counts; fewer than 2^64 rows in total"
)]
pub fn group_sums_dense<const K: usize>(
    gid: impl Fn(usize) -> usize,
    cols: [&[i64]; K],
    rows: usize,
    sel: Option<&[u32]>,
    acc: &mut [[i128; K]],
    count: &mut [u64],
) {
    each(rows, sel, |i| {
        let g = gid(i);
        count[g] += 1;
        for j in 0..K {
            acc[g][j] += cols[j][i] as i128;
        }
    });
}

/// Single pass for K SUM columns that may have NULLs: group id, count, and per column the sum and the
/// number of NULLs skipped (so the non-NULL count is `count - nulls[j]`).
#[allow(
    clippy::arithmetic_side_effects,
    reason = "per-group i128 sums and u64 counts; fewer than 2^64 rows in total"
)]
pub fn group_sums_masked<const K: usize>(
    gid: impl Fn(usize) -> usize,
    cols: [Col<i64>; K],
    rows: usize,
    sel: Option<&[u32]>,
    acc: &mut [[i128; K]],
    count: &mut [u64],
    nulls: &mut [Vec<u64>; K],
) {
    each(rows, sel, |i| {
        let g = gid(i);
        count[g] += 1;
        for j in 0..K {
            let ok = cols[j].1.ok(i);
            acc[g][j] += (cols[j].0[i] as i128) * (ok as i128);
            nulls[j][g] += !ok as u64;
        }
    });
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "test data generation and scalar references on small values"
)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    /// Scalar reference: Option semantics, row by row.
    fn ref_mul(
        a: &[i64],
        va: &[u8],
        b: &[i64],
        vb: &[u8],
        sel: &[usize],
    ) -> Result<Vec<Option<i64>>, ()> {
        let mut out = vec![None; a.len()];
        for &i in sel {
            if va[i] != 0 && vb[i] != 0 {
                out[i] = Some(a[i].checked_mul(b[i]).ok_or(())?);
            }
        }
        Ok(out)
    }

    type Case = (Vec<i64>, Vec<u8>, Vec<i64>, Vec<u8>, Option<Vec<u32>>);

    fn case(rng: &mut Rng, with_nulls: bool, with_sel: bool, big: bool) -> Case {
        let n = 1 + (rng.next() % 200) as usize;
        let mag = |r: &mut Rng| {
            if big && r.next().is_multiple_of(7) {
                (r.next() >> 2) as i64
            } else {
                (r.next() % 1000) as i64 - 500
            }
        };
        let a: Vec<i64> = (0..n).map(|_| mag(rng)).collect();
        let b: Vec<i64> = (0..n).map(|_| mag(rng)).collect();
        let va: Vec<u8> = (0..n)
            .map(|_| {
                if with_nulls {
                    (!rng.next().is_multiple_of(5)) as u8
                } else {
                    1
                }
            })
            .collect();
        let vb: Vec<u8> = (0..n)
            .map(|_| {
                if with_nulls {
                    (!rng.next().is_multiple_of(5)) as u8
                } else {
                    1
                }
            })
            .collect();
        let sel = with_sel.then(|| {
            (0..n as u32)
                .filter(|_| !rng.next().is_multiple_of(3))
                .collect()
        });
        (a, va, b, vb, sel)
    }

    #[test]
    fn mul_matches_the_scalar_reference_including_errors() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for round in 0..3000 {
            let (nulls, selected, big) = (round % 2 == 0, round % 3 != 0, round % 5 == 0);
            let (a, va, b, vb, sel) = case(&mut rng, nulls, selected, big);
            let sel_idx: Vec<usize> = sel.as_ref().map_or((0..a.len()).collect(), |s| {
                s.iter().map(|&i| i as usize).collect()
            });
            let got = mul(
                (&a, valid_of(nulls, &va)),
                (&b, valid_of(nulls, &vb)),
                a.len(),
                sel.as_deref(),
            );
            match ref_mul(&a, &va, &b, &vb, &sel_idx) {
                Err(()) => assert!(
                    got.is_err(),
                    "round {round}: reference overflows, kernel did not"
                ),
                Ok(want) => {
                    let got = got.unwrap_or_else(|e| {
                        panic!("round {round}: kernel failed but reference did not: {e:?}")
                    });
                    for &i in &sel_idx {
                        let ok = got.valid.as_ref().is_none_or(|v| v[i] != 0);
                        assert_eq!(want[i].is_some(), ok, "round {round} row {i}: validity");
                        if let Some(w) = want[i] {
                            assert_eq!(got.v[i], w, "round {round} row {i}: value");
                        }
                    }
                }
            }
        }
    }

    fn valid_of(nulls: bool, v: &[u8]) -> Valid<'_> {
        if nulls { Valid::Bytes(v) } else { Valid::All }
    }

    #[test]
    fn an_overflow_in_an_unselected_row_is_not_an_error() {
        let a = [1i64, i64::MAX, 3];
        let b = [1i64, 2, 3];
        let sel = [0u32, 2];
        let out =
            mul((&a, Valid::All), (&b, Valid::All), 3, Some(&sel)).expect("row 1 is not selected");
        assert_eq!((out.v[0], out.v[2]), (1, 9));
        assert!(
            mul((&a, Valid::All), (&b, Valid::All), 3, None).is_err(),
            "selected, so it is an error"
        );
    }

    #[test]
    fn an_overflow_in_a_null_slot_is_not_an_error() {
        let a = [i64::MAX, 5];
        let b = [2i64, 5];
        let valid = [0u8, 1];
        let out = mul((&a, Valid::Bytes(&valid)), (&b, Valid::All), 2, None)
            .expect("the overflowing row is NULL");
        assert_eq!(out.valid.as_deref(), Some(&[0u8, 1][..]));
        assert_eq!(out.v[1], 25);
    }

    #[test]
    fn a_guard_filter_protects_the_later_kernel() {
        // WHERE b <> 0 AND big * b ... : rows where the guard fails must not raise the error.
        let big = [i64::MAX, 4, i64::MAX];
        let guard = [0i64, 1, 0];
        let sel = filter_range(&guard, Valid::All, 1, 1, 3, None);
        assert_eq!(sel, vec![1]);
        let out = mul((&big, Valid::All), (&guard, Valid::All), 3, Some(&sel))
            .expect("guarded rows are skipped");
        assert_eq!(out.v[1], 4);
    }

    #[test]
    fn filters_drop_nulls_and_respect_the_input_selection() {
        let x = [5i32, 6, 7, 8, 9];
        let valid = [1u8, 0, 1, 1, 1];
        assert_eq!(
            filter_range(&x, Valid::Bytes(&valid), 6, 8, 5, None),
            vec![2, 3]
        );
        let sel = [0u32, 1, 3];
        assert_eq!(
            filter_range(&x, Valid::Bytes(&valid), 5, 9, 5, Some(&sel)),
            vec![0, 3]
        );
    }

    #[test]
    fn subtraction_overflow_follows_the_same_rules_as_multiplication() {
        let a = [i64::MIN, 4, i64::MIN];
        let valid = [0u8, 1, 1];
        // row 0 would overflow (0 - MIN) but is NULL; row 2 overflows and is valid.
        assert!(
            sub_const_left(0, (&a, Valid::Bytes(&valid)), 3, None).is_err(),
            "row 2 overflows and is valid"
        );
        let sel = [0u32, 1];
        let out = sub_const_left(0, (&a, Valid::Bytes(&valid)), 3, Some(&sel))
            .expect("the overflowing rows are NULL or unselected");
        assert_eq!(
            (out.v[1], out.valid.as_deref().map(|v| (v[0], v[1]))),
            (-4, Some((0, 1)))
        );
        assert!(
            sub_const_left(0, (&a, Valid::All), 3, Some(&sel)).is_err(),
            "all valid: row 0 overflows"
        );
    }

    #[test]
    fn the_dense_filter_respects_the_input_selection() {
        let x = [5i32, 6, 7, 8, 9];
        let sel = [0u32, 2, 4];
        assert_eq!(
            filter_range(&x, Valid::All, 6, 9, 5, Some(&sel)),
            vec![2, 4]
        );
    }

    #[test]
    fn sums_skip_nulls_and_count_the_valid_values() {
        let v = [10i64, 20, 30, 40];
        let valid = [1u8, 0, 1, 1];
        assert_eq!(sum((&v, Valid::Bytes(&valid)), 4, None), (80, 3));
        let sel = [1u32, 2];
        assert_eq!(sum((&v, Valid::Bytes(&valid)), 4, Some(&sel)), (30, 1));
        assert_eq!(sum((&v, Valid::All), 4, Some(&sel)), (50, 2));
    }

    #[test]
    fn masked_grouped_sums_skip_nulls_and_follow_the_selection() {
        let v = [1i64, 2, 3, 4, 5];
        let valid = [1u8, 1, 0, 1, 1];
        let sel = [0u32, 2, 4];
        let ids = [0usize, 0, 1, 0, 0]; // row 0 -> g0, row 2 -> g1, row 4 -> g0
        let (mut acc, mut count, mut nulls) = (vec![[0i128; 1]; 2], vec![0u64; 2], [vec![0u64; 2]]);
        group_sums_masked(
            |i| ids[i],
            [(&v, Valid::Bytes(&valid))],
            5,
            Some(&sel),
            &mut acc,
            &mut count,
            &mut nulls,
        );
        assert_eq!(acc, vec![[1 + 5], [0]], "the NULL at row 2 adds nothing");
        assert_eq!(
            (count, nulls),
            (vec![2, 1], [vec![0, 1]]),
            "row 2 is counted but is a NULL for the SUM"
        );
    }

    #[test]
    fn dense_multi_sums_match_separate_sums() {
        let (a, b) = ([1i64, 2, 3, 4, 5], [10i64, 20, 30, 40, 50]);
        let sel = [0u32, 1, 3, 4];
        let ids = [0usize, 1, 0, 1, 1];
        let (mut acc, mut count) = (vec![[0i128; 2]; 2], vec![0u64; 2]);
        group_sums_dense(|i| ids[i], [&a, &b], 5, Some(&sel), &mut acc, &mut count);
        // rows 0, 1, 3, 4 are selected; their groups are ids[0]=0, ids[1]=1, ids[3]=1, ids[4]=1
        assert_eq!(acc, vec![[1, 10], [2 + 4 + 5, 20 + 40 + 50]]);
        assert_eq!(count, vec![1, 3]);
    }

    /// A divisor/dividend drawn from the values that matter: 0, -1, 1, MIN, MAX, small and large.
    fn edge(r: &mut Rng) -> i64 {
        match r.next() % 9 {
            0 => 0,
            1 => -1,
            2 => 1,
            3 => i64::MIN,
            4 => i64::MAX,
            5 => (r.next() >> 2) as i64,
            _ => (r.next() % 21) as i64 - 10,
        }
    }

    #[derive(Debug, PartialEq)]
    enum RefErr {
        Zero,
        Overflow,
    }

    /// Scalar reference for division: Option semantics, row by row, errors only on selected valid rows.
    /// Reports "division by zero" in preference to overflow, as the kernel does.
    fn ref_div(
        a: &[i64],
        va: &[u8],
        b: &[i64],
        vb: &[u8],
        sel: &[usize],
    ) -> Result<Vec<Option<i64>>, RefErr> {
        let mut out = vec![None; a.len()];
        let (mut zero, mut ovf) = (false, false);
        for &i in sel {
            if va[i] != 0 && vb[i] != 0 {
                if b[i] == 0 {
                    zero = true;
                } else if a[i] == i64::MIN && b[i] == -1 {
                    ovf = true;
                } else {
                    out[i] = Some(a[i] / b[i]);
                }
            }
        }
        if zero {
            Err(RefErr::Zero)
        } else if ovf {
            Err(RefErr::Overflow)
        } else {
            Ok(out)
        }
    }

    fn classify(e: &EngineError) -> RefErr {
        if e.0.contains("division by zero") {
            RefErr::Zero
        } else {
            RefErr::Overflow
        }
    }

    #[test]
    fn div_matches_the_scalar_reference_including_errors() {
        let mut rng = Rng(0xD1B5_4A32_D192_ED03);
        let (mut errs, mut oks) = (0, 0);
        for round in 0..6000 {
            let (nulls, selected) = (round % 2 == 0, round % 3 != 0);
            let n = 1 + (rng.next() % 40) as usize;
            let a: Vec<i64> = (0..n).map(|_| edge(&mut rng)).collect();
            let b: Vec<i64> = (0..n).map(|_| edge(&mut rng)).collect();
            let va: Vec<u8> = (0..n)
                .map(|_| {
                    if nulls {
                        (!rng.next().is_multiple_of(4)) as u8
                    } else {
                        1
                    }
                })
                .collect();
            let vb: Vec<u8> = (0..n)
                .map(|_| {
                    if nulls {
                        (!rng.next().is_multiple_of(4)) as u8
                    } else {
                        1
                    }
                })
                .collect();
            let sel: Option<Vec<u32>> = selected.then(|| {
                (0..n as u32)
                    .filter(|_| !rng.next().is_multiple_of(2))
                    .collect()
            });
            let sel_idx: Vec<usize> = sel.as_ref().map_or((0..n).collect(), |s| {
                s.iter().map(|&i| i as usize).collect()
            });
            let got = div(
                (&a, valid_of(nulls, &va)),
                (&b, valid_of(nulls, &vb)),
                n,
                sel.as_deref(),
            );
            match ref_div(&a, &va, &b, &vb, &sel_idx) {
                Err(want) => {
                    errs += 1;
                    assert_eq!(
                        classify(&got.expect_err(&format!(
                            "round {round}: reference fails, kernel did not"
                        ))),
                        want,
                        "round {round}"
                    );
                }
                Ok(want) => {
                    oks += 1;
                    let got = got.unwrap_or_else(|e| {
                        panic!("round {round}: kernel failed but reference did not: {e:?}")
                    });
                    for &i in &sel_idx {
                        let ok = got.valid.as_ref().is_none_or(|v| v[i] != 0);
                        assert_eq!(want[i].is_some(), ok, "round {round} row {i}: validity");
                        if let Some(w) = want[i] {
                            assert_eq!(got.v[i], w, "round {round} row {i}: value");
                        }
                    }
                }
            }
        }
        assert!(
            errs > 500 && oks > 500,
            "the generator must hit both outcomes often: {errs} errors, {oks} successes"
        );
    }

    #[test]
    fn div_const_left_matches_the_scalar_reference_including_errors() {
        let mut rng = Rng(0x2545_F491_4F6C_DD1D);
        let (mut errs, mut oks) = (0, 0);
        for round in 0..6000 {
            let (nulls, selected) = (round % 2 == 0, round % 3 != 0);
            let n = 1 + (rng.next() % 40) as usize;
            let c = edge(&mut rng);
            let a: Vec<i64> = (0..n).map(|_| edge(&mut rng)).collect();
            let va: Vec<u8> = (0..n)
                .map(|_| {
                    if nulls {
                        (!rng.next().is_multiple_of(4)) as u8
                    } else {
                        1
                    }
                })
                .collect();
            let sel: Option<Vec<u32>> = selected.then(|| {
                (0..n as u32)
                    .filter(|_| !rng.next().is_multiple_of(2))
                    .collect()
            });
            let sel_idx: Vec<usize> = sel.as_ref().map_or((0..n).collect(), |s| {
                s.iter().map(|&i| i as usize).collect()
            });
            let got = div_const_left(c, (&a, valid_of(nulls, &va)), n, sel.as_deref());
            let cs = vec![c; n];
            let ones = vec![1u8; n];
            match ref_div(&cs, &ones, &a, &va, &sel_idx) {
                Err(want) => {
                    errs += 1;
                    assert_eq!(
                        classify(&got.expect_err(&format!(
                            "round {round}: reference fails, kernel did not"
                        ))),
                        want,
                        "round {round}"
                    );
                }
                Ok(want) => {
                    oks += 1;
                    let got = got.unwrap_or_else(|e| {
                        panic!("round {round}: kernel failed but reference did not: {e:?}")
                    });
                    for &i in &sel_idx {
                        let ok = got.valid.as_ref().is_none_or(|v| v[i] != 0);
                        assert_eq!(want[i].is_some(), ok, "round {round} row {i}: validity");
                        if let Some(w) = want[i] {
                            assert_eq!(got.v[i], w, "round {round} row {i}: value");
                        }
                    }
                }
            }
        }
        assert!(
            errs > 500 && oks > 500,
            "the generator must hit both outcomes often: {errs} errors, {oks} successes"
        );
    }

    #[test]
    fn a_zero_divisor_in_an_unselected_row_is_not_an_error() {
        let (a, b) = ([10i64, 10, 10], [2i64, 0, 5]);
        let sel = [0u32, 2];
        let out =
            div((&a, Valid::All), (&b, Valid::All), 3, Some(&sel)).expect("row 1 is not selected");
        assert_eq!((out.v[0], out.v[2]), (5, 2));
        let e = div((&a, Valid::All), (&b, Valid::All), 3, None)
            .expect_err("selected, so it is an error");
        assert_eq!(e, EngineError("division by zero".into()));
    }

    #[test]
    fn a_zero_divisor_in_a_null_slot_is_not_an_error() {
        let (a, b) = ([10i64, 10], [0i64, 5]);
        // the zero divisor is in a NULL slot
        let out =
            div((&a, Valid::All), (&b, Valid::Bytes(&[0, 1])), 2, None).expect("the zero is NULL");
        assert_eq!((out.valid.as_deref(), out.v[1]), (Some(&[0u8, 1][..]), 2));
        // the dividend is NULL while the divisor is zero: the row is NULL, so no error
        let out =
            div((&a, Valid::Bytes(&[0, 1])), (&b, Valid::All), 2, None).expect("the row is NULL");
        assert_eq!(out.valid.as_deref(), Some(&[0u8, 1][..]));
    }

    #[test]
    fn min_divided_by_minus_one_is_an_overflow_only_when_selected_and_valid() {
        let (a, b) = ([i64::MIN, 8], [-1i64, -1]);
        let e = div((&a, Valid::All), (&b, Valid::All), 2, None).expect_err("MIN / -1 overflows");
        assert_eq!(e, EngineError("overflow in division".into()));
        assert!(
            div((&a, Valid::All), (&b, Valid::All), 2, Some(&[1])).is_ok(),
            "row 0 is unselected"
        );
        assert!(
            div((&a, Valid::Bytes(&[0, 1])), (&b, Valid::All), 2, None).is_ok(),
            "row 0 is NULL"
        );
        assert!(
            div((&a, Valid::All), (&b, Valid::Bytes(&[0, 1])), 2, None).is_ok(),
            "row 0 is NULL"
        );
        // other divisions of MIN are fine, and truncation is toward zero
        let out = div(
            (&[i64::MIN, -7, 7], Valid::All),
            (&[1, 2, -2], Valid::All),
            3,
            None,
        )
        .expect("no failure");
        assert_eq!(out.v, vec![i64::MIN, -3, -3]);
    }

    #[test]
    fn div_const_left_follows_the_same_rules() {
        let a = [0i64, 4, -1];
        assert_eq!(
            div_const_left(100, (&a, Valid::All), 3, None),
            Err(EngineError("division by zero".into()))
        );
        let out =
            div_const_left(100, (&a, Valid::All), 3, Some(&[1, 2])).expect("row 0 is unselected");
        assert_eq!((out.v[1], out.v[2]), (25, -100));
        assert!(
            div_const_left(100, (&a, Valid::Bytes(&[0, 1, 1])), 3, None).is_ok(),
            "the zero is NULL"
        );
        assert_eq!(
            div_const_left(i64::MIN, (&a, Valid::All), 3, Some(&[2])),
            Err(EngineError("overflow in division".into()))
        );
        assert!(div_const_left(i64::MIN, (&a, Valid::All), 3, Some(&[1])).is_ok());
    }

    #[test]
    fn filter_ne_drops_nulls_and_respects_the_selection() {
        let x = [0i64, 3, 0, 5, 7];
        let valid = [1u8, 1, 1, 0, 1];
        assert_eq!(
            filter_ne(&x, Valid::Bytes(&valid), 0, 5, None),
            vec![1, 4],
            "row 3 is NULL, rows 0 and 2 are zero"
        );
        assert_eq!(
            filter_ne(&x, Valid::All, 0, 5, Some(&[0, 1, 3])),
            vec![1, 3]
        );
    }

    #[test]
    fn split_ne_sends_null_comparisons_to_the_else_branch() {
        let x = [0i64, 3, 0, 5, 7];
        let valid = [1u8, 1, 1, 0, 1];
        let (yes, no) = split_ne(&x, Valid::Bytes(&valid), 0, 5, Some(&[0, 1, 3, 4]));
        assert_eq!((yes, no), (vec![1, 4], vec![0, 3]));
    }

    #[test]
    fn merge_branches_places_each_part_and_leaves_other_rows_null() {
        let one = Out {
            v: vec![9, 9, 9, 9],
            valid: Some(vec![1, 0, 1, 1]),
        };
        let two = constant(0, 4);
        let m = merge_branches(4, &[(&[0, 1], &one), (&[3], &two)]);
        assert_eq!(m.v[0], 9);
        assert_eq!(m.v[3], 0);
        assert_eq!(
            m.valid,
            Some(vec![1, 0, 0, 1]),
            "row 1 is NULL in its branch, row 2 is in no branch"
        );
    }

    #[test]
    fn merge_branches_lets_a_later_part_win_on_an_overlap() {
        let first = Out {
            v: vec![1, 1],
            valid: Some(vec![0, 1]),
        };
        let second = Out {
            v: vec![2, 2],
            valid: Some(vec![1, 0]),
        };
        let m = merge_branches(2, &[(&[0, 1], &first), (&[0, 1], &second)]);
        assert_eq!((m.v[0], m.v[1]), (2, 2));
        assert_eq!(
            m.valid,
            Some(vec![1, 0]),
            "the later part's validity replaces the earlier one"
        );
    }
}
