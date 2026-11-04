//! hash-ops: modes `check`, `nullcheck`, `bench`, `membench`, `rss`, `loop`.
use std::time::Instant;

use arrow::array::RecordBatch;
use hash_ops::fused::run_fused;
use hash_ops::kernels::R;
use hash_ops::out::{Digest, Groups, digest_of, output_of};
use hash_ops::queries::*;
use hash_ops::reference::{self, RefTables};
use hash_ops::source::Source;
use hash_ops::testkit::mask_columns;

type VisitBox<'a> = Box<dyn Fn(usize, &mut dyn FnMut(&RecordBatch)) + Sync + 'a>;

fn quantile(v: &mut [f64], q: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * q).round() as usize]
}

/// Peak resident set size of this process (`VmHWM` in /proc/self/status), for the memory guard.
fn peak_rss() -> String {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmHWM:"))
        .and_then(|v| v.trim().strip_suffix("kB"))
        .and_then(|v| v.trim().parse::<f64>().ok())
        .map_or("unknown".into(), |kb| format!("{:.0} MiB", kb / 1024.0))
}

fn load() -> String {
    std::fs::read_to_string("/proc/loadavg").map(|s| s.split_whitespace().take(3).collect::<Vec<_>>().join(" ")).unwrap_or_default()
}

fn all_batches(src: &Source, names: &[&str], pct: u32) -> Vec<RecordBatch> {
    let mut v = Vec::new();
    for rg in 0..src.row_groups {
        let mut offset = 0;
        for rb in src.reader(rg) {
            let rb = rb.expect("decode");
            let rb = if pct > 0 { mask_columns(&rb, names, rg, offset, pct) } else { rb };
            offset += rb.num_rows();
            v.push(rb);
        }
    }
    v
}

/// Where the batches of a table come from.
enum Mode {
    Parquet,
    Memory,
}

struct Loaded {
    sources: Vec<Source>,
    mem: Vec<Vec<RecordBatch>>,
    mode: Mode,
}

const TABLES: [Table; 3] = [Table::Lineitem, Table::Orders, Table::Customer];

impl Loaded {
    fn new(dir: &str, q: Query, mode: Mode) -> Loaded {
        let sources: Vec<Source> = TABLES.iter().map(|&t| Source::open(&format!("{dir}/{}", t.file()), q.columns(t))).collect();
        let mem = match mode {
            Mode::Memory => sources.iter().zip(TABLES).map(|(s, t)| if q.columns(t).is_empty() { Vec::new() } else { all_batches(s, &[], 0) }).collect(),
            Mode::Parquet => Vec::new(),
        };
        Loaded { sources, mem, mode }
    }

    fn visits(&self) -> Vec<(usize, VisitBox<'_>)> {
        (0..3)
            .map(|i| match self.mode {
                Mode::Parquet => {
                    let s: &Source = &self.sources[i];
                    let n = s.row_groups;
                    // SAFETY-free: the closure borrows the Source for the lifetime of `self` via a raw reference.
                    let v: VisitBox = Box::new(move |rg, f| {
                        for rb in s.reader(rg) {
                            f(&rb.expect("decode"));
                        }
                    });
                    (n, v)
                }
                Mode::Memory => {
                    let b: &Vec<RecordBatch> = &self.mem[i];
                    (b.len(), Box::new(move |m: usize, f: &mut dyn FnMut(&RecordBatch)| f(&b[m])) as VisitBox)
                }
            })
            .collect()
    }
}

fn with_inputs<T>(l: &Loaded, f: impl FnOnce(&Inputs) -> T) -> T {
    let v = l.visits();
    let inp = Inputs { lineitem: (v[0].0, &*v[0].1), orders: (v[1].0, &*v[1].1), customer: (v[2].0, &*v[2].1) };
    f(&inp)
}

fn write_lines(path: &str, lines: &[String]) {
    std::fs::write(path, lines.join("\n") + if lines.is_empty() { "" } else { "\n" }).expect("write result");
}

fn timed_engine(q: Query, inp: &Inputs, threads: usize) -> (f64, Digest) {
    let start = Instant::now();
    let r = run(q, inp, threads).expect("engine run");
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    let d = digest_of(&r);
    std::hint::black_box(&r);
    (ms, d)
}

fn timed_fused(q: Query, inp: &Inputs, threads: usize) -> (f64, Digest) {
    let start = Instant::now();
    let r: R<Box<dyn Groups>> = run_fused(q, inp, threads);
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    let r = r.expect("fused run");
    let d = digest_of(r.as_ref());
    std::hint::black_box(&r);
    (ms, d)
}

