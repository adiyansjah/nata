//! The kernels, the scalar reference, and the checksum.
//!
//! Two kernel kinds, each one *stage* of a chain (stage `i` adds `stage_const(i)`):
//!
//! * **Cheap**: `v + k` over a morsel. One column in, one column out, no selection.
//! * **Medium**: add, compare, filter with a selection vector. `cols[0]` is the values (dense: the add is computed for
//!   every row, as an Arrow kernel would), `cols[1]` is the selection vector (indices of the rows still alive; absent
//!   means all rows). A row survives a stage if `(v + k) & 7 != 0`. With the constants of `stage_const` the survivors are
//!   7/8, 5/8 and 3/8 of the rows after 1, 3 and 6 stages (the dead residues are fixed by the cumulative sums).
//!
//! All unfused arms call the SAME kernel functions (`#[inline(never)]`, so they are the same machine code in every arm
//! and a difference between arms is dispatch and plumbing, not loop code generation). The hand-fused control has its own
//! loops (`fused_cheap`, `fused_medium`), which is the point of a control.
//!
//! The checksum of a run is `Acc { sum, count }`: the wrapping sum and the number of the rows that reached the end of the
//! chain. It depends on every morsel (the source adds the morsel number to a fixed random template), so a lost,
//! duplicated or mis-computed morsel changes it.

use crate::api::{OpResult, Operator, Result};
use crate::batch::Batch;

/// A row survives a medium stage if `(v & KEEP_MASK) != 0`.
pub const KEEP_MASK: i64 = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Cheap,
    Medium,
}

impl Kind {
    pub const ALL: [Kind; 2] = [Kind::Cheap, Kind::Medium];

    pub fn name(self) -> &'static str {
        match self {
            Kind::Cheap => "cheap",
            Kind::Medium => "medium",
        }
    }
}

/// The constant added by stage `i` of a chain.
pub fn stage_const(i: usize) -> i64 {
    3 * i as i64 + 1
}

#[inline(always)]
fn keep(v: i64) -> bool {
    v & KEEP_MASK != 0
}

/// `rows` pseudo-random `i64` (splitmix64, fixed seed).
pub fn template(rows: usize) -> Vec<i64> {
    let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..rows)
        .map(|_| {
            s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = s;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            (z ^ (z >> 31)) as i64
        })
        .collect()
}

/// Morsel `m`: the template plus `m`. One allocation and one cheap pass; identical in every arm.
#[inline(never)]
pub fn gen_morsel(t: &[i64], m: usize) -> Vec<i64> {
    let a = m as i64;
    t.iter().map(|&x| x.wrapping_add(a)).collect()
}

/// The result of a run, compared across arms before any time is used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Acc {
    pub sum: u64,
    pub count: u64,
}

impl Acc {
    pub fn merge(self, o: Acc) -> Acc {
        Acc { sum: self.sum.wrapping_add(o.sum), count: self.count + o.count }
    }
}

// ---------------------------------------------------------------- the unfused stage kernels

#[inline(never)]
fn cheap_stage(k: i64, input: &Batch, out: &mut Batch) {
    if out.cols.len() != 1 {
        out.cols.clear();
        out.cols.push(Vec::new());
    }
    let o = &mut out.cols[0];
    o.clear();
    o.extend(input.cols[0].iter().map(|&v| v.wrapping_add(k)));
}

#[inline(never)]
fn medium_stage(k: i64, input: &Batch, out: &mut Batch) {
    let vals = &input.cols[0];
    let n = vals.len();
    out.cols.resize_with(2, Vec::new);
    let (a, b) = out.cols.split_at_mut(1);
    let o = &mut a[0];
    let sel = &mut b[0];
    o.clear();
    o.extend(vals.iter().map(|&v| v.wrapping_add(k)));
    sel.clear();
    sel.resize(n, 0);
    let mut w = 0usize;
    match input.cols.get(1) {
        None => {
            for (i, &v) in o.iter().enumerate() {
                sel[w] = i as i64;
                w += keep(v) as usize;
            }
        }
        Some(s) => {
            for &i in s {
                sel[w] = i;
                w += keep(o[i as usize]) as usize;
            }
        }
    }
    sel.truncate(w);
}

