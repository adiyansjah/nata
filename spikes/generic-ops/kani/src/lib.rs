//! Kani proofs for the real `../../src/kernels.rs` (included by path, not copied).
//! Run: `cargo kani --harness <name>` (see ../run.sh). Bound: rows <= N, all values arbitrary.
//! Preconditions the harnesses assume (the engine's own contract): every column slice has >= rows entries,
//! the selection is sorted, unique and < rows, and group ids are < the number of groups.
#![allow(dead_code)]

#[path = "../../src/kernels.rs"]
mod kernels;

#[cfg(kani)]
mod proofs {
    use super::kernels::{self, Valid};
    use arrow::buffer::NullBuffer;

    /// Row bound. Arrays are N long; `rows` is symbolic in 0..=N.
    const N: usize = 4;
    /// Groups for the grouped-sum proofs.
    const G: usize = 2;

    /// An arbitrary validity representation: All, Bytes (any byte values, nonzero = valid) or Bits (stub NullBuffer).
    struct Vs {
        kind: u8,
        bytes: [u8; N],
        nb: NullBuffer,
    }
    fn any_vs(rows: usize) -> Vs {
        let kind: u8 = kani::any();
        kani::assume(kind < 3);
        let mut bits = [false; 8];
        for i in 0..N {
            bits[i] = kani::any();
        }
        Vs { kind, bytes: kani::any(), nb: NullBuffer { bits, len: rows } }
    }
    impl Vs {
        fn valid(&self, rows: usize) -> Valid<'_> {
            match self.kind {
                0 => Valid::All,
                1 => Valid::Bytes(&self.bytes[..rows]),
                _ => Valid::Bits(&self.nb),
            }
        }
        /// The specification of validity, written without Valid::ok.
        fn ok(&self, i: usize) -> bool {
            match self.kind {
                0 => true,
                1 => self.bytes[i] != 0,
                _ => self.nb.bits[i],
            }
        }
    }

    /// An arbitrary selection: None, or a sorted unique subset of 0..rows.
    struct Sel {
        present: bool,
        mask: u8,
        buf: [u32; N],
        n: usize,
    }
    fn any_sel(rows: usize) -> Sel {
        let present: bool = kani::any();
        let mask: u8 = kani::any();
        kani::assume((mask as u32) >> rows == 0);
        let (mut buf, mut n) = ([0u32; N], 0usize);
        for i in 0..N {
            if i < rows && (mask >> i) & 1 == 1 {
                buf[n] = i as u32;
                n += 1;
            }
        }
        Sel { present, mask, buf, n }
    }
    /// Selection of a CONCRETE length `n` whose entries are symbolic, strictly
    /// increasing and < rows. Needed because filter_range allocates `vec![0; sel.len()]`, and a symbolic allocation
    /// size makes CBMC run out of memory.
    fn any_sel_len(rows: usize, n: usize) -> Sel {
        let (mut buf, mut mask) = ([0u32; N], 0u8);
        for j in 0..N {
            if j < n {
                let x: u32 = kani::any();
                kani::assume((x as usize) < rows);
                if j > 0 {
                    kani::assume(x > buf[j - 1]);
                }
                buf[j] = x;
                mask |= 1 << x;
            }
        }
        Sel { present: true, mask, buf, n }
    }
    fn no_sel(rows: usize) -> Sel {
        Sel { present: false, mask: ((1u16 << rows) - 1) as u8, buf: [0; N], n: 0 }
    }
    impl Sel {
        fn get(&self) -> Option<&[u32]> {
            self.present.then(|| &self.buf[..self.n])
        }
        fn has(&self, i: usize) -> bool {
            !self.present || (self.mask >> i) & 1 == 1
        }
    }

    // ---- each() ----------------------------------------------------------------------------------------

    /// `each` visits exactly the entries of the selection, in order, or 0..rows for None. Any slice, not only
    /// sorted unique ones, so this is stronger than the precondition the other proofs assume.
    fn each_visits_the_selection_in_order<const ROWS: usize>() {
        let rows = ROWS;
        let s: [u32; N] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= N);
        let use_sel: bool = kani::any();
        let mut seen = [usize::MAX; N];
        let mut k = 0usize;
        kernels::each(rows, use_sel.then(|| &s[..len]), |i| {
            seen[k] = i;
            k += 1;
        });
        if use_sel {
            assert!(k == len);
            for j in 0..len {
                assert!(seen[j] == s[j] as usize);
            }
        } else {
            assert!(k == rows);
            for j in 0..rows {
                assert!(seen[j] == j);
            }
        }
    }

    // ---- arithmetic ------------------------------------------------------------------------------------

    /// Shared postcondition for mul / sub_const_left. `exact(i)` is the non-wrapping result or None on overflow;
    /// `valid(i)` the specified validity of the row.
    fn check_arith(
        r: Result<kernels::Out, kernels::EngineError>,
        rows: usize,
        sel: &Sel,
        exact: impl Fn(usize) -> Option<i64>,
        valid: impl Fn(usize) -> bool,
    ) {
        let mut expect_err = false;
        let (mut masked_overflow, mut any_valid_selected) = (false, false);
        for i in 0..N {
            if i < rows {
                let ovf = exact(i).is_none();
                if sel.has(i) && valid(i) && ovf {
                    expect_err = true;
                }
                if ovf && !(sel.has(i) && valid(i)) {
                    masked_overflow = true;
                }
                if sel.has(i) && valid(i) {
                    any_valid_selected = true;
                }
            }
        }
        // Non-vacuity: all of these situations are reachable.
        kani::cover!(expect_err);
        kani::cover!(!expect_err && masked_overflow);
        kani::cover!(!expect_err && !masked_overflow && any_valid_selected && sel.present);
        assert!(r.is_err() == expect_err);
        if let Ok(out) = r {
            assert!(out.v.len() == rows);
            if let Some(ov) = &out.valid {
                assert!(ov.len() == rows);
            }
            for i in 0..N {
                if i < rows {
                    if sel.has(i) {
                        let ok = out.valid.as_ref().is_none_or(|v| v[i] != 0);
                        assert!(ok == valid(i));
                        if valid(i) {
                            assert!(Some(out.v[i]) == exact(i));
                        }
                    } else {
                        // documented: kernels never touch unselected rows
                        assert!(out.v[i] == 0);
                        assert!(out.valid.as_ref().is_none_or(|v| v[i] == 0));
                    }
                }
            }
        }
    }

    /// Kani cannot afford the `format!` inside `kernels::overflow` (CBMC does not finish on fmt::write), so the
    /// proofs replace it with a constructor of the same type. Unproven consequence: the error MESSAGE text.
    fn stub_overflow<T>(_what: &str) -> kernels::R<T> {
        Err(kernels::EngineError(String::new()))
    }

    /// Stand-ins for `i64::overflowing_mul` / `overflowing_sub` in the LOGIC proofs: any cheap pure function with
    /// an independent result and overflow flag per call (the flag is bit 0 of x, the result mixes both operands, so
    /// with free inputs every row can independently overflow or not and take any result). The logic proofs then hold
    /// for the real loop structure of kernels.rs whatever the primitive computes; that the primitive is the exact
    /// overflowing operation is proved separately (primitive_* harnesses) and is a trust point for mul at 64 bits.
    fn fake_op(x: i64, y: i64) -> (i64, bool) {
        (x ^ y.rotate_left(1), x & 1 == 1)
    }

    fn mul_logic_matches_the_spec<const ROWS: usize>() {
        let rows = ROWS;
        let (a, b): ([i64; N], [i64; N]) = (kani::any(), kani::any());
        let (sa, sb) = (any_vs(rows), any_vs(rows));
        let sel = any_sel(rows);
        let r = kernels::mul((&a[..rows], sa.valid(rows)), (&b[..rows], sb.valid(rows)), rows, sel.get());
        check_arith(r, rows, &sel, |i| { let (r, o) = fake_op(a[i], b[i]); (!o).then_some(r) }, |i| sa.ok(i) && sb.ok(i));
    }

    fn sub_const_left_logic_matches_the_spec<const ROWS: usize>() {
        let rows = ROWS;
        let c: i64 = kani::any();
        let a: [i64; N] = kani::any();
        let sa = any_vs(rows);
        let sel = any_sel(rows);
        let r = kernels::sub_const_left(c, (&a[..rows], sa.valid(rows)), rows, sel.get());
        check_arith(r, rows, &sel, |i| { let (r, o) = fake_op(c, a[i]); (!o).then_some(r) }, |i| sa.ok(i));
    }

    /// Subtraction needs no abstraction: the real `overflowing_sub`, spec written in i128.
    fn sub_const_left_exact_matches_the_spec<const ROWS: usize>() {
        let rows = ROWS;
        let c: i64 = kani::any();
        let a: [i64; N] = kani::any();
        let sa = any_vs(rows);
        let sel = any_sel(rows);
        let r = kernels::sub_const_left(c, (&a[..rows], sa.valid(rows)), rows, sel.get());
        check_arith(r, rows, &sel, |i| i64::try_from(c as i128 - a[i] as i128).ok(), |i| sa.ok(i));
    }

    // ---- the primitives the logic proofs abstract ------------------------------------------------------

    #[kani::proof]
    fn primitive_overflowing_sub_i64_is_exact() {
        let (x, y): (i64, i64) = kani::any();
        let (r, o) = x.overflowing_sub(y);
        let e = x as i128 - y as i128;
        assert!(o == (e < i64::MIN as i128 || e > i64::MAX as i128));
        assert!(r as i128 == e || o);
    }

    /// Same statement for multiplication at 64 bits: a 64x64 multiplier miter, which CBMC may not finish.
    #[kani::proof]
    fn primitive_overflowing_mul_i64_is_exact() {
        let (x, y): (i64, i64) = kani::any();
        let (r, o) = x.overflowing_mul(y);
        let e = x as i128 * y as i128;
        assert!(o == (e < i64::MIN as i128 || e > i64::MAX as i128));
        assert!(o || r as i128 == e);
    }

    /// The same statement at 16 bits (fits in CBMC): shows the shape of the argument for the width-generic std op.
    #[kani::proof]
    fn primitive_overflowing_mul_i16_is_exact() {
        let (x, y): (i16, i16) = kani::any();
        let (r, o) = x.overflowing_mul(y);
        let e = x as i32 * y as i32;
        assert!(o == (e < i16::MIN as i32 || e > i16::MAX as i32));
        assert!(o || r as i32 == e);
    }

    // ---- filter ----------------------------------------------------------------------------------------

    fn check_filter<T: Copy + PartialOrd + kani::Arbitrary, const ROWS: usize, const SEL: i8>() {
        let rows = ROWS;
        let col: [T; N] = kani::any();
        let (lo, hi): (T, T) = (kani::any(), kani::any());
        let vs = any_vs(rows);
        // SEL < 0: no selection; otherwise a selection of exactly SEL entries (concrete length, see any_sel_len)
        let sel = if SEL < 0 { no_sel(rows) } else { any_sel_len(rows, SEL as usize) };
        let got = kernels::filter_range(&col[..rows], vs.valid(rows), lo, hi, rows, sel.get());
        let (mut want, mut k) = ([0u32; N], 0usize);
        for i in 0..N {
            if i < rows && sel.has(i) && vs.ok(i) && col[i] >= lo && col[i] <= hi {
                want[k] = i as u32;
                k += 1;
            }
        }
        kani::cover!(k > 0 && sel.present);
        kani::cover!(k == 0 && rows > 0);
        assert!(got.len() == k);
        for j in 0..k {
            assert!(got[j] == want[j]);
        }
    }

    // ---- sum -------------------------------------------------------------------------------------------

    fn sum_matches_the_spec<const ROWS: usize>() {
        let rows = ROWS;
        let v: [i64; N] = kani::any();
        let vs = any_vs(rows);
        let sel = any_sel(rows);
        let (s, n) = kernels::sum((&v[..rows], vs.valid(rows)), rows, sel.get());
        let (mut want_s, mut want_n) = (0i128, 0u64);
        for i in 0..N {
            if i < rows && sel.has(i) && vs.ok(i) {
                want_s += v[i] as i128;
                want_n += 1;
            }
        }
        kani::cover!(want_n > 0 && want_n < rows as u64);
        assert!((s, n) == (want_s, want_n));
    }

    // ---- grouped sums ----------------------------------------------------------------------------------

    /// Accumulators start from arbitrary values (no overflow assumed on the way in); the kernel must add exactly
    /// the selected rows' contribution. Kani also checks the kernel's own += for overflow under this assumption.
    fn any_acc() -> [[i128; 2]; G] {
        let acc: [[i128; 2]; G] = kani::any();
        for g in 0..G {
            for j in 0..2 {
                kani::assume(acc[g][j] > -(1i128 << 100) && acc[g][j] < (1i128 << 100));
            }
        }
        acc
    }
    fn any_count() -> [u64; G] {
        let c: [u64; G] = kani::any();
        for g in 0..G {
            kani::assume(c[g] < (1u64 << 60));
        }
        c
    }
    fn any_gids() -> [usize; N] {
        let ids: [usize; N] = kani::any();
        for i in 0..N {
            kani::assume(ids[i] < G);
        }
        ids
    }

    fn group_sums_dense_matches_the_spec<const ROWS: usize>() {
        let rows = ROWS;
        let (c0, c1): ([i64; N], [i64; N]) = (kani::any(), kani::any());
        let ids = any_gids();
        let sel = any_sel(rows);
        let (acc0, count0) = (any_acc(), any_count());
        let (mut acc, mut count) = (acc0, count0);
        kernels::group_sums_dense(|i| ids[i], [&c0[..rows], &c1[..rows]], rows, sel.get(), &mut acc, &mut count);
        let (mut want_acc, mut want_count) = (acc0, count0);
        for i in 0..N {
            if i < rows && sel.has(i) {
                want_count[ids[i]] += 1;
                want_acc[ids[i]][0] += c0[i] as i128;
                want_acc[ids[i]][1] += c1[i] as i128;
            }
        }
        kani::cover!(sel.present && sel.n >= 2 && ids[0] != ids[1]);
        for g in 0..G {
            assert!(count[g] == want_count[g]);
            for j in 0..2 {
                assert!(acc[g][j] == want_acc[g][j]);
            }
        }
    }

    fn group_sums_masked_matches_the_spec<const ROWS: usize>() {
        let rows = ROWS;
        let (c0, c1): ([i64; N], [i64; N]) = (kani::any(), kani::any());
        let (v0, v1) = (any_vs(rows), any_vs(rows));
        let ids = any_gids();
        let sel = any_sel(rows);
        let (acc0, count0) = (any_acc(), any_count());
        let nulls0: [[u64; G]; 2] = kani::any();
        for j in 0..2 {
            for g in 0..G {
                kani::assume(nulls0[j][g] < (1u64 << 60));
            }
        }
        let (mut acc, mut count) = (acc0, count0);
        let mut nulls = [nulls0[0].to_vec(), nulls0[1].to_vec()];
        kernels::group_sums_masked(
            |i| ids[i],
            [(&c0[..rows], v0.valid(rows)), (&c1[..rows], v1.valid(rows))],
            rows,
            sel.get(),
            &mut acc,
            &mut count,
            &mut nulls,
        );
        let (mut want_acc, mut want_count, mut want_nulls) = (acc0, count0, nulls0);
        for i in 0..N {
            if i < rows && sel.has(i) {
                let g = ids[i];
                want_count[g] += 1;
                if v0.ok(i) {
                    want_acc[g][0] += c0[i] as i128;
                } else {
                    want_nulls[0][g] += 1;
                }
                if v1.ok(i) {
                    want_acc[g][1] += c1[i] as i128;
                } else {
                    want_nulls[1][g] += 1;
                }
            }
        }
        kani::cover!(want_nulls[0][0] != nulls0[0][0] && want_acc[0][0] != acc0[0][0]);
        for g in 0..G {
            assert!(count[g] == want_count[g]);
            for j in 0..2 {
                assert!(acc[g][j] == want_acc[g][j]);
            }
        }
        for j in 0..2 {
            assert!(nulls[j].len() == G);
            for g in 0..G {
                assert!(nulls[j][g] == want_nulls[j][g]);
            }
        }
    }

    // ---- harnesses: one per concrete row count 0..=N -------------------------------------------------
    // A symbolic `rows` makes the Vec allocations inside the kernels run CBMC out of memory, so each row count
    // is its own proof. Together they cover every row count up to the bound: 4 for everything except the two grouped
    // sums, which stop at 3 (their rows=4 proofs did not finish in 400 s with i128 accumulators).
    #[kani::proof]
    #[kani::unwind(7)]
    fn each_visits_the_selection_in_order_rows0() {
        each_visits_the_selection_in_order::<0>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn each_visits_the_selection_in_order_rows1() {
        each_visits_the_selection_in_order::<1>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn each_visits_the_selection_in_order_rows2() {
        each_visits_the_selection_in_order::<2>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn each_visits_the_selection_in_order_rows3() {
        each_visits_the_selection_in_order::<3>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn each_visits_the_selection_in_order_rows4() {
        each_visits_the_selection_in_order::<4>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::overflowing_mul, fake_op)]
    fn mul_logic_matches_the_spec_rows0() {
        mul_logic_matches_the_spec::<0>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::overflowing_mul, fake_op)]
    fn mul_logic_matches_the_spec_rows1() {
        mul_logic_matches_the_spec::<1>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::overflowing_mul, fake_op)]
    fn mul_logic_matches_the_spec_rows2() {
        mul_logic_matches_the_spec::<2>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::overflowing_mul, fake_op)]
    fn mul_logic_matches_the_spec_rows3() {
        mul_logic_matches_the_spec::<3>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::overflowing_mul, fake_op)]
    fn mul_logic_matches_the_spec_rows4() {
        mul_logic_matches_the_spec::<4>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::overflowing_sub, fake_op)]
    fn sub_const_left_logic_matches_the_spec_rows0() {
        sub_const_left_logic_matches_the_spec::<0>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::overflowing_sub, fake_op)]
    fn sub_const_left_logic_matches_the_spec_rows1() {
        sub_const_left_logic_matches_the_spec::<1>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::overflowing_sub, fake_op)]
    fn sub_const_left_logic_matches_the_spec_rows2() {
        sub_const_left_logic_matches_the_spec::<2>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::overflowing_sub, fake_op)]
    fn sub_const_left_logic_matches_the_spec_rows3() {
        sub_const_left_logic_matches_the_spec::<3>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::overflowing_sub, fake_op)]
    fn sub_const_left_logic_matches_the_spec_rows4() {
        sub_const_left_logic_matches_the_spec::<4>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    fn sub_const_left_exact_matches_the_spec_rows0() {
        sub_const_left_exact_matches_the_spec::<0>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    fn sub_const_left_exact_matches_the_spec_rows1() {
        sub_const_left_exact_matches_the_spec::<1>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    fn sub_const_left_exact_matches_the_spec_rows2() {
        sub_const_left_exact_matches_the_spec::<2>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    fn sub_const_left_exact_matches_the_spec_rows3() {
        sub_const_left_exact_matches_the_spec::<3>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    fn sub_const_left_exact_matches_the_spec_rows4() {
        sub_const_left_exact_matches_the_spec::<4>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn sum_matches_the_spec_rows0() {
        sum_matches_the_spec::<0>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn sum_matches_the_spec_rows1() {
        sum_matches_the_spec::<1>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn sum_matches_the_spec_rows2() {
        sum_matches_the_spec::<2>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn sum_matches_the_spec_rows3() {
        sum_matches_the_spec::<3>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn sum_matches_the_spec_rows4() {
        sum_matches_the_spec::<4>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn group_sums_dense_matches_the_spec_rows0() {
        group_sums_dense_matches_the_spec::<0>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn group_sums_dense_matches_the_spec_rows1() {
        group_sums_dense_matches_the_spec::<1>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn group_sums_dense_matches_the_spec_rows2() {
        group_sums_dense_matches_the_spec::<2>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn group_sums_dense_matches_the_spec_rows3() {
        group_sums_dense_matches_the_spec::<3>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn group_sums_masked_matches_the_spec_rows0() {
        group_sums_masked_matches_the_spec::<0>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn group_sums_masked_matches_the_spec_rows1() {
        group_sums_masked_matches_the_spec::<1>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn group_sums_masked_matches_the_spec_rows2() {
        group_sums_masked_matches_the_spec::<2>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn group_sums_masked_matches_the_spec_rows3() {
        group_sums_masked_matches_the_spec::<3>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i64_rows0_none() {
        check_filter::<i64, 0, -1>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i64_rows0_s0() {
        check_filter::<i64, 0, 0>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i64_rows1_none() {
        check_filter::<i64, 1, -1>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i64_rows1_s0() {
        check_filter::<i64, 1, 0>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i64_rows1_s1() {
        check_filter::<i64, 1, 1>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i64_rows2_none() {
        check_filter::<i64, 2, -1>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i64_rows2_s0() {
        check_filter::<i64, 2, 0>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i64_rows2_s1() {
        check_filter::<i64, 2, 1>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i64_rows2_s2() {
        check_filter::<i64, 2, 2>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i64_rows3_none() {
        check_filter::<i64, 3, -1>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i64_rows3_s0() {
        check_filter::<i64, 3, 0>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i64_rows3_s1() {
        check_filter::<i64, 3, 1>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i64_rows3_s2() {
        check_filter::<i64, 3, 2>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i64_rows3_s3() {
        check_filter::<i64, 3, 3>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i64_rows4_none() {
        check_filter::<i64, 4, -1>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i64_rows4_s0() {
        check_filter::<i64, 4, 0>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i64_rows4_s1() {
        check_filter::<i64, 4, 1>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i64_rows4_s2() {
        check_filter::<i64, 4, 2>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i64_rows4_s3() {
        check_filter::<i64, 4, 3>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i64_rows4_s4() {
        check_filter::<i64, 4, 4>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i32_rows4_none() {
        check_filter::<i32, 4, -1>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i32_rows4_s0() {
        check_filter::<i32, 4, 0>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i32_rows4_s1() {
        check_filter::<i32, 4, 1>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i32_rows4_s2() {
        check_filter::<i32, 4, 2>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i32_rows4_s3() {
        check_filter::<i32, 4, 3>()
    }
    #[kani::proof]
    #[kani::unwind(7)]
    fn filter_range_i32_rows4_s4() {
        check_filter::<i32, 4, 4>()
    }
}
