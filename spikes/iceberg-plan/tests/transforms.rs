use iceplan::transform::*;
use iceplan::types::*;

fn date(y: i64, m: i64, d: i64) -> i32 { days_from_civil(y, m, d) as i32 }
fn ts(y: i64, m: i64, d: i64, h: i64, mi: i64, s: i64, us: i64) -> i64 { days_from_civil(y, m, d) * MICROS_PER_DAY + ((h * 60 + mi) * 60 + s) * 1_000_000 + us }

#[test]
fn spec_appendix_b_hash_vectors() {
    assert_eq!(iceberg_hash(&Value::Int(34)), 2017239379);
    assert_eq!(iceberg_hash(&Value::Long(34)), 2017239379);
    assert_eq!(iceberg_hash(&Value::Decimal(1420)), -500754589);                                           // 14.20
    assert_eq!(iceberg_hash(&Value::Date(date(2017, 11, 16))), -653330422);
    assert_eq!(hash_long((22 * 3600 + 31 * 60 + 8) * 1_000_000), -662762989);                              // time 22:31:08
    assert_eq!(iceberg_hash(&Value::Ts(ts(2017, 11, 16, 22, 31, 8, 0))), -2047944441);
    assert_eq!(iceberg_hash(&Value::Ts(ts(2017, 11, 16, 22, 31, 8, 1))), -1207196810);
    assert_eq!(iceberg_hash(&Value::Ts(ts(2017, 11, 16, 14, 31, 8, 0) + 8 * MICROS_PER_HOUR)), -2047944441);   // timestamptz 14:31:08-08:00
    assert_eq!(iceberg_hash(&Value::Str("iceberg".into())), 1210000089);
    let u: [u8; 16] = [0xF7, 0x9C, 0x3E, 0x09, 0x67, 0x7C, 0x4B, 0xBD, 0xA4, 0x79, 0x3F, 0x34, 0x9C, 0xB7, 0x85, 0xE7];
    assert_eq!(iceberg_hash(&Value::Uuid(u)), 1488055340);
    assert_eq!(iceberg_hash(&Value::Bin(vec![0, 1, 2, 3])), -188683207);
    assert_eq!(iceberg_hash(&Value::Bool(true)), 1392991556);
    assert_eq!(iceberg_hash(&Value::Double(1.0)), -142385009);
    assert_eq!(iceberg_hash(&Value::Double(0.0)), 1669671676);
    assert_eq!(iceberg_hash(&Value::Double(-0.0)), 1669671676);
    assert_eq!(iceberg_hash(&Value::Float(1.0)), -142385009);
    assert_eq!(iceberg_hash(&Value::Float(-0.0)), 1669671676);
}

#[test]
fn spec_truncate_and_time_transform_examples() {
    let t = Transform::Truncate(10);
    assert_eq!(t.apply(&Value::Int(1)), Some(Value::Int(0)));
    assert_eq!(t.apply(&Value::Int(-1)), Some(Value::Int(-10)));
    assert_eq!(Transform::Truncate(50).apply(&Value::Decimal(1065)), Some(Value::Decimal(1050)));          // 10.65 -> 10.50 (W=50, scale 2)
    assert_eq!(Transform::Truncate(3).apply(&Value::Str("iceberg".into())), Some(Value::Str("ice".into())));
    assert_eq!(Transform::Truncate(3).apply(&Value::Bin(vec![1, 2, 3, 4, 5])), Some(Value::Bin(vec![1, 2, 3])));
    assert_eq!(Transform::Truncate(2).apply(&Value::Str("é😀x".into())), Some(Value::Str("é😀".into())));   // code points, not bytes
    assert_eq!(Transform::Year.apply(&Value::Date(date(2017, 11, 16))), Some(Value::Int(47)));
    assert_eq!(Transform::Month.apply(&Value::Date(date(2017, 11, 16))), Some(Value::Int(47 * 12 + 10)));
    assert_eq!(Transform::Day.apply(&Value::Date(date(2017, 11, 16))), Some(Value::Date(date(2017, 11, 16))));
    assert_eq!(Transform::Hour.apply(&Value::Ts(ts(1970, 1, 1, 1, 30, 0, 0))), Some(Value::Int(1)));
    // before the epoch: floor, not truncation toward zero
    assert_eq!(Transform::Year.apply(&Value::Date(date(1969, 12, 31))), Some(Value::Int(-1)));
    assert_eq!(Transform::Month.apply(&Value::Date(date(1969, 12, 31))), Some(Value::Int(-1)));
    assert_eq!(Transform::Day.apply(&Value::Ts(-1)), Some(Value::Date(-1)));
    assert_eq!(Transform::Hour.apply(&Value::Ts(-1)), Some(Value::Int(-1)));
    assert_eq!(Transform::Month.apply(&Value::Date(date(1960, 1, 1))), Some(Value::Int(-120)));
    assert_eq!(Transform::Void.apply(&Value::Int(5)), None);
    let b = Transform::Bucket(16);
    assert_eq!(b.apply(&Value::Int(34)), Some(Value::Int((2017239379i32 & i32::MAX) % 16)));
    assert!(matches!(b.apply(&Value::Str("iceberg".into())), Some(Value::Int(x)) if (0..16).contains(&x)));
    assert!(matches!(Transform::Bucket(7).apply(&Value::Long(i64::MIN)), Some(Value::Int(x)) if (0..7).contains(&x)));
    // a NEGATIVE hash: the sign bit is dropped before the modulo
    assert_eq!(Transform::Bucket(7).apply(&Value::Decimal(1420)), Some(Value::Int((-500754589i32 & i32::MAX) % 7)));
    assert_eq!(Transform::Bucket(16).apply(&Value::Date(date(2017, 11, 16))), Some(Value::Int((-653330422i32 & i32::MAX) % 16)));
}

