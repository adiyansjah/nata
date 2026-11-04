//! Deterministic test data: column types, NULL patterns, selection shapes. No external RNG crate.
//!
//! NULL slots carry NON-ZERO, in-range garbage on purpose (lesson of spikes/null-path: a kernel that lets NULL slots into
//! a sum is not caught when the slots hold 0).

use std::sync::Arc;

use arrow::array::{ArrayRef, BooleanArray, Decimal64Array, Decimal128Array, DictionaryArray, Int32Array, Int64Array, StringArray, UInt32Array};
use arrow::buffer::{BooleanBuffer, Buffer, NullBuffer, OffsetBuffer, ScalarBuffer};
use arrow::datatypes::Int32Type;

/// splitmix64.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `0..n` (n > 0).
    pub fn below(&mut self, n: u64) -> u64 {
        u64::try_from((u128::from(self.next_u64()) * u128::from(n)) >> 64).unwrap_or(0)
    }
    /// True with probability `pct` percent.
    pub fn chance(&mut self, pct: f64) -> bool {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64 * 100.0 < pct
    }
}

/// The column types of the benchmark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Int64,
    Decimal64,
    Decimal128,
    Utf8,
    Dict,
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::Int64, Kind::Decimal64, Kind::Decimal128, Kind::Utf8, Kind::Dict];
    pub fn name(self) -> &'static str {
        match self {
            Kind::Int64 => "Int64",
            Kind::Decimal64 => "Decimal64(18,2)",
            Kind::Decimal128 => "Decimal128(38,4)",
            Kind::Utf8 => "Utf8",
            Kind::Dict => "Dictionary<Int32,Utf8>",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Kind::Int64 => "int64",
            Kind::Decimal64 => "dec64",
            Kind::Decimal128 => "dec128",
            Kind::Utf8 => "utf8",
            Kind::Dict => "dict",
        }
    }
    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.key() == s)
    }
}

/// Number of distinct values of the dictionary column.
pub const DICT_VALUES: usize = 256;

fn make_nulls(n: usize, nulls_pct: f64, rng: &mut Rng) -> Option<NullBuffer> {
    if nulls_pct <= 0.0 {
        return None;
    }
    let valid: Vec<bool> = (0..n).map(|_| !rng.chance(nulls_pct)).collect();
    Some(NullBuffer::from(valid))
}

fn random_string_bytes(rng: &mut Rng, out: &mut Vec<u8>) {
    let len = 4 + usize::try_from(rng.below(21)).unwrap_or(0); // 4..=24 bytes
    for _ in 0..len {
        out.push(b'a' + u8::try_from(rng.below(26)).unwrap_or(0));
    }
}

fn string_array(n: usize, nulls: Option<NullBuffer>, rng: &mut Rng) -> StringArray {
    // Every slot, NULL or not, holds a real string (garbage under NULL).
    let mut offsets = Vec::with_capacity(n + 1);
    let mut bytes = Vec::with_capacity(n * 14);
    offsets.push(0i32);
    for _ in 0..n {
        random_string_bytes(rng, &mut bytes);
        offsets.push(i32::try_from(bytes.len()).expect("test data fits i32 offsets"));
    }
    StringArray::try_new(OffsetBuffer::new(ScalarBuffer::from(offsets)), Buffer::from_vec(bytes), nulls).expect("valid string array")
}

/// A column of `n` rows of type `kind` with `nulls_pct` percent NULLs (0 = no null buffer at all).
pub fn make_column(kind: Kind, n: usize, nulls_pct: f64, seed: u64) -> ArrayRef {
    let mut rng = Rng::new(seed);
    let nulls = make_nulls(n, nulls_pct, &mut rng);
    match kind {
        Kind::Int64 => {
            let v: Vec<i64> = (0..n).map(|_| i64::try_from(rng.below(2_000_000_000)).unwrap_or(0) - 1_000_000_000).collect();
            Arc::new(Int64Array::new(ScalarBuffer::from(v), nulls))
        }
        Kind::Decimal64 => {
            let v: Vec<i64> = (0..n).map(|_| i64::try_from(rng.below(2_000_000_000)).unwrap_or(0) - 1_000_000_000).collect();
            Arc::new(Decimal64Array::new(ScalarBuffer::from(v), nulls).with_precision_and_scale(18, 2).expect("precision"))
        }
        Kind::Decimal128 => {
            let v: Vec<i128> = (0..n)
                .map(|_| {
                    let hi = i128::from(rng.below(1 << 30));
                    let lo = i128::from(rng.next_u64());
                    (hi << 64) | lo
                })
                .collect();
            Arc::new(Decimal128Array::new(ScalarBuffer::from(v), nulls).with_precision_and_scale(38, 4).expect("precision"))
        }
        Kind::Utf8 => Arc::new(string_array(n, nulls, &mut rng)),
        Kind::Dict => {
            let values = string_array(DICT_VALUES, None, &mut rng);
            let keys: Vec<i32> = (0..n).map(|_| i32::try_from(rng.below(DICT_VALUES as u64)).unwrap_or(0)).collect();
            let keys = Int32Array::new(ScalarBuffer::from(keys), nulls);
            Arc::new(DictionaryArray::<Int32Type>::try_new(keys, Arc::new(values)).expect("valid dictionary"))
        }
    }
}

// ------------------------------------------------------------------------------------------------ selections

/// The three selection shapes of the benchmark. `Random` and `Runs` are carried as a sorted index vector, `Mask` as a bitmap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// k distinct rows chosen uniformly at random, as sorted `u32` indices.
    Random,
    /// k rows in at most 4 long runs, as sorted `u32` indices.
    Runs,
    /// k rows chosen uniformly at random, as a bitmap (`u64` words, bit j of word w = row 64w+j, tail bits zero).
    Mask,
}

