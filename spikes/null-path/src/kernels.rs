//! Checked, NULL-aware, selection-aware kernels.
//!
//! Rules (ADR 0003, decision 4; docs/decimal-rules.md):
//! - an arithmetic overflow is an error, but only for a row that is selected AND valid;
//! - a NULL input gives a NULL output; a NULL in a filter column drops the row;
//! - the selection is a vector of row indexes (None = every row); kernels never touch other rows.
//! Error checks are branch-free: every selected row computes, and an overflow flag is masked by validity.
use arrow::buffer::NullBuffer;

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
    n.map_or(Valid::All, |n| if n.null_count() == 0 { Valid::All } else { Valid::Bits(n) })
}

pub type Col<'a, T> = (&'a [T], Valid<'a>);

/// A computed column: full length, only the selected positions are meaningful.
#[derive(Debug, Clone)]
pub struct Out {
    pub v: Vec<i64>,
    pub valid: Option<Vec<u8>>,
}

impl Out {
    pub fn col(&self) -> Col<'_, i64> {
        (&self.v, self.valid.as_deref().map_or(Valid::All, Valid::Bytes))
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
    Ok(Out { v, valid: Some(valid) })
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
    Ok(Out { v, valid: Some(valid) })
}

/// Rows of the input selection whose value lies in `[lo, hi]` and is not NULL.
pub fn filter_range<T: Copy + PartialOrd>(col: &[T], valid: Valid, lo: T, hi: T, rows: usize, sel: Option<&[u32]>) -> Vec<u32> {
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
    fn ref_mul(a: &[i64], va: &[u8], b: &[i64], vb: &[u8], sel: &[usize]) -> Result<Vec<Option<i64>>, ()> {
        let mut out = vec![None; a.len()];
        for &i in sel {
            if va[i] != 0 && vb[i] != 0 {
                out[i] = Some(a[i].checked_mul(b[i]).ok_or(())?);
            }
        }
        Ok(out)
    }

    fn case(rng: &mut Rng, with_nulls: bool, with_sel: bool, big: bool) -> (Vec<i64>, Vec<u8>, Vec<i64>, Vec<u8>, Option<Vec<u32>>) {
        let n = 1 + (rng.next() % 200) as usize;
        let mag = |r: &mut Rng| if big && r.next() % 7 == 0 { (r.next() >> 2) as i64 } else { (r.next() % 1000) as i64 - 500 };
        let a: Vec<i64> = (0..n).map(|_| mag(rng)).collect();
        let b: Vec<i64> = (0..n).map(|_| mag(rng)).collect();
        let va: Vec<u8> = (0..n).map(|_| if with_nulls { (rng.next() % 5 != 0) as u8 } else { 1 }).collect();
        let vb: Vec<u8> = (0..n).map(|_| if with_nulls { (rng.next() % 5 != 0) as u8 } else { 1 }).collect();
        let sel = with_sel.then(|| (0..n as u32).filter(|_| rng.next() % 3 != 0).collect());
        (a, va, b, vb, sel)
    }

    #[test]
    fn mul_matches_the_scalar_reference_including_errors() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for round in 0..3000 {
            let (nulls, selected, big) = (round % 2 == 0, round % 3 != 0, round % 5 == 0);
            let (a, va, b, vb, sel) = case(&mut rng, nulls, selected, big);
            let sel_idx: Vec<usize> = sel.as_ref().map_or((0..a.len()).collect(), |s| s.iter().map(|&i| i as usize).collect());
            let got = mul((&a, valid_of(nulls, &va)), (&b, valid_of(nulls, &vb)), a.len(), sel.as_deref());
            match ref_mul(&a, &va, &b, &vb, &sel_idx) {
                Err(()) => assert!(got.is_err(), "round {round}: reference overflows, kernel did not"),
                Ok(want) => {
                    let got = got.unwrap_or_else(|e| panic!("round {round}: kernel failed but reference did not: {e:?}"));
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
        let out = mul((&a, Valid::All), (&b, Valid::All), 3, Some(&sel)).expect("row 1 is not selected");
        assert_eq!((out.v[0], out.v[2]), (1, 9));
        assert!(mul((&a, Valid::All), (&b, Valid::All), 3, None).is_err(), "selected, so it is an error");
    }

    #[test]
    fn an_overflow_in_a_null_slot_is_not_an_error() {
        let a = [i64::MAX, 5];
        let b = [2i64, 5];
        let valid = [0u8, 1];
        let out = mul((&a, Valid::Bytes(&valid)), (&b, Valid::All), 2, None).expect("the overflowing row is NULL");
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
        let out = mul((&big, Valid::All), (&guard, Valid::All), 3, Some(&sel)).expect("guarded rows are skipped");
        assert_eq!(out.v[1], 4);
    }

    #[test]
    fn filters_drop_nulls_and_respect_the_input_selection() {
        let x = [5i32, 6, 7, 8, 9];
        let valid = [1u8, 0, 1, 1, 1];
        assert_eq!(filter_range(&x, Valid::Bytes(&valid), 6, 8, 5, None), vec![2, 3]);
        let sel = [0u32, 1, 3];
        assert_eq!(filter_range(&x, Valid::Bytes(&valid), 5, 9, 5, Some(&sel)), vec![0, 3]);
    }

    #[test]
    fn subtraction_overflow_follows_the_same_rules_as_multiplication() {
        let a = [i64::MIN, 4, i64::MIN];
        let valid = [0u8, 1, 1];
        // row 0 would overflow (0 - MIN) but is NULL; row 2 overflows and is valid.
        assert!(sub_const_left(0, (&a, Valid::Bytes(&valid)), 3, None).is_err(), "row 2 overflows and is valid");
        let sel = [0u32, 1];
        let out = sub_const_left(0, (&a, Valid::Bytes(&valid)), 3, Some(&sel)).expect("the overflowing rows are NULL or unselected");
        assert_eq!((out.v[1], out.valid.as_deref().map(|v| (v[0], v[1]))), (-4, Some((0, 1))));
        assert!(sub_const_left(0, (&a, Valid::All), 3, Some(&sel)).is_err(), "all valid: row 0 overflows");
    }

    #[test]
    fn the_dense_filter_respects_the_input_selection() {
        let x = [5i32, 6, 7, 8, 9];
        let sel = [0u32, 2, 4];
        assert_eq!(filter_range(&x, Valid::All, 6, 9, 5, Some(&sel)), vec![2, 4]);
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
        group_sums_masked(|i| ids[i], [(&v, Valid::Bytes(&valid))], 5, Some(&sel), &mut acc, &mut count, &mut nulls);
        assert_eq!(acc, vec![[1 + 5], [0]], "the NULL at row 2 adds nothing");
        assert_eq!((count, nulls), (vec![2, 1], [vec![0, 1]]), "row 2 is counted but is a NULL for the SUM");
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
}
