//! The per-row functions and the selection-aware / dense passes over a column ("our own kernels", engine style).
//!
//! A pass computes `sum over selected AND valid rows of g_F(row)` (wrapping `u64`). Three functions per type, from cheap
//! to content-touching, so the amortisation arm (one compaction, three passes) has something to amortise over:
//!
//! | F | Int64 / Decimal64 | Decimal128 | Utf8 / Dictionary strings |
//! |---|---|---|---|
//! | 0 | the value | low + high word | byte length |
//! | 1 | multiply-shift hash | multiply-shift hash | first byte |
//! | 2 | xor-shift-multiply-rotate mix | same on low ^ high | FNV-1a over all bytes (about 1 ns per byte) |
//!
//! Numeric rows are masked branch-free (`row & -valid`), as ADR 0003 asks of own kernels; string rows are heavy, so a NULL
//! row is skipped with a branch (computing a hash for a NULL slot and throwing it away would be a strawman).
//! All NULL handling is on the validity bytes of the array, NULL slots hold non-zero garbage (see `data.rs`).

use arrow::array::{Array, AsArray, StringArray};
use arrow::datatypes::{DataType, Decimal64Type, Decimal128Type, Int32Type, Int64Type};

// ------------------------------------------------------------------------------------------------ pure per-value functions
// Shared with the scalar reference (`reference.rs`): they define WHAT is computed; the passes define HOW rows are visited.

#[inline(always)]
pub fn g_i64<const F: usize>(x: i64) -> u64 {
    let u = x as u64;
    match F {
        0 => u,
        1 => u.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 29,
        _ => (u ^ (u >> 17)).wrapping_mul(0xBF58_476D_1CE4_E5B9).rotate_left(11) ^ u,
    }
}

#[inline(always)]
pub fn g_i128<const F: usize>(x: i128) -> u64 {
    let lo = x as u64;
    let hi = (x >> 64) as u64;
    match F {
        0 => lo.wrapping_add(hi),
        1 => (lo.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ hi.rotate_left(7)) >> 29,
        _ => g_i64::<2>((lo ^ hi.rotate_left(32)) as i64),
    }
}

#[inline(always)]
pub fn g_bytes<const F: usize>(b: &[u8]) -> u64 {
    match F {
        0 => b.len() as u64,
        1 => u64::from(b.first().copied().unwrap_or(0)),
        _ => {
            let mut h = 0xCBF2_9CE4_8422_2325u64;
            for &c in b {
                h = (h ^ u64::from(c)).wrapping_mul(0x0000_0100_0000_01B3);
            }
            h
        }
    }
}

/// Run-time dispatch of a per-value function, for the reference.
pub fn g_i64_dyn(f: usize, x: i64) -> u64 {
    match f {
        0 => g_i64::<0>(x),
        1 => g_i64::<1>(x),
        _ => g_i64::<2>(x),
    }
}
pub fn g_i128_dyn(f: usize, x: i128) -> u64 {
    match f {
        0 => g_i128::<0>(x),
        1 => g_i128::<1>(x),
        _ => g_i128::<2>(x),
    }
}
pub fn g_bytes_dyn(f: usize, b: &[u8]) -> u64 {
    match f {
        0 => g_bytes::<0>(b),
        1 => g_bytes::<1>(b),
        _ => g_bytes::<2>(b),
    }
}

// ------------------------------------------------------------------------------------------------ views

/// Borrowed raw buffers of an array (offset 0, as built by `data.rs` and returned by take/filter).
pub enum View<'a> {
    I64 { vals: &'a [i64], valid: Option<&'a [u8]> },
    I128 { vals: &'a [i128], valid: Option<&'a [u8]> },
    Str { offsets: &'a [i32], bytes: &'a [u8], valid: Option<&'a [u8]> },
    Dict { keys: &'a [i32], offsets: &'a [i32], bytes: &'a [u8], valid: Option<&'a [u8]> },
}

pub fn view(a: &dyn Array) -> View<'_> {
    assert_eq!(a.offset(), 0, "views are for unsliced arrays");
    let valid = a.nulls().map(|n| {
        assert_eq!(n.offset(), 0);
        n.validity()
    });
    match a.data_type() {
        DataType::Int64 => View::I64 { vals: a.as_primitive::<Int64Type>().values(), valid },
        DataType::Decimal64(..) => View::I64 { vals: a.as_primitive::<Decimal64Type>().values(), valid },
        DataType::Decimal128(..) => View::I128 { vals: a.as_primitive::<Decimal128Type>().values(), valid },
        DataType::Utf8 => {
            let s: &StringArray = a.as_string::<i32>();
            View::Str { offsets: s.value_offsets(), bytes: s.value_data(), valid }
        }
        DataType::Dictionary(..) => {
            let d = a.as_dictionary::<Int32Type>();
            let s: &StringArray = d.values().as_string::<i32>();
            View::Dict { keys: d.keys().values(), offsets: s.value_offsets(), bytes: s.value_data(), valid }
        }
        t => panic!("unsupported type {t:?}"),
    }
}

