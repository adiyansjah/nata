//! The benchmark: cells of (kernel, chain length, batch size, threads) for the linear chain and (kernel, batch size,
//! exchange capacity) for the branching topology; in every cell all arms are run interleaved, round after round, the
//! starting arm rotating every round; every run's checksum is compared with the scalar reference BEFORE its time is used.
//!
//! Reported per cell and arm: median wall nanoseconds per row, nanoseconds per batch per worker thread
//! (`wall * threads / batches`), and the overhead against the control, round-paired: `(arm - control) / control` with
//! the quartiles over the rounds, plus the same difference in ns per batch. A cell is flagged when the interquartile
//! range of the overhead is wider than the overhead itself.

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use std::time::Instant;

use crate::arms::*;
use crate::branch::*;
use crate::kernels::*;
use crate::stats::*;
use crate::sys;

pub struct Opts {
    pub rounds: usize,
    pub total_log2: u32,
    pub csv: Option<String>,
    pub smoke: bool,
    pub selftest_only: bool,
    pub ignore_selftest: bool,
    /// A FOCUSED run for an investigation (default: the whole grid). Each narrows the linear-chain grid; `--no-branch` skips the branching section.
    /// A narrowed run is a probe of one cell, not a replacement for the full grid.
    pub threads: Option<Vec<usize>>,
    pub lens: Option<Vec<usize>>,
    pub batches: Option<Vec<usize>>,
    pub kernels: Option<Vec<String>>,
    pub no_branch: bool,
}

impl Opts {
    pub fn parse(args: &[String]) -> Result<Opts, String> {
        let mut o = Opts { rounds: 25, total_log2: 25, csv: None, smoke: false, selftest_only: false, ignore_selftest: false, threads: None, lens: None, batches: None, kernels: None, no_branch: false };
        let list = |v: String, what: &str| -> Result<Vec<usize>, String> { v.split(',').map(|x| x.trim().parse::<usize>().map_err(|e| format!("{what}: {e}"))).collect() };
        let mut i = 0;
        while i < args.len() {
            let take = |i: &mut usize| -> Result<String, String> {
                *i += 1;
                args.get(*i).cloned().ok_or_else(|| format!("{} needs a value", args[*i - 1]))
            };
            match args[i].as_str() {
                "--rounds" => o.rounds = take(&mut i)?.parse().map_err(|e| format!("--rounds: {e}"))?,
                "--total-log2" => o.total_log2 = take(&mut i)?.parse().map_err(|e| format!("--total-log2: {e}"))?,
                "--csv" => o.csv = Some(take(&mut i)?),
                "--smoke" => o.smoke = true,
                "--selftest-only" => o.selftest_only = true,
                "--ignore-selftest" => o.ignore_selftest = true,
                "--threads" => o.threads = Some(list(take(&mut i)?, "--threads")?),
                "--lens" => o.lens = Some(list(take(&mut i)?, "--lens")?),
                "--batches" => o.batches = Some(list(take(&mut i)?, "--batches")?),
                "--kernels" => o.kernels = Some(take(&mut i)?.split(',').map(|x| x.trim().to_string()).collect()),
                "--no-branch" => o.no_branch = true,
                other => return Err(format!("unknown argument {other}")),
            }
            i += 1;
        }
        if o.smoke {
            o.rounds = o.rounds.min(3);
            o.total_log2 = o.total_log2.min(18);
        }
        if o.rounds < 3 {
            return Err("--rounds must be at least 3".into());
        }
        if !(16..=32).contains(&o.total_log2) {
            return Err("--total-log2 must be between 16 and 32 (it must be a multiple of the largest batch, 65536)".into());
        }
        Ok(o)
    }
}

