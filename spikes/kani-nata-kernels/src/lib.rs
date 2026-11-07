//! Kani proofs for the REAL `crates/nata-kernels/src/kernels.rs` (included by path, never copied, so no drift).
//! Run: `./run.sh <cap-seconds> [regex]` (one harness at a time) or `cargo kani -Z stubbing --harness <name>`.
//! Bound: rows <= N (a separate harness per concrete row count), all values arbitrary.
//! Preconditions the harnesses assume (the engine's own contract): every column slice has >= rows entries,
//! the selection given to arithmetic / filter_range / sums is sorted, unique and < rows, group ids are < the number
//! of groups. split_ne / filter_ne / merge_branches are proved for ANY sequence of indexes < rows (duplicates and
//! unsorted included), which is stronger.
//!
//! Every spec below is written WITHOUT calling the function under test (plain loops over the inputs, i128, or the
//! characterisation of truncating division), and every harness has `kani::cover!` checks so a proof cannot pass
//! vacuously (each cover is gated by the minimum row count at which it is satisfiable; run1.sh flags any harness whose
//! covers are not all satisfied). The 64-bit multiplication / division primitives are handled by the two-part argument of
//! bench/results/20261104-kani-kernels.md: the LOGIC proofs replace the primitive by an arbitrary stand-in, the
//! `primitive_*` proofs show std's operation is exact.
#![allow(dead_code)]

#[path = "../../../crates/nata-kernels/src/kernels.rs"]
mod kernels;

#[cfg(kani)]
mod proofs {
    use super::kernels::{self, Valid};
    use arrow_buffer::NullBuffer;

    /// Row bound. Arrays are N long; `rows` is a concrete const per harness.
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
    /// Selection of a CONCRETE length `n` whose entries are symbolic, strictly increasing and < rows. Needed because
    /// filter_range allocates `vec![0; sel.len()]`, and a symbolic allocation size makes CBMC run out of memory.
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

    /// An arbitrary SEQUENCE of row indexes < rows: unsorted, duplicates allowed, symbolic length <= N. Used where
    /// the kernel allocates nothing sized by the selection (split_ne pushes, merge_branches only reads).
    struct Seq {
        present: bool,
        buf: [u32; N],
        n: usize,
    }
    fn any_seq(rows: usize) -> Seq {
        let present: bool = kani::any();
        let buf: [u32; N] = kani::any();
        let n: usize = kani::any();
        kani::assume(n <= N);
        for j in 0..N {
            if j < n {
                kani::assume((buf[j] as usize) < rows);
            }
        }
        Seq { present, buf, n }
    }
    impl Seq {
        fn get(&self) -> Option<&[u32]> {
            self.present.then(|| &self.buf[..self.n])
        }
        /// number of input positions
        fn len(&self, rows: usize) -> usize {
            if self.present { self.n } else { rows }
        }
        /// the row at input position j (j < len)
        fn at(&self, j: usize) -> usize {
            if self.present { self.buf[j] as usize } else { j }
        }
        fn has(&self, rows: usize, i: usize) -> bool {
            (0..N).any(|j| j < self.len(rows) && self.at(j) == i)
        }
    }

    // ---- stubs and stand-ins -----------------------------------------------------------------------------

    /// Kani cannot afford the `format!` inside `kernels::overflow` (CBMC does not finish on fmt::write), so the
    /// proofs replace it with a constructor of the same type and a message that differs from "division by zero".
    /// Unproven consequence: the TEXT "overflow in ..." (the zero-divisor text IS checked, it is built without
    /// format!).
    fn stub_overflow<T>(_what: &str) -> kernels::R<T> {
        Err(kernels::EngineError(String::from("OVF")))
    }

    /// Stand-in for `i64::overflowing_mul` / `overflowing_sub` in the LOGIC proofs: a cheap pure function with an
    /// independent result and overflow flag per call (the flag is bit 0 of x, the result mixes both operands, so with
    /// free inputs every row can independently overflow or not and take any result). The logic proofs then hold for
    /// the real loop structure of kernels.rs whatever the primitive computes.
    fn fake_op(x: i64, y: i64) -> (i64, bool) {
        (x ^ y.rotate_left(1), x & 1 == 1)
    }

