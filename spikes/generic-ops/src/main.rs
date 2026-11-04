//! generic-ops: modes `check`, `nullcheck`, `bench` (end to end), `membench` (engine only), `nullbench`.
mod engine;
mod fused;
mod kernels;
mod nulls;
mod source;

use std::time::Instant;

use engine::{Output, Plan, Visit, q1, q6, run};
use kernels::R;
use nulls::{mem_visit, parquet_visit, preload};
use source::Source;

fn quantile(v: &mut [f64], q: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * q).round() as usize]
}

fn plans() -> [Plan; 2] {
    [q1(), q6()]
}

fn fused(name: &str, morsels: usize, visit: Visit, threads: usize) -> R<Output> {
    if name == "q1" { fused::fused_q1(morsels, visit, threads) } else { fused::fused_q6(morsels, visit, threads) }
}

fn ratio_cell(v: &mut Vec<f64>) -> String {
    format!("{:.2} [{:.2}, {:.2}]", quantile(v, 0.5), quantile(v, 0.25), quantile(v, 0.75))
}

fn mm(v: &mut Vec<f64>) -> String {
    format!("{:.0} / {:.0}", quantile(v, 0.0), quantile(v, 0.5))
}

/// Generic against hand-fused with a generic-against-generic control, interleaved in a rotating order.
fn compare(title: &str, path: &str, rounds: usize, threads_list: &[usize], in_memory: bool) {
    println!("{title}\nrounds: {rounds} (+1 warm-up); arms interleaved in rotating order; min / median ms; ratios are medians of paired per-round ratios [Q1, Q3]\n");
    println!("| query | threads | generic ms | hand-fused ms | generic / fused | control: generic2 / generic |\n|---|---:|---|---|---|---|");
    for plan in plans() {
        let src = Source::open(path, plan.columns);
        let batches = if in_memory { preload(&src, 0) } else { Vec::new() };
        let (pq, mem);
        let (morsels, visit): (usize, Visit) = if in_memory {
            mem = mem_visit(&batches);
            (batches.len(), &mem)
        } else {
            pq = parquet_visit(&src, 0);
            (src.row_groups, &pq)
        };
        let reference = fused(plan.name, morsels, visit, 4).expect("fused");
        for &threads in threads_list {
            let mut t: [Vec<f64>; 3] = Default::default();
            for round in 0..=rounds {
                let mut ms = [0.0f64; 3];
                for k in 0..3 {
                    let arm = (round + k) % 3;
                    let start = Instant::now();
                    let out = if arm == 1 { fused(plan.name, morsels, visit, threads) } else { run(morsels, visit, &plan, threads) };
                    ms[arm] = start.elapsed().as_secs_f64() * 1000.0;
                    assert_eq!(out.expect("run"), reference, "{} arm {arm} disagrees", plan.name);
                }
                if round > 0 {
                    (0..3).for_each(|a| t[a].push(ms[a]));
                }
            }
            let mut gf: Vec<f64> = t[0].iter().zip(&t[1]).map(|(g, f)| g / f).collect();
            let mut cg: Vec<f64> = t[2].iter().zip(&t[0]).map(|(c, g)| c / g).collect();
            println!("| {} | {threads} | {} | {} | {} | {} |", plan.name, mm(&mut t[0]), mm(&mut t[1]), ratio_cell(&mut gf), ratio_cell(&mut cg));
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map_or("", String::as_str);
    let path = args.get(2).expect("usage: generic-ops <mode> <lineitem.parquet> [rounds]").as_str();
    let n: usize = args.get(3).map_or(15, |s| s.parse().expect("number"));
    match mode {
        "check" => {
            for plan in plans() {
                let src = Source::open(path, plan.columns);
                let v = parquet_visit(&src, 0);
                let generic = run(src.row_groups, &v, &plan, 4).expect("generic");
                assert_eq!(generic, run(src.row_groups, &v, &plan, 1).expect("generic 1"), "{}: result depends on thread count", plan.name);
                assert_eq!(generic, fused(plan.name, src.row_groups, &v, 4).expect("fused"), "{}: generic and fused disagree", plan.name);
                println!("== {}\n{}", plan.name, generic.csv());
            }
        }
        "nullcheck" => {
            for plan in plans() {
                let src = Source::open(path, plan.columns);
                for pct in [10u32, 50, 100] {
                    let v = parquet_visit(&src, pct);
                    let got = run(src.row_groups, &v, &plan, 4).expect("generic");
                    let want = if plan.name == "q1" { nulls::reference_q1(&src, pct) } else { nulls::reference_q6(&src, pct) };
                    assert_eq!(got, want, "{} with {pct}% NULLs disagrees with the scalar reference", plan.name);
                    println!("ok   {} with {pct:>3}% NULLs equals the scalar reference ({})", plan.name, got.csv().lines().next().unwrap_or("").chars().take(60).collect::<String>());
                }
            }
        }
        "loop" => {
            let which = args.get(4).map_or("q1", String::as_str);
            let plan = if which == "q6" { q6() } else { q1() };
            let src = Source::open(path, plan.columns);
            let batches = preload(&src, 0);
            let v = mem_visit(&batches);
            // loop <file> <n> <q1|q6> [fused]: the generic engine, or the hand-fused baseline, one thread, batches in
            // memory; for `perf record` (the same query shape and data, only the implementation differs).
            if args.get(5).map(String::as_str) == Some("fused") {
                for _ in 0..n {
                    std::hint::black_box(fused(plan.name, batches.len(), &v, 1).expect("fused"));
                }
            } else {
                for _ in 0..n {
                    std::hint::black_box(run(batches.len(), &v, &plan, 1).expect("run"));
                }
            }
        }
        "bench" => compare("End to end (Parquet decode included)", path, n, &[1, 2, 4, 8], false),
        "membench" => compare("Engine only (batches already decoded, in memory)", path, n, &[1, 2, 4, 8], true),
        "nullbench" => {
            println!("Engine only, batches in memory; rounds: {n} (+1 warm-up); ratio = time with 10% NULLs / time without NULLs, paired per round\n");
            println!("| query | threads | no NULLs ms | 10% NULLs ms | 10% / 0% [Q1, Q3] |\n|---|---:|---|---|---|");
            for plan in plans() {
                let src = Source::open(path, plan.columns);
                let (clean, dirty) = (preload(&src, 0), preload(&src, 10));
                let (vc, vd) = (mem_visit(&clean), mem_visit(&dirty));
                for threads in [1usize, 8] {
                    let mut t: [Vec<f64>; 2] = Default::default();
                    for round in 0..=n {
                        let mut ms = [0.0f64; 2];
                        for k in 0..2 {
                            let arm = (round + k) % 2;
                            let start = Instant::now();
                            if arm == 1 { run(dirty.len(), &vd, &plan, threads) } else { run(clean.len(), &vc, &plan, threads) }.expect("run");
                            ms[arm] = start.elapsed().as_secs_f64() * 1000.0;
                        }
                        if round > 0 {
                            (0..2).for_each(|a| t[a].push(ms[a]));
                        }
                    }
                    let mut r: Vec<f64> = t[1].iter().zip(&t[0]).map(|(a, b)| a / b).collect();
                    println!("| {} | {threads} | {} | {} | {} |", plan.name, mm(&mut t[0]), mm(&mut t[1]), ratio_cell(&mut r));
                }
            }
        }
        _ => eprintln!("modes: check | nullcheck | bench | membench | nullbench"),
    }
}
