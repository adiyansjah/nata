//! The benchmark arms. One `Fixture` = one column + one selection; every arm is a function of the fixture only.
//!
//! | id | what | selection form it needs |
//! |---|---|---|
//! | A  | engine style: ONE function over the selected rows only, no copy | native (no conversion) |
//! | A3 | engine style: THREE functions, three passes, each over the selected rows only | native |
//! | Bt | compact with `take`, then ONE function densely | indices (a bitmap is converted: cost counted) |
//! | Bf | compact with `filter`, then ONE function densely | boolean mask (an index vector is converted: cost counted) |
//! | Ct | compact once with `take`, then THREE functions densely | as Bt |
//! | Cf | compact once with `filter`, then THREE functions densely | as Bf |
//! | D  | ONE function densely over ALL rows, selection ignored (the control for selectivity: what not selecting costs) | none |
//! | Et | `take` only, no function afterwards (the copy cost alone) | as Bt |
//! | Ef | `filter` only, no function afterwards | as Bf |
//!
//! Compaction allocates its output every call and frees it at the end of the call (no buffer pooling); it copies ONE column
//! (the kernel's input), not the whole batch.

use arrow::array::{Array, ArrayRef};
use arrow::compute::{filter, take};

use crate::data::Sel;
use crate::kernels::{SelRef, combine, pass, pass_mutant, view};

pub struct Fixture {
    pub col: ArrayRef,
    pub sel: Sel,
    pub n: usize,
    pub k: usize,
}

pub const IDS: [&str; 9] = ["A", "A3", "Bt", "Bf", "Ct", "Cf", "D", "Et", "Ef"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    Take,
    Filter,
}

pub fn compact(fx: &Fixture, how: How) -> ArrayRef {
    match how {
        How::Take => take(fx.col.as_ref(), &fx.sel.to_indices(), None).expect("take"),
        How::Filter => filter(fx.col.as_ref(), &fx.sel.to_bool(fx.n)).expect("filter"),
    }
}

fn sel_ref(sel: &Sel) -> SelRef<'_> {
    match sel {
        Sel::Idx(a) => SelRef::Idx(a.values()),
        Sel::Mask(b) => SelRef::Mask(b.typed_data::<u64>()),
    }
}

/// A / A3 (with an optional deliberate kernel breakage 1..=5 for the mutation tests; 0 = the real thing).
pub fn arm_select(fx: &Fixture, nf: usize, bug: u8) -> u64 {
    let v = view(fx.col.as_ref());
    combine((0..nf).map(|f| pass_mutant(bug, &v, f, sel_ref(&fx.sel))))
}

/// B / C: compact, then `nf` dense passes over the compacted array.
pub fn arm_compact(fx: &Fixture, how: How, nf: usize) -> u64 {
    let c = compact(fx, how);
    let v = view(c.as_ref());
    combine((0..nf).map(|f| pass(&v, f, SelRef::All)))
}

/// D: one dense pass over all `n` rows of the original column.
pub fn arm_dense_all(fx: &Fixture) -> u64 {
    pass(&view(fx.col.as_ref()), 0, SelRef::All)
}

/// E: the compaction alone; returns the number of rows produced.
pub fn arm_compact_only(fx: &Fixture, how: How) -> u64 {
    compact(fx, how).len() as u64
}

/// Run arm `id` once.
pub fn run_arm(fx: &Fixture, id: &str) -> u64 {
    match id {
        "A" => arm_select(fx, 1, 0),
        "A3" => arm_select(fx, 3, 0),
        "Bt" => arm_compact(fx, How::Take, 1),
        "Bf" => arm_compact(fx, How::Filter, 1),
        "Ct" => arm_compact(fx, How::Take, 3),
        "Cf" => arm_compact(fx, How::Filter, 3),
        "D" => arm_dense_all(fx),
        "Et" => arm_compact_only(fx, How::Take),
        "Ef" => arm_compact_only(fx, How::Filter),
        _ => panic!("unknown arm {id}"),
    }
}
