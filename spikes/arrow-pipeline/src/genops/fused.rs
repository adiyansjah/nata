//! Hand-fused baselines: one loop per batch, no selection vector, no NULL handling (the data has none).
//! They stand in for what an expression compiler could emit for each query.
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use arrow::array::{Array, AsArray, RecordBatch};
use arrow::datatypes::{Date32Type, Decimal64Type, Int32Type};

use crate::genops::engine::{Output, Q1Acc, Q1_CUTOFF, Q6Acc};
use crate::genops::kernels::{EngineError, R, from_nulls, overflow};
use crate::genops::engine::Visit;

fn no_nulls(a: Option<&arrow::buffer::NullBuffer>) {
    assert!(from_nulls(a).is_all(), "the fused baseline does not handle NULLs");
}

pub fn fused_q1(morsels: usize, visit: Visit, threads: usize) -> R<Output> {
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        let hs: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| -> R<Output> {
                    let mut acc = vec![Q1Acc::default(); 65536];
                    let mut bad = false;
                    loop {
                        let rg = next.fetch_add(1, Relaxed);
                        if rg >= morsels {
                            break;
                        }
                        visit(rg, &mut |rb: &RecordBatch| {
                            let col = |n: &str| rb.column(rb.schema().index_of(n).expect("column")).clone();
                            let (date, qty, price, disc) = (col("l_shipdate"), col("l_quantity"), col("l_extendedprice"), col("l_discount"));
                            let date = date.as_primitive::<Date32Type>();
                            let (qty, price, disc) = (qty.as_primitive::<Decimal64Type>(), price.as_primitive::<Decimal64Type>(), disc.as_primitive::<Decimal64Type>());
                            for n in [date.nulls(), qty.nulls(), price.nulls(), disc.nulls()] {
                                no_nulls(n);
                            }
                            let (rf, ls) = (col("l_returnflag"), col("l_linestatus"));
                            let (rf, ls) = (rf.as_dictionary::<Int32Type>(), ls.as_dictionary::<Int32Type>());
                            let first = |d: &arrow::array::DictionaryArray<Int32Type>| -> Vec<u8> {
                                let v = d.values().as_string::<i32>();
                                (0..v.len()).map(|j| v.value(j).as_bytes()[0]).collect()
                            };
                            let (t0, t1) = (first(rf), first(ls));
                            let (k0, k1) = (rf.keys().values(), ls.keys().values());
                            let (date, qty, price, disc) = (date.values(), qty.values(), price.values(), disc.values());
                            for i in 0..rb.num_rows() {
                                if date[i] <= Q1_CUTOFF {
                                    let g = ((t0[k0[i] as usize] as usize) << 8) | t1[k1[i] as usize] as usize;
                                    let (t, o1) = 100i64.overflowing_sub(disc[i]);
                                    let (m, o2) = price[i].overflowing_mul(t);
                                    bad |= o1 | o2;
                                    let a = &mut acc[g];
                                    a.sums[0] += qty[i] as i128;
                                    a.sums[1] += price[i] as i128;
                                    a.sums[2] += m as i128;
                                    a.nn = [a.nn[0] + 1, a.nn[1] + 1, a.nn[2] + 1];
                                    a.count += 1;
                                }
                            }
                        });
                    }
                    if bad {
                        return overflow("multiplication");
                    }
                    let mut m = std::collections::BTreeMap::new();
                    for (g, a) in acc.into_iter().enumerate() {
                        if a.count > 0 {
                            m.insert((((g >> 8) as u8 as char).to_string(), ((g & 255) as u8 as char).to_string()), a);
                        }
                    }
                    Ok(Output::Q1(m))
                })
            })
            .collect();
        merge_all(hs)
    })
}

pub fn fused_q6(morsels: usize, visit: Visit, threads: usize) -> R<Output> {
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        let hs: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| -> R<Output> {
                    let (mut sum, mut n, mut bad) = (0i128, 0u64, false);
                    loop {
                        let rg = next.fetch_add(1, Relaxed);
                        if rg >= morsels {
                            break;
                        }
                        visit(rg, &mut |rb: &RecordBatch| {
                            let col = |n: &str| rb.column(rb.schema().index_of(n).expect("column")).clone();
                            let (date, qty, price, disc) = (col("l_shipdate"), col("l_quantity"), col("l_extendedprice"), col("l_discount"));
                            let date = date.as_primitive::<Date32Type>();
                            let (qty, price, disc) = (qty.as_primitive::<Decimal64Type>(), price.as_primitive::<Decimal64Type>(), disc.as_primitive::<Decimal64Type>());
                            for nb in [date.nulls(), qty.nulls(), price.nulls(), disc.nulls()] {
                                no_nulls(nb);
                            }
                            let (date, qty, price, disc) = (date.values(), qty.values(), price.values(), disc.values());
                            for i in 0..rb.num_rows() {
                                let keep = (date[i] >= 8766) & (date[i] <= 9130) & (disc[i] >= 5) & (disc[i] <= 7) & (qty[i] <= 2399);
                                let (m, o) = price[i].overflowing_mul(disc[i]);
                                bad |= o & keep;
                                sum += (m as i128) * (keep as i128);
                                n += keep as u64;
                            }
                        });
                    }
                    if bad {
                        return overflow("multiplication");
                    }
                    Ok(Output::Q6(Q6Acc { sum, nn: n }))
                })
            })
            .collect();
        merge_all(hs)
    })
}

fn merge_all(hs: Vec<std::thread::ScopedJoinHandle<'_, R<Output>>>) -> R<Output> {
    let mut total: Option<Output> = None;
    for h in hs {
        let out = h.join().map_err(|_| EngineError("worker panicked".into()))??;
        match &mut total {
            None => total = Some(out),
            Some(t) => t.merge(out),
        }
    }
    Ok(total.expect("at least one worker"))
}
