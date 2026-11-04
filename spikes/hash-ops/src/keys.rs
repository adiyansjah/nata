//! Typed key columns: hashing, equality, and the group-key store of the hash aggregate.
//!
//! Key types: Int64 / Decimal64 (as i64), Int32 / Date32 (as i32), dictionary strings (Int32 keys, Utf8
//! values) and plain Utf8. A dictionary string and a plain string with the same bytes hash and compare
//! equal, so the two sides of a join (or two batches of one scan, whose dictionaries differ) may use
//! different representations.
use std::hash::BuildHasher;

use arrow::array::{Array, AsArray};
use arrow::buffer::NullBuffer;
use arrow::datatypes::{DataType, Date32Type, Decimal64Type, Int32Type, Int64Type};

use crate::kernels::{EngineError, R, each};

/// Number of hash partitions used by the join build and the aggregate merge (bits 32..38 of the hash).
pub const PARTS: usize = 64;

#[inline(always)]
pub fn part_of(h: u64) -> usize {
    ((h >> 32) as usize) & (PARTS - 1)
}

const MUL: u64 = 0x2d35_8dcc_aa6c_78a5;
const SEED: u64 = 0x9e37_79b9_7f4a_7c15;
const NULL_H: u64 = 0x517c_c1b7_2722_0a95;

/// Folded multiply: the same primitive as foldhash's fast hasher for a `u64`.
#[inline(always)]
pub fn mix(x: u64) -> u64 {
    let m = ((x ^ SEED) as u128).wrapping_mul(MUL as u128);
    (m as u64) ^ ((m >> 64) as u64)
}

#[inline(always)]
pub fn hash_bytes(b: &[u8]) -> u64 {
    foldhash::fast::FixedState::with_seed(0x5eed).hash_one(b)
}

#[inline(always)]
fn combine(first: bool, h: &mut u64, hc: u64) {
    *h = if first { hc } else { mix(h.rotate_left(23) ^ hc) };
}

#[derive(Clone, Copy)]
pub enum Nul<'a> {
    None,
    Bits(&'a NullBuffer),
    /// One byte per row, 1 = NULL (used by the group-key store).
    Bytes(&'a [u8]),
}

impl Nul<'_> {
    #[inline(always)]
    pub fn is_null(&self, i: usize) -> bool {
        match *self {
            Nul::None => false,
            Nul::Bits(n) => n.is_null(i),
            Nul::Bytes(b) => b[i] != 0,
        }
    }
    pub fn any(&self) -> bool {
        !matches!(self, Nul::None)
    }
}

