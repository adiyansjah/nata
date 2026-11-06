//! Word-wise kernels: checked, NULL-aware, selection-aware, with validity and dense selections as bitmaps.
//!
//! Same rules as `kernels.rs` (ADR 0003 decision 4): an arithmetic overflow is an error only for a row that
//! is selected AND valid; a NULL input gives a NULL output; a NULL in a filter column drops the row.
//!
//! Two forms of selection:
//! - dense (`Sel::All`, `Sel::Mask`): the kernel walks every row of the batch 64 at a time. Validity operands
//!   are combined with one AND per word, the output validity is a bitmap built the same way, and the error
//!   flag is `(overflow_bits & valid_word & selected_word) != 0` per word. Rows that are not selected or not
//!   valid are computed with wrapping/overflowing operations and masked out, never branched on.
//! - sparse (`Sel::Idx`): the kernel visits only the listed rows and tests one validity bit per row.
use crate::bits::*;

pub use crate::kernels::{EngineError, R, overflow};

/// A column: values and an optional validity bitmap (`None` = no NULLs).
pub type WCol<'a, T> = (&'a [T], Option<&'a [u64]>);

/// A computed column: full length, only the selected rows are meaningful.
#[derive(Debug, Clone)]
pub struct Out {
    pub v: Vec<i64>,
    pub valid: Option<Vec<u64>>,
}

impl Out {
    pub fn col(&self) -> WCol<'_, i64> {
        (&self.v, self.valid.as_deref())
    }
}

#[inline(always)]
fn bit(valid: Option<&[u64]>, i: usize) -> bool {
    valid.is_none_or(|v| (v[i >> 6] >> (i & 63)) & 1 == 1)
}

/// Settings for how a filter hands its result on.
#[derive(Clone, Copy)]
pub struct FilterOpts {
    /// Keep a bitmap while at least `rows / dense_div` rows survive; 0 = always build the index vector.
    pub dense_div: usize,
}

/// Rows of the input selection whose value lies in `[lo, hi]` and is not NULL.
pub fn filter_range<T: Copy + PartialOrd>(col: &[T], valid: Option<&[u64]>, lo: T, hi: T, rows: usize, sel: &Sel, o: FilterOpts) -> Sel {
    match sel {
        Sel::Idx(idx) => {
            let mut out = vec![0u32; idx.len()];
            let mut k = 0usize;
            match valid {
                None => idx.iter().for_each(|&i| {
                    let x = col[i as usize];
                    out[k] = i;
                    k += ((x >= lo) & (x <= hi)) as usize;
                }),
                Some(v) => idx.iter().for_each(|&i| {
                    let x = col[i as usize];
                    out[k] = i;
                    k += ((x >= lo) & (x <= hi) & ((v[(i >> 6) as usize] >> (i & 63)) & 1 == 1)) as usize;
                }),
            }
            out.truncate(k);
            Sel::Idx(out)
        }
        _ => {
            let nw = nwords(rows);
            let mut out = vec![0u64; nw];
            let mut count = 0usize;
            for (w, chunk) in col[..rows].chunks(64).enumerate() {
                let mut m = 0u64;
                for (j, &x) in chunk.iter().enumerate() {
                    m |= (((x >= lo) & (x <= hi)) as u64) << j;
                }
                m &= sel.word(rows, w);
                if let Some(v) = valid {
                    m &= v[w];
                }
                out[w] = m;
                count += m.count_ones() as usize;
            }
            if count == rows {
                Sel::All
            } else if o.dense_div > 0 && count * o.dense_div >= rows {
                Sel::Mask(out)
            } else {
                Sel::Idx(mask_to_idx(&out))
            }
        }
    }
}