/// The rows a pass visits.
#[derive(Clone, Copy)]
pub enum SelRef<'a> {
    /// Every row of the array (a dense pass).
    All,
    Idx(&'a [u32]),
    Mask(&'a [u64]),
}

// ------------------------------------------------------------------------------------------------ passes

trait RowFn {
    /// Skipping NULL rows by a branch (heavy rows) or masking the result (cheap rows).
    const BRANCHLESS: bool;
    fn len(&self) -> usize;
    /// The value of row `i`, ignoring validity.
    fn row(&self, i: usize) -> u64;
}

struct I64R<'a, const F: usize>(&'a [i64]);
impl<const F: usize> RowFn for I64R<'_, F> {
    const BRANCHLESS: bool = true;
    #[inline(always)]
    fn len(&self) -> usize {
        self.0.len()
    }
    #[inline(always)]
    fn row(&self, i: usize) -> u64 {
        g_i64::<F>(self.0[i])
    }
}

struct I128R<'a, const F: usize>(&'a [i128]);
impl<const F: usize> RowFn for I128R<'_, F> {
    const BRANCHLESS: bool = true;
    #[inline(always)]
    fn len(&self) -> usize {
        self.0.len()
    }
    #[inline(always)]
    fn row(&self, i: usize) -> u64 {
        g_i128::<F>(self.0[i])
    }
}

struct StrR<'a, const F: usize> {
    offsets: &'a [i32],
    bytes: &'a [u8],
}
impl<const F: usize> RowFn for StrR<'_, F> {
    const BRANCHLESS: bool = false;
    #[inline(always)]
    fn len(&self) -> usize {
        self.offsets.len() - 1
    }
    #[inline(always)]
    fn row(&self, i: usize) -> u64 {
        // Offsets of arrays we built or that arrow produced are non-negative and ordered.
        let (a, b) = (self.offsets[i] as usize, self.offsets[i + 1] as usize);
        g_bytes::<F>(&self.bytes[a..b])
    }
}

struct DictR<'a, const F: usize> {
    keys: &'a [i32],
    offsets: &'a [i32],
    bytes: &'a [u8],
}
impl<const F: usize> RowFn for DictR<'_, F> {
    const BRANCHLESS: bool = false;
    #[inline(always)]
    fn len(&self) -> usize {
        self.keys.len()
    }
    #[inline(always)]
    fn row(&self, i: usize) -> u64 {
        let key = self.keys[i] as usize;
        let (a, b) = (self.offsets[key] as usize, self.offsets[key + 1] as usize);
        g_bytes::<F>(&self.bytes[a..b])
    }
}

#[inline(always)]
fn vbit(v: &[u8], i: usize) -> bool {
    (v[i >> 3] >> (i & 7)) & 1 == 1
}

/// 64 validity bits starting at row `64 * wi` (bits past the end of the buffer read as 0).
#[inline(always)]
fn vword(v: &[u8], wi: usize) -> u64 {
    let s = wi * 8;
    if s + 8 <= v.len() {
        u64::from_le_bytes(v[s..s + 8].try_into().expect("8 bytes"))
    } else if s < v.len() {
        let mut b = [0u8; 8];
        b[..v.len() - s].copy_from_slice(&v[s..]);
        u64::from_le_bytes(b)
    } else {
        0
    }
}