/// Dense compaction: apply the selection vector, leave one column. Used before an exchange (ADR 0001: batches are
/// compacted before they enter an exchange).
#[inline(never)]
fn compact(input: &Batch, out: &mut Batch) {
    if out.cols.len() != 1 {
        out.cols.clear();
        out.cols.push(Vec::new());
    }
    let o = &mut out.cols[0];
    o.clear();
    let vals = &input.cols[0];
    match input.cols.get(1) {
        Some(sel) => o.extend(sel.iter().map(|&i| vals[i as usize])),
        None => o.extend_from_slice(vals),
    }
}

/// One stage of a chain, as a protocol operator. The same type is used (a) behind `Box<dyn Operator>` in the scheduler,
/// (b) as a concrete type in the monomorphised driver.
#[derive(Clone, Copy, Debug)]
pub struct StageOp {
    pub kind: Kind,
    pub k: i64,
}

impl Operator for StageOp {
    #[inline]
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        match self.kind {
            Kind::Cheap => cheap_stage(self.k, input, out),
            Kind::Medium => medium_stage(self.k, input, out),
        }
        Ok(OpResult::NeedMoreInput)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CompactOp;

impl Operator for CompactOp {
    #[inline]
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        compact(input, out);
        Ok(OpResult::NeedMoreInput)
    }
}

/// The sink: add the (selected) values of a batch to the checksum.
#[inline(never)]
pub fn sum_batch(b: &Batch, acc: &mut Acc) {
    let vals = &b.cols[0];
    match b.cols.get(1) {
        None => {
            acc.sum = vals.iter().fold(acc.sum, |a, &v| a.wrapping_add(v as u64));
            acc.count += vals.len() as u64;
        }
        Some(sel) => {
            let mut s = acc.sum;
            for &i in sel {
                s = s.wrapping_add(vals[i as usize] as u64);
            }
            acc.sum = s;
            acc.count += sel.len() as u64;
        }
    }
}

// ---------------------------------------------------------------- the hand-fused control

/// The whole cheap chain in one pass over the morsel.
#[inline(never)]
pub fn fused_cheap<const L: usize>(ks: &[i64; L], vals: &[i64], acc: &mut Acc) {
    let mut s = acc.sum;
    for &v in vals {
        let mut x = v;
        for &k in ks {
            x = x.wrapping_add(k);
        }
        s = s.wrapping_add(x as u64);
    }
    acc.sum = s;
    acc.count += vals.len() as u64;
}

/// The whole medium chain in one pass over the morsel; no selection vector, no intermediate column.
#[inline(never)]
pub fn fused_medium<const L: usize>(ks: &[i64; L], vals: &[i64], acc: &mut Acc) {
    let (mut s, mut c) = (acc.sum, 0u64);
    for &v in vals {
        let mut x = v;
        let mut alive = true;
        for &k in ks {
            x = x.wrapping_add(k);
            alive &= keep(x);
        }
        s = s.wrapping_add(if alive { x as u64 } else { 0 });
        c += alive as u64;
    }
    acc.sum = s;
    acc.count += c;
}

// ---------------------------------------------------------------- the scalar reference