/// `c - a`
pub fn sub_const_left(c: i64, a: WCol<i64>, rows: usize, sel: &Sel, defer: bool) -> R<Out> {
    let (a, va) = a;
    let mut v = vec![0i64; rows];
    if let Sel::Idx(idx) = sel {
        let mut valid = va.map(|_| vec![0u64; nwords(rows)]);
        let mut bad = false;
        for &i in idx {
            let i = i as usize;
            let (r, o) = c.overflowing_sub(a[i]);
            v[i] = r;
            let ok = bit(va, i);
            if let Some(w) = valid.as_mut() {
                w[i >> 6] |= (ok as u64) << (i & 63);
            }
            bad |= o & ok;
        }
        return if bad { overflow("subtraction") } else { Ok(Out { v, valid }) };
    }
    let mut bad = 0u64;
    for (w, (ac, vc)) in a[..rows].chunks(64).zip(v.chunks_mut(64)).enumerate() {
        let live = sel.word(rows, w) & va.map_or(!0, |m| m[w]);
        if live == 0 {
            continue; // nothing in this word is selected and valid: nothing to compute, nothing to check
        }
        if defer {
            // Branch-free in the common case: one OR of the overflow flags; the exact masked check runs only if any row overflowed.
            let mut any = false;
            for (x, r) in ac.iter().zip(vc.iter_mut()) {
                let (q, o) = c.overflowing_sub(*x);
                *r = q;
                any |= o;
            }
            if any {
                bad |= ac.iter().enumerate().fold(0u64, |ov, (j, x)| ov | ((c.overflowing_sub(*x).1 as u64) << j)) & live;
            }
        } else {
            let mut ov = 0u64;
            for (j, (x, r)) in ac.iter().zip(vc.iter_mut()).enumerate() {
                let (q, o) = c.overflowing_sub(*x);
                *r = q;
                ov |= (o as u64) << j;
            }
            bad |= ov & live;
        }
    }
    if bad != 0 {
        return overflow("subtraction");
    }
    Ok(Out { v, valid: va.map(<[u64]>::to_vec) })
}

/// `a * b`
pub fn mul(a: WCol<i64>, b: WCol<i64>, rows: usize, sel: &Sel, defer: bool) -> R<Out> {
    let ((a, va), (b, vb)) = (a, b);
    let mut v = vec![0i64; rows];
    if let Sel::Idx(idx) = sel {
        let mut valid = (va.is_some() || vb.is_some()).then(|| vec![0u64; nwords(rows)]);
        let mut bad = false;
        for &i in idx {
            let i = i as usize;
            let (r, o) = a[i].overflowing_mul(b[i]);
            v[i] = r;
            let ok = bit(va, i) & bit(vb, i);
            if let Some(w) = valid.as_mut() {
                w[i >> 6] |= (ok as u64) << (i & 63);
            }
            bad |= o & ok;
        }
        return if bad { overflow("multiplication") } else { Ok(Out { v, valid }) };
    }
    let mut bad = 0u64;
    let mut valid: Option<Vec<u64>> = None;
    if va.is_some() || vb.is_some() {
        valid = Some((0..nwords(rows)).map(|w| va.map_or(!0, |m| m[w]) & vb.map_or(!0, |m| m[w]) & word_mask(rows, w)).collect());
    }
    for (w, ((ac, bc), vc)) in a[..rows].chunks(64).zip(b[..rows].chunks(64)).zip(v.chunks_mut(64)).enumerate() {
        let live = sel.word(rows, w) & valid.as_ref().map_or(!0, |m| m[w]);
        if live == 0 {
            continue;
        }
        if defer {
            let mut any = false;
            for ((x, y), r) in ac.iter().zip(bc).zip(vc.iter_mut()) {
                let (q, o) = x.overflowing_mul(*y);
                *r = q;
                any |= o;
            }
            if any {
                bad |= ac.iter().zip(bc).enumerate().fold(0u64, |ov, (j, (x, y))| ov | ((x.overflowing_mul(*y).1 as u64) << j)) & live;
            }
        } else {
            let mut ov = 0u64;
            for (j, ((x, y), r)) in ac.iter().zip(bc).zip(vc.iter_mut()).enumerate() {
                let (q, o) = x.overflowing_mul(*y);
                *r = q;
                ov |= (o as u64) << j;
            }
            bad |= ov & live;
        }
    }
    if bad != 0 {
        return overflow("multiplication");
    }
    Ok(Out { v, valid })
}

