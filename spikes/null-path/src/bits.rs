//! Validity and selection as bitmaps: 64 rows per `u64` word, bit `j` of word `w` is row `64 * w + j`.
//! Invariant everywhere: bits at positions >= `rows` (the tail of the last word) are zero.
use arrow::buffer::NullBuffer;

#[inline(always)]
pub fn nwords(rows: usize) -> usize {
    rows.div_ceil(64)
}

/// Mask of the bits of word `w` that are rows (all ones except in the last, partial word).
#[inline(always)]
pub fn word_mask(rows: usize, w: usize) -> u64 {
    let left = rows - 64 * w;
    if left >= 64 { !0 } else { (1u64 << left) - 1 }
}

/// Reads 64 bits starting at bit `pos` of a little-endian bitmap; bits past the end read as zero.
#[inline]
pub fn read_word(bytes: &[u8], pos: usize) -> u64 {
    let (byte, shift) = (pos / 8, (pos % 8) as u32);
    if shift == 0 && byte + 8 <= bytes.len() {
        return u64::from_le_bytes(bytes[byte..byte + 8].try_into().expect("8 bytes"));
    }
    let get = |from: usize| -> u64 {
        let mut b = [0u8; 8];
        if from < bytes.len() {
            let n = (bytes.len() - from).min(8);
            b[..n].copy_from_slice(&bytes[from..from + n]);
        }
        u64::from_le_bytes(b)
    };
    let lo = get(byte) >> shift;
    if shift == 0 { lo } else { lo | (get(byte + 8) << (64 - shift)) }
}

/// The validity of an Arrow column as words aligned to row 0 (so a sliced array with a bit offset works),
/// with the tail cleared. `None` when there are no NULLs.
pub fn validity_words(n: Option<&NullBuffer>, rows: usize) -> Option<Vec<u64>> {
    let n = n.filter(|n| n.null_count() > 0)?;
    let (bytes, off) = (n.validity(), n.offset());
    // `n.validity()` is the whole buffer; the logical bit i is at off + i.
    let mut w: Vec<u64> = (0..nwords(rows)).map(|k| read_word(bytes, off + 64 * k)).collect();
    if let Some(last) = w.last_mut() {
        *last &= word_mask(rows, nwords(rows) - 1);
    }
    Some(w)
}

/// Row indexes of the set bits, in order. A word with many set bits is expanded by a branch-free compress over
/// all 64 positions, a sparse word by a loop over its set bits (`trailing_zeros`), whichever mispredicts less.
pub fn mask_to_idx(mask: &[u64]) -> Vec<u32> {
    let n = popcount(mask);
    let mut out = vec![0u32; n + 64];
    let mut k = 0usize;
    for (w, &m) in mask.iter().enumerate() {
        let base = (64 * w) as u32;
        if m.count_ones() >= DENSE_WORD {
            for j in 0..64u32 {
                out[k] = base + j;
                k += ((m >> j) & 1) as usize;
            }
        } else {
            let mut m = m;
            while m != 0 {
                out[k] = base + m.trailing_zeros();
                k += 1;
                m &= m - 1;
            }
        }
    }
    out.truncate(k);
    out
}

/// Set bits in a word from which the compress over 64 positions beats the loop over set bits (a guess; tuned by measurement).
const DENSE_WORD: u32 = 16;

pub fn idx_to_mask(idx: &[u32], rows: usize) -> Vec<u64> {
    let mut m = vec![0u64; nwords(rows)];
    for &i in idx {
        m[(i >> 6) as usize] |= 1u64 << (i & 63);
    }
    m
}

pub fn popcount(mask: &[u64]) -> usize {
    mask.iter().map(|w| w.count_ones() as usize).sum()
}

/// A set of rows of a batch.
#[derive(Debug, Clone, PartialEq)]
pub enum Sel {
    /// Every row.
    All,
    /// A bitmap; dense selections stay in this form and are processed 64 rows at a time.
    Mask(Vec<u64>),
    /// Row indexes, ascending; sparse selections.
    Idx(Vec<u32>),
}

impl Sel {
    pub fn len(&self, rows: usize) -> usize {
        match self {
            Sel::All => rows,
            Sel::Mask(m) => popcount(m),
            Sel::Idx(i) => i.len(),
        }
    }

    pub fn to_vec(&self, rows: usize) -> Vec<u32> {
        match self {
            Sel::All => (0..rows as u32).collect(),
            Sel::Mask(m) => mask_to_idx(m),
            Sel::Idx(i) => i.clone(),
        }
    }

    /// The selection word `w` (dense forms only).
    #[inline(always)]
    pub fn word(&self, rows: usize, w: usize) -> u64 {
        match self {
            Sel::All => word_mask(rows, w),
            Sel::Mask(m) => m[w],
            Sel::Idx(_) => unreachable!("an index selection has no words"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_word_handles_every_shift_and_the_end() {
        let bytes: Vec<u8> = (0..20u8).map(|b| b.wrapping_mul(37).wrapping_add(11)).collect();
        let bit = |i: usize| (i / 8 < bytes.len() && (bytes[i / 8] >> (i % 8)) & 1 == 1) as u64;
        for pos in 0..150 {
            let want = (0..64).fold(0u64, |a, j| a | (bit(pos + j) << j));
            assert_eq!(read_word(&bytes, pos), want, "pos {pos}");
        }
    }

    #[test]
    fn validity_words_of_a_sliced_buffer_match_the_bits_and_clear_the_tail() {
        let bools: Vec<bool> = (0..300).map(|i| (i * 7 + i / 5) % 4 != 0).collect();
        let full = NullBuffer::from(bools.clone());
        for (off, len) in [(0usize, 300usize), (3, 130), (64, 64), (65, 200), (5, 1), (0, 64), (0, 65)] {
            let s = full.slice(off, len);
            let w = validity_words(Some(&s), len);
            let nulls = bools[off..off + len].iter().filter(|b| !**b).count();
            if nulls == 0 {
                assert!(w.is_none());
                continue;
            }
            let w = w.expect("has NULLs");
            assert_eq!(w.len(), nwords(len));
            for i in 0..len {
                assert_eq!((w[i / 64] >> (i % 64)) & 1 == 1, bools[off + i], "off {off} len {len} row {i}");
            }
            assert_eq!(*w.last().unwrap() & !word_mask(len, w.len() - 1), 0, "tail bits are zero");
        }
    }

    #[test]
    fn mask_idx_round_trip_and_word_mask() {
        let rows = 200;
        let idx: Vec<u32> = (0..rows as u32).filter(|i| i % 3 == 0 || *i == 199).collect();
        let m = idx_to_mask(&idx, rows);
        assert_eq!(mask_to_idx(&m), idx);
        assert_eq!(popcount(&m), idx.len());
        assert_eq!(word_mask(200, 0), !0);
        assert_eq!(word_mask(200, 3), (1 << 8) - 1);
        assert_eq!(word_mask(192, 2), !0);
        assert_eq!(Sel::All.word(200, 3), (1 << 8) - 1);
    }
}
