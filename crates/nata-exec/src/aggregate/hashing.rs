//! Version 1 canonical XXH3-64 keys. Fixed seed within/between queries for replay;
//! recursion changes the seed, never the equality relation. No native layouts,
//! Rust enum discriminants or dictionary codes enter this representation.
use super::{Atom, Col, Data};
use twox_hash::XxHash3_64;

const SEED: u64 = 0x6e61_7461_0000_0001;
fn seed(depth: u8) -> u64 {
    SEED ^ u64::from(depth)
}
fn value(tag: u8, bytes: &[u8]) -> u64 {
    XxHash3_64::oneshot_with_seed(SEED ^ (u64::from(tag) << 32), bytes)
}
fn combine(previous: u64, next: u64) -> u64 {
    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(&previous.to_le_bytes());
    bytes[8..].copy_from_slice(&next.to_le_bytes());
    XxHash3_64::oneshot_with_seed(SEED, &bytes)
}
fn float(v: f64) -> u64 {
    if let Atom::Float(bits) = Atom::float(v) {
        bits
    } else {
        unreachable!()
    }
}
fn atom(v: &Atom) -> u64 {
    match v {
        Atom::Null => value(0, &[]),
        Atom::Bool(v) => value(1, &[u8::from(*v)]),
        Atom::Int(v) => value(2, &v.to_le_bytes()),
        Atom::Dec(v) => value(3, &v.to_le_bytes()),
        Atom::Float(v) => value(4, &v.to_le_bytes()),
        Atom::Str(v) => value(5, v.as_bytes()),
        Atom::Date(v) => value(6, &v.to_le_bytes()),
    }
}
pub(super) fn key(key: &[Atom], depth: u8) -> u64 {
    key.iter().fold(seed(depth), |h, v| combine(h, atom(v)))
}
/// One type dispatch per column, one hash per selected value. The hash vector
/// belongs to the evaluated batch and is covered by its decode reservation.
pub(super) fn columns(keys: &[Col], rows: &[u32]) -> Vec<u64> {
    let mut hashes = vec![seed(0); rows.len()];
    for col in keys {
        let mut apply = |hash_value: &dyn Fn(usize) -> u64| {
            for (h, &row) in hashes.iter_mut().zip(rows) {
                let i = row as usize;
                let v = if col.valid[i] {
                    hash_value(i)
                } else {
                    value(0, &[])
                };
                *h = combine(*h, v);
            }
        };
        match &col.data {
            Data::Bool(v) => apply(&|i| value(1, &[u8::from(v[i])])),
            Data::Int(v) => apply(&|i| value(2, &v[i].to_le_bytes())),
            Data::Dec(v) => apply(&|i| value(3, &v[i].to_le_bytes())),
            Data::F32(v) => apply(&|i| value(4, &float(f64::from(v[i])).to_le_bytes())),
            Data::F64(v) => apply(&|i| value(4, &float(v[i]).to_le_bytes())),
            Data::Str(v) => apply(&|i| value(5, v.get(i).as_bytes())),
            Data::Date(v) => apply(&|i| value(6, &v[i].to_le_bytes())),
        }
    }
    hashes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::StrData;
    use arrow_array::StringArray;
    use nata_sql::types::SqlType;
    use std::sync::Arc;

    #[test]
    fn columns_match_owned_keys_for_selected_null_dictionary_and_float_values() {
        let types_and_data = [
            (SqlType::Boolean, Data::Bool(vec![false, true, false])),
            (SqlType::BigInt, Data::Int(vec![i64::MIN, i64::MAX, 7])),
            (
                SqlType::Decimal {
                    precision: 38,
                    scale: 6,
                },
                Data::Dec(vec![-123, 456, 0]),
            ),
            (
                SqlType::Double,
                Data::F64(vec![-0.0, -f64::NAN, f64::INFINITY]),
            ),
            (
                SqlType::Float,
                Data::F32(vec![0.0, f32::NAN, f32::INFINITY]),
            ),
            (SqlType::Date, Data::Date(vec![-1, i32::MAX, 0])),
            (
                SqlType::Varchar,
                Data::Str(StrData::Owned(vec!["雪\0".into(), "".into(), "x".into()])),
            ),
            (
                SqlType::Varchar,
                Data::Str(StrData::Dict {
                    values: Arc::new(StringArray::from(vec!["x", "雪\0", ""])),
                    keys: vec![1, 2, u32::MAX],
                }),
            ),
        ];
        let cols: Vec<_> = types_and_data
            .into_iter()
            .map(|(ty, data)| {
                let mut c = Col::empty(ty, 3);
                c.data = data;
                c.valid = vec![true, true, false];
                c
            })
            .collect();
        let rows = [2, 1, 0];
        for end in 0..=cols.len() {
            let cols = &cols[..end];
            let hashes = columns(cols, &rows);
            for (&row, h) in rows.iter().zip(hashes) {
                let owned: Vec<_> = cols.iter().map(|c| Atom::get(c, row as usize)).collect();
                assert_eq!(h, key(&owned, 0));
            }
        }
        assert_eq!(columns(&cols[6..7], &rows), columns(&cols[7..8], &rows));
        assert_eq!(columns(&cols[3..4], &[0, 1]), columns(&cols[4..5], &[0, 1]));
    }
    #[test]
    fn canonical_hash_vectors_and_recursive_seeds() {
        let keys = [
            vec![],
            vec![Atom::Null],
            vec![Atom::Str(String::new())],
            vec![
                Atom::Bool(true),
                Atom::Int(-1),
                Atom::Dec(-12345678901234567890),
                Atom::Date(-719528),
            ],
            vec![
                Atom::float(-0.0),
                Atom::float(-f64::NAN),
                Atom::Str("雪\0ab".into()),
            ],
            vec![Atom::Str("ab".into()), Atom::Str("c".into())],
            vec![Atom::Str("a".into()), Atom::Str("bc".into())],
        ];
        let got: Vec<_> = keys
            .iter()
            .map(|k| (key(k, 0), key(k, 1), key(k, 8)))
            .collect();
        // Fixed numbers are shared by the ARM64 and x86-64 test runs.
        assert_eq!(
            got,
            vec![
                (
                    7953766376873656321,
                    7953766376873656320,
                    7953766376873656329
                ),
                (
                    13553917536884033798,
                    956188376720846103,
                    7034392023310925015
                ),
                (
                    15858104530678892483,
                    14361038463711366233,
                    1449736871628806105
                ),
                (
                    2356305683524327382,
                    13029555116741579857,
                    4745519467832359231
                ),
                (
                    14782503665781616874,
                    8038311123683699322,
                    3667420635459449015
                ),
                (
                    16840122798863910996,
                    12381770228872161720,
                    13597398806054149522
                ),
                (
                    690546385129792018,
                    12131449407888934713,
                    9480658346766053146
                )
            ]
        );
        assert!(got.windows(2).all(|w| w[0] != w[1]));
        for (zero, one, eight) in got {
            assert_ne!(zero, one);
            assert_ne!(one, eight);
        }
    }
}