    /// Stand-in for `i64::checked_div` in the division LOGIC proofs: any Option, any value (None when the dividend is
    /// odd, otherwise a mix of both operands). Deliberately NOT tied to "None iff zero divisor or MIN / -1": the
    /// kernel's error flags are computed by comparisons and must not depend on it.
    fn fake_checked_div(a: i64, b: i64) -> Option<i64> {
        if a & 1 == 1 { None } else { Some(a ^ b.rotate_left(1)) }
    }

    /// Truncating division, characterised without a division: q is the quotient of a / b iff a = q*b + r with
    /// |r| < |b| and r is 0 or has the sign of a. (Unique q for b != 0. `characterisation_*` proves it equals
    /// division at 8 and 16 bits.) i128 so nothing overflows for 64-bit inputs.
    fn is_trunc_quotient(a: i128, b: i128, q: i128) -> bool {
        let r = a - q * b;
        r.unsigned_abs() < b.unsigned_abs() && (r == 0 || (r < 0) == (a < 0))
    }

    // ---- each() ----------------------------------------------------------------------------------------

    /// `each` visits exactly the entries of the selection, in order, or 0..rows for None. Any slice, not only
    /// sorted unique ones.
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
        kani::cover!(use_sel && len > 0);
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

    // ---- mul / sub_const_left ----------------------------------------------------------------------------

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
        if rows >= 1 { kani::cover!(expect_err); }
        if rows >= 1 { kani::cover!(!expect_err && masked_overflow); }
        if rows >= 1 { kani::cover!(!expect_err && !masked_overflow && any_valid_selected && sel.present); }
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

    /// Multiplication at 64 bits: a 64x64 multiplier miter (28 s with 64 GB in the previous run).
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

    /// `checked_div` at 64 bits, for ALL pairs: None exactly for a zero divisor and MIN / -1; otherwise the exact
    /// truncating quotient (spec via the characterisation, so no second divider in the miter). A 64-bit divider miter:
    /// expensive for CBMC, GCP-only.
    #[kani::proof]
    fn primitive_checked_div_i64_is_exact() {
        let (a, b): (i64, i64) = kani::any();
        let fails = b == 0 || (a == i64::MIN && b == -1);
        kani::cover!(fails);
        kani::cover!(!fails && a < 0 && b > 1);
        match a.checked_div(b) {
            None => assert!(fails),
            Some(q) => {
                assert!(!fails);
                assert!(is_trunc_quotient(a as i128, b as i128, q as i128));
            }
        }
    }

    /// The same at 8, 16 and 32 bits, with the quotient computed by a division in the next wider type (cannot
    /// overflow there) instead of the characterisation: shows the shape of the argument for std's width-generic
    /// `checked_div` (a macro over all integer types) and is cheap enough to run locally at 8 and 16 bits.
    #[kani::proof]
    fn primitive_checked_div_i8_is_exact() {
        let (a, b): (i8, i8) = kani::any();
        let fails = b == 0 || (a == i8::MIN && b == -1);
        kani::cover!(fails);
        kani::cover!(!fails && a < 0 && b > 1);
        match a.checked_div(b) {
            None => assert!(fails),
            Some(q) => {
                assert!(!fails);
                assert!(q as i32 == (a as i32) / (b as i32));
            }
        }
    }
    #[kani::proof]
    fn primitive_checked_div_i16_is_exact() {
        let (a, b): (i16, i16) = kani::any();
        let fails = b == 0 || (a == i16::MIN && b == -1);
        kani::cover!(fails);
        kani::cover!(!fails && a < 0 && b > 1);
        match a.checked_div(b) {
            None => assert!(fails),
            Some(q) => {
                assert!(!fails);
                assert!(q as i32 == (a as i32) / (b as i32));
            }
        }
    }
    #[kani::proof]
    fn primitive_checked_div_i32_is_exact() {
        let (a, b): (i32, i32) = kani::any();
        let fails = b == 0 || (a == i32::MIN && b == -1);
        kani::cover!(fails);
        kani::cover!(!fails && a < 0 && b > 1);
        match a.checked_div(b) {
            None => assert!(fails),
            Some(q) => {
                assert!(!fails);
                assert!(q as i64 == (a as i64) / (b as i64));
            }
        }
    }