/// Exact sum of the valid selected values, and how many there were. i128 cannot overflow here.
///
/// Dense form: the selected rows are summed without looking at validity (only the selection masks a word, and
/// not even that when the whole word is selected), then the selected NULL rows, which are few, are subtracted
/// again through the set bits of `selected & !valid`. The number of valid rows is `popcount(selected & valid)`.
pub fn sum(col: WCol<i64>, rows: usize, sel: &Sel) -> (i128, u64) {
    let (v, valid) = col;
    let (mut s, mut n) = (0i128, 0u64);
    if let Sel::Idx(idx) = sel {
        for &i in idx {
            let i = i as usize;
            let ok = bit(valid, i);
            s += (v[i] as i128) * (ok as i128);
            n += ok as u64;
        }
        return (s, n);
    }
    for (w, chunk) in v[..rows].chunks(64).enumerate() {
        let sw = sel.word(rows, w);
        if sw == 0 {
            continue;
        }
        let vw = valid.map_or(!0, |x| x[w]);
        n += (sw & vw).count_ones() as u64;
        let mut block = 0i128;
        if sw == word_mask(rows, w) {
            for &x in chunk {
                block += x as i128;
            }
        } else {
            for (j, &x) in chunk.iter().enumerate() {
                block += (x as i128) & -(((sw >> j) & 1) as i128);
            }
        }
        let mut nulls = sw & !vw;
        while nulls != 0 {
            block -= chunk[nulls.trailing_zeros() as usize] as i128;
            nulls &= nulls - 1;
        }
        s += block;
    }
    (s, n)
}

/// Number of group slots of `group_sums`; group ids are below this by construction (the sink checks it).
pub const NG: usize = 1024;

/// Single pass over the selected rows for K SUM columns: count of selected rows per group, and per column the
/// sum of the valid selected values and `nulls`, the number of selected rows where the column is NULL (so the
/// non-NULL count of a group is `count - nulls[k]`). A column without a bitmap has no NULLs and `nulls[k]` is
/// not touched. `gids[i]` is the group of row i (below `NG`); `acc`, `count` and `nulls[k]` have `NG` slots.
///
/// The accumulations use `wrapping_add` on purpose: they cannot overflow (an `i128` sum of fewer than 2^64
/// values of at most 2^63 in magnitude stays below 2^127; a `u64` count is below the number of rows), and the
/// checked `+` of an `overflow-checks` build puts a branch after each of the ~10 accumulations of every row.
/// `WRAP = false` keeps the checked `+` so the cost of the choice can be measured.
/// `SUB = false` is the first word-wise design (per row: validity bit of every column turned into a mask, NULL
/// counters updated for every row), kept only to measure it against `SUB = true` (see below).
pub fn group_sums<const K: usize, const WRAP: bool, const SUB: bool>(
    gids: &[u16],
    cols: [WCol<i64>; K],
    rows: usize,
    sel: &Sel,
    acc: &mut [[i128; K]],
    count: &mut [u64],
    nulls: &mut [Vec<u64>; K],
) {
    assert!(acc.len() == NG && count.len() == NG && nulls.iter().all(|n| n.len() == NG) && gids.len() >= rows);
    if !matches!(sel, Sel::Idx(_)) {
        // (OR of values < NG = 2^10 is < NG exactly when every value is; an index selection reads only its own rows)
        assert!(gids[..rows].iter().fold(0u16, |a, &g| a | g) < NG as u16, "group id out of range");
    }
    // the accumulations live in wrapadd.rs so that the Kani proof of the WRAP = true case runs on the same code
    let add128 = |a: &mut i128, x: i128| crate::wrapadd::add128::<WRAP>(a, x);
    let add64 = |a: &mut u64, x: u64| crate::wrapadd::add64::<WRAP>(a, x);
    if let Sel::Idx(idx) = sel {
        for &i in idx {
            let i = i as usize;
            assert!((gids[i] as usize) < NG, "group id out of range");
            let g = gids[i] as usize;
            add64(&mut count[g], 1);
            for j in 0..K {
                let ok = bit(cols[j].1, i);
                add128(&mut acc[g][j], (cols[j].0[i] as i128) * (ok as i128));
                add64(&mut nulls[j][g], !ok as u64);
            }
        }
        return;
    }
    if !SUB {
        for w in 0..nwords(rows) {
            let sw = sel.word(rows, w);
            let m: [u64; K] = std::array::from_fn(|j| sw & cols[j].1.map_or(!0, |x| x[w]));
            let null_w: [u64; K] = std::array::from_fn(|j| sw & !m[j]);
            let base = 64 * w;
            let n = (rows - base).min(64);
            let vs: [&[i64]; K] = std::array::from_fn(|k| &cols[k].0[base..base + n]);
            for (j, &g) in gids[base..base + n].iter().enumerate() {
                let g = g as usize % NG;
                add64(&mut count[g], (sw >> j) & 1);
                for k in 0..K {
                    add128(&mut acc[g][k], (vs[k][j] as i128) & -(((m[k] >> j) & 1) as i128));
                    add64(&mut nulls[k][g], (null_w[k] >> j) & 1);
                }
            }
        }
        return;
    }
    // Dense selection. The main loop does not look at validity: it adds every selected row, as if the columns had
    // no NULLs (the selection masks a word, and not even that when every row is selected). The selected rows
    // where a column is NULL are few; each is subtracted again from that column's sum through the set bits of
    // `selected & !valid`, and counted in `nulls`.
    let all_rows = matches!(sel, Sel::All);
    for w in 0..nwords(rows) {
        let sw = sel.word(rows, w);
        let base = 64 * w;
        let n = (rows - base).min(64);
        let vs: [&[i64]; K] = std::array::from_fn(|k| &cols[k].0[base..base + n]);
        let gs = &gids[base..base + n];
        if all_rows {
            for (j, &g) in gs.iter().enumerate() {
                let g = g as usize % NG;
                add64(&mut count[g], 1);
                for k in 0..K {
                    add128(&mut acc[g][k], vs[k][j] as i128);
                }
            }
        } else if sw != 0 {
            for (j, &g) in gs.iter().enumerate() {
                let (g, b) = (g as usize % NG, (sw >> j) & 1);
                add64(&mut count[g], b);
                for k in 0..K {
                    add128(&mut acc[g][k], (vs[k][j] as i128) & -(b as i128));
                }
            }
        }
        for k in 0..K {
            if let Some(valid) = cols[k].1 {
                let mut nw = sw & !valid[w];
                while nw != 0 {
                    let j = nw.trailing_zeros() as usize;
                    nw &= nw - 1;
                    let g = gs[j] as usize % NG;
                    add128(&mut acc[g][k], (vs[k][j] as i128).wrapping_neg());
                    add64(&mut nulls[k][g], 1);
                }
            }
        }
    }
}

