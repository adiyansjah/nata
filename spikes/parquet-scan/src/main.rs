//! `pscan`: the baseline spike of docs/slices/0001-parquet-scan.md.
//!
//!   pscan gate <lineitem.parquet>                          every arm must return what DuckDB's SQL returns (no timing)
//!   pscan r1 <lineitem.parquet> [--rounds N]               R1: parquet-rs (plain / engine types / twin) against DuckDB, ONE thread, two projections
//!   pscan r3 <big.parquet> [--rounds N]                    R3: morsel = row group or a quarter of one, 1 to 8 threads
//!   pscan mem <big.parquet> <rg|quarter> <threads>         peak resident memory of one arm (one process per arm)
//!   pscan mkpageindex <in.parquet> <out.parquet>           rewrite a file with 4 row groups and a Parquet page index (parquet-rs writer)
//! Every timed run is checked against the reference result before its time is kept.

use std::process::ExitCode;
use std::time::Instant;

use pscan_lib::*;

const Q1: [&str; 7] = ["l_returnflag", "l_linestatus", "l_quantity", "l_extendedprice", "l_discount", "l_tax", "l_shipdate"];
const Q6: [&str; 4] = ["l_shipdate", "l_discount", "l_quantity", "l_extendedprice"];

fn load1() -> f64 {
    std::fs::read_to_string("/proc/loadavg").ok().and_then(|s| s.split_whitespace().next().and_then(|x| x.parse().ok())).unwrap_or(f64::NAN)
}

fn status_kb(key: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status").unwrap().lines().find(|l| l.starts_with(key)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse().ok()).unwrap_or(0)
}

fn quantile(v: &mut [f64], q: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * q).round() as usize]
}

/// Every arm once per round, the arm that goes first rotating; one discarded warm-up round. Returns times[arm][round] in seconds.
fn interleave(n: usize, rounds: usize, mut run: impl FnMut(usize) -> f64) -> Vec<Vec<f64>> {
    let mut t = vec![Vec::new(); n];
    for r in 0..=rounds {
        for k in 0..n {
            let a = (r + k) % n;
            let s = run(a);
            if r > 0 {
                t[a].push(s);
            }
        }
    }
    t
}

fn cell(v: &[f64]) -> String {
    let mut x = v.to_vec();
    format!("{:.1} [{:.1}, {:.1}]", quantile(&mut x, 0.5) * 1000.0, quantile(&mut x, 0.25) * 1000.0, quantile(&mut x, 0.75) * 1000.0)
}

/// median [q1, q3] of the per-round ratio num/den
fn ratio(num: &[f64], den: &[f64]) -> (f64, f64, f64) {
    let mut r: Vec<f64> = num.iter().zip(den).map(|(a, b)| a / b).collect();
    (quantile(&mut r, 0.5), quantile(&mut r, 0.25), quantile(&mut r, 0.75))
}