    /// The characterisation used as the 64-bit division spec equals division (checked here at 8 bits for ALL
    /// dividend, divisor and candidate quotient values with b != 0, and at 16 bits).
    #[kani::proof]
    fn characterisation_of_truncating_division_i8() {
        let (a, b, q): (i8, i8, i16) = kani::any();
        kani::assume(b != 0);
        let want = q as i32 == (a as i32) / (b as i32);
        kani::cover!(want && a < 0 && b > 0);
        assert!(is_trunc_quotient(a as i128, b as i128, q as i128) == want);
    }
    #[kani::proof]
    fn characterisation_of_truncating_division_i16() {
        let (a, b, q): (i16, i16, i32) = kani::any();
        kani::assume(b != 0);
        let want = q as i64 == (a as i64) / (b as i64);
        kani::cover!(want && a < 0 && b > 0);
        assert!(is_trunc_quotient(a as i128, b as i128, q as i128) == want);
    }

    // ---- div / div_const_left ----------------------------------------------------------------------------

    /// The kind of an error, by length: the zero-divisor message "division by zero" has 16 bytes, the stubbed overflow
    /// error ("OVF") 3. (A byte-by-byte compare needs unwind(20) and doubled the cost of every division proof; the
    /// exact text is proved once, in `div_zero_message_text_*`.)
    fn is_zero_msg(e: &kernels::EngineError) -> bool {
        e.0.len() == 16
    }

