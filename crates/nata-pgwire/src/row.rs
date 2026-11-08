//! Typed result encoding into one reused, length-checked current-row scratch.
//! The bridge still owns at most two 64 KiB buffers; this never collects rows.
use arrow_array::{Array, RecordBatch, cast::AsArray, types::*};
use nata_sql::types::SqlType;

use crate::{binary, codec, server::WireError};

fn limit() -> WireError {
    WireError::new("22000", "unrepresentable text").at(nata_sql::Pos::START)
}
fn frame_len(total: usize) -> Result<i32, WireError> {
    i32::try_from(total.checked_sub(1).ok_or_else(limit)?).map_err(|_| limit())
}
fn append(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), WireError> {
    // Backend frames retain packet()'s signed-i32 length limit. MAX_FRAME is
    // the separate frontend cap: supported backend rows can exceed 64 KiB
    // and are split only into bounded bridge buffers, never smaller frames.
    frame_len(out.len().checked_add(bytes.len()).ok_or_else(limit)?)?;
    // Avoid geometric over-allocation for wide current-row scratch.
    if out.len() + bytes.len() > out.capacity() {
        out.reserve_exact(bytes.len());
    }
    out.extend_from_slice(bytes);
    Ok(())
}

pub(crate) fn encode(
    batch: &RecordBatch,
    fields: &[(String, SqlType)],
    formats: &[i16],
    row: usize,
    copy_first: Option<bool>,
    out: &mut Vec<u8>,
) -> Result<(), WireError> {
    out.clear();
    append(
        out,
        &[if copy_first.is_some() { b'd' } else { b'D' }, 0, 0, 0, 0],
    )?;
    if copy_first == Some(true) {
        append(out, crate::copy::HEADER)?;
    }
    append(
        out,
        &i16::try_from(fields.len())
            .map_err(|_| limit())?
            .to_be_bytes(),
    )?;
    for (col, (array, (_, ty))) in batch.columns().iter().zip(fields).enumerate() {
        if array.is_null(row) {
            append(out, &(-1i32).to_be_bytes())?;
            continue;
        }
        let at = out.len();
        append(out, &[0; 4])?;
        let binary = copy_first.is_some()
            || if formats.len() == 1 {
                formats[0] == 1
            } else {
                formats.get(col) == Some(&1)
            };
        if binary {
            match ty {
                SqlType::Boolean => append(out, &[u8::from(array.as_boolean().value(row))])?,
                SqlType::SmallInt => append(
                    out,
                    &array.as_primitive::<Int16Type>().value(row).to_be_bytes(),
                )?,
                SqlType::Integer => append(
                    out,
                    &array.as_primitive::<Int32Type>().value(row).to_be_bytes(),
                )?,
                SqlType::BigInt => append(
                    out,
                    &array.as_primitive::<Int64Type>().value(row).to_be_bytes(),
                )?,
                SqlType::Varchar => {
                    let value = string(array.as_ref(), row)?;
                    checked_text(value)?;
                    append(out, value.as_bytes())?;
                }
                SqlType::Date => append(
                    out,
                    &crate::date::binary_days(array.as_primitive::<Date32Type>().value(row))
                        .map_err(|e| e.at(nata_sql::Pos::START))?,
                )?,
                _ => {
                    let text = text(array.as_ref(), row, *ty)?;
                    append(out, &binary::encode(codec::type_info(*ty).0, &text)?)?;
                }
            }
        } else {
            match ty {
                SqlType::Boolean => append(
                    out,
                    if array.as_boolean().value(row) {
                        b"t"
                    } else {
                        b"f"
                    },
                )?,
                SqlType::TinyInt => {
                    integer(out, i64::from(array.as_primitive::<Int8Type>().value(row)))?
                }
                SqlType::SmallInt => {
                    integer(out, i64::from(array.as_primitive::<Int16Type>().value(row)))?
                }
                SqlType::Integer => {
                    integer(out, i64::from(array.as_primitive::<Int32Type>().value(row)))?
                }
                SqlType::BigInt => integer(out, array.as_primitive::<Int64Type>().value(row))?,
                SqlType::Varchar => {
                    let value = string(array.as_ref(), row)?;
                    checked_text(value)?;
                    append(out, value.as_bytes())?;
                }
                _ => {
                    let text = text(array.as_ref(), row, *ty)?;
                    checked_text(&text)?;
                    append(out, text.as_bytes())?;
                }
            }
        }
        let len = i32::try_from(out.len() - at - 4).map_err(|_| limit())?;
        out[at..at + 4].copy_from_slice(&len.to_be_bytes());
    }
    let len = frame_len(out.len())?;
    out[1..5].copy_from_slice(&len.to_be_bytes());
    Ok(())
}
fn integer(out: &mut Vec<u8>, value: i64) -> Result<(), WireError> {
    let mut digits = itoa::Buffer::new();
    append(out, digits.format(value).as_bytes())
}
fn checked_text(s: &str) -> Result<(), WireError> {
    if s.contains('\0') {
        return Err(WireError::new("22000", "unrepresentable text").at(nata_sql::Pos::START));
    }
    Ok(())
}
fn string(array: &dyn Array, row: usize) -> Result<&str, WireError> {
    if let Some(a) = array.as_string_opt::<i32>() {
        Ok(a.value(row))
    } else if let Some(a) = array.as_string_opt::<i64>() {
        Ok(a.value(row))
    } else if let Some(a) = array.as_dictionary_opt::<Int32Type>() {
        let values = a
            .values()
            .as_string_opt::<i32>()
            .ok_or_else(WireError::internal)?;
        let key = usize::try_from(a.keys().value(row)).map_err(|_| WireError::internal())?;
        if key >= values.len() || values.is_null(key) {
            return Err(WireError::internal());
        }
        Ok(values.value(key))
    } else {
        Err(WireError::internal())
    }
}
fn text(array: &dyn Array, row: usize, ty: SqlType) -> Result<String, WireError> {
    if ty == SqlType::Date {
        return crate::date::text(array.as_primitive::<Date32Type>().value(row))
            .map_err(|e| e.at(nata_sql::Pos::START));
    }
    let text = nata_parquet::csv::cell_text(array, row)
        .map_err(WireError::scan)?
        .ok_or_else(WireError::internal)?;
    super::server::wire_text(ty, text).map_err(WireError::scan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::*;
    use std::{hint::black_box, sync::Arc, time::Instant};

    fn cases() -> Vec<(SqlType, Arc<dyn Array>)> {
        vec![
            (
                SqlType::Boolean,
                Arc::new(BooleanArray::from(vec![Some(true), Some(false), None])),
            ),
            (
                SqlType::TinyInt,
                Arc::new(Int8Array::from(vec![Some(i8::MIN), Some(i8::MAX), None])),
            ),
            (
                SqlType::SmallInt,
                Arc::new(Int16Array::from(vec![Some(i16::MIN), Some(i16::MAX), None])),
            ),
            (
                SqlType::Integer,
                Arc::new(Int32Array::from(vec![Some(i32::MIN), Some(i32::MAX), None])),
            ),
            (
                SqlType::BigInt,
                Arc::new(Int64Array::from(vec![Some(i64::MIN), Some(i64::MAX), None])),
            ),
            (
                SqlType::Float,
                Arc::new(Float32Array::from(vec![Some(-0.), Some(f32::NAN), None])),
            ),
            (
                SqlType::Double,
                Arc::new(Float64Array::from(vec![
                    Some(f64::INFINITY),
                    Some(f64::NEG_INFINITY),
                    None,
                ])),
            ),
            (
                SqlType::Decimal {
                    precision: 38,
                    scale: 3,
                },
                Arc::new(
                    Decimal128Array::from(vec![
                        Some(-12345678901234567890123456789012345678),
                        Some(0),
                        None,
                    ])
                    .with_precision_and_scale(38, 3)
                    .unwrap_or_else(|_| panic!("fixture")),
                ),
            ),
            (
                SqlType::Decimal {
                    precision: 3,
                    scale: 3,
                },
                Arc::new(
                    Decimal64Array::from(vec![Some(-500), Some(0), None])
                        .with_precision_and_scale(3, 3)
                        .unwrap_or_else(|_| panic!("fixture")),
                ),
            ),
            (
                SqlType::Varchar,
                Arc::new(StringArray::from(vec![Some("µ\t\n\\\""), Some(""), None])),
            ),
            (
                SqlType::Varchar,
                Arc::new(
                    vec![Some("dictionary"), Some(""), None]
                        .into_iter()
                        .collect::<DictionaryArray<Int32Type>>(),
                ),
            ),
            (
                SqlType::Date,
                Arc::new(Date32Array::from(vec![
                    Some(-2_440_588),
                    Some(2_145_042_905),
                    None,
                ])),
            ),
        ]
    }
    fn legacy(
        batch: &RecordBatch,
        fields: &[(String, SqlType)],
        formats: &[i16],
        desc: &[u8],
        i: usize,
    ) -> Vec<u8> {
        let values = batch
            .columns()
            .iter()
            .zip(fields)
            .map(|(a, (_, ty))| {
                (!a.is_null(i))
                    .then(|| text(a.as_ref(), i, *ty).unwrap_or_else(|_| panic!("fixture")))
            })
            .collect::<Vec<_>>();
        binary::packet(
            codec::row(&values).unwrap_or_else(|_| panic!("fixture")),
            desc,
            formats,
        )
        .unwrap_or_else(|_| panic!("fixture"))
    }
    #[test]
    fn typed_rows_match_legacy_bytes_and_copy() {
        let cs = cases();
        let fields = cs
            .iter()
            .enumerate()
            .map(|(i, (ty, _))| (format!("c{i}"), *ty))
            .collect::<Vec<_>>();
        let batch = RecordBatch::try_from_iter(
            cs.iter()
                .enumerate()
                .map(|(i, (_, a))| (format!("c{i}"), Arc::clone(a))),
        )
        .unwrap_or_else(|_| panic!("fixture"));
        let mut out = vec![];
        let desc = codec::description(&fields).unwrap_or_else(|_| panic!("fixture"));
        for fs in [vec![], vec![1], vec![0, 1, 0, 1, 0, 1, 0, 1, 0, 1, 0, 1]] {
            for i in 0..3 {
                encode(&batch, &fields, &fs, i, None, &mut out)
                    .unwrap_or_else(|_| panic!("fixture"));
                assert_eq!(out, legacy(&batch, &fields, &fs, &desc, i));
            }
        }
        for i in 0..3 {
            let row = legacy(&batch, &fields, &[1], &desc, i);
            let mut body = if i == 0 {
                crate::copy::HEADER.to_vec()
            } else {
                vec![]
            };
            body.extend_from_slice(&row[5..]);
            encode(&batch, &fields, &[1], i, Some(i == 0), &mut out)
                .unwrap_or_else(|_| panic!("fixture"));
            assert_eq!(
                out,
                codec::packet(b'd', &body).unwrap_or_else(|_| panic!("fixture"))
            );
        }
    }
    #[test]
    fn typed_row_caps_null_nul_and_dates() {
        assert_eq!(frame_len(i32::MAX as usize + 1).ok(), Some(i32::MAX));
        assert!(frame_len(i32::MAX as usize + 2).is_err());
        assert!(frame_len(usize::MAX).is_err());
        assert!(frame_len(0).is_err());
        let mut out = vec![];
        for (s, ok) in [("x".repeat(4 * 1024 * 1024), true), ("a\0b".into(), false)] {
            let b = RecordBatch::try_from_iter([(
                "v",
                Arc::new(StringArray::from(vec![s.as_str()])) as Arc<dyn Array>,
            )])
            .unwrap_or_else(|_| panic!("fixture"));
            for fs in [vec![], vec![1]] {
                assert_eq!(
                    encode(
                        &b,
                        &[("v".into(), SqlType::Varchar)],
                        &fs,
                        0,
                        None,
                        &mut out
                    )
                    .is_ok(),
                    ok
                );
            }
        }
        for days in [i32::MIN, -2_440_589, 2_145_042_906, i32::MAX] {
            let b = RecordBatch::try_from_iter([(
                "d",
                Arc::new(Date32Array::from(vec![days])) as Arc<dyn Array>,
            )])
            .unwrap_or_else(|_| panic!("fixture"));
            for fs in [vec![], vec![1]] {
                assert!(
                    encode(&b, &[("d".into(), SqlType::Date)], &fs, 0, None, &mut out).is_err()
                );
            }
        }
    }
    #[test]
    fn profile_typed_rows() {
        if std::env::var_os("P16_ENCODER_PROFILE").is_none() {
            return;
        }
        for (ty, a) in cases() {
            let fields = [("v".into(), ty)];
            let b = RecordBatch::try_from_iter([("v", a)]).unwrap_or_else(|_| panic!("fixture"));
            let desc = codec::description(&fields).unwrap_or_else(|_| panic!("fixture"));
            for fs in [vec![], vec![1]] {
                let mut out = vec![];
                let start = Instant::now();
                for i in 0..100_000 {
                    black_box(legacy(&b, &fields, &fs, &desc, i % 3));
                }
                let old = start.elapsed();
                let start = Instant::now();
                for i in 0..100_000 {
                    encode(&b, &fields, &fs, i % 3, None, &mut out)
                        .unwrap_or_else(|_| panic!("fixture"));
                    black_box(&out);
                }
                println!("{ty:?} {fs:?} legacy={old:?} typed={:?}", start.elapsed());
            }
        }
    }
}
