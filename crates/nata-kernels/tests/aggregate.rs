#![allow(
    clippy::arithmetic_side_effects,
    reason = "small independent test vectors and loop indices; production arithmetic stays checked"
)]
use arrow_buffer::i256;
use nata_kernels::aggregate::{
    AggregateError as E, ExactSum, average_type, checked_count, finish_count,
};

#[test]
fn arbitrary_precision_oracle_in_every_merge_order() {
    let mut cases = 0;
    for line in include_str!("fixtures/aggregate.txt")
        .lines()
        .filter(|s| !s.starts_with('#'))
    {
        let fields: Vec<_> = line.split('|').map(str::trim).collect();
        let mut input = fields[0].split_whitespace();
        let p: u8 = input.next().expect("p").parse().expect("p integer");
        let s: u8 = input.next().expect("s").parse().expect("s integer");
        let values: Vec<i128> = input
            .next()
            .expect("values")
            .split(',')
            .map(|v| v.parse().expect("i128"))
            .collect();
        let expected_sum: i256 = fields[1].parse().expect("wide sum");
        let expected_average: i128 = fields[2].parse().expect("average");
        for reverse in [false, true] {
            let mut values = values.clone();
            if reverse {
                values.reverse();
            }
            for width in 1..=values.len() {
                let mut merged = ExactSum::default();
                for chunk in values.chunks(width) {
                    let mut partial = ExactSum::default();
                    for &value in chunk {
                        partial.add(value).expect("add");
                    }
                    let replay = ExactSum::from_spill(partial.spill_sum(), partial.count())
                        .expect("valid replay");
                    assert_eq!(replay, partial);
                    merged.merge(replay).expect("merge");
                }
                assert_eq!(i256::from_le_bytes(merged.spill_sum()), expected_sum);
                assert_eq!(merged.average(p, s), Ok(Some(expected_average)), "{line}");
                let sum = merged.decimal_sum(38);
                match expected_sum
                    .to_i128()
                    .filter(|v| v.unsigned_abs() < 10u128.pow(38))
                {
                    Some(n) => assert_eq!(sum, Ok(Some(n))),
                    None => assert_eq!(sum, Err(E::Overflow)),
                }
            }
        }
        cases += 1;
    }
    assert_eq!(cases, 347);
}

#[test]
fn empty_null_and_integer_result_edges() {
    let mut s = ExactSum::default();
    assert_eq!(s.decimal_sum(38), Ok(None));
    assert_eq!(s.integer_sum(), Ok(None));
    assert_eq!(s.average(10, 0), Ok(None));
    assert_eq!(finish_count(0), Ok(0));
    s.add(i128::from(i64::MAX)).expect("add");
    assert_eq!(s.integer_sum(), Ok(Some(i64::MAX)));
    s.add(1).expect("wide state permits this");
    assert_eq!(s.integer_sum(), Err(E::Overflow));
    assert_eq!(
        s.decimal_sum(38),
        Ok(Some(i128::from(i64::MAX).checked_add(1).expect("sum")))
    );
    assert_eq!(average_type(10, 0), Ok((16, 6)));
    assert_eq!(average_type(19, 0), Ok((25, 6)));
    assert_eq!(average_type(15, 2), Ok((21, 8)));
    assert_eq!(average_type(38, 38), Ok((38, 38)));
    assert_eq!(average_type(0, 0), Err(E::InvalidType));
    assert_eq!(average_type(39, 0), Err(E::InvalidType));
    assert_eq!(average_type(10, 11), Err(E::InvalidType));
    assert_eq!(
        checked_count(u64::MAX.checked_sub(1).expect("count"), 1),
        Err(E::Overflow)
    );
    assert_eq!(finish_count(u64::MAX), Err(E::Overflow));
}

#[test]
fn wide_intermediates_cancel_and_avg_does_not_narrow_sum() {
    let max = 10i128.pow(38).checked_sub(1).expect("max");
    let mut precision_overflow = ExactSum::default();
    precision_overflow.add(max).expect("38 digit input");
    precision_overflow.add(1).expect("wide partial");
    assert_eq!(precision_overflow.decimal_sum(38), Err(E::Overflow));
    let mut s = ExactSum::default();
    s.add(max).expect("add");
    s.add(max).expect("partial exceeds i128 and 38 digits");
    assert_eq!(s.decimal_sum(38), Err(E::Overflow));
    assert_eq!(s.average(38, 0), Ok(Some(max)));
    let mut negative = ExactSum::default();
    negative.add(-max).expect("negative");
    s.merge(negative).expect("cancel before narrowing");
    assert_eq!(s.decimal_sum(38), Ok(Some(max)));
    assert_eq!(s.add(i128::MAX), Err(E::InvalidState));
}

#[test]
fn hostile_spilled_states_and_count_merges_are_checked() {
    assert_eq!(
        ExactSum::from_spill(i256::ONE.to_le_bytes(), 0),
        Err(E::InvalidState)
    );
    assert_eq!(
        ExactSum::from_spill(i256::MIN.to_le_bytes(), 1),
        Err(E::InvalidState)
    );
    assert_eq!(
        ExactSum::from_spill(i256::ZERO.to_le_bytes(), u64::MAX),
        Err(E::InvalidState)
    );
    let huge = ExactSum::from_spill(
        i256::ZERO.to_le_bytes(),
        u64::MAX.checked_sub(1).expect("count"),
    )
    .expect("valid huge count");
    let mut s = huge;
    assert_eq!(s.merge(huge), Err(E::Overflow));
    assert_eq!(s, huge, "failed merge leaves old state intact");
    assert_eq!(s.add(1), Err(E::Overflow));
    assert_eq!(s, huge, "failed update leaves old state intact");
}

#[test]
fn maximum_valid_count_rescales_before_dividing() {
    let count = u64::MAX.checked_sub(1).expect("valid count");
    let value = 10i128.pow(32).checked_sub(1).expect("32 digit input");
    let sum = i256::from_i128(value)
        .checked_mul(i256::from_i128(i128::from(count)))
        .expect("wide bound");
    let state = ExactSum::from_spill(sum.to_le_bytes(), count).expect("valid state");
    assert_eq!(
        state.average(32, 0),
        Ok(Some(value.checked_mul(1_000_000).expect("38 digit mean")))
    );
    let negative = ExactSum::from_spill(sum.checked_neg().expect("negative").to_le_bytes(), count)
        .expect("valid negative state");
    assert_eq!(
        negative.average(32, 0),
        Ok(Some(value.checked_mul(-1_000_000).expect("negative mean")))
    );
    assert_eq!(state.decimal_sum(38), Err(E::Overflow));
}
