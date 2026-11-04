// One operation per argument, values hidden from the optimizer. Built in release with overflow-checks on and off.
#![allow(clippy::arithmetic_side_effects, clippy::cast_possible_truncation, reason = "this file exists to overflow")]
use std::hint::black_box;

fn main() {
    let op = std::env::args().nth(1).unwrap_or_default();
    let r: i64 = match op.as_str() {
        "add_i32" => i64::from(black_box(i32::MAX) + black_box(1)),
        "sub_i32" => i64::from(black_box(i32::MIN) - black_box(1)),
        "mul_i32" => i64::from(black_box(i32::MAX) * black_box(2)),
        "add_i64" => black_box(i64::MAX) + black_box(1),
        "neg_i32_min" => i64::from(-black_box(i32::MIN)),
        "abs_i32_min" => i64::from(black_box(i32::MIN).abs()),
        "pow_i32" => i64::from(black_box(i32::MAX).pow(2)),
        "shl_amount" => i64::from(black_box(1i32) << black_box(40u32)),
        "shl_bits_lost" => black_box(1i64) << black_box(63u32),
        "div_min_neg1" => black_box(i64::MIN) / black_box(-1),
        "div_zero" => 1 / black_box(0i64),
        "truncating_cast" => black_box(1i64 << 40) as i32 as i64,
        _ => panic!("unknown op {op}"),
    };
    println!("{r}");
}
