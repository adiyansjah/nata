//! null-path: generic-ops (the control, unchanged) plus the word-wise engine. Modes: `check`, `nullcheck`, `ladder`,
//! and the generic-ops modes `bench`, `membench`, `nullbench`.
mod bits;
mod engine;
mod fused;
mod kernels;
mod nulls;
mod source;
mod wengine;
mod wk;

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

#[derive(Clone, Copy)]
enum Arm {
    Old,
    New(wengine::Opts),
}

const STEPS: [(&str, wengine::Opts); 7] = [
    ("1 bitmaps + mask filter", wengine::Opts::STEP1),
    ("2 + word-wise project", wengine::Opts::STEP2),
    ("3a + word-wise sink, per-row masks", wengine::Opts::STEP3A),
    ("3 + word-wise sink, subtract NULL rows", wengine::Opts::STEP3),
    ("4 + null-rejecting filters", wengine::Opts::STEP4),
    ("5 + deferred overflow flag", wengine::Opts::STEP5),
    ("6 + wrapping sink adds", wengine::Opts::STEP6),
];

/// The step whose label starts with `name` followed by a space ("1", "3a", "6", ...).
fn step_named(name: &str) -> wengine::Opts {
    STEPS.iter().find(|(l, _)| l.split(' ').next() == Some(name)).unwrap_or_else(|| panic!("no step '{name}'")).1
}

fn all_arms() -> Vec<(String, Arm)> {
    let mut v = vec![("0 old (generic-ops)".to_string(), Arm::Old)];
    v.extend(STEPS.iter().map(|(n, o)| (n.to_string(), Arm::New(*o))));
    v
}