impl Shape {
    pub const ALL: [Shape; 3] = [Shape::Random, Shape::Runs, Shape::Mask];
    pub fn name(self) -> &'static str {
        match self {
            Shape::Random => "random idx",
            Shape::Runs => "4 runs idx",
            Shape::Mask => "bitmap",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Shape::Random => "random",
            Shape::Runs => "runs",
            Shape::Mask => "mask",
        }
    }
    pub fn parse(s: &str) -> Option<Shape> {
        Shape::ALL.into_iter().find(|k| k.key() == s)
    }
}

/// The selection as the engine would carry it (the "native" form).
#[derive(Clone)]
pub enum Sel {
    /// Sorted, distinct row indices.
    Idx(UInt32Array),
    /// Bitmap words (a `Buffer` of `u64`s), so cloning is O(1) like moving an owned vector.
    Mask(Buffer),
}

pub fn words_for(n: usize) -> usize {
    n.div_ceil(64)
}

pub fn idx_to_words(idx: &[u32], n: usize) -> Vec<u64> {
    let mut w = vec![0u64; words_for(n)];
    for &i in idx {
        let i = i as usize;
        w[i >> 6] |= 1u64 << (i & 63);
    }
    w
}

pub fn mask_to_idx(words: &[u64]) -> Vec<u32> {
    let k: usize = words.iter().map(|w| w.count_ones() as usize).sum();
    let mut out = Vec::with_capacity(k);
    for (wi, &w) in words.iter().enumerate() {
        let mut w = w;
        while w != 0 {
            out.push(u32::try_from(wi * 64 + w.trailing_zeros() as usize).expect("row index fits u32"));
            w &= w - 1;
        }
    }
    out
}

impl Sel {
    /// Build the native form of the selection of `rows` (sorted, distinct, all `< n`).
    pub fn from_rows(shape: Shape, n: usize, rows: &[u32]) -> Sel {
        match shape {
            Shape::Random | Shape::Runs => Sel::Idx(UInt32Array::from(rows.to_vec())),
            Shape::Mask => Sel::Mask(Buffer::from_vec(idx_to_words(rows, n))),
        }
    }
    pub fn rows(&self) -> Vec<usize> {
        match self {
            Sel::Idx(a) => a.values().iter().map(|&i| i as usize).collect(),
            Sel::Mask(b) => mask_to_idx(b.typed_data::<u64>()).into_iter().map(|i| i as usize).collect(),
        }
    }
    pub fn count(&self) -> usize {
        match self {
            Sel::Idx(a) => a.len(),
            Sel::Mask(b) => b.typed_data::<u64>().iter().map(|w| w.count_ones() as usize).sum(),
        }
    }
    /// Indices for `take`. For an index selection this is free; for a bitmap it is a conversion that compaction pays.
    pub fn to_indices(&self) -> UInt32Array {
        match self {
            Sel::Idx(a) => a.clone(),
            Sel::Mask(b) => UInt32Array::from(mask_to_idx(b.typed_data::<u64>())),
        }
    }
    /// Boolean mask for `filter`. For a bitmap this is free; for an index selection it is a conversion that compaction pays.
    pub fn to_bool(&self, n: usize) -> BooleanArray {
        match self {
            Sel::Idx(a) => BooleanArray::new(BooleanBuffer::new(Buffer::from_vec(idx_to_words(a.values(), n)), 0, n), None),
            Sel::Mask(b) => BooleanArray::new(BooleanBuffer::new(b.clone(), 0, n), None),
        }
    }
}

/// Number of selected rows for a selectivity in percent: at least 1, at most `n`.
pub fn selected_count(n: usize, sel_pct: f64) -> usize {
    ((n as f64 * sel_pct / 100.0).round() as usize).clamp(1, n)
}

/// The sorted rows of a selection of `k` of `n` rows in the given shape.
pub fn make_rows(shape: Shape, n: usize, k: usize, seed: u64) -> Vec<u32> {
    assert!(k <= n);
    let mut rng = Rng::new(seed);
    match shape {
        Shape::Random | Shape::Mask => {
            // Partial Fisher-Yates: k distinct rows.
            let mut v: Vec<u32> = (0..u32::try_from(n).expect("n fits u32")).collect();
            for i in 0..k {
                let j = i + usize::try_from(rng.below((n - i) as u64)).unwrap_or(0);
                v.swap(i, j);
            }
            v.truncate(k);
            v.sort_unstable();
            v
        }
        Shape::Runs => {
            if k == 0 {
                return Vec::new();
            }
            let runs = k.min(4);
            // Run lengths: k split as evenly as possible. Gaps: the n-k unselected rows split at random into runs+1 pieces.
            let lens: Vec<usize> = (0..runs).map(|r| k / runs + usize::from(r < k % runs)).collect();
            let mut cuts: Vec<usize> = (0..runs).map(|_| usize::try_from(rng.below((n - k + 1) as u64)).unwrap_or(0)).collect();
            cuts.sort_unstable();
            let mut out = Vec::with_capacity(k);
            let mut done = 0usize; // selected rows placed so far
            for (r, &len) in lens.iter().enumerate() {
                let start = cuts[r] + done; // cuts[r] unselected rows precede run r in total
                out.extend((start..start + len).map(|i| u32::try_from(i).expect("row fits u32")));
                done += len;
            }
            out
        }
    }
}

pub fn make_selection(shape: Shape, n: usize, k: usize, seed: u64) -> Sel {
    Sel::from_rows(shape, n, &make_rows(shape, n, k, seed))
}
