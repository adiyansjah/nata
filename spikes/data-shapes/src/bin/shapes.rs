//! shapes: the hash aggregate and the inner hash join of spikes/hash-ops (the library crate `hash_ops`, used as it
//! is) on the data sets that `gcp-check.sh` generates with DuckDB. Every query exists twice, in one table: as an
//! engine plan and as the SQL text that DuckDB runs (`shapes sql <name>`), so the two cannot drift apart.
//!
//!   shapes list [tag]                       one line per query: name, data-set tag (which files it reads)
//!   shapes sql <name>                       the DuckDB SQL (table names are bare; the harness makes views)
//!   shapes run <dir> <name> <threads> <out> run the engine, write the result lines (sorted bytewise) to <out>
//!
//! Text form = spikes/hash-ops `Output::lines()`: key columns (NULL as NULL, DATE as days since 1970-01-01), then
//! COUNT(*), then every SUM (NULL when no input was non-NULL), joined by '|'.
//!
//! Exit status of `run`: 0 ok, 3 the engine refused with "not supported yet" (loud, by design), 4 any other engine
//! error; a panic exits 101. `SHAPES_PERTURB=count|sum|drop|key` deliberately corrupts the result before it is
//! written: the harness uses it to show that a difference IS reported (self-test).
use std::sync::Arc;
use std::time::Instant;

use arrow::array::RecordBatch;
use hash_ops::agg::{HashAgg, merge_all};
use hash_ops::engine::{FilterDec, FilterStrIn, Op, Ref, Visit, run_pipeline};
use hash_ops::join::{JoinBuild, Probe, build_table};
use hash_ops::kernels::R;
use hash_ops::out::{Groups, KVal, Output, output_of};
use hash_ops::source::Source;

#[path = "../qtable.rs"]
mod qtable;
use qtable::{Flt, Shape, S, queries, sql};

fn filter_op(f: &Flt) -> Box<dyn Op> {
    match *f {
        Flt::Between(col, lo, hi) => Box::new(FilterDec { col, lo, hi }),
        Flt::In(col, set) => Box::new(FilterStrIn { col, set }),
    }
}

/// A table on disk: every column the plan touches is projected, nothing else is decoded.
struct Scan {
    src: Source,
}

impl Scan {
    fn open(dir: &str, table: S, cols: &[S]) -> Scan {
        let mut c: Vec<S> = cols.to_vec();
        c.sort_unstable();
        c.dedup();
        Scan { src: Source::open(&format!("{dir}/{table}.parquet"), &c) }
    }

    fn visit(&self) -> impl Fn(usize, &mut dyn FnMut(&RecordBatch)) + Sync + '_ {
        move |m, f| {
            for rb in self.src.reader(m) {
                f(&rb.expect("decode"));
            }
        }
    }
}

fn ops(filters: &[Flt]) -> Vec<Box<dyn Op>> {
    filters.iter().map(filter_op).collect()
}

fn run_shape(dir: &str, s: &Shape, threads: usize) -> R<Vec<HashAgg>> {
    match s {
        Shape::Agg { t, keys, sums } => {
            let mut cols: Vec<S> = t.filters.iter().map(Flt::col).collect();
            cols.extend(keys.iter().map(|k| k.col));
            cols.extend(sums);
            let scan = Scan::open(dir, t.table, &cols);
            let v = scan.visit();
            let (kn, refs): (Vec<S>, Vec<Ref>) = (keys.iter().map(|k| k.col).collect(), sums.iter().map(|&c| Ref::Col(c)).collect());
            let locals = run_pipeline(scan.src.row_groups, &v as Visit, threads, &|| ops(&t.filters), &|| HashAgg::new(kn.clone(), refs.clone()))?;
            merge_all(locals, threads)
        }
        Shape::Join { build, bkey, bpay, probe, pkey, pout, keys, sums } => {
            let mut bcols: Vec<S> = build.filters.iter().map(Flt::col).collect();
            bcols.push(bkey);
            bcols.extend(bpay);
            let bscan = Scan::open(dir, build.table, &bcols);
            let bv = bscan.visit();
            let sinks = run_pipeline(bscan.src.row_groups, &bv as Visit, threads, &|| ops(&build.filters), &|| JoinBuild::new(vec![*bkey], bpay.clone()))?;
            let table = Arc::new(build_table(sinks, threads)?);
            let mut pcols: Vec<S> = probe.filters.iter().map(Flt::col).collect();
            pcols.push(pkey);
            pcols.extend(pout);
            let pscan = Scan::open(dir, probe.table, &pcols);
            let pv = pscan.visit();
            let (kn, refs): (Vec<S>, Vec<Ref>) = (keys.iter().map(|k| k.col).collect(), sums.iter().map(|&c| Ref::Col(c)).collect());
            let locals = run_pipeline(
                pscan.src.row_groups,
                &pv as Visit,
                threads,
                &|| {
                    let mut o = ops(&probe.filters);
                    o.push(Box::new(Probe::new(table.clone(), vec![*pkey], pout.clone())));
                    o
                },
                &|| HashAgg::new(kn.clone(), refs.clone()),
            )?;
            merge_all(locals, threads)
        }
    }
}

