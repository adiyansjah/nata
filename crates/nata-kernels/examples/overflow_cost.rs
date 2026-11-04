//! Timing of the kernels in release, to compare `overflow-checks` on and off (see the results file).
//! Run twice: `cargo run --release -p nata-kernels --example overflow_cost` and the same with
//! `--config profile.release.overflow-checks=false` (use a separate CARGO_TARGET_DIR for the second).
#![allow(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "benchmark driver"
)]
use nata_kernels::kernels::{Valid, div, filter_range, group_sums_dense, mul, sum};
use std::hint::black_box;
use std::time::Instant;

const ROWS: usize = 8192;

fn best_ns_per_row(mut f: impl FnMut()) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..7 {
        let t = Instant::now();
        for _ in 0..2000 {
            f();
        }
        best = best.min(t.elapsed().as_secs_f64() * 1e9 / (2000.0 * ROWS as f64));
    }
    best
}

fn main() {
    let a: Vec<i64> = (0..ROWS as i64).map(|i| (i % 1000) + 1).collect();
    let b: Vec<i64> = (0..ROWS as i64).map(|i| (i % 77) + 1).collect();
    let valid: Vec<u8> = (0..ROWS).map(|i| (i % 9 != 0) as u8).collect();
    let ids: Vec<usize> = (0..ROWS).map(|i| i % 4).collect();
    let sel: Vec<u32> = (0..ROWS as u32).filter(|i| i % 3 != 0).collect();
    println!("kernel                          ns/row (best of 7)");
    let row = |name: &str, f: &mut dyn FnMut()| println!("{name:<30} {:6.3}", best_ns_per_row(f));
    row("mul, no NULLs", &mut || {
        drop(black_box(mul(
            (&a, Valid::All),
            (&b, Valid::All),
            ROWS,
            None,
        )))
    });
    row("mul, NULLs", &mut || {
        drop(black_box(mul(
            (&a, Valid::Bytes(&valid)),
            (&b, Valid::All),
            ROWS,
            None,
        )))
    });
    row("div, no NULLs", &mut || {
        drop(black_box(div(
            (&a, Valid::All),
            (&b, Valid::All),
            ROWS,
            None,
        )))
    });
    row("div, NULLs, selection", &mut || {
        drop(black_box(div(
            (&a, Valid::Bytes(&valid)),
            (&b, Valid::All),
            ROWS,
            Some(&sel),
        )))
    });
    row("filter_range, no NULLs", &mut || {
        drop(black_box(filter_range(&a, Valid::All, 10, 500, ROWS, None)))
    });
    row("sum, no NULLs", &mut || {
        black_box(sum((&a, Valid::All), ROWS, None));
    });
    let (mut acc, mut count) = (vec![[0i128; 1]; 4], vec![0u64; 4]);
    row("group_sums_dense K=1", &mut || {
        group_sums_dense(|i| ids[i], [&a[..]], ROWS, None, &mut acc, &mut count)
    });
    black_box((&acc, &count));
}