/// The checksum computed row by row with no shared code (apart from the template): the value every arm must reach.
/// The "keep" test is written out here (`& 7`), so changing `KEEP_MASK` without changing this makes every test fail.
pub fn reference(kind: Kind, len: usize, batch: usize, morsels: usize, tmpl: &[i64]) -> Acc {
    assert_eq!(tmpl.len(), batch, "the template must have one morsel's rows");
    let mut acc = Acc::default();
    for m in 0..morsels {
        for &t in tmpl {
            let mut v = t.wrapping_add(m as i64);
            let mut ok = true;
            for s in 0..len {
                v = v.wrapping_add(stage_const(s));
                if kind == Kind::Medium && (v & 7) == 0 {
                    ok = false;
                    break;
                }
            }
            if ok {
                acc.sum = acc.sum.wrapping_add(v as u64);
                acc.count += 1;
            }
        }
    }
    acc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_chain_equals_reference_on_one_morsel() {
        for kind in Kind::ALL {
            for len in [1usize, 2, 3, 6] {
                let t = template(300);
                let mut ops: Vec<StageOp> = (0..len).map(|i| StageOp { kind, k: stage_const(i) }).collect();
                let mut cur = Batch { cols: vec![gen_morsel(&t, 5)] };
                for op in ops.iter_mut() {
                    let mut out = Batch::default();
                    op.execute(&cur, &mut out).unwrap();
                    cur = out;
                }
                let mut acc = Acc::default();
                sum_batch(&cur, &mut acc);
                assert_eq!(acc, reference_one(kind, len, &t, 5), "{kind:?} len {len}");
            }
        }
    }

    fn reference_one(kind: Kind, len: usize, t: &[i64], m: usize) -> Acc {
        let mut acc = Acc::default();
        for &x in t {
            let mut v = x.wrapping_add(m as i64);
            let mut ok = true;
            for s in 0..len {
                v = v.wrapping_add(stage_const(s));
                if kind == Kind::Medium && v & 7 == 0 {
                    ok = false;
                    break;
                }
            }
            if ok {
                acc.sum = acc.sum.wrapping_add(v as u64);
                acc.count += 1;
            }
        }
        acc
    }

    #[test]
    fn medium_kernel_really_filters_and_cheap_does_not() {
        let t = template(8192);
        // Stage s kills the rows whose value is congruent to -(k_0 + .. + k_s) mod 8. With the constants 1, 4, 7, 10, 13, 16
        // the cumulative sums are 1, 5, 12, 22, 35, 51: the residues 1, 5, 4, 6, 3, 3, so 1, 3 and 5 distinct residues are
        // killed after 1, 3 and 6 stages and 7/8, 5/8 and 3/8 of the rows survive.
        for (len, survive) in [(1usize, 7.0 / 8.0), (3, 5.0 / 8.0), (6, 3.0 / 8.0)] {
            let cheap = reference(Kind::Cheap, len, 8192, 3, &t);
            let medium = reference(Kind::Medium, len, 8192, 3, &t);
            assert_eq!(cheap.count, 3 * 8192);
            let frac = medium.count as f64 / cheap.count as f64;
            assert!((frac - survive).abs() < 0.03, "len {len}: survival {frac}, expected about {survive}");
        }
    }

    #[test]
    fn fused_loops_equal_reference() {
        let t = template(1000);
        let ks: [i64; 3] = std::array::from_fn(stage_const);
        for kind in Kind::ALL {
            let mut acc = Acc::default();
            for m in 0..4 {
                let v = gen_morsel(&t, m);
                match kind {
                    Kind::Cheap => fused_cheap(&ks, &v, &mut acc),
                    Kind::Medium => fused_medium(&ks, &v, &mut acc),
                }
            }
            assert_eq!(acc, reference(kind, 3, 1000, 4, &t), "{kind:?}");
        }
    }

    #[test]
    fn the_checksum_notices_a_lost_morsel_a_changed_constant_and_a_swapped_kind() {
        let t = template(512);
        let full = reference(Kind::Medium, 3, 512, 8, &t);
        assert_ne!(full, reference(Kind::Medium, 3, 512, 7, &t), "a lost morsel");
        assert_ne!(full, reference(Kind::Medium, 2, 512, 8, &t), "a stage fewer");
        assert_ne!(full, reference(Kind::Cheap, 3, 512, 8, &t), "the wrong kind");
        let mut ks: [i64; 3] = std::array::from_fn(stage_const);
        ks[1] += 1;
        let mut acc = Acc::default();
        for m in 0..8 {
            fused_medium(&ks, &gen_morsel(&t, m), &mut acc);
        }
        assert_ne!(full, acc, "a changed constant");
    }

    #[test]
    fn compact_applies_the_selection() {
        let input = Batch { cols: vec![vec![10, 20, 30, 40], vec![1, 3]] };
        let mut out = Batch::default();
        CompactOp.execute(&input, &mut out).unwrap();
        assert_eq!(out.cols, vec![vec![20, 40]]);
        let dense = Batch { cols: vec![vec![1, 2, 3]] };
        let mut out = Batch::default();
        CompactOp.execute(&dense, &mut out).unwrap();
        assert_eq!(out.cols, vec![vec![1, 2, 3]]);
    }
}