/// Run every arm once per round, `rounds` rounds after one discarded warm-up round, the arm that goes first rotating every
/// round. Every result is compared with `want` right after its run, before its time is kept. Returns `times[arm][round]`
/// in seconds.
pub fn interleave(names: &[String], rounds: usize, want: Acc, mut run: impl FnMut(usize) -> Acc) -> Vec<Vec<f64>> {
    let n = names.len();
    let mut times = vec![Vec::with_capacity(rounds); n];
    for round in 0..=rounds {
        for k in 0..n {
            let a = (k + round) % n;
            let t0 = Instant::now();
            let got = run(a);
            let dt = t0.elapsed().as_secs_f64();
            assert_eq!(got, want, "WRONG RESULT in arm `{}` (round {round}): a fast wrong answer is not a data point", names[a]);
            if round > 0 {
                times[a].push(dt);
            }
        }
    }
    times
}

struct Row {
    name: String,
    ns_row: f64,
    ns_batch: f64,
    overhead: Option<Summary>,
    delta: Option<Summary>,
}

fn analyse(names: &[String], times: &[Vec<f64>], threads: usize, total_rows: usize, morsels: usize) -> Vec<Row> {
    (0..names.len())
        .map(|a| {
            let med = summarize(&times[a]).median;
            let (overhead, delta) = if a == 0 {
                (None, None)
            } else {
                (
                    Some(summarize(&paired_overhead_pct(&times[a], &times[0]))),
                    Some(summarize(&paired_delta_ns_per_batch(&times[a], &times[0], threads, morsels))),
                )
            };
            Row {
                name: names[a].clone(),
                ns_row: med * 1e9 / total_rows as f64,
                ns_batch: med * 1e9 * threads as f64 / morsels as f64,
                overhead,
                delta,
            }
        })
        .collect()
}

fn fmt_s(s: &Summary, prec: usize) -> String {
    format!("{:+.p$} [{:+.p$}, {:+.p$}]", s.median, s.q1, s.q3, p = prec)
}

fn print_rows(rows: &[Row]) {
    println!("| arm | ns/row (median) | ns/batch/worker (median) | overhead % vs control: median [q1, q3] | difference ns/batch/worker: median [q1, q3] | verdict |");
    println!("|---|---|---|---|---|---|");
    for r in rows {
        match (&r.overhead, &r.delta) {
            (Some(o), Some(d)) => println!(
                "| {} | {:.3} | {:.0} | {} | {} | {} |",
                r.name,
                r.ns_row,
                r.ns_batch,
                fmt_s(o, 1),
                fmt_s(d, 0),
                verdict(o)
            ),
            _ => println!("| {} (control) | {:.3} | {:.0} | - | - | - |", r.name, r.ns_row, r.ns_batch),
        }
    }
    println!();
}

fn cell_status(load_start: f64, l0: f64, l1: f64, foreign: f64, threads: usize) -> (String, bool) {
    let oversubscribed = threads > sys::ncpu();
    let label = if oversubscribed { "SANITY ONLY" } else { sys::label(load_start, foreign) };
    let quiet = label == "quiet";
    let note = if oversubscribed { format!(" ({} threads on {} CPUs)", threads, sys::ncpu()) } else { String::new() };
    (
        format!(
            "load1 {l0:.2} -> {l1:.2} (includes this benchmark's own {threads} threads), other processes used {foreign:.2} cores, run started at load1 {load_start:.2}: **{label}**{note}"
        ),
        quiet,
    )
}

struct Csv(Option<std::io::BufWriter<std::fs::File>>);

impl Csv {
    fn open(path: &Option<String>) -> Csv {
        Csv(path.as_ref().map(|p| {
            let mut w = std::io::BufWriter::new(std::fs::File::create(p).expect("cannot create the csv file"));
            writeln!(w, "section,kernel,len,batch,threads,cap,round,arm,seconds").unwrap();
            w
        }))
    }

    #[allow(clippy::too_many_arguments)]
    fn cell(&mut self, section: &str, kernel: &str, len: usize, batch: usize, threads: usize, cap: usize, names: &[String], times: &[Vec<f64>]) {
        if let Some(w) = &mut self.0 {
            for (a, t) in times.iter().enumerate() {
                for (r, s) in t.iter().enumerate() {
                    writeln!(w, "{section},{kernel},{len},{batch},{threads},{cap},{},{},{s:.9}", r + 1, names[a]).unwrap();
                }
            }
            w.flush().unwrap();
        }
    }
}