/// `BUG` is for the mutation tests only (0 = the real kernel); each non-zero value is a deliberate breakage that the
/// agreement tests must catch. It is a const generic so the production instances contain none of this.
///   1: validity ignored everywhere        2: bitmap path does not AND the validity word
///   3: index path drops the last index    4: bitmap path drops the last word
///   5: dense path with validity includes the rows past the last full 64 without validity
#[inline(always)]
fn run_pass<R: RowFn, const BUG: u8>(r: &R, sel: SelRef<'_>, valid: Option<&[u8]>) -> u64 {
    let valid = if BUG == 1 { None } else { valid };
    let mut acc = 0u64;
    match sel {
        SelRef::All => {
            let n = r.len();
            match valid {
                None => {
                    for i in 0..n {
                        acc = acc.wrapping_add(r.row(i));
                    }
                }
                Some(v) => {
                    // Word-wise: 64 validity bits per step (the null-path result).
                    for wi in 0..words(n) {
                        let base = wi * 64;
                        let end = (base + 64).min(n);
                        let w = vword(v, wi);
                        if BUG == 5 && end - base < 64 {
                            for i in base..end {
                                acc = acc.wrapping_add(r.row(i));
                            }
                        } else if R::BRANCHLESS {
                            for i in base..end {
                                let m = ((w >> (i - base)) & 1).wrapping_neg();
                                acc = acc.wrapping_add(r.row(i) & m);
                            }
                        } else {
                            for i in base..end {
                                if (w >> (i - base)) & 1 == 1 {
                                    acc = acc.wrapping_add(r.row(i));
                                }
                            }
                        }
                    }
                }
            }
        }
        SelRef::Idx(ix) => {
            let ix = if BUG == 3 && !ix.is_empty() { &ix[..ix.len() - 1] } else { ix };
            match valid {
                None => {
                    for &i in ix {
                        acc = acc.wrapping_add(r.row(i as usize));
                    }
                }
                Some(v) if R::BRANCHLESS => {
                    for &i in ix {
                        let i = i as usize;
                        let m = u64::from(vbit(v, i)).wrapping_neg();
                        acc = acc.wrapping_add(r.row(i) & m);
                    }
                }
                Some(v) => {
                    for &i in ix {
                        let i = i as usize;
                        if vbit(v, i) {
                            acc = acc.wrapping_add(r.row(i));
                        }
                    }
                }
            }
        }
        SelRef::Mask(ws) => {
            let ws = if BUG == 4 && !ws.is_empty() { &ws[..ws.len() - 1] } else { ws };
            for (wi, &w) in ws.iter().enumerate() {
                let mut w = w;
                if let Some(v) = valid
                    && BUG != 2
                {
                    w &= vword(v, wi);
                }
                if w == !0u64 {
                    // A full word of selected rows: a contiguous run of 64.
                    for j in 0..64 {
                        acc = acc.wrapping_add(r.row(wi * 64 + j));
                    }
                } else {
                    while w != 0 {
                        let j = w.trailing_zeros() as usize;
                        acc = acc.wrapping_add(r.row(wi * 64 + j));
                        w &= w - 1;
                    }
                }
            }
        }
    }
    acc
}

fn words(n: usize) -> usize {
    n.div_ceil(64)
}

fn pass_f<R: RowFn, const BUG: u8>(r: &R, sel: SelRef<'_>, valid: Option<&[u8]>) -> u64 {
    run_pass::<R, BUG>(r, sel, valid)
}

fn pass_bug<const BUG: u8>(v: &View<'_>, f: usize, sel: SelRef<'_>) -> u64 {
    macro_rules! by_f {
        ($make:ident, $valid:expr, $($arg:tt)*) => {
            match f {
                0 => pass_f::<_, BUG>(&$make::<0> { $($arg)* }, sel, $valid),
                1 => pass_f::<_, BUG>(&$make::<1> { $($arg)* }, sel, $valid),
                _ => pass_f::<_, BUG>(&$make::<2> { $($arg)* }, sel, $valid),
            }
        };
    }
    match *v {
        View::I64 { vals, valid } => match f {
            0 => pass_f::<_, BUG>(&I64R::<0>(vals), sel, valid),
            1 => pass_f::<_, BUG>(&I64R::<1>(vals), sel, valid),
            _ => pass_f::<_, BUG>(&I64R::<2>(vals), sel, valid),
        },
        View::I128 { vals, valid } => match f {
            0 => pass_f::<_, BUG>(&I128R::<0>(vals), sel, valid),
            1 => pass_f::<_, BUG>(&I128R::<1>(vals), sel, valid),
            _ => pass_f::<_, BUG>(&I128R::<2>(vals), sel, valid),
        },
        View::Str { offsets, bytes, valid } => by_f!(StrR, valid, offsets, bytes),
        View::Dict { keys, offsets, bytes, valid } => by_f!(DictR, valid, keys, offsets, bytes),
    }
}

/// Function `f` (0..3) over the rows `sel` of the array behind `v`, NULL rows skipped.
pub fn pass(v: &View<'_>, f: usize, sel: SelRef<'_>) -> u64 {
    pass_bug::<0>(v, f, sel)
}

/// Same with a deliberate breakage (1..=5), for the mutation tests.
#[doc(hidden)]
pub fn pass_mutant(bug: u8, v: &View<'_>, f: usize, sel: SelRef<'_>) -> u64 {
    match bug {
        0 => pass_bug::<0>(v, f, sel),
        1 => pass_bug::<1>(v, f, sel),
        2 => pass_bug::<2>(v, f, sel),
        3 => pass_bug::<3>(v, f, sel),
        4 => pass_bug::<4>(v, f, sel),
        5 => pass_bug::<5>(v, f, sel),
        _ => panic!("no mutant {bug}"),
    }
}

/// Combine the results of `nf` functions into one checksum (order-sensitive, so a swapped function is noticed).
pub fn combine(results: impl IntoIterator<Item = u64>) -> u64 {
    results.into_iter().enumerate().fold(0u64, |acc, (f, r)| acc.wrapping_add(r.rotate_left(21 * f as u32)))
}