#[derive(Clone, Copy)]
pub enum KData<'a> {
    I64(&'a [i64]),
    I32(&'a [i32]),
    Dict { keys: &'a [i32], offs: &'a [i32], bytes: &'a [u8] },
    Str { offs: &'a [i32], bytes: &'a [u8] },
}

#[derive(Clone, Copy)]
pub struct KCol<'a> {
    pub d: KData<'a>,
    pub nulls: Nul<'a>,
}

impl<'a> KCol<'a> {
    pub fn from_array(a: &'a dyn Array) -> R<KCol<'a>> {
        let nulls = a.nulls().filter(|n| n.null_count() > 0).map_or(Nul::None, Nul::Bits);
        let d = match a.data_type() {
            DataType::Int64 => KData::I64(a.as_primitive::<Int64Type>().values()),
            DataType::Decimal64(..) => KData::I64(a.as_primitive::<Decimal64Type>().values()),
            DataType::Int32 => KData::I32(a.as_primitive::<Int32Type>().values()),
            DataType::Date32 => KData::I32(a.as_primitive::<Date32Type>().values()),
            DataType::Utf8 => {
                let s = a.as_string::<i32>();
                KData::Str { offs: s.value_offsets(), bytes: s.value_data() }
            }
            DataType::Dictionary(k, v) if **k == DataType::Int32 && **v == DataType::Utf8 => {
                let d = a.as_dictionary::<Int32Type>();
                let s = d.values().as_string::<i32>();
                KData::Dict { keys: d.keys().values(), offs: s.value_offsets(), bytes: s.value_data() }
            }
            other => return Err(EngineError(format!("not supported yet: key column of type {other:?}"))),
        };
        Ok(KCol { d, nulls })
    }

    #[inline(always)]
    pub fn is_null(&self, i: usize) -> bool {
        self.nulls.is_null(i)
    }

    /// The bytes of a string value (dictionary or plain); empty for a non-string column.
    #[inline(always)]
    pub fn bytes(&self, i: usize) -> &'a [u8] {
        match self.d {
            KData::Dict { keys, offs, bytes } => {
                let k = keys[i] as usize;
                &bytes[offs[k] as usize..offs[k + 1] as usize]
            }
            KData::Str { offs, bytes } => &bytes[offs[i] as usize..offs[i + 1] as usize],
            _ => &[],
        }
    }

    /// Are the (non-NULL) values at `i` here and `j` there equal? Mismatched types are never equal.
    #[inline(always)]
    pub fn eq_at(&self, i: usize, other: &KCol, j: usize) -> bool {
        match (&self.d, &other.d) {
            (KData::I64(a), KData::I64(b)) => a[i] == b[j],
            (KData::I32(a), KData::I32(b)) => a[i] == b[j],
            (KData::I64(_) | KData::I32(_), _) | (_, KData::I64(_) | KData::I32(_)) => false,
            _ => self.bytes(i) == other.bytes(j),
        }
    }

    pub fn same_kind(&self, other: &KCol) -> bool {
        matches!(
            (&self.d, &other.d),
            (KData::I64(_), KData::I64(_))
                | (KData::I32(_), KData::I32(_))
                | (KData::Dict { .. } | KData::Str { .. }, KData::Dict { .. } | KData::Str { .. })
        )
    }
}

#[inline(always)]
fn pass(first: bool, rows: usize, sel: Option<&[u32]>, nulls: Nul, out: &mut [u64], f: impl Fn(usize) -> u64) {
    let mut k = 0usize;
    match nulls {
        Nul::None => each(rows, sel, |i| {
            combine(first, &mut out[k], f(i));
            k += 1;
        }),
        _ => each(rows, sel, |i| {
            let hc = if nulls.is_null(i) { NULL_H } else { f(i) };
            combine(first, &mut out[k], hc);
            k += 1;
        }),
    }
}

/// One hash per selected row (in selection order) over all key columns. A NULL hashes to a constant.
/// `mask` is ANDed into the result: tests pass a small mask to force collisions.
pub fn hash_cols(cols: &[KCol], rows: usize, sel: Option<&[u32]>, mask: u64, out: &mut Vec<u64>) {
    let n = sel.map_or(rows, <[u32]>::len);
    out.clear();
    out.resize(n, 0);
    for (ci, c) in cols.iter().enumerate() {
        let first = ci == 0;
        match c.d {
            KData::I64(v) => pass(first, rows, sel, c.nulls, out, |i| mix(v[i] as u64)),
            KData::I32(v) => pass(first, rows, sel, c.nulls, out, |i| mix(v[i] as u32 as u64)),
            KData::Str { .. } => pass(first, rows, sel, c.nulls, out, |i| hash_bytes(c.bytes(i))),
            KData::Dict { offs, bytes, keys } => {
                let nd = offs.len().saturating_sub(1);
                if nd <= 2 * n {
                    // Few distinct values compared with the rows: hash each dictionary entry once.
                    let dh: Vec<u64> = (0..nd).map(|j| hash_bytes(&bytes[offs[j] as usize..offs[j + 1] as usize])).collect();
                    pass(first, rows, sel, c.nulls, out, |i| dh[keys[i] as usize]);
                } else {
                    pass(first, rows, sel, c.nulls, out, |i| hash_bytes(c.bytes(i)));
                }
            }
        }
    }
    if mask != u64::MAX {
        out.iter_mut().for_each(|h| *h &= mask);
    }
}

/// Column-wise storage of the distinct group keys of a hash aggregate.
pub struct KStoreCol {
    pub v: KV,
    /// Empty until the first NULL key is stored; then one byte per group (1 = NULL).
    pub nulls: Vec<u8>,
}

pub enum KV {
    I64(Vec<i64>),
    I32(Vec<i32>),
    Str { offs: Vec<i32>, bytes: Vec<u8> },
}

pub struct KStore {
    pub cols: Vec<KStoreCol>,
    pub len: usize,
    /// Set when the string bytes outgrew 32-bit offsets; the owner turns it into an error.
    pub poisoned: bool,
}

impl KStore {
    pub fn like(cols: &[KCol]) -> KStore {
        let cols = cols
            .iter()
            .map(|c| KStoreCol {
                v: match c.d {
                    KData::I64(_) => KV::I64(Vec::new()),
                    KData::I32(_) => KV::I32(Vec::new()),
                    KData::Dict { .. } | KData::Str { .. } => KV::Str { offs: vec![0], bytes: Vec::new() },
                },
                nulls: Vec::new(),
            })
            .collect();
        KStore { cols, len: 0, poisoned: false }
    }

    /// Views of the stored keys as key columns (row = group id).
    pub fn as_cols(&self) -> Vec<KCol<'_>> {
        self.cols
            .iter()
            .map(|c| KCol {
                d: match &c.v {
                    KV::I64(v) => KData::I64(v),
                    KV::I32(v) => KData::I32(v),
                    KV::Str { offs, bytes } => KData::Str { offs, bytes },
                },
                nulls: if c.nulls.is_empty() { Nul::None } else { Nul::Bytes(&c.nulls) },
            })
            .collect()
    }

    /// Is group `g` equal to the key at `row` of `cols`? NULL equals NULL here (a NULL key is a group).
    #[inline(always)]
    pub fn eq(&self, g: usize, cols: &[KCol], row: usize) -> bool {
        for (sc, c) in self.cols.iter().zip(cols) {
            let sn = !sc.nulls.is_empty() && sc.nulls[g] != 0;
            if sn != c.is_null(row) {
                return false;
            }
            if sn {
                continue;
            }
            let same = match (&sc.v, &c.d) {
                (KV::I64(s), KData::I64(v)) => s[g] == v[row],
                (KV::I32(s), KData::I32(v)) => s[g] == v[row],
                (KV::Str { offs, bytes }, KData::Str { .. } | KData::Dict { .. }) => &bytes[offs[g] as usize..offs[g + 1] as usize] == c.bytes(row),
                _ => false,
            };
            if !same {
                return false;
            }
        }
        true
    }

    pub fn push(&mut self, cols: &[KCol], row: usize) {
        let len = self.len;
        for (sc, c) in self.cols.iter_mut().zip(cols) {
            let null = c.is_null(row);
            // Invariant: `nulls` is empty (no NULL key yet) or has exactly one byte per stored group. The
            // first NULL backfills zeros and must then push its own flag even when `len` is 0 (the case
            // where the very first group has a NULL key: `resize(0, 0)` leaves the vector empty, so a
            // test of `is_empty()` AFTER the resize would wrongly skip the push and lose the flag).
            if null {
                if sc.nulls.is_empty() {
                    sc.nulls.resize(len, 0);
                }
                sc.nulls.push(1);
            } else if !sc.nulls.is_empty() {
                sc.nulls.push(0);
            }
            match (&mut sc.v, &c.d) {
                (KV::I64(s), KData::I64(v)) => s.push(if null { 0 } else { v[row] }),
                (KV::I32(s), KData::I32(v)) => s.push(if null { 0 } else { v[row] }),
                (KV::Str { offs, bytes }, KData::Str { .. } | KData::Dict { .. }) => {
                    if !null {
                        let b = c.bytes(row);
                        if bytes.len() + b.len() > i32::MAX as usize {
                            self.poisoned = true;
                        } else {
                            bytes.extend_from_slice(b);
                        }
                    }
                    offs.push(bytes.len() as i32);
                }
                _ => panic!("key column changed type between batches"),
            }
        }
        self.len += 1;
    }
}