fn peak_rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("VmHWM:"))
        .and_then(|v| v.trim().strip_suffix("kB"))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0)
}

/// Deliberate corruption of a result, for the harness self-test.
fn perturb(o: &mut Output, kind: &str) {
    match kind {
        "" => {}
        "count" => {
            if let Some(g) = o.rows.values_mut().next() {
                g.count += 1;
            }
        }
        "sum" => {
            if let Some(g) = o.rows.values_mut().next().filter(|g| !g.sums.is_empty()) {
                g.sums[0].0 += 1;
                g.sums[0].1 = g.sums[0].1.max(1);
            }
        }
        "drop" => {
            o.rows.pop_last();
        }
        "key" => {
            if let Some((mut k, g)) = o.rows.pop_first() {
                match &mut k[0] {
                    KVal::I(v) => *v += 1,
                    KVal::S(s) => s.push('x'),
                    n @ KVal::Null => *n = KVal::I(7),
                }
                o.rows.insert(k, g);
            }
        }
        other => panic!("unknown SHAPES_PERTURB {other}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let all = queries();
    let find = |name: &str| all.iter().find(|q| q.name == name).unwrap_or_else(|| panic!("unknown query {name}"));
    match args.get(1).map(String::as_str) {
        Some("list") => {
            for q in all.iter().filter(|q| args.get(2).is_none_or(|t| *t == q.tag)) {
                println!("{}\t{}\tok", q.name, q.tag);
            }
        }
        Some("sql") => println!("{}", sql(&find(args.get(2).expect("query name")).shape)),
        Some("run") => {
            let (dir, name) = (args.get(2).expect("dir"), args.get(3).expect("query"));
            let threads: usize = args.get(4).expect("threads").parse().expect("threads");
            let out_path = args.get(5).expect("outfile");
            let q = find(name);
            let nsums = match &q.shape {
                Shape::Agg { sums, .. } | Shape::Join { sums, .. } => sums.len(),
            };
            let t0 = Instant::now();
            let groups = match run_shape(dir, &q.shape, threads) {
                Ok(g) => g,
                Err(e) => {
                    println!("ENGINE-ERROR {}", e.0);
                    std::process::exit(if e.0.contains("not supported yet") { 3 } else { 4 });
                }
            };
            let engine_ms = t0.elapsed().as_secs_f64() * 1000.0;
            let rss_engine = peak_rss_kb();
            let mut out = output_of(&groups as &dyn Groups, vec![0; nsums], true);
            let out_rows: u128 = out.rows.values().map(|g| g.count as u128).sum();
            perturb(&mut out, &std::env::var("SHAPES_PERTURB").unwrap_or_default());
            let lines = out.lines();
            let mut text = lines.join("\n");
            if !lines.is_empty() {
                text.push('\n');
            }
            std::fs::write(out_path, text).expect("write result");
            println!("engine_ms={engine_ms:.1} groups={} out_rows={out_rows} rss_engine_kb={rss_engine} rss_total_kb={}", lines.len(), peak_rss_kb());
        }
        _ => eprintln!("usage: shapes list [tag] | sql <name> | run <dir> <name> <threads> <outfile>"),
    }
}