struct SummaryLine {
    cell: String,
    ctl_ns_row: f64,
    /// (arm name, overhead summary) for the reported arms, then the twin
    arms: Vec<(String, Summary)>,
    twin: Summary,
    quiet: bool,
}

pub fn run(o: &Opts) -> i32 {
    let ncpu = sys::ncpu();
    let load_start = sys::load1();
    let total_rows = 1usize << o.total_log2;
    println!("# push-cost benchmark\n");
    println!(
        "- arch `{}`, {ncpu} CPUs visible, rounds {} (after 1 discarded warm-up round), rows per run 2^{} = {total_rows}{}",
        std::env::consts::ARCH,
        o.rounds,
        o.total_log2,
        if o.smoke { " (SMOKE RUN: mechanics only, no numbers)" } else { "" }
    );
    println!("- 1-minute load average at start, before any benchmark thread ran: **{load_start:.2}**");
    if !(load_start < sys::QUIET_LOAD_LIMIT) {
        println!("- **SANITY ONLY: the load average at start is >= {} (or unknown). No number below may be called quiet.**", sys::QUIET_LOAD_LIMIT);
    }
    if o.smoke {
        println!("- **SANITY ONLY: smoke run.**");
    }
    println!();

    // "validated" = the harness self-test ran and passed; without it no cell may be called quiet
    let mut validated = false;
    if !o.smoke {
        let ok = crate::selftest::run(o.rounds.max(15), total_rows, ncpu);
        if !ok && !o.ignore_selftest {
            println!("HARNESS SELF-TEST FAILED: stopping before the benchmark (pass --ignore-selftest to run anyway; the numbers would then be unvalidated).");
            return 1;
        }
        if !ok {
            println!("**HARNESS NOT VALIDATED (self-test failed, --ignore-selftest given): treat every number below as SANITY ONLY.**\n");
        }
        validated = ok;
    }
    if o.selftest_only {
        return 0;
    }
    let mut csv = Csv::open(&o.csv);

    fixed_cost(ncpu);

    // ------------------------------------------------------------ the linear chain
    println!("## Linear chain: control vs static vs dyn\n");
    println!("Arms: see `src/arms.rs`. Overhead = (arm - control) / control per round, then median [q1, q3] over the rounds.\n");
    let (kernels, lens, batches, threads_list): (Vec<Kind>, Vec<usize>, Vec<usize>, Vec<usize>) = if o.smoke {
        (Kind::ALL.to_vec(), vec![1, 3], vec![1024], vec![1, 4])
    } else {
        (Kind::ALL.to_vec(), vec![1, 3, 6], vec![1024, 8192, 65536], vec![1, 4])
    };
    let kernels: Vec<Kind> = match &o.kernels {
        Some(want) => {
            let k: Vec<Kind> = kernels.into_iter().filter(|k| want.iter().any(|w| w == k.name())).collect();
            assert!(!k.is_empty(), "--kernels matched no kernel (cheap, medium)");
            k
        }
        None => kernels,
    };
    let lens = o.lens.clone().unwrap_or(lens);
    let batches = o.batches.clone().unwrap_or(batches);
    let threads_list = o.threads.clone().unwrap_or(threads_list);
    if o.threads.is_some() || o.lens.is_some() || o.batches.is_some() || o.kernels.is_some() || o.no_branch {
        println!("**FOCUSED RUN** (narrowed grid: threads {threads_list:?}, stages {lens:?}, rows per batch {batches:?}, kernels {:?}, branching {}): a probe of those cells, not the full grid.\n", kernels.iter().map(|k| k.name()).collect::<Vec<_>>(), if o.no_branch { "skipped" } else { "run" });
    }
    let names: Vec<String> = CHAIN_ARMS.iter().map(|a| a.name()).collect();
    let mut lines: Vec<SummaryLine> = Vec::new();
    let mut refs: HashMap<(usize, usize, usize), Acc> = HashMap::new();
    for (ki, &kind) in kernels.iter().enumerate() {
        for &len in &lens {
            for &batch in &batches {
                let tmpl = Arc::new(template(batch));
                let morsels = total_rows / batch;
                let want = *refs.entry((ki, len, batch)).or_insert_with(|| reference(kind, len, batch, morsels, &tmpl));
                for &threads in &threads_list {
                    let cell = Cell { kind, len, batch, threads, total_rows };
                    let (l0, s0) = (sys::load1(), sys::snap());
                    let times = interleave(&names, o.rounds, want, |a| run_chain(CHAIN_ARMS[a], &cell, &tmpl));
                    let (l1, s1) = (sys::load1(), sys::snap());
                    let foreign = sys::foreign_cores(s0, s1);
                    let (status, quiet) = cell_status(load_start, l0, l1, foreign, threads);
                    println!("### {} kernel, {len} stage(s), {batch} rows/batch, {threads} thread(s)\n", kind.name());
                    println!("{status}\n");
                    println!("checksum (all {} arms identical, equal to the scalar reference): sum {} count {}\n", names.len(), want.sum, want.count);
                    let rows = analyse(&names, &times, threads, total_rows, morsels);
                    print_rows(&rows);
                    csv.cell("chain", kind.name(), len, batch, threads, 0, &names, &times);
                    let tag = format!("{} L{len} B{batch} T{threads}", kind.name());
                    lines.push(SummaryLine {
                        cell: tag,
                        ctl_ns_row: rows[0].ns_row,
                        arms: [2usize, 3, 4, 5].iter().map(|&a| (names[a].clone(), rows[a].overhead.unwrap())).collect(),
                        twin: rows[1].overhead.unwrap(),
                        quiet: quiet && validated,
                    });
                }
            }
        }
    }

    // ------------------------------------------------------------ the branching topology
    println!("## Branching: 1 producer -> exchange -> 2 consumers, on {BRANCH_THREADS} threads\n");
    println!("`hand` = 3 OS threads and a Mutex/Condvar bounded queue calling the same kernels; `proto` = the real scheduler and exchange of spikes/operator-trait. Control = `hand`.\n");
    let bnames: Vec<String> = BRANCH_ARMS.iter().map(|a| a.name().to_string()).collect();
    let (bkernels, bbatches, caps): (Vec<Kind>, Vec<usize>, Vec<usize>) = if o.no_branch {
        (vec![], vec![], vec![])
    } else if o.smoke {
        (Kind::ALL.to_vec(), vec![1024], vec![1, 4])
    } else {
        (Kind::ALL.to_vec(), vec![1024, 8192, 65536], vec![1, 4])
    };
    let mut blines: Vec<(String, f64, Summary, Summary, bool)> = Vec::new();
    for &kind in &bkernels {
        for &batch in &bbatches {
            let tmpl = Arc::new(template(batch));
            let morsels = total_rows / batch;
            let want = reference(kind, BRANCH_LEN, batch, morsels, &tmpl);
            for &cap in &caps {
                let cell = BCell { kind, batch, cap, total_rows };
                let (l0, s0) = (sys::load1(), sys::snap());
                let times = interleave(&bnames, o.rounds, want, |a| run_branch(BRANCH_ARMS[a], &cell, &tmpl));
                let (l1, s1) = (sys::load1(), sys::snap());
                let foreign = sys::foreign_cores(s0, s1);
                let (status, quiet) = cell_status(load_start, l0, l1, foreign, BRANCH_THREADS);
                println!("### branching, {} kernel, {batch} rows/batch, exchange capacity {cap}\n", kind.name());
                println!("{status}\n");
                println!("checksum (all arms identical, equal to the scalar reference of a 3-stage chain): sum {} count {}\n", want.sum, want.count);
                let rows = analyse(&bnames, &times, BRANCH_THREADS, total_rows, morsels);
                print_rows(&rows);
                csv.cell("branch", kind.name(), BRANCH_LEN, batch, BRANCH_THREADS, cap, &bnames, &times);
                blines.push((
                    format!("{} B{batch} cap{cap}", kind.name()),
                    rows[0].ns_row,
                    rows[2].overhead.unwrap(),
                    rows[1].overhead.unwrap(),
                    quiet && validated,
                ));
            }
        }
    }

    // ------------------------------------------------------------ one-table summary
    println!("## Summary\n");
    println!("Overhead of each arm against the hand-fused control, median of the paired per-round ratios, in percent. `?` = the interquartile range is wider than the overhead (not a result). `twin` is the control against itself: the noise floor of that cell. Cells not labelled quiet are marked `S` (SANITY ONLY).\n");
    println!("| cell | control ns/row | static | dyn-direct | dyn | static-in-sched | twin | quiet |");
    println!("|---|---|---|---|---|---|---|---|");
    for l in &lines {
        let f = |s: &Summary| format!("{:+.1}{}", s.median, if is_noisy(s) { " ?" } else { "" });
        println!(
            "| {} | {:.3} | {} | {} | {} | {} | {} | {} |",
            l.cell,
            l.ctl_ns_row,
            f(&l.arms[0].1),
            f(&l.arms[1].1),
            f(&l.arms[2].1),
            f(&l.arms[3].1),
            f(&l.twin),
            if l.quiet { "yes" } else { "S" }
        );
    }
    println!();
    println!("| branching cell | hand ns/row | proto overhead % | twin | quiet |");
    println!("|---|---|---|---|---|");
    for (cell, ns_row, proto, twin, quiet) in &blines {
        let f = |s: &Summary| format!("{:+.1}{}", s.median, if is_noisy(s) { " ?" } else { "" });
        println!("| {cell} | {ns_row:.3} | {} | {} | {} |", f(proto), f(twin), if *quiet { "yes" } else { "S" });
    }
    println!();
    let not_quiet = lines.iter().filter(|l| !l.quiet).count() + blines.iter().filter(|b| !b.4).count();
    if not_quiet == 0 {
        println!("All cells were measured with the run-start load average < {}, no other process using >= {} core, and a validated harness: **quiet**.", sys::QUIET_LOAD_LIMIT, sys::QUIET_FOREIGN_CORES);
    } else {
        println!("**SANITY ONLY: {not_quiet} cell(s) are not quiet (load, other processes, too few CPUs, smoke run or unvalidated harness).**");
    }
    0
}