fn ratio_cell(v: &mut [f64]) -> String {
    format!("{:.2} [{:.2}, {:.2}]", quantile(v, 0.5), quantile(v, 0.25), quantile(v, 0.75))
}

fn compare(dir: &str, rounds: usize, in_memory: bool, threads_list: &[usize], queries: &[Query]) {
    println!(
        "{} \nrounds: {rounds} (+1 warm-up); arms interleaved in rotating order; min / median ms; ratios are medians of paired per-round ratios [Q1, Q3]; load average at start {}\n",
        if in_memory { "Engine only (batches already decoded, in memory)" } else { "End to end (Parquet decode included)" },
        load()
    );
    println!("| query | threads | generic ms | hand-fused ms | generic / fused | control: generic2 / generic | load (1/5/15 min) after |\n|---|---:|---|---|---|---|---|");
    for &q in queries {
        let l = Loaded::new(dir, q, if in_memory { Mode::Memory } else { Mode::Parquet });
        with_inputs(&l, |inp| {
            let want = timed_fused(q, inp, 4).1;
            assert_eq!(timed_engine(q, inp, 4).1, want, "{}: engine and fused disagree", q.name());
            for &threads in threads_list {
                let mut t: [Vec<f64>; 3] = Default::default();
                for round in 0..=rounds {
                    let mut ms = [0.0f64; 3];
                    for k in 0..3 {
                        let arm = (round + k) % 3;
                        let (m, d) = if arm == 1 { timed_fused(q, inp, threads) } else { timed_engine(q, inp, threads) };
                        ms[arm] = m;
                        if round == 0 {
                            assert_eq!(d, want, "{} arm {arm} at {threads} threads disagrees", q.name());
                        }
                    }
                    if round > 0 {
                        (0..3).for_each(|a| t[a].push(ms[a]));
                    }
                }
                let mut gf: Vec<f64> = t[0].iter().zip(&t[1]).map(|(g, f)| g / f).collect();
                let mut cg: Vec<f64> = t[2].iter().zip(&t[0]).map(|(c, g)| c / g).collect();
                let mm = |v: &mut Vec<f64>| format!("{:.0} / {:.0}", quantile(v, 0.0), quantile(v, 0.5));
                println!(
                    "| {} | {threads} | {} | {} | {} | {} | {} |",
                    q.name(),
                    mm(&mut t[0]),
                    mm(&mut t[1]),
                    ratio_cell(&mut gf),
                    ratio_cell(&mut cg),
                    load()
                );
            }
        });
    }
}