fn args_rounds(a: &[String]) -> usize {
    a.iter().position(|x| x == "--rounds").and_then(|i| a.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(25)
}

fn reference(path: &str, cols: &[&str]) -> Result<Acc, String> {
    let src = Src::open(path, cols, Types::Engine);
    let acc = scan_all(&src);
    let (line, _) = duck_run(&duck_sql(path, cols))?;
    if line != acc_line(&acc) {
        return Err(format!("DuckDB says {line}, the scan says {}", acc_line(&acc)));
    }
    Ok(acc)
}

fn gate(path: &str) -> bool {
    let mut ok = true;
    for (name, cols) in [("q1", &Q1[..]), ("q6", &Q6[..])] {
        match reference(path, cols) {
            Ok(acc) => {
                let plain = scan_all(&Src::open(path, cols, Types::Plain));
                let same = plain == acc;
                // R1 (i): the engine types really come back as dictionary arrays, not expanded
                let dict = cols.iter().any(|c| *c == "l_returnflag").then(|| {
                    let src = Src::open(path, &["l_returnflag"], Types::Engine);
                    let b = src.reader(0, None).next().unwrap().unwrap();
                    matches!(b.column(0).data_type(), arrow::datatypes::DataType::Dictionary(_, _))
                });
                println!("gate {name}: DuckDB == engine scan ({} rows): PASS; plain == engine: {}; l_returnflag is a dictionary array: {}", acc.rows, if same { "PASS" } else { "FAIL" }, dict.map_or("n/a".into(), |d| if d { "PASS".to_string() } else { "FAIL".to_string() }));
                ok &= same && dict.unwrap_or(true);
            }
            Err(e) => {
                println!("gate {name}: FAIL: {e}");
                ok = false;
            }
        }
    }
    ok
}

fn r1(path: &str, rounds: usize) -> bool {
    if !gate(path) {
        println!("GATE FAILED: no timing is evidence");
        return false;
    }
    println!("\n# R1: single-thread decode of the projected columns, {rounds} rounds (+1 warm-up), arms interleaved. Load at start: {:.2}", load1());
    println!("DuckDB's time includes its cheap aggregates (sum/count over every column); the other arms' include the equally cheap fold. Median [q1, q3] in ms.\n");
    println!("| projection | plain ms | engine ms | engine-twin ms | duckdb ms | engine / twin | engine / duckdb (time) | plain / duckdb (time) | R1 (ii): engine within 1.25x of DuckDB's time |");
    println!("|---|---|---|---|---|---|---|---|---|");
    let mut all_pass = true;
    for (name, cols) in [("q1 (7 columns)", &Q1[..]), ("q6 (4 columns)", &Q6[..])] {
        let want = reference(path, cols).unwrap();
        let (plain, engine) = (Src::open(path, cols, Types::Plain), Src::open(path, cols, Types::Engine));
        let sql = duck_sql(path, cols);
        let line = acc_line(&want);
        let t = interleave(4, rounds, |a| match a {
            0 | 1 | 2 => {
                let src = if a == 0 { &plain } else { &engine };
                let s = Instant::now();
                let acc = scan_all(src);
                let el = s.elapsed().as_secs_f64();
                assert_eq!(acc, want, "an arm returned a different result");
                el
            }
            _ => {
                let (l, secs) = duck_run(&sql).expect("duckdb");
                assert_eq!(l, line, "DuckDB returned a different result");
                secs
            }
        });
        let (e_t, _, _) = ratio(&t[1], &t[2]);
        let (e_d, lo, hi) = ratio(&t[1], &t[3]);
        let (p_d, _, _) = ratio(&t[0], &t[3]);
        let pass = e_d <= 1.25;
        all_pass &= pass;
        println!("| {name} | {} | {} | {} | {} | {e_t:.2} | {e_d:.2} [{lo:.2}, {hi:.2}] | {p_d:.2} | {} |", cell(&t[0]), cell(&t[1]), cell(&t[2]), cell(&t[3]), if pass { "PASS" } else { "FAIL" });
    }
    println!("\nR1 (i): dictionary arrays are returned without expanding: see the gate lines above.");
    println!("R1 (ii) on this machine: {}", if all_pass { "PASS (both projections)" } else { "FAIL (at least one projection)" });
    true
}

fn r3(path: &str, rounds: usize) -> bool {
    let cols = &Q1[..];
    let want = reference(path, cols).unwrap();
    let src = Src::open(path, cols, Types::Engine);
    let (rg, q) = (by_row_group(&src), by_parts(&src, 4));
    println!("\n# R3: {path}: {} row groups of {:?} rows, page index: {}; morsels: {} (row group) or {} (quarter). {rounds} rounds, load at start {:.2}", src.row_group_rows.len(), &src.row_group_rows[..src.row_group_rows.len().min(4)], if src.has_page_index { "YES" } else { "no" }, rg.len(), q.len(), load1());
    println!("\n| threads | row group ms | quarter ms | row-group twin ms | throughput gain of the quarter (row group time / quarter time) | twin noise |");
    println!("|---|---|---|---|---|---|");
    let mut gain8 = f64::NAN;
    for threads in [1usize, 2, 4, 8] {
        let t = interleave(3, rounds, |a| {
            let m = if a == 1 { &q } else { &rg };
            let s = Instant::now();
            let acc = run(&src, m, threads);
            let el = s.elapsed().as_secs_f64();
            assert_eq!(acc, want, "an arm returned a different result");
            el
        });
        let (g, lo, hi) = ratio(&t[0], &t[1]);
        let (tw, _, _) = ratio(&t[0], &t[2]);
        if threads == 8 {
            gain8 = g;
        }
        println!("| {threads} | {} | {} | {} | {g:.2} [{lo:.2}, {hi:.2}] | {tw:.2} |", cell(&t[0]), cell(&t[1]), cell(&t[2]));
    }
    println!("\nR3 (throughput half) on this file: the quarter's 8-thread gain is {gain8:.2}x (the rule wants at least 1.15x AND peak memory up by at most 25%: run `pscan mem` for both arms).");
    true
}

fn mem(path: &str, arm: &str, threads: usize) -> bool {
    let cols = &Q1[..];
    let src = Src::open(path, cols, Types::Engine);
    let morsels = match arm {
        "rg" => by_row_group(&src),
        "quarter" => by_parts(&src, 4),
        other => panic!("arm {other}: rg or quarter"),
    };
    let base = status_kb("VmRSS:");
    let acc = run(&src, &morsels, threads);
    let peak = status_kb("VmHWM:");
    println!("MEM arm={arm} threads={threads} rows={} above_kib={} (peak {} KiB, baseline {} KiB: the resident memory before the scan, the file is opened and its footer read)", acc.rows, peak.saturating_sub(base), peak, base);
    true
}

fn mkpageindex(input: &str, output: &str) -> bool {
    use parquet::arrow::ArrowWriter;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use parquet::basic::Compression;
    use parquet::file::properties::{EnabledStatistics, WriterProperties};
    let total = Src::open(input, &["l_orderkey"], Types::Plain).total_rows();
    let builder = ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(input).unwrap()).unwrap();
    let schema = builder.schema().clone();
    let reader = builder.with_batch_size(65536).build().unwrap();
    // four row groups, many small pages, page-level statistics: this is what makes the offset index and the column index appear
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_max_row_group_row_count(Some(total.div_ceil(4)))
        .set_data_page_row_count_limit(20_000)
        .set_statistics_enabled(EnabledStatistics::Page)
        .build();
    let mut w = ArrowWriter::try_new(std::fs::File::create(output).unwrap(), schema, Some(props)).unwrap();
    for b in reader {
        w.write(&b.unwrap()).unwrap();
    }
    w.close().unwrap();
    let s = Src::open(output, &["l_orderkey"], Types::Plain);
    println!("wrote {output}: {} rows, {} row groups, page index: {}", s.total_rows(), s.row_group_rows.len(), if s.has_page_index { "yes" } else { "NO" });
    s.has_page_index && s.total_rows() == total
}

fn main() -> ExitCode {
    let a: Vec<String> = std::env::args().collect();
    let ok = match a.get(1).map(String::as_str) {
        Some("gate") => gate(&a[2]),
        Some("r1") => r1(&a[2], args_rounds(&a)),
        Some("r3") => r3(&a[2], args_rounds(&a)),
        Some("mem") => mem(&a[2], &a[3], a[4].parse().unwrap()),
        Some("mkpageindex") => mkpageindex(&a[2], &a[3]),
        _ => {
            eprintln!("usage: pscan gate|r1|r3 <file> [--rounds N] | mem <file> <rg|quarter> <threads> | mkpageindex <in> <out>");
            false
        }
    };
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