/// The fixed cost of one run: every arm with ZERO morsels, so what is timed is thread start, scheduler set-up, and
/// join. It is part of every number above (about a tenth of a millisecond against runs of tens of milliseconds).
fn fixed_cost(ncpu: usize) {
    println!("## Fixed cost of one run (zero morsels)\n");
    println!("Median of 31 interleaved runs, microseconds. This is included in every timing below.\n");
    println!("| threads | control | static | dyn-direct | dyn | static-in-sched |");
    println!("|---|---|---|---|---|---|");
    let arms = [Arm::Control, Arm::Static, Arm::DynDirect, Arm::Dyn, Arm::StaticInSched];
    let names: Vec<String> = arms.iter().map(|a| a.name()).collect();
    let tmpl = Arc::new(template(1024));
    for threads in [1usize, 4] {
        if threads > ncpu {
            continue;
        }
        let cell = Cell { kind: Kind::Cheap, len: 3, batch: 1024, threads, total_rows: 0 };
        let times = interleave(&names, 31, Acc::default(), |a| run_chain(arms[a], &cell, &tmpl));
        let med: Vec<String> = times.iter().map(|t| format!("{:.0}", summarize(t).median * 1e6)).collect();
        println!("| {threads} | {} |", med.join(" | "));
    }
    println!();
}