/// Prefix of the row groups, so the row-by-row reference stays small enough in memory.
fn prefix_batches(src: &Source, names: &[&str], pct: u32, row_groups: usize) -> Vec<RecordBatch> {
    let mut v = Vec::new();
    for rg in 0..src.row_groups.min(row_groups) {
        let mut offset = 0;
        for rb in src.reader(rg) {
            let rb = rb.expect("decode");
            let rb = if pct > 0 { mask_columns(&rb, names, rg, offset, pct) } else { rb };
            offset += rb.num_rows();
            v.push(rb);
        }
    }
    v
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map_or("", String::as_str);
    let dir = args.get(2).map_or("", String::as_str);
    let queries_from = |i: usize| -> Vec<Query> {
        if args.len() > i { args[i..].iter().map(|s| Query::parse(s).unwrap_or_else(|| panic!("unknown query {s}"))).collect() } else { ALL.to_vec() }
    };
    match mode {
        "check" => {
            // check <dir> <outdir> [queries]: write the engine's result lines for the diff against DuckDB.
            let out = args.get(3).expect("outdir");
            for q in queries_from(4) {
                let l = Loaded::new(dir, q, Mode::Parquet);
                with_inputs(&l, |inp| {
                    let e4 = run(q, inp, 4).expect("engine 4");
                    let e1 = run(q, inp, 1).expect("engine 1");
                    let e8 = run(q, inp, 8).expect("engine 8");
                    let (o4, o1, o8) = (output_of(&e4, q.scales(), q.show_count()), output_of(&e1, q.scales(), q.show_count()), output_of(&e8, q.scales(), q.show_count()));
                    assert_eq!(o4.rows, o1.rows, "{}: result depends on the thread count (4 vs 1)", q.name());
                    assert_eq!(o4.rows, o8.rows, "{}: result depends on the thread count (4 vs 8)", q.name());
                    let f4 = run_fused(q, inp, 4).expect("fused");
                    assert_eq!(output_of(f4.as_ref(), q.scales(), q.show_count()).rows, o4.rows, "{}: engine and fused disagree", q.name());
                    for (t, o) in [(1, &o1), (4, &o4), (8, &o8)] {
                        write_lines(&format!("{out}/{}.engine.t{t}.txt", q.name()), &o.lines());
                    }
                    println!("{}: {} groups; identical at 1, 4, 8 threads and to the fused baseline", q.name(), o4.rows.len());
                });
            }
        }
        "nullcheck" => {
            // nullcheck <dir> [queries]: every column the query reads gets pct% NULLs; engine against the scalar reference.
            for q in queries_from(3) {
                for pct in [10u32, 50, 100] {
                    let open = |t: Table| Source::open(&format!("{dir}/{}", t.file()), q.columns(t));
                    let names = |t: Table| q.columns(t).to_vec();
                    let mk = |t: Table, rgs: usize| {
                        if q.columns(t).is_empty() { Vec::new() } else { prefix_batches(&open(t), &names(t), pct, rgs) }
                    };
                    let t = RefTables { lineitem: mk(Table::Lineitem, 12), orders: mk(Table::Orders, 3), customer: mk(Table::Customer, 2) };
                    let want = reference::query(q, &t);
                    let visit = |b: &Vec<RecordBatch>| -> VisitBox<'static> {
                        let b = b.clone();
                        Box::new(move |m, f| f(&b[m]))
                    };
                    let (vl, vo, vc) = (visit(&t.lineitem), visit(&t.orders), visit(&t.customer));
                    let inp = Inputs { lineitem: (t.lineitem.len(), &*vl), orders: (t.orders.len(), &*vo), customer: (t.customer.len(), &*vc) };
                    for threads in [1usize, 4] {
                        let got = output_of(&run(q, &inp, threads).expect("engine"), q.scales(), q.show_count());
                        assert_eq!(got.rows, want.rows, "{} with {pct}% NULLs at {threads} threads disagrees with the scalar reference", q.name());
                    }
                    println!("ok   {:<13} {pct:>3}% NULLs: equals the scalar reference at 1 and 4 threads ({} groups)", q.name(), want.rows.len());
                }
            }
        }
        "bench" | "membench" => {
            let rounds: usize = args.get(3).map_or(25, |s| s.parse().expect("rounds"));
            let threads: Vec<usize> = args.get(4).map_or(vec![1, 2, 4, 8], |s| s.split(',').map(|x| x.parse().expect("threads")).collect());
            compare(dir, rounds, mode == "membench", &threads, &queries_from(5));
        }
        "rss" => {
            // rss <dir> <query> <engine|fused> <threads> [mem]: one run, for /usr/bin/time -v
            let q = Query::parse(args.get(3).expect("query")).expect("known query");
            let arm = args.get(4).map_or("engine", String::as_str);
            let threads: usize = args.get(5).map_or(4, |s| s.parse().expect("threads"));
            let l = Loaded::new(dir, q, if args.get(6).is_some() { Mode::Memory } else { Mode::Parquet });
            with_inputs(&l, |inp| {
                let (ms, d) = if arm == "fused" { timed_fused(q, inp, threads) } else { timed_engine(q, inp, threads) };
                println!("{} {arm} {threads} threads: {ms:.0} ms, {} groups, digest {:016x}, peak RSS {}", q.name(), d.groups, d.sum, peak_rss());
            });
        }
        "loop" => {
            // loop <dir> <query> <engine|fused> <threads> <n>: in-memory repeats for perf
            let q = Query::parse(args.get(3).expect("query")).expect("known query");
            let arm = args.get(4).map_or("engine", String::as_str);
            let threads: usize = args.get(5).map_or(1, |s| s.parse().expect("threads"));
            let n: usize = args.get(6).map_or(10, |s| s.parse().expect("n"));
            let l = Loaded::new(dir, q, Mode::Memory);
            with_inputs(&l, |inp| {
                for _ in 0..n {
                    if arm == "fused" { timed_fused(q, inp, threads) } else { timed_engine(q, inp, threads) };
                }
            });
        }
        _ => eprintln!("modes: check <dir> <outdir> [q..] | nullcheck <dir> [q..] | bench|membench <dir> [rounds] [threads,..] [q..] | rss <dir> <q> <engine|fused> <threads> [mem] | loop <dir> <q> <arm> <threads> <n>"),
    }
}