#[test]
fn single_value_serialization_round_trips_and_has_the_spec_widths() {
    for v in [Value::Int(-5), Value::Long(i64::MIN), Value::Float(-0.0), Value::Double(1.5), Value::Date(-3), Value::Ts(-1), Value::Bool(true), Value::Str("é".into()), Value::Bin(vec![0, 255]),
              Value::Decimal(0), Value::Decimal(-1), Value::Decimal(255), Value::Decimal(-256), Value::Decimal(i128::MAX), Value::Decimal(i128::MIN), Value::Uuid([7; 16])] {
        let t = match &v { Value::Int(_) => Type::Int, Value::Long(_) => Type::Long, Value::Float(_) => Type::Float, Value::Double(_) => Type::Double, Value::Date(_) => Type::Date,
                           Value::Ts(_) => Type::Timestamp, Value::Bool(_) => Type::Boolean, Value::Str(_) => Type::String, Value::Bin(_) => Type::Binary, Value::Decimal(_) => Type::Decimal(38, 0), Value::Uuid(_) => Type::Uuid };
        let b = serialize(&v);
        let back = deserialize(&t, &b).unwrap();
        assert_eq!(serialize(&back), b, "{v:?}");
    }
    assert_eq!(serialize(&Value::Decimal(0)), vec![0]);
    assert_eq!(serialize(&Value::Decimal(-1)), vec![0xFF]);
    assert_eq!(serialize(&Value::Decimal(128)), vec![0x00, 0x80]);
    assert_eq!(serialize(&Value::Decimal(-129)), vec![0xFF, 0x7F]);
    assert_eq!(serialize(&Value::Decimal(1420)), vec![0x05, 0x8C]);
    assert_eq!(serialize(&Value::Long(34)), 34i64.to_le_bytes().to_vec());
}

struct R(u64);
impl R { fn n(&mut self) -> u64 { self.0 ^= self.0 >> 12; self.0 ^= self.0 << 25; self.0 ^= self.0 >> 27; self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) } }

/// If a row satisfies `v OP lit` then its partition value satisfies the projected predicate. For every transform, with values on both sides of the epoch.
#[test]
fn inclusive_projection_is_sound() {
    let mut r = R(0x1234_5678_9abc_def1);
    let ops = [Op::Eq, Op::Lt, Op::Le, Op::Gt, Op::Ge];
    let mut checked = 0u64;
    let mk_int = |r: &mut R| Value::Int((r.n() % 400) as i32 - 200);
    let mk_long = |r: &mut R| Value::Long((r.n() % 4000) as i64 - 2000);
    let mk_date = |r: &mut R| Value::Date((r.n() % 40000) as i32 - 20000);
    let mk_ts = |r: &mut R| Value::Ts((r.n() % (400 * MICROS_PER_DAY as u64)) as i64 - 200 * MICROS_PER_DAY);
    let mk_dec = |r: &mut R| Value::Decimal((r.n() % 4000) as i128 - 2000);
    let strs = ["", "a", "ab", "abc", "abcd", "b", "é", "éa", "😀", "😀a", "zz"];
    let mk_str = |r: &mut R| Value::Str(strs[(r.n() % strs.len() as u64) as usize].to_string());
    let cases: Vec<(Transform, Box<dyn Fn(&mut R) -> Value>)> = vec![
        (Transform::Identity, Box::new(mk_int)), (Transform::Bucket(5), Box::new(mk_long)), (Transform::Truncate(10), Box::new(mk_int)), (Transform::Truncate(7), Box::new(mk_long)),
        (Transform::Truncate(50), Box::new(mk_dec)), (Transform::Year, Box::new(mk_date)), (Transform::Month, Box::new(mk_date)), (Transform::Day, Box::new(mk_ts)),
        (Transform::Hour, Box::new(mk_ts)), (Transform::Month, Box::new(mk_ts)), (Transform::Truncate(2), Box::new(mk_str)), (Transform::Truncate(1), Box::new(mk_str)),
    ];
    for (t, mk) in &cases {
        for _ in 0..30000 {
            let (v, lit) = (mk(&mut r), mk(&mut r));
            let pv = t.apply(&v).unwrap();
            let all: Vec<Op> = if matches!(v, Value::Str(_)) { vec![Op::Eq, Op::Lt, Op::Le, Op::Gt, Op::Ge, Op::StartsWith] } else { ops.to_vec() };
            for op in all {
                if !holds(&v, op, &lit) { continue; }
                if let Some((pop, plit)) = t.project(op, &lit) {
                    assert!(holds(&pv, pop, &plit), "{t:?}: {v:?} {op:?} {lit:?} holds, but the partition value {pv:?} does not satisfy {pop:?} {plit:?}");
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 100_000, "only {checked} projections were exercised");
}