    /// The zero-divisor error text is exactly "division by zero" (built without format!, so no stub is needed), for
    /// `div` and `div_const_left`, with the divisor zero in a selected valid row.
    #[kani::proof]
    #[kani::unwind(20)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    fn div_zero_message_text() {
        let a: [i64; 1] = kani::any();
        let e = kernels::div((&a, Valid::All), (&[0i64], Valid::All), 1, None).err();
        assert!(e == Some(kernels::EngineError(String::from("division by zero"))));
        let e = kernels::div_const_left(a[0], (&[0i64], Valid::All), 1, None).err();
        assert!(e == Some(kernels::EngineError(String::from("division by zero"))));
    }

    /// Shared postcondition for div / div_const_left.
    /// Spec (ADR 0003 decision 4, kernels.rs tests): an error is raised exactly when some row that is selected AND
    /// valid has a zero divisor or is MIN / -1; it says "division by zero" if any such row has a zero divisor and
    /// is an overflow otherwise; unselected rows and NULL slots never raise. On success: the output has `rows`
    /// values; for selected rows the validity is `valid(i)` and, when valid, the value satisfies `value_ok`;
    /// unselected rows are untouched (0, not valid). `zero_when_failed`: a selected NULL slot whose division would
    /// fail holds the quotient 0 (the doc of div_step; needs the real, exact checked_div).
    fn check_div(
        r: Result<kernels::Out, kernels::EngineError>,
        rows: usize,
        sel: &Sel,
        dividend: impl Fn(usize) -> i64,
        divisor: impl Fn(usize) -> i64,
        valid: impl Fn(usize) -> bool,
        value_ok: impl Fn(usize, i64) -> bool,
        zero_when_failed: bool,
    ) {
        let (mut want_zero, mut want_ovf, mut masked_fail, mut any_ok) = (false, false, false, false);
        for i in 0..N {
            if i < rows {
                let z = divisor(i) == 0;
                let o = dividend(i) == i64::MIN && divisor(i) == -1;
                let live = sel.has(i) && valid(i);
                want_zero |= live && z;
                want_ovf |= live && o;
                masked_fail |= (z || o) && !live;
                any_ok |= live && !z && !o;
            }
        }
        if rows >= 1 { kani::cover!(want_zero); }
        if rows >= 1 { kani::cover!(want_ovf && !want_zero); }
        if rows >= 2 { kani::cover!(want_zero && want_ovf); }
        if rows >= 1 { kani::cover!(!want_zero && !want_ovf && masked_fail); }
        if rows >= 1 { kani::cover!(!want_zero && !want_ovf && !masked_fail && any_ok && sel.present); }
        assert!(r.is_err() == (want_zero || want_ovf));
        match r {
            Err(e) => assert!(is_zero_msg(&e) == want_zero),
            Ok(out) => {
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
                                assert!(value_ok(i, out.v[i]));
                            } else if zero_when_failed && (divisor(i) == 0 || (dividend(i) == i64::MIN && divisor(i) == -1)) {
                                assert!(out.v[i] == 0);
                            }
                        } else {
                            assert!(out.v[i] == 0);
                            assert!(out.valid.as_ref().is_none_or(|v| v[i] == 0));
                        }
                    }
                }
            }
        }
    }

    /// LOGIC proof: `checked_div` replaced by `fake_checked_div`; the value spec is the stand-in's value.
    fn div_logic_matches_the_spec<const ROWS: usize>() {
        let rows = ROWS;
        let (a, b): ([i64; N], [i64; N]) = (kani::any(), kani::any());
        let (sa, sb) = (any_vs(rows), any_vs(rows));
        let sel = any_sel(rows);
        let r = kernels::div((&a[..rows], sa.valid(rows)), (&b[..rows], sb.valid(rows)), rows, sel.get());
        check_div(r, rows, &sel, |i| a[i], |i| b[i], |i| sa.ok(i) && sb.ok(i),
            |i, got| got == fake_checked_div(a[i], b[i]).unwrap_or(0), false);
    }

    fn div_const_left_logic_matches_the_spec<const ROWS: usize>() {
        let rows = ROWS;
        let c: i64 = kani::any();
        let a: [i64; N] = kani::any();
        let sa = any_vs(rows);
        let sel = any_sel(rows);
        let r = kernels::div_const_left(c, (&a[..rows], sa.valid(rows)), rows, sel.get());
        check_div(r, rows, &sel, |_| c, |i| a[i], |i| sa.ok(i),
            |i, got| got == fake_checked_div(c, a[i]).unwrap_or(0), false);
    }

    /// NO abstraction: the real 64-bit `checked_div`, the value spec is the characterisation of truncating division.
    /// Expensive (a 64-bit divider and a 64x64 multiplier per row): GCP-only, rows <= 2.
    fn div_exact_matches_the_spec<const ROWS: usize>() {
        let rows = ROWS;
        let (a, b): ([i64; N], [i64; N]) = (kani::any(), kani::any());
        let (sa, sb) = (any_vs(rows), any_vs(rows));
        let sel = any_sel(rows);
        let r = kernels::div((&a[..rows], sa.valid(rows)), (&b[..rows], sb.valid(rows)), rows, sel.get());
        check_div(r, rows, &sel, |i| a[i], |i| b[i], |i| sa.ok(i) && sb.ok(i),
            |i, got| is_trunc_quotient(a[i] as i128, b[i] as i128, got as i128), true);
    }

    fn div_const_left_exact_matches_the_spec<const ROWS: usize>() {
        let rows = ROWS;
        let c: i64 = kani::any();
        let a: [i64; N] = kani::any();
        let sa = any_vs(rows);
        let sel = any_sel(rows);
        let r = kernels::div_const_left(c, (&a[..rows], sa.valid(rows)), rows, sel.get());
        check_div(r, rows, &sel, |_| c, |i| a[i], |i| sa.ok(i),
            |i, got| is_trunc_quotient(c as i128, a[i] as i128, got as i128), true);
    }

    // ---- filter_ne / split_ne / merge_branches / the CASE pipeline ---------------------------------------

    /// split_ne partitions the input selection. Spec, from the OUTPUTS and the inputs only: walking the input
    /// sequence in order, every element equals the next unread element of `yes` when `p(row)` (valid and
    /// col <> value) and of `no` otherwise, and both outputs are fully consumed. That is: no row lost, none
    /// duplicated, order kept, each row in the right branch. Plus: the branches share no row, and a guard row
    /// (NULL, or equal to the value) never appears in `yes`.
    fn split_ne_partitions_the_selection<const ROWS: usize>() {
        let rows = ROWS;
        let col: [i64; N] = kani::any();
        let value: i64 = kani::any();
        let vs = any_vs(rows);
        let seq = any_seq(rows);
        let (yes, no) = kernels::split_ne(&col[..rows], vs.valid(rows), value, rows, seq.get());
        let p = |i: usize| vs.ok(i) && col[i] != value;
        let m = seq.len(rows);
        let (mut py, mut pn) = (0usize, 0usize);
        let (mut seen_yes, mut seen_no) = (false, false);
        for j in 0..N {
            if j < m {
                let i = seq.at(j);
                if p(i) {
                    seen_yes = true;
                    assert!(py < yes.len());
                    assert!(yes[py] as usize == i);
                    py += 1;
                } else {
                    seen_no = true;
                    assert!(pn < no.len());
                    assert!(no[pn] as usize == i);
                    pn += 1;
                }
            }
        }
        if rows >= 2 { kani::cover!(seen_yes && seen_no); }
        if rows >= 1 { kani::cover!(seen_no && !seen_yes && m > 0); }
        if rows >= 2 { kani::cover!(seen_yes && vs.kind != 0 && (0..N).any(|i| i < rows && !vs.ok(i) && seq.has(rows, i))); }
        assert!(py == yes.len() && pn == no.len());
        assert!(yes.len() + no.len() == m);
        // guard rows never reach `yes` (p fails for them); every `no` row fails p; so the branches share no row
        // (p(x) and !p(x) cannot both hold)
        for x in &yes {
            assert!(p(*x as usize));
        }
        for y in &no {
            assert!(!p(*y as usize));
        }
    }

    /// filter_ne returns exactly the in-order rows of the input sequence where the comparison is TRUE.
    fn filter_ne_matches_the_spec<const ROWS: usize>() {
        let rows = ROWS;
        let col: [i64; N] = kani::any();
        let value: i64 = kani::any();
        let vs = any_vs(rows);
        let seq = any_seq(rows);
        let got = kernels::filter_ne(&col[..rows], vs.valid(rows), value, rows, seq.get());
        let m = seq.len(rows);
        let mut k = 0usize;
        for j in 0..N {
            if j < m {
                let i = seq.at(j);
                if vs.ok(i) && col[i] != value {
                    assert!(k < got.len());
                    assert!(got[k] as usize == i);
                    k += 1;
                }
            }
        }
        if rows >= 2 { kani::cover!(k > 0 && k < m); }
        assert!(got.len() == k);
    }

    /// merge_branches with two parts of arbitrary index sequences (overlaps allowed: the later part wins) and
    /// arbitrary branch outputs. Spec: output position i holds the value and validity of the LAST part whose
    /// sequence contains i (validity: that part's byte, or 1 if the part has no validity vector); a row in no part
    /// is NULL; the output has `rows` entries and always a validity vector.
    fn merge_branches_matches_the_spec<const ROWS: usize>() {
        let rows = ROWS;
        let (s0, s1) = (any_seq(rows), any_seq(rows));
        let (v0, v1): ([i64; N], [i64; N]) = (kani::any(), kani::any());
        let (b0, b1): ([u8; N], [u8; N]) = (kani::any(), kani::any());
        let (has0, has1): (bool, bool) = (kani::any(), kani::any());
        let o0 = kernels::Out { v: v0[..rows].to_vec(), valid: has0.then(|| b0[..rows].to_vec()) };
        let o1 = kernels::Out { v: v1[..rows].to_vec(), valid: has1.then(|| b1[..rows].to_vec()) };
        // the parts are plain index sequences here (present = false would mean "all rows": use 0..rows)
        let all: [u32; N] = [0, 1, 2, 3];
        let p0: &[u32] = if s0.present { &s0.buf[..s0.n] } else { &all[..rows] };
        let p1: &[u32] = if s1.present { &s1.buf[..s1.n] } else { &all[..rows] };
        let m = kernels::merge_branches(rows, &[(p0, &o0), (p1, &o1)]);
        assert!(m.v.len() == rows);
        let mv = m.valid.as_ref().expect("merge_branches always has a validity vector");
        assert!(mv.len() == rows);
        let (mut in1, mut only0, mut none) = (false, false, false);
        for i in 0..N {
            if i < rows {
                let i0 = (0..N).any(|j| j < p0.len() && p0[j] as usize == i);
                let i1 = (0..N).any(|j| j < p1.len() && p1[j] as usize == i);
                if i1 {
                    in1 = true;
                    assert!(m.v[i] == v1[i]);
                    assert!(mv[i] == if has1 { b1[i] } else { 1 });
                } else if i0 {
                    only0 = true;
                    assert!(m.v[i] == v0[i]);
                    assert!(mv[i] == if has0 { b0[i] } else { 1 });
                } else {
                    none = true;
                    assert!(mv[i] == 0);
                }
            }
        }
        if rows >= 3 { kani::cover!(in1 && only0 && none); }
        if rows >= 1 { kani::cover!(in1 && (0..N).any(|i| i < rows && (0..N).any(|j| j < p0.len() && p0[j] as usize == i) && (0..N).any(|j| j < p1.len() && p1[j] as usize == i))); }
    }

    /// `CASE WHEN x <> 0 THEN a / x ELSE 0 END` over a selection, composed from the real split_ne, div,
    /// constant and merge_branches (division with the stand-in). Spec: an error iff some selected row with x valid
    /// and x <> 0 and a valid has a = MIN and x = -1 (an overflow, NEVER "division by zero": a guard row does not
    /// reach the division); otherwise unselected rows are NULL, rows with x NULL or x = 0 are valid 0, and the rest
    /// are NULL when a is NULL and the division value otherwise.
    fn case_guarded_division_matches_the_spec<const ROWS: usize>() {
        let rows = ROWS;
        let (a, x): ([i64; N], [i64; N]) = (kani::any(), kani::any());
        let (sa, sx) = (any_vs(rows), any_vs(rows));
        let sel = any_sel(rows);
        let (yes, no) = kernels::split_ne(&x[..rows], sx.valid(rows), 0, rows, sel.get());
        let q = kernels::div((&a[..rows], sa.valid(rows)), (&x[..rows], sx.valid(rows)), rows, Some(yes.as_slice()));
        let zero = kernels::constant(0, rows);
        let r = q.map(|q| kernels::merge_branches(rows, &[(yes.as_slice(), &q), (no.as_slice(), &zero)]));
        let mut want_err = false;
        let (mut guarded, mut divided) = (false, false);
        for i in 0..N {
            if i < rows && sel.has(i) {
                if sx.ok(i) && x[i] != 0 {
                    divided = true;
                    want_err |= sa.ok(i) && a[i] == i64::MIN && x[i] == -1;
                } else {
                    guarded = true;
                }
            }
        }
        if rows >= 1 { kani::cover!(want_err); }
        if rows >= 2 { kani::cover!(!want_err && guarded && divided); }
        if rows >= 1 { kani::cover!(!want_err && (0..N).any(|i| i < rows && sel.has(i) && sx.ok(i) && x[i] == 0)); }
        assert!(r.is_err() == want_err);
        match r {
            Err(e) => assert!(!is_zero_msg(&e)),
            Ok(out) => {
                assert!(out.v.len() == rows);
                let ov = out.valid.as_ref().expect("merge always has validity");
                assert!(ov.len() == rows);
                for i in 0..N {
                    if i < rows {
                        if !sel.has(i) {
                            assert!(ov[i] == 0);
                        } else if sx.ok(i) && x[i] != 0 {
                            assert!((ov[i] != 0) == sa.ok(i));
                            if sa.ok(i) {
                                assert!(out.v[i] == fake_checked_div(a[i], x[i]).unwrap_or(0));
                            }
                        } else {
                            assert!(ov[i] == 1 && out.v[i] == 0);
                        }
                    }
                }
            }
        }
    }

    // ---- filter_range ----------------------------------------------------------------------------------

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
        if SEL >= 1 { kani::cover!(k > 0 && sel.present); }
        if rows >= 1 { kani::cover!(k == 0 && rows > 0); }
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
        if rows >= 2 { kani::cover!(want_n > 0 && want_n < rows as u64); }
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
        if rows >= 2 { kani::cover!(sel.present && sel.n >= 2 && ids[0] != ids[1]); }
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
        if rows >= 1 { kani::cover!(want_nulls[0][0] != nulls0[0][0] && want_acc[0][0] != acc0[0][0]); }
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

    // ---- harnesses: one per concrete row count -------------------------------------------------------
    // A symbolic `rows` makes the Vec allocations inside the kernels run CBMC out of memory, so each row count is
    // its own proof. Together they cover every row count up to the bound (see the results file for the bounds).
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
    #[kani::unwind(5)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::checked_div, fake_checked_div)]
    fn div_logic_matches_the_spec_rows0() {
        div_logic_matches_the_spec::<0>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::checked_div, fake_checked_div)]
    fn div_logic_matches_the_spec_rows1() {
        div_logic_matches_the_spec::<1>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::checked_div, fake_checked_div)]
    fn div_logic_matches_the_spec_rows2() {
        div_logic_matches_the_spec::<2>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::checked_div, fake_checked_div)]
    fn div_logic_matches_the_spec_rows3() {
        div_logic_matches_the_spec::<3>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::checked_div, fake_checked_div)]
    fn div_logic_matches_the_spec_rows4() {
        div_logic_matches_the_spec::<4>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::checked_div, fake_checked_div)]
    fn div_const_left_logic_matches_the_spec_rows0() {
        div_const_left_logic_matches_the_spec::<0>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::checked_div, fake_checked_div)]
    fn div_const_left_logic_matches_the_spec_rows1() {
        div_const_left_logic_matches_the_spec::<1>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::checked_div, fake_checked_div)]
    fn div_const_left_logic_matches_the_spec_rows2() {
        div_const_left_logic_matches_the_spec::<2>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::checked_div, fake_checked_div)]
    fn div_const_left_logic_matches_the_spec_rows3() {
        div_const_left_logic_matches_the_spec::<3>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::checked_div, fake_checked_div)]
    fn div_const_left_logic_matches_the_spec_rows4() {
        div_const_left_logic_matches_the_spec::<4>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    fn div_exact_matches_the_spec_rows0() {
        div_exact_matches_the_spec::<0>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    fn div_exact_matches_the_spec_rows1() {
        div_exact_matches_the_spec::<1>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    fn div_exact_matches_the_spec_rows2() {
        div_exact_matches_the_spec::<2>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    fn div_const_left_exact_matches_the_spec_rows0() {
        div_const_left_exact_matches_the_spec::<0>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    fn div_const_left_exact_matches_the_spec_rows1() {
        div_const_left_exact_matches_the_spec::<1>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    fn div_const_left_exact_matches_the_spec_rows2() {
        div_const_left_exact_matches_the_spec::<2>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    fn split_ne_partitions_the_selection_rows0() {
        split_ne_partitions_the_selection::<0>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    fn split_ne_partitions_the_selection_rows1() {
        split_ne_partitions_the_selection::<1>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    fn split_ne_partitions_the_selection_rows2() {
        split_ne_partitions_the_selection::<2>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    fn split_ne_partitions_the_selection_rows3() {
        split_ne_partitions_the_selection::<3>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    fn split_ne_partitions_the_selection_rows4() {
        split_ne_partitions_the_selection::<4>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    fn filter_ne_matches_the_spec_rows0() {
        filter_ne_matches_the_spec::<0>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    fn filter_ne_matches_the_spec_rows1() {
        filter_ne_matches_the_spec::<1>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    fn filter_ne_matches_the_spec_rows2() {
        filter_ne_matches_the_spec::<2>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    fn filter_ne_matches_the_spec_rows3() {
        filter_ne_matches_the_spec::<3>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    fn filter_ne_matches_the_spec_rows4() {
        filter_ne_matches_the_spec::<4>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    fn merge_branches_matches_the_spec_rows0() {
        merge_branches_matches_the_spec::<0>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    fn merge_branches_matches_the_spec_rows1() {
        merge_branches_matches_the_spec::<1>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    fn merge_branches_matches_the_spec_rows2() {
        merge_branches_matches_the_spec::<2>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    fn merge_branches_matches_the_spec_rows3() {
        merge_branches_matches_the_spec::<3>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    fn merge_branches_matches_the_spec_rows4() {
        merge_branches_matches_the_spec::<4>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::checked_div, fake_checked_div)]
    fn case_guarded_division_matches_the_spec_rows0() {
        case_guarded_division_matches_the_spec::<0>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::checked_div, fake_checked_div)]
    fn case_guarded_division_matches_the_spec_rows1() {
        case_guarded_division_matches_the_spec::<1>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::checked_div, fake_checked_div)]
    fn case_guarded_division_matches_the_spec_rows2() {
        case_guarded_division_matches_the_spec::<2>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::checked_div, fake_checked_div)]
    fn case_guarded_division_matches_the_spec_rows3() {
        case_guarded_division_matches_the_spec::<3>()
    }
    #[kani::proof]
    #[kani::unwind(5)]
    #[kani::stub(kernels::overflow, stub_overflow)]
    #[kani::stub(i64::checked_div, fake_checked_div)]
    fn case_guarded_division_matches_the_spec_rows4() {
        case_guarded_division_matches_the_spec::<4>()
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

#[path = "../../../crates/nata-kernels/src/decimal.rs"]
mod decimal;

#[cfg(kani)]
mod decimal_proofs {
    use super::decimal::{Decimal, Fail, Op};
    // Two rows, arbitrary signed 8-bit values, both validity maps and every
    // subset selection. The specification uses bounded exact i128 arithmetic.
    fn selected_null_and_bounds(op: Op) {
        let a = [i128::from(kani::any::<i8>()), i128::from(kani::any::<i8>())];
        let b = [i128::from(kani::any::<i8>()), i128::from(kani::any::<i8>())];
        let av: [bool;2] = kani::any();
        let bv: [bool;2] = kani::any();
        let selected: [bool;2] = kani::any();
        let which = match op { Op::Add => 0, Op::Sub => 1, _ => 2 };
        let mut sel = [0;2]; let mut n = 0;
        let mut expected = [0;2]; let mut valid = [false;2]; let mut bad = false;
        for i in 0..2 {
            if selected[i] {
                sel[n] = i as u32; n += 1;
                if av[i] && bv[i] {
                    let v = match op { Op::Add => a[i]+b[i], Op::Sub => a[i]-b[i], _ => a[i]*b[i] };
                    bad |= a[i].unsigned_abs() >= 100 && which < 2;
                    bad |= b[i].unsigned_abs() >= 100 && which < 2;
                    bad |= v.unsigned_abs() >= 100;
                    expected[i] = v; valid[i] = true;
                }
            }
        }
        let (mut got, mut gv) = ([0;2], [false;2]);
        let result = Decimal::new(op,0,0,2,0).unwrap().column(&a,&av,&b,&bv,&sel[..n],&mut got,&mut gv);
        assert_eq!(result.is_err(), bad);
        if !bad {
            for i in 0..2 { assert!(got[i] == expected[i]); assert!(gv[i] == valid[i]); }
        }
        kani::cover!(bad);
        kani::cover!(!bad && valid[0] && !valid[1]);
    }
    #[kani::proof]
    #[kani::unwind(3)]
    fn decimal_column_selected_null_and_bounds() { selected_null_and_bounds(Op::Mul); }
    #[kani::proof]
    #[kani::unwind(3)]
    fn decimal_column_add_selected_null_and_bounds() { selected_null_and_bounds(Op::Add); }
    #[kani::proof]
    #[kani::unwind(3)]
    fn decimal_column_sub_selected_null_and_bounds() { selected_null_and_bounds(Op::Sub); }
    #[kani::proof]
    #[kani::unwind(8)]
    fn decimal_column_machine_overflow() {
        let x = if kani::any::<bool>() { i128::MAX } else { i128::MIN };
        let ok: bool = kani::any();
        let mut out = [0]; let mut valid = [false];
        let result = Decimal::new(Op::Mul,0,0,38,0).unwrap().column(&[x],&[ok],&[2],&[true],&[0],&mut out,&mut valid);
        assert_eq!(result, if ok { Err(Fail::Overflow) } else { Ok(()) });
        kani::cover!(ok); kani::cover!(!ok);
    }
}

mod perf2c;

#[path = "../../../bench/results/raw/20261108-hash-aggregate-perf2c/retry-product-decimal.rs"]
mod rejected_decimal;

#[path = "../../../bench/results/raw/20261108-hash-aggregate-perf2c/retry-sum-admission.rs"]
mod sum_admission;