fn run_arm(arm: Arm, name: &str, morsels: usize, visit: Visit, threads: usize) -> R<Output> {
    match arm {
        Arm::Old => run(morsels, visit, &if name == "q1" { q1() } else { q6() }, threads),
        Arm::New(mut o) => {
            // experiment knob: NATA_DENSE_DIV overrides the density threshold of every word-wise step
            if let Some(d) = std::env::var("NATA_DENSE_DIV").ok().and_then(|s| s.parse().ok()) {
                o.dense_div = d;
            }
            wengine::run(morsels, visit, &if name == "q1" { wengine::q1() } else { wengine::q6() }, threads, o)
        }
    }
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

/// Engine only, batches in memory. For each arm (old representation, then each step) the same query on batches
/// without NULLs and with 10% NULLs, interleaved in rotating order; ratios are medians of paired per-round ratios.
fn ladder(path: &str, rounds: usize) {
    println!("Engine only, batches in memory; rounds: {rounds} (+1 warm-up); all arms and both datasets interleaved in rotating order; clock: {}\n", std::env::var("NATA_CLOCK").unwrap_or("wall".into()));
    let arms = all_arms();
    for plan in plans() {
        let src = Source::open(path, plan.columns);
        let (clean, dirty) = (preload(&src, 0), preload(&src, 10));
        let (vc, vd) = (mem_visit(&clean), mem_visit(&dirty));
        let want_clean = run_arm(Arm::Old, plan.name, clean.len(), &vc, 4).expect("old");
        let want_dirty = run_arm(Arm::Old, plan.name, dirty.len(), &vd, 4).expect("old");
        for threads in [1usize, 4] {
            let slots = arms.len() * 2; // (arm, dataset)
            let mut t: Vec<Vec<f64>> = vec![Vec::new(); slots];
            for round in 0..=rounds {
                let mut ms = vec![0.0f64; slots];
                for k in 0..slots {
                    let slot = (round + k) % slots;
                    let (arm, dirty_set) = (arms[slot / 2].1, slot % 2 == 1);
                    let (b, v, want) = if dirty_set { (&dirty, &vd, &want_dirty) } else { (&clean, &vc, &want_clean) };
                    let start = now_ms();
                    let out = run_arm(arm, plan.name, b.len(), v, threads).expect("run");
                    ms[slot] = now_ms() - start;
                    assert_eq!(&out, want, "{} slot {slot} disagrees", plan.name);
                }
                if round > 0 {
                    (0..slots).for_each(|s| t[s].push(ms[s]));
                }
            }
            println!("### {} , {threads} thread(s)\n", plan.name);
            println!("| step | no NULLs ms (min / med) | 10% NULLs ms (min / med) | NULL penalty 10%/0% [Q1, Q3] | 10% vs old 10% | 0% vs old 0% |\n|---|---|---|---|---|---|");
            for (a, (label, _)) in arms.iter().enumerate() {
                let pen: Vec<f64> = t[2 * a + 1].iter().zip(&t[2 * a]).map(|(d, c)| d / c).collect();
                let vs10: Vec<f64> = t[2 * a + 1].iter().zip(&t[1]).map(|(d, o)| d / o).collect();
                let vs0: Vec<f64> = t[2 * a].iter().zip(&t[0]).map(|(d, o)| d / o).collect();
                let (mut c, mut d) = (t[2 * a].clone(), t[2 * a + 1].clone());
                println!("| {label} | {} | {} | {} | {:.2} | {:.2} |", mm1(&mut c), mm1(&mut d), ratio_cell(&mut pen.clone()), quantile(&mut vs10.clone(), 0.5), quantile(&mut vs0.clone(), 0.5));
            }
            println!();
        }
    }
}

/// Milliseconds on the clock chosen by NATA_CLOCK: "wall" (default) or "cpu" (CPU time of the whole process,
/// which a loaded machine inflates less than wall time, though not by nothing: SMT neighbours and caches are shared).
fn now_ms() -> f64 {
    thread_local!(static CPU: bool = std::env::var("NATA_CLOCK").is_ok_and(|v| v == "cpu"));
    if CPU.with(|c| *c) {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: ts is a valid out-pointer for the duration of the call.
        unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
        ts.tv_sec as f64 * 1e3 + ts.tv_nsec as f64 / 1e6
    } else {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1e3
    }
}

/// Old engine and the last two word-wise steps (5: checked sink adds, 6: wrapping sink adds) at several NULL densities (4 columns), one thread and four, interleaved.
/// Penalty = time at that density / time of the same arm without NULLs (paired per round).
fn density(path: &str, rounds: usize) {
    let pcts = [0u32, 1, 10, 50, 100];
    let arms = [("old", Arm::Old), ("step 5", Arm::New(wengine::Opts::STEP5)), ("step 6", Arm::New(wengine::Opts::STEP6))];
    println!("Engine only, in memory; rounds: {rounds} (+1 warm-up); clock: {}\n", std::env::var("NATA_CLOCK").unwrap_or("wall".into()));
    for plan in plans() {
        let src = Source::open(path, plan.columns);
        let sets: Vec<Vec<_>> = pcts.iter().map(|&p| preload(&src, p)).collect();
        let wants: Vec<Output> = sets.iter().map(|s| run_arm(Arm::Old, plan.name, s.len(), &mem_visit(s), 4).expect("old")).collect();
        for threads in [1usize, 4] {
            let slots = arms.len() * pcts.len();
            let mut t: Vec<Vec<f64>> = vec![Vec::new(); slots];
            for round in 0..=rounds {
                let mut ms = vec![0.0f64; slots];
                for k in 0..slots {
                    let slot = (round + k) % slots;
                    let (a, p) = (slot / pcts.len(), slot % pcts.len());
                    let v = mem_visit(&sets[p]);
                    let start = now_ms();
                    let out = run_arm(arms[a].1, plan.name, sets[p].len(), &v, threads).expect("run");
                    ms[slot] = now_ms() - start;
                    assert_eq!(out, wants[p], "{} {} at {}% disagrees", plan.name, arms[a].0, pcts[p]);
                }
                if round > 0 {
                    (0..slots).for_each(|s| t[s].push(ms[s]));
                }
            }
            println!("### {}, {threads} thread(s)\n\n| arm | % NULLs | ms (min / med) | penalty vs own 0% [Q1, Q3] | vs old arm without NULLs |\n|---|---:|---|---|---|", plan.name);
            for (a, (label, _)) in arms.iter().enumerate() {
                for (p, pct) in pcts.iter().enumerate() {
                    let s = a * pcts.len() + p;
                    let own: Vec<f64> = t[s].iter().zip(&t[a * pcts.len()]).map(|(x, z)| x / z).collect();
                    let vs_old0: Vec<f64> = t[s].iter().zip(&t[0]).map(|(x, z)| x / z).collect();
                    println!("| {label} | {pct} | {} | {} | {:.2} |", mm1(&mut t[s].clone()), ratio_cell(&mut own.clone()), quantile(&mut vs_old0.clone(), 0.5));
                }
            }
            println!();
        }
    }
}

fn mm1(v: &mut Vec<f64>) -> String {
    format!("{:.1} / {:.1}", quantile(v, 0.0), quantile(v, 0.5))
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
                for (label, arm) in all_arms() {
                    for threads in [1usize, 4] {
                        let got = run_arm(arm, plan.name, src.row_groups, &v, threads).expect("arm");
                        assert_eq!(got, generic, "{} step '{label}' at {threads} threads disagrees with the generic-ops engine", plan.name);
                    }
                }
                println!("ok   {}: generic-ops, hand-fused and all {} word-wise steps agree at 1 and 4 threads", plan.name, STEPS.len());
                println!("== {}\n{}", plan.name, generic.csv());
            }
        }
        "nullcheck" => {
            // Densities that were once missing (1, 3, 5, 25, 75, 95%) next to the original 10, 50, 100%, at the default
            // batch size; and two batch sizes that are not multiples of 64 (partial last words in every batch) for
            // the densities where a tail error would show.
            let cases: Vec<(usize, Vec<u32>)> = vec![
                (source::BATCH_ROWS, vec![1, 3, 5, 10, 25, 50, 75, 95, 100]),
                (1000, vec![5, 50]),
                (97, vec![5, 50]),
            ];
            for plan in plans() {
                for (batch_rows, pcts) in &cases {
                    let src = Source::open(path, plan.columns).with_batch_rows(*batch_rows);
                    for &pct in pcts {
                        let v = parquet_visit(&src, pct);
                        let got = run(src.row_groups, &v, &plan, 4).expect("generic");
                        let want = if plan.name == "q1" { nulls::reference_q1(&src, pct) } else { nulls::reference_q6(&src, pct) };
                        assert_eq!(got, want, "{} with {pct}% NULLs, {batch_rows}-row batches, disagrees with the scalar reference", plan.name);
                        for (label, arm) in all_arms() {
                            for threads in [1usize, 4] {
                                let got = run_arm(arm, plan.name, src.row_groups, &v, threads).expect("arm");
                                assert_eq!(got, want, "{} with {pct}% NULLs, {batch_rows}-row batches, step '{label}', {threads} threads disagrees with the scalar reference", plan.name);
                            }
                        }
                        println!("ok   {} with {pct:>3}% NULLs, {batch_rows:>4}-row batches, old engine and {} steps x 1,4 threads equal the scalar reference ({})", plan.name, STEPS.len(), got.csv().lines().next().unwrap_or("").chars().take(40).collect::<String>());
                    }
                }
            }
        }
        "loop" => {
            let which = args.get(4).map_or("q1", String::as_str);
            let plan = if which == "q6" { q6() } else { q1() };
            let src = Source::open(path, plan.columns);
            let batches = preload(&src, 0);
            let v = mem_visit(&batches);
            for _ in 0..n {
                std::hint::black_box(run(batches.len(), &v, &plan, 1).expect("run"));
            }
        }
        "ladder" => ladder(path, n),
        "density" => density(path, n),
        // stages <file> <passes> <q1|q6> <step 1..5>: per-stage time for 0% and 10% NULLs, min over passes, interleaved
        "stages" => {
            let which = args.get(4).map_or("q1", String::as_str);
            let step = args.get(5).map_or("6", String::as_str);
            let mut opts = step_named(step);
            if let Some(d) = std::env::var("NATA_DENSE_DIV").ok().and_then(|s| s.parse().ok()) {
                opts.dense_div = d;
            }
            let plan = if which == "q6" { wengine::q6() } else { wengine::q1() };
            let src = Source::open(path, plan.columns);
            let sets = [(0u32, preload(&src, 0)), (10, preload(&src, 10)), (50, preload(&src, 50)), (100, preload(&src, 100))];
            let mut best: Vec<Vec<f64>> = Vec::new();
            for _ in 0..n {
                for (k, (_, b)) in sets.iter().enumerate() {
                    let r = wengine::run_staged(b, &plan, opts);
                    if best.len() <= k {
                        best.push(r.iter().map(|x| x.1).collect());
                    }
                    for (i, x) in r.iter().enumerate() {
                        best[k][i] = best[k][i].min(x.1);
                    }
                }
            }
            let names: Vec<String> = wengine::run_staged(&sets[0].1[..1], &plan, opts).into_iter().map(|x| x.0).collect();
            println!("{which}, step {step}, one thread, min over {n} passes, ms\n\n| stage | 0% NULLs | 10% | 50% | 100% |\n|---|---|---|---|---|");
            for (i, name) in names.iter().enumerate() {
                println!("| {name} | {:.1} | {:.1} | {:.1} | {:.1} |", best[0][i], best[1][i], best[2][i], best[3][i]);
            }
            println!("| total | {:.1} | {:.1} | {:.1} | {:.1} |", best[0].iter().sum::<f64>(), best[1].iter().sum::<f64>(), best[2].iter().sum::<f64>(), best[3].iter().sum::<f64>());
        }
        // wloop <file> <passes> <q1|q6> <old|1|2|3|4> <pct NULLs>: one thread, in memory, for `perf record`
        "wloop" => {
            let which = args.get(4).map_or("q1", String::as_str);
            let arm = match args.get(5).map_or("old", String::as_str) {
                "old" => Arm::Old,
                k => Arm::New(step_named(k)),
            };
            let pct: u32 = args.get(6).map_or(0, |s| s.parse().expect("pct"));
            let plan = if which == "q6" { q6() } else { q1() };
            let src = Source::open(path, plan.columns);
            let batches = preload(&src, pct);
            let v = mem_visit(&batches);
            for _ in 0..n {
                std::hint::black_box(run_arm(arm, which, batches.len(), &v, 1).expect("run"));
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
