//! Bounded mixed-radix dictionary code traversal. The caller supplies value-based
//! dictionary hashes; codes are only batch-local addresses, never SQL identity.
/// Append a column to batch-local combination IDs. `radix - 1` denotes NULL.
/// Invalid codes in masked rows are never inspected. A live invalid code asks
/// the caller to use its general path. All indexing and arithmetic is checked.
pub fn append(
    codes: &[u32],
    valid: &[bool],
    rows: &[u32],
    radix: usize,
    combinations: &mut [usize],
) -> Option<()> {
    let null = radix.checked_sub(1)?;
    if combinations.len() != rows.len() {
        return None;
    }
    for (&row, combination) in rows.iter().zip(combinations) {
        let i = usize::try_from(row).ok()?;
        let code = if *valid.get(i)? {
            let code = usize::try_from(*codes.get(i)?).ok()?;
            if code >= null {
                return None;
            }
            code
        } else {
            null
        };
        *combination = combination.checked_mul(radix)?.checked_add(code)?;
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_null_junk_and_checked_bounds() {
        let mut ids = [0; 3];
        append(
            &[1, u32::MAX, 0, u32::MAX],
            &[true, false, true, true],
            &[2, 1, 0],
            3,
            &mut ids,
        )
        .expect("dictionary");
        assert_eq!(ids, [0, 2, 1]);
        append(&[0, 1, 1], &[true; 3], &[2, 1, 0], 3, &mut ids).expect("dictionary");
        assert_eq!(ids, [1, 7, 3]);
        assert_eq!(append(&[3], &[true], &[0], 3, &mut [0]), None);
        assert_eq!(append(&[], &[false], &[0], 1, &mut [0]), Some(()));
        assert_eq!(append(&[], &[], &[0], 1, &mut [0]), None);
        assert_eq!(append(&[0], &[true], &[0], 2, &mut [usize::MAX]), None);
        assert_eq!(append(&[], &[], &[], 0, &mut []), None);
        assert_eq!(append(&[], &[], &[], 1, &mut [0]), None);
    }
}