#[cfg(test)]
pub mod tests_support {
    pub struct Rng(pub u64);
    impl Rng {
        pub fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        pub fn pct(&mut self, p: u64) -> bool {
            self.next() % 100 < p
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::Rng;
    use super::*;

    /// Truth as plain booleans; every kernel input is derived from it.
    pub struct Case {
        pub n: usize,
        pub a: Vec<i64>,
        pub b: Vec<i64>,
        pub va: Vec<bool>,
        pub vb: Vec<bool>,
        pub selected: Vec<bool>,
    }

    pub fn words_of(bits: &[bool]) -> Vec<u64> {
        let mut w = vec![0u64; nwords(bits.len())];
        for (i, &b) in bits.iter().enumerate() {
            w[i / 64] |= (b as u64) << (i % 64);
        }
        w
    }

    /// `null_pct` of the rows NULL, `sel_pct` selected, `big_pct` of the values large enough to overflow a product.
    pub fn case(rng: &mut Rng, n: usize, null_pct: u64, sel_pct: u64, big_pct: u64) -> Case {
        let mut val = |r: &mut Rng| if r.pct(big_pct) { (r.next() >> 1) as i64 * if r.pct(50) { 1 } else { -1 } } else { (r.next() % 2001) as i64 - 1000 };
        let a = (0..n).map(|_| val(rng)).collect();
        let b = (0..n).map(|_| val(rng)).collect();
        let va = (0..n).map(|_| !rng.pct(null_pct)).collect();
        let vb = (0..n).map(|_| !rng.pct(null_pct)).collect();
        let selected = (0..n).map(|_| rng.pct(sel_pct)).collect();
        Case { n, a, b, va, vb, selected }
    }

    /// The same selected set in the three forms a kernel accepts.
    pub fn forms(selected: &[bool]) -> Vec<(&'static str, Sel)> {
        let idx: Vec<u32> = (0..selected.len() as u32).filter(|&i| selected[i as usize]).collect();
        let mut v = vec![("mask", Sel::Mask(words_of(selected))), ("idx", Sel::Idx(idx))];
        if selected.iter().all(|&s| s) {
            v.push(("all", Sel::All));
        }
        v
    }

    fn opt(has: bool, bits: &[bool]) -> Option<Vec<u64>> {
        has.then(|| words_of(bits))
    }

    pub fn sizes() -> [usize; 12] {
        [1, 2, 63, 64, 65, 127, 128, 129, 191, 200, 257, 1000]
    }

    const DENSITIES: [u64; 6] = [0, 3, 10, 50, 90, 100];

    // ---- scalar references (Option semantics, row by row; share no code with the kernels) ----

    fn ref_mul(c: &Case) -> Result<Vec<Option<i64>>, ()> {
        let mut out = vec![None; c.n];
        for i in 0..c.n {
            if c.selected[i] && c.va[i] && c.vb[i] {
                out[i] = Some(c.a[i].checked_mul(c.b[i]).ok_or(())?);
            }
        }
        Ok(out)
    }

    fn ref_sub(k: i64, c: &Case) -> Result<Vec<Option<i64>>, ()> {
        let mut out = vec![None; c.n];
        for i in 0..c.n {
            if c.selected[i] && c.va[i] {
                out[i] = Some(k.checked_sub(c.a[i]).ok_or(())?);
            }
        }
        Ok(out)
    }

    fn check_out(want: Result<Vec<Option<i64>>, ()>, got: R<Out>, c: &Case, ctx: &str) {
        match want {
            Err(()) => assert!(got.is_err(), "{ctx}: the reference overflows on a selected valid row, the kernel did not"),
            Ok(want) => {
                let got = got.unwrap_or_else(|e| panic!("{ctx}: kernel failed but the reference did not: {e:?}"));
                for i in 0..c.n {
                    if !c.selected[i] {
                        continue;
                    }
                    let ok = got.valid.as_ref().is_none_or(|v| (v[i / 64] >> (i % 64)) & 1 == 1);
                    assert_eq!(want[i].is_some(), ok, "{ctx} row {i}: validity");
                    if let Some(w) = want[i] {
                        assert_eq!(got.v[i], w, "{ctx} row {i}: value");
                    }
                }
            }
        }
    }

    #[test]
    fn mul_matches_the_reference_for_every_form_density_and_size() {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for round in 0..400 {
            for &n in &sizes() {
                let (np, sp) = (DENSITIES[round % 6], DENSITIES[(round / 6) % 6]);
                let big = [0, 0, 1, 5][round % 4];
                let c = case(&mut rng, n, np, sp, big);
                let (has_a, has_b) = (round % 3 != 0, round % 5 != 0);
                // a column with no NULLs has no validity bitmap and is all valid
                let (mut c, _) = (c, ());
                if !has_a { c.va = vec![true; n]; }
                if !has_b { c.vb = vec![true; n]; }
                let (wa, wb) = (opt(has_a, &c.va), opt(has_b, &c.vb));
                for (name, sel) in forms(&c.selected) {
                    for defer in [false, true] {
                        let got = mul((&c.a, wa.as_deref()), (&c.b, wb.as_deref()), n, &sel, defer);
                        check_out(ref_mul(&c), got, &c, &format!("mul round {round} n {n} form {name} defer {defer}"));
                    }
                }
            }
        }
    }

    #[test]
    fn sub_const_left_matches_the_reference_for_every_form_density_and_size() {
        let mut rng = Rng(0xD1B5_4A32_D192_ED03);
        for round in 0..400 {
            for &n in &sizes() {
                let (np, sp) = (DENSITIES[round % 6], DENSITIES[(round / 6) % 6]);
                let mut c = case(&mut rng, n, np, sp, [0, 0, 1, 5][round % 4]);
                let has_a = round % 3 != 0;
                if !has_a { c.va = vec![true; n]; }
                let wa = opt(has_a, &c.va);
                // i64::MIN as a possible input so that `0 - x` can overflow, in either direction for k = -1
                for i in 0..n { if rng.pct(2) { c.a[i] = i64::MIN; } }
                for k in [100i64, 0, -1] {
                    for (name, sel) in forms(&c.selected) {
                        for defer in [false, true] {
                            let got = sub_const_left(k, (&c.a, wa.as_deref()), n, &sel, defer);
                            check_out(ref_sub(k, &c), got, &c, &format!("sub round {round} n {n} k {k} form {name} defer {defer}"));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn filter_matches_the_reference_and_drops_nulls() {
        let mut rng = Rng(0x2545_F491_4F6C_DD1D);
        for round in 0..300 {
            for &n in &sizes() {
                let c = case(&mut rng, n, DENSITIES[round % 6], DENSITIES[(round / 6) % 6], 0);
                let has = round % 4 != 0;
                let valid: Vec<bool> = if has { c.va.clone() } else { vec![true; n] };
                let w = opt(has, &valid);
                let (lo, hi) = (-300i64, 400i64);
                let want: Vec<u32> = (0..n as u32).filter(|&i| c.selected[i as usize] && valid[i as usize] && c.a[i as usize] >= lo && c.a[i as usize] <= hi).collect();
                // i32 column too
                let a32: Vec<i32> = c.a.iter().map(|&x| (x % 700) as i32).collect();
                let want32: Vec<u32> = (0..n as u32).filter(|&i| c.selected[i as usize] && valid[i as usize] && a32[i as usize] >= -300 && a32[i as usize] <= 400).collect();
                for (name, sel) in forms(&c.selected) {
                    for dense_div in [0usize, 8] {
                        let got = filter_range(&c.a, w.as_deref(), lo, hi, n, &sel, FilterOpts { dense_div });
                        assert_eq!(got.to_vec(n), want, "filter round {round} n {n} form {name} div {dense_div}");
                        if let Sel::Mask(m) = &got {
                            assert_eq!(m.len(), nwords(n));
                            assert_eq!(*m.last().unwrap() & !word_mask(n, m.len() - 1), 0, "tail bits of a result mask are zero");
                            assert!(dense_div > 0 && want.len() * dense_div >= n && want.len() < n, "mask only when dense");
                        }
                        if matches!(got, Sel::All) {
                            assert_eq!(want.len(), n);
                        }
                        let got32 = filter_range(&a32, w.as_deref(), -300i32, 400, n, &sel, FilterOpts { dense_div });
                        assert_eq!(got32.to_vec(n), want32, "filter i32 round {round} n {n} form {name}");
                    }
                }
            }
        }
    }

    #[test]
    fn sum_matches_an_exact_reference_including_extreme_values() {
        let mut rng = Rng(0x1234_5678_9ABC_DEF1);
        for round in 0..300 {
            for &n in &sizes() {
                let mut c = case(&mut rng, n, DENSITIES[round % 6], DENSITIES[(round / 6) % 6], 50);
                if round % 7 == 0 { c.a.iter_mut().for_each(|x| *x = i64::MAX); }
                if round % 11 == 0 { c.a.iter_mut().for_each(|x| *x = i64::MIN); }
                let has = round % 4 != 0;
                let valid: Vec<bool> = if has { c.va.clone() } else { vec![true; n] };
                let w = opt(has, &valid);
                let mut want = (0i128, 0u64);
                for i in 0..n {
                    if c.selected[i] && valid[i] {
                        want.0 += c.a[i] as i128;
                        want.1 += 1;
                    }
                }
                for (name, sel) in forms(&c.selected) {
                    assert_eq!(sum((&c.a, w.as_deref()), n, &sel), want, "sum round {round} n {n} form {name}");
                }
            }
        }
    }

    #[test]
    fn group_sums_match_a_reference_per_group() {
        let mut rng = Rng(0x0F0F_1234_ABCD_9876);
        for round in 0..300 {
            for &n in &sizes() {
                let c = case(&mut rng, n, DENSITIES[round % 6], DENSITIES[(round / 6) % 6], 20);
                let gids: Vec<u16> = (0..n).map(|_| (rng.next() % 5) as u16).collect();
                let c2: Vec<i64> = c.b.clone();
                let (h1, h2, h3) = (round % 3 != 0, round % 4 != 0, round % 5 != 0);
                let (v1, v2, v3) = (opt(h1, &c.va), opt(h2, &c.vb), opt(h3, &c.va));
                let t = |h: bool, v: &[bool]| if h { v.to_vec() } else { vec![true; n] };
                let (t1, t2, t3) = (t(h1, &c.va), t(h2, &c.vb), t(h3, &c.va));
                let mut want_acc = vec![[0i128; 3]; 5];
                let mut want_count = vec![0u64; 5];
                let mut want_nulls = [vec![0u64; 5], vec![0u64; 5], vec![0u64; 5]];
                for i in 0..n {
                    if !c.selected[i] { continue; }
                    let g = gids[i] as usize;
                    want_count[g] += 1;
                    for (k, (vals, ok)) in [(&c.a, &t1), (&c2, &t2), (&c.a, &t3)].into_iter().enumerate() {
                        if ok[i] {
                            want_acc[g][k] += vals[i] as i128;
                        } else {
                            want_nulls[k][g] += 1;
                        }
                    }
                }
                for (name, sel) in forms(&c.selected) {
                    // an index selection reads the group ids of its own rows only: poison the others
                    let gids: Vec<u16> = match &sel {
                        Sel::Idx(_) => (0..n).map(|i| if c.selected[i] { gids[i] } else { 0xFFFF }).collect(),
                        _ => gids.clone(),
                    };
                    for (wrap, subtract) in [(false, false), (false, true), (true, false), (true, true)] {
                        let (mut acc, mut count, mut nulls) = (vec![[0i128; 3]; NG], vec![0u64; NG], [vec![0u64; NG], vec![0u64; NG], vec![0u64; NG]]);
                        let cols = [(&c.a[..], v1.as_deref()), (&c2[..], v2.as_deref()), (&c.a[..], v3.as_deref())];
                        match (wrap, subtract) {
                            (false, false) => group_sums::<3, false, false>(&gids, cols, n, &sel, &mut acc, &mut count, &mut nulls),
                            (false, true) => group_sums::<3, false, true>(&gids, cols, n, &sel, &mut acc, &mut count, &mut nulls),
                            (true, false) => group_sums::<3, true, false>(&gids, cols, n, &sel, &mut acc, &mut count, &mut nulls),
                            (true, true) => group_sums::<3, true, true>(&gids, cols, n, &sel, &mut acc, &mut count, &mut nulls),
                        }
                        acc.truncate(5);
                        count.truncate(5);
                        nulls.iter_mut().for_each(|x| x.truncate(5));
                        assert_eq!((acc, count, nulls), (want_acc.clone(), want_count.clone(), want_nulls.clone()), "group_sums round {round} n {n} form {name} wrap {wrap} subtract {subtract}");
                    }
                }
            }
        }
    }

    fn mul2(a: WCol<i64>, b: WCol<i64>, n: usize, sel: &Sel) -> R<Out> {
        let (r0, r1) = (mul(a, b, n, sel, false), mul(a, b, n, sel, true));
        assert_eq!(r0.is_ok(), r1.is_ok(), "deferred and immediate overflow checks disagree");
        r0
    }

    fn sub2(c: i64, a: WCol<i64>, n: usize, sel: &Sel) -> R<Out> {
        let (r0, r1) = (sub_const_left(c, a, n, sel, false), sub_const_left(c, a, n, sel, true));
        assert_eq!(r0.is_ok(), r1.is_ok(), "deferred and immediate overflow checks disagree");
        r0
    }

    // ---- the named cases of the rules ----

    #[test]
    fn an_overflow_in_an_unselected_or_null_slot_is_not_an_error_in_any_word_or_form() {
        for n in [1usize, 63, 64, 65, 130, 200] {
            for pos in [0, n / 2, n - 1, 63.min(n - 1), 64.min(n - 1)] {
                let mut a = vec![1i64; n];
                a[pos] = i64::MAX;
                let b = vec![2i64; n];
                // unselected
                let mut selected = vec![true; n];
                selected[pos] = false;
                for (name, sel) in forms(&selected) {
                    assert!(mul2((&a, None), (&b, None), n, &sel).is_ok(), "n {n} pos {pos} unselected, form {name}");
                                    }
                // NULL
                let mut valid = vec![true; n];
                valid[pos] = false;
                let w = words_of(&valid);
                for (name, sel) in forms(&vec![true; n]) {
                    assert!(mul2((&a, Some(&w)), (&b, None), n, &sel).is_ok(), "n {n} pos {pos} NULL in a, form {name}");
                    assert!(mul2((&b, None), (&a, Some(&w)), n, &sel).is_ok(), "n {n} pos {pos} NULL in b, form {name}");
                    // selected and valid: an error
                    assert!(mul2((&a, None), (&b, None), n, &sel).is_err(), "n {n} pos {pos} selected+valid, form {name}");
                }
            }
        }
    }

    #[test]
    fn subtraction_overflow_in_null_unselected_and_selected_rows() {
        for n in [1usize, 64, 65, 200] {
            for pos in [0, n - 1, 64.min(n - 1)] {
                let mut a = vec![5i64; n];
                a[pos] = i64::MIN; // 0 - MIN overflows
                let mut valid = vec![true; n];
                valid[pos] = false;
                let w = words_of(&valid);
                let mut selected = vec![true; n];
                selected[pos] = false;
                for (name, sel) in forms(&vec![true; n]) {
                    assert!(sub2(0, (&a, Some(&w)), n, &sel).is_ok(), "NULL slot, n {n} pos {pos} form {name}");
                    assert!(sub2(0, (&a, None), n, &sel).is_err(), "valid selected, n {n} pos {pos} form {name}");
                }
                for (name, sel) in forms(&selected) {
                    assert!(sub2(0, (&a, None), n, &sel).is_ok(), "unselected, n {n} pos {pos} form {name}");
                }
            }
        }
    }

    #[test]
    fn a_guard_filter_protects_the_later_kernel() {
        let big = [i64::MAX, 4, i64::MAX];
        let guard = [0i64, 1, 0];
        let sel = filter_range(&guard, None, 1, 1, 3, &Sel::All, FilterOpts { dense_div: 8 });
        assert_eq!(sel.to_vec(3), vec![1]);
        for sel in [sel.clone(), Sel::Idx(vec![1]), Sel::Mask(vec![0b010])] {
            let out = mul2((&big, None), (&guard, None), 3, &sel).expect("guarded rows are skipped");
            assert_eq!(out.v[1], 4);
        }
    }

    #[test]
    fn the_last_partial_word_is_handled() {
        // 70 rows: word 1 holds rows 64..70; the tail bits of every mask must not select or validate phantom rows.
        let n = 70;
        let a: Vec<i64> = (0..n as i64).collect();
        let valid: Vec<bool> = (0..n).map(|i| i != 69).collect();
        let w = words_of(&valid);
        assert_eq!(sum((&a, Some(&w)), n, &Sel::All), ((0..69).sum::<i64>() as i128, 69));
        assert_eq!(sum((&a, None), n, &Sel::All), ((0..70).sum::<i64>() as i128, 70));
        let s = filter_range(&a, None, 60, 1000, n, &Sel::All, FilterOpts { dense_div: 1 });
        assert_eq!(s.len(n), 10);
        let mut acc = vec![[0i128; 1]; NG];
        let (mut count, mut nulls) = (vec![0u64; NG], [vec![0u64; NG]]);
        group_sums::<1, true, true>(&vec![0u16; n], [(&a[..], Some(&w[..]))], n, &Sel::All, &mut acc, &mut count, &mut nulls);
        assert_eq!((acc[0][0], count[0], nulls[0][0]), ((0..69).sum::<i64>() as i128, 70, 1));
    }

    #[test]
    fn everything_null_or_nothing_selected() {
        let n = 130;
        let a = vec![i64::MAX; n];
        let none = words_of(&vec![false; n]);
        assert!(mul2((&a, Some(&none)), (&a, None), n, &Sel::All).is_ok());
        assert_eq!(sum((&a, Some(&none)), n, &Sel::All), (0, 0));
        assert_eq!(sum((&a, None), n, &Sel::Mask(none.clone())), (0, 0));
        assert!(mul2((&a, None), (&a, None), n, &Sel::Idx(vec![])).is_ok());
        assert_eq!(filter_range(&a, Some(&none), i64::MIN, i64::MAX, n, &Sel::All, FilterOpts { dense_div: 8 }).len(n), 0);
    }
}
