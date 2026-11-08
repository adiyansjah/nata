//! Batch boundary for the native decimal DAG and aggregate inputs. Arrow parents
//! are retained only until the admitted batch has finished (including resume).
use super::*;
use arrow_array::Decimal64Array;
use arrow_buffer::{BooleanBuffer, NullBuffer};

#[derive(Clone, Debug)]
pub(super) enum Argument {
    General(Arc<Col>),
    Decimal64(Arc<Decimal64Array>),
}
impl Argument {
    pub(super) fn general(&self) -> Option<&Arc<Col>> {
        match self {
            Self::General(c) => Some(c),
            Self::Decimal64(_) => None,
        }
    }
}

/// Selection is sorted and unique in the aggregate evaluator, unlike a gather.
/// Word masks are built once; Arrow bit_chunks handles sliced bitmap offsets.
pub(super) fn selection(rows: &[u32], len: usize) -> Vec<u64> {
    let mut words = vec![0; len.div_ceil(64)];
    for &r in rows {
        words[r as usize / 64] |= 1u64 << (r % 64);
    }
    words
}
pub(super) fn intersect(words: &mut [u64], nulls: Option<&NullBuffer>) {
    if let Some(nulls) = nulls.filter(|n| n.null_count() != 0) {
        let chunks = nulls.inner().bit_chunks();
        for (word, bits) in words.iter_mut().zip(
            chunks
                .iter()
                .chain(std::iter::once(chunks.remainder_bits())),
        ) {
            *word &= bits;
        }
    }
}
pub(super) fn nulls(words: &[u64], len: usize) -> Option<NullBuffer> {
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let n = NullBuffer::new(BooleanBuffer::new(bytes.into(), 0, len));
    (n.null_count() != 0).then_some(n)
}

// Fixed-width allocations remain inside the existing 128-byte/expression/row
// allowance; borrowed Arrow parents stay in the decode grant. No charge is cut.
pub(super) fn record_owned(len: usize, copied: bool, bitmap: bool) {
    crate::query_stats::current(|stats| {
        use crate::query_stats::Counter;
        let bytes = len * 8 + if bitmap { len.div_ceil(64) * 8 } else { 0 };
        stats.count(Counter::ColMaterializations, 1);
        stats.count(Counter::MaterializedBytes, bytes as u64);
        if copied {
            stats.count(Counter::CopiedBytes, bytes as u64);
        }
        stats.count(Counter::AllocationCapacityBytes, bytes as u64);
        stats.count(Counter::AllocationSites, 1 + u64::from(bitmap));
    });
}

pub(super) fn narrow(col: &Col) -> Option<Arc<Decimal64Array>> {
    let SqlType::Decimal { precision, scale } = col.ty else {
        return None;
    };
    if precision > 18 {
        return None;
    }
    let Data::Dec(values) = &col.data else {
        return None;
    };
    let values = values
        .iter()
        .zip(&col.valid)
        .map(|(&v, &ok)| if ok { i64::try_from(v).ok() } else { Some(0) })
        .collect::<Option<Vec<_>>>()?;
    record_owned(col.len(), true, true);
    Some(Arc::new(
        Decimal64Array::new(values.into(), Some(NullBuffer::from(col.valid.clone())))
            .with_precision_and_scale(precision, i8::try_from(scale).ok()?)
            .ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sliced_bitmap_offsets_and_null_junk() {
        for offset in [0, 1, 7, 63, 64, 65] {
            let mut valid = vec![true; offset + 131];
            for i in 0..131 {
                valid[offset + i] = i % 3 != 1;
            }
            let n = NullBuffer::from(valid).slice(offset, 131);
            let rows: Vec<u32> = (0..131).filter(|i| i % 5 != 0).collect();
            let mut mask = selection(&rows, 131);
            intersect(&mut mask, Some(&n));
            let back = nulls(&mask, 131).expect("nulls");
            for i in 0..131 {
                assert_eq!(
                    back.is_valid(i),
                    i % 5 != 0 && i % 3 != 1,
                    "offset {offset} row {i}"
                );
            }
        }
    }
}
