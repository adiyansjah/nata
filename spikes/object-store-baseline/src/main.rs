//! `osb`: the command line of the baseline spike. See README in the results file for the subcommands.

use std::process::{Command, Stdio};

use object_store::{ObjectStore as _, ObjectStoreExt as _};
use spike_object_store_baseline::bench::{self, Bench, Cell, Row, StoreCfg};
use spike_object_store_baseline::cache::CacheMutants;
use spike_object_store_baseline::engine::{Exec, ExecKind};
use spike_object_store_baseline::policy::Config;
use spike_object_store_baseline::cachecases;
use spike_object_store_baseline::faultcases::{self, CaseResult};
use spike_object_store_baseline::policy::Mutants;

fn mutant_by_name(name: &str) -> Mutants {
    if name.is_empty() || name == "none" {
        return Mutants::default();
    }
    Mutants::all().into_iter().find(|(n, _)| *n == name).map(|(_, m)| m).unwrap_or_else(|| {
        eprintln!("unknown mutant {name}");
        std::process::exit(2)
    })
}

fn cache_mutant_by_name(name: &str) -> CacheMutants {
    if name.is_empty() || name == "none" {
        return CacheMutants::default();
    }
    CacheMutants::all().into_iter().find(|(n, _)| *n == name).map(|(_, m)| m).unwrap_or_else(|| {
        eprintln!("unknown cache mutant {name}");
        std::process::exit(2)
    })
}

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn print_case(c: &CaseResult) {
    println!("CASE {}: {} {}", c.name, if c.ok { "PASS" } else { "FAIL" }, c.detail);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("help");
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().build().unwrap();
    match cmd {
        "faults" => {
            let m = mutant_by_name(&arg(&args, "--mutant").unwrap_or_default());
            let res = rt.block_on(faultcases::all_cases(&m));
            res.iter().for_each(print_case);
            let bad = res.iter().filter(|c| !c.ok).count();
            println!("RESULT faults: {} cases, {} failed", res.len(), bad);
            std::process::exit(if bad == 0 { 0 } else { 1 });
        }
        "footer-charge" => {
            // osb footer-charge FILE... : the footer's wire length and what the cache would charge for its parsed form (ParquetMetaData::memory_size)
            for f in args.iter().skip(1) {
                let b = std::fs::read(f).unwrap();
                let n = b.len();
                let mlen = u32::from_le_bytes(b[n - 8..n - 4].try_into().unwrap()) as usize;
                let meta = parquet::file::metadata::ParquetMetaDataReader::decode_metadata(&b[n - 8 - mlen..n - 8]).unwrap();
                let ms = meta.memory_size();
                println!("footer-charge {} : row_groups={} columns={} wire_bytes={} parsed_memory_size={} ratio={:.2} entries_in_64MiB={}", std::path::Path::new(f).parent().and_then(|p| p.file_name()).unwrap().to_string_lossy(), meta.num_row_groups(), meta.file_metadata().schema_descr().num_columns(), mlen, ms, ms as f64 / mlen as f64, (64usize << 20) / (ms + 200));
            }
        }
        "expected" => {
            // the oracle: DuckDB on LOCAL files. osb expected --name sf1 --sf 1 --glob '<path or glob>' --out FILE (appends "name|query|text")
            let name = arg(&args, "--name").unwrap();
            let sf: f64 = arg(&args, "--sf").unwrap().parse().unwrap();
            let glob = arg(&args, "--glob").unwrap();
            let out = arg(&args, "--out").unwrap();
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&out).unwrap();
            for qn in ["full", "proj", "sel", "meta"] {
                let q = spike_object_store_baseline::engine::QueryKind::parse(qn, sf).unwrap();
                let sql = q.sql().replace("{T}", &format!("read_parquet('{glob}')"));
                let o = Command::new("duckdb").args(["-list", "-noheader", "-separator", "|", "-nullvalue", "NULL", "-c", &sql]).env_remove("AI_AGENT").output().expect("duckdb");
                if !o.status.success() {
                    eprintln!("duckdb failed: {}", String::from_utf8_lossy(&o.stderr));
                    std::process::exit(4);
                }
                let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
                use std::io::Write;
                writeln!(f, "{name}|{qn}|{text}").unwrap();
                println!("expected {name} {qn}: {text}");
            }
        }
        "upload" => {
            // osb upload --dir D (files under D/<rel> go to <prefix>/<rel>). Needs write credentials (the write-test identity).
            let dir = std::path::PathBuf::from(arg(&args, "--dir").unwrap());
            let sc = StoreCfg::from_env().unwrap_or_else(|e| { eprintln!("{e}"); std::process::exit(4) });
            let store = sc.build(&Config::adr(), &Mutants::default()).unwrap_or_else(|e| { eprintln!("{e}"); std::process::exit(4) });
            let mut files = Vec::new();
            fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) { for e in std::fs::read_dir(d).unwrap() { let p = e.unwrap().path(); if p.is_dir() { walk(&p, out) } else if p.extension().is_some_and(|x| x == "parquet") { out.push(p) } } }
            walk(&dir, &mut files);
            files.sort();
            let n = rt.block_on(async {
                use futures::StreamExt;
                let results = futures::stream::iter(files.iter().cloned()).map(|f| {
                    let store = store.clone();
                    let rel = f.strip_prefix(&dir).unwrap().to_string_lossy().to_string();
                    let dest = sc.path(&rel);
                    async move {
                        use tokio::io::AsyncReadExt;
                        let up = store.put_multipart(&dest).await.map_err(|e| e.to_string())?;
                        let mut w = object_store::WriteMultipart::new_with_chunk_size(up, 32 << 20);
                        let mut file = tokio::fs::File::open(&f).await.map_err(|e| e.to_string())?;
                        let mut buf = vec![0u8; 8 << 20];
                        let mut total = 0usize;
                        loop {
                            let n = file.read(&mut buf).await.map_err(|e| e.to_string())?;
                            if n == 0 { break; }
                            w.wait_for_capacity(4).await.map_err(|e| e.to_string())?; // bounded memory: at most 4 parts in flight per file
                            w.write(&buf[..n]);
                            total += n;
                        }
                        w.finish().await.map_err(|e| e.to_string())?;
                        Ok::<usize, String>(total)
                    }
                }).buffer_unordered(4).collect::<Vec<_>>().await;
                let mut bytes = 0usize;
                for r in results { match r { Ok(b) => bytes += b, Err(e) => { eprintln!("upload failed: {}", sc.redactor().apply(&e)); std::process::exit(5) } } }
                bytes
            });
            println!("uploaded {} files, {} bytes", files.len(), n);
        }
        "list" | "delete" => {
            // osb list|delete : every object under <prefix>. list prints the count; delete removes them and then proves the prefix is empty.
            let sc = StoreCfg::from_env().unwrap_or_else(|e| { eprintln!("{e}"); std::process::exit(4) });
            let store = sc.build(&Config::adr(), &Mutants::default()).unwrap_or_else(|e| { eprintln!("{e}"); std::process::exit(4) });
            let prefix = object_store::path::Path::from(sc.prefix.clone());
            let del = cmd == "delete";
            let (before, after) = rt.block_on(async {
                use futures::TryStreamExt;
                let metas: Vec<_> = store.list(Some(&prefix)).try_collect().await.unwrap_or_default();
                let before = metas.len();
                if del {
                    for m in &metas { let _ = store.delete(&m.location).await; }
                }
                let after: Vec<_> = store.list(Some(&prefix)).try_collect().await.unwrap_or_default();
                (before, after.len())
            });
            println!("{} objects under the prefix{}", before, if del { format!(" before; {after} after deleting") } else { String::new() });
            if del && after != 0 { std::process::exit(6); }
        }
        "serve" => {
            // osb serve --dir D --bucket B --port-file F: the fake endpoint populated from D (layouts), for local trials of the DuckDB arm
            let dir = std::path::PathBuf::from(arg(&args, "--dir").unwrap());
            let bucket = arg(&args, "--bucket").unwrap_or("bkt".into());
            let pf = arg(&args, "--port-file").unwrap();
            rt.block_on(async {
                let fake = spike_object_store_baseline::fake::FakeS3::start(&bucket).await;
                fn walk(d: &std::path::Path, base: &std::path::Path, f: &spike_object_store_baseline::fake::FakeS3) { for e in std::fs::read_dir(d).unwrap() { let p = e.unwrap().path(); if p.is_dir() { walk(&p, base, f) } else if p.extension().is_some_and(|x| x == "parquet") { f.put(&p.strip_prefix(base).unwrap().to_string_lossy(), std::fs::read(&p).unwrap()) } } }
                walk(&dir, &dir, &fake);
                if let Some(ms) = arg(&args, "--delay-ms").and_then(|s| s.parse::<u64>().ok()) {
                    fake.set_plan(spike_object_store_baseline::fake::Plan::always(spike_object_store_baseline::fake::Fault::Delay { ms }));
                }
                std::fs::write(&pf, &fake.addr).unwrap();
                std::future::pending::<()>().await;
            });
        }
        "bench" => {
            // osb bench --cells 'sf1/small1:1,...' --sf 1 --queries full,proj --arms duck,ctl,wrap --rounds 5 --workers 8 --io-threads 2 --expected F --out CSV [--saturate] [--exec-arms]
            let sc = StoreCfg::from_env().unwrap_or_else(|e| { eprintln!("{e}"); std::process::exit(4) });
            let workers: usize = arg(&args, "--workers").and_then(|s| s.parse().ok()).unwrap_or(8);
            let io_threads: usize = arg(&args, "--io-threads").and_then(|s| s.parse().ok()).unwrap_or(2);
            let rounds: usize = arg(&args, "--rounds").and_then(|s| s.parse().ok()).unwrap_or(5);
            let out = arg(&args, "--out").unwrap();
            let arms: Vec<String> = arg(&args, "--arms").unwrap_or("duck,ctl,wrap".into()).split(',').flat_map(|a| [a.to_string(), format!("{a}2")]).collect();
            let queries: Vec<String> = arg(&args, "--queries").unwrap_or("full,proj,sel,meta".into()).split(',').map(|s| s.to_string()).collect();
            let sanity = args.iter().any(|a| a == "--sanity-only");
            let mut expected = std::collections::HashMap::new();
            for l in std::fs::read_to_string(arg(&args, "--expected").unwrap()).unwrap().lines() {
                let p: Vec<&str> = l.splitn(3, '|').collect();
                if p.len() == 3 { expected.insert(format!("{}|{}", p[0], p[1]), p[2].to_string()); }
            }
            let cells: Vec<Cell> = arg(&args, "--cells").unwrap().split(',').flat_map(|c| {
                let (dir, n) = c.split_once(':').unwrap();
                let sf: f64 = std::env::var("NATA_SF_OVERRIDE").ok().and_then(|s| s.parse().ok()).unwrap_or_else(|| dir.trim_start_matches("sf").split('/').next().unwrap().parse().unwrap());
                let queries = queries.clone();
                queries.into_iter().map(move |q| Cell { dir: dir.to_string(), files: n.parse().unwrap(), sf, query: q }).collect::<Vec<_>>()
            }).collect();
            let io = tokio::runtime::Builder::new_multi_thread().worker_threads(io_threads).enable_all().build().unwrap();
            let mut execs = std::collections::HashMap::new();
            execs.insert("split", Exec::new(ExecKind::Split, workers));
            execs.insert("tokio", Exec::new(ExecKind::TokioCpu, workers));
            let b = Bench { redact: sc.redactor(), sc, cfg: Config::adr(), io, execs, workers, expected, saturate: args.iter().any(|a| a == "--saturate") };
            let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&out).unwrap();
            use std::io::Write;
            if f.metadata().map(|m| m.len() == 0).unwrap_or(true) { writeln!(f, "{}", Row::csv_header()).unwrap(); }
            let idle_rss = std::fs::read_to_string("/proc/self/status").unwrap_or_default().lines().find(|l| l.starts_with("VmRSS")).unwrap_or("").to_string();
            println!("idle runtime {idle_rss}");
            if !sanity || true {
                // startup probe on the first cell's first file: authentication + connection + first request, cold and warm
                let c0 = &cells[0];
                let (cold, warm) = bench::startup_probe(&b.sc, &b.cfg, &b.io, &c0.rels()[0], 8);
                println!("STARTUP cold_store_first_head_ms median={:.1} min={:.1} | warm_head_ms median={:.1} min={:.1}", bench::median_of(&cold), cold.iter().cloned().fold(f64::MAX, f64::min), bench::median_of(&warm), warm.iter().cloned().fold(f64::MAX, f64::min));
            }
            // correctness first: every cell x arm once; a wrong answer stops the run before any timing is kept
            let mut bad = 0;
            for c in &cells {
                for a in arms.iter().filter(|a| !a.ends_with('2')) {
                    for r in b.run_arm(c, a, 0) {
                        if !r.ok { bad += 1; println!("NOT OK (verification) {} {} {}", r.cell, r.arm, r.note); }
                    }
                }
            }
            if bad > 0 { println!("RESULT bench: {bad} verification runs not ok: no timing kept"); std::process::exit(3); }
            println!("verification: every cell x arm equals the DuckDB-on-local-files answer");
            let mut total = 0; let mut notok = 0;
            // a timed block starts on a quiet machine: wait once for the whole step (the 1-minute load decays too slowly to wait per cell), then log the load before every cell
            println!("{}", bench::settle("start"));
            for round in 1..=rounds {
                for (ci, c) in cells.iter().enumerate() {
                    println!("LOAD round {round} cell {ci}: {:.2}", std::fs::read_to_string("/proc/loadavg").ok().and_then(|s| s.split_whitespace().next().and_then(|x| x.parse::<f64>().ok())).unwrap_or(0.0));
                    // alternate: the arm order rotates every round and cell (A B A' B' ... then shifted), so no arm always runs first
                    let k = (round + ci) % arms.len();
                    for j in 0..arms.len() {
                        let a = &arms[(k + j) % arms.len()];
                        for r in b.run_arm(c, a, round) {
                            total += 1;
                            if !r.ok { notok += 1; println!("NOT OK {} {} {}", r.cell, r.arm, r.note); }
                            writeln!(f, "{}", r.csv()).unwrap();
                        }
                    }
                }
            }
            println!("RESULT bench{}: {total} runs, {notok} not ok", if sanity { " (SANITY ONLY)" } else { "" });
            std::process::exit(if notok == 0 { 0 } else { 3 });
        }
        "summarize" => {
            // osb summarize CSV...  : per cell and arm: n, min, median, max, twin ratio, ratio to duck
            let mut rows: Vec<Vec<String>> = Vec::new();
            for p in args.iter().skip(1) {
                for l in std::fs::read_to_string(p).unwrap().lines().skip(1) { rows.push(l.split(',').map(|x| x.to_string()).collect()); }
            }
            let mut g: std::collections::BTreeMap<(String, String, String), Vec<(f64, f64, f64, f64, f64, f64, f64)>> = Default::default();
            for r in &rows {
                if r[5] != "1" { continue; }
                let rep: usize = r[3].parse().unwrap_or(0);
                let key = (r[0].clone(), if rep > 0 { format!("{}#rep{}", r[1], rep) } else { r[1].clone() }, r[4].clone());
                g.entry(key).or_default().push((r[6].parse().unwrap_or(f64::NAN), r[7].parse().unwrap_or(0.0), r[9].parse().unwrap_or(0.0), r[15].parse().unwrap_or(0.0), r[16].parse().unwrap_or(0.0), r[17].parse().unwrap_or(0.0), r[18].parse().unwrap_or(0.0)));
            }
            println!("cell | arm | workers | n | min s | median s | max s | spread | median first-unit s | fetched MiB | cpu s | rss MiB | first-byte p50/p99 ms");
            for ((cell, arm, w), v) in &g {
                let walls: Vec<f64> = v.iter().map(|x| x.0).collect();
                let mn = walls.iter().cloned().fold(f64::MAX, f64::min); let mx = walls.iter().cloned().fold(0.0, f64::max);
                let med = bench::median_of(&walls);
                let firsts: Vec<f64> = v.iter().map(|x| x.1).collect(); let fetched: Vec<f64> = v.iter().map(|x| x.2 / 1048576.0).collect();
                let cpu: Vec<f64> = v.iter().map(|x| x.3).filter(|x| *x > 0.0).collect(); let rss: Vec<f64> = v.iter().map(|x| x.4).filter(|x| *x > 0.0).collect();
                let f50: Vec<f64> = v.iter().map(|x| x.5).filter(|x| *x > 0.0).collect(); let f99: Vec<f64> = v.iter().map(|x| x.6).filter(|x| *x > 0.0).collect();
                println!("{cell} | {arm} | {w} | {} | {mn:.3} | {med:.3} | {mx:.3} | {:.2} | {:.3} | {:.1} | {:.2} | {:.0} | {:.0}/{:.0}", walls.len(), mx / mn.max(1e-9), bench::median_of(&firsts), bench::median_of(&fetched), bench::median_of(&cpu), bench::median_of(&rss), bench::median_of(&f50), bench::median_of(&f99));
            }
            println!();
            println!("cell | arm | median s | twin ratio (arm / arm2) | ratio to duck (arm / duck)");
            let med = |c: &str, a: &str, w: &str| g.get(&(c.to_string(), a.to_string(), w.to_string())).map(|v| bench::median_of(&v.iter().map(|x| x.0).collect::<Vec<_>>()));
            for ((cell, arm, w), _) in &g {
                if arm.contains('2') && !arm.contains("#") || arm.ends_with('2') { continue; }
                let base = arm.split('#').next().unwrap(); let suffix = arm.strip_prefix(base).unwrap_or("");
                let m = med(cell, arm, w).unwrap();
                let tw = med(cell, &format!("{base}2{suffix}"), w).map(|t| format!("{:.3}", m / t)).unwrap_or("-".into());
                let du = med(cell, &format!("duck{suffix}"), w).map(|d| format!("{:.3}", m / d)).unwrap_or("-".into());
                println!("{cell} | {arm} | {m:.3} | {tw} | {du}");
            }
        }
        "cache" => {
            let m = mutant_by_name(&arg(&args, "--mutant").unwrap_or_default());
            let cm = cache_mutant_by_name(&arg(&args, "--cache-mutant").unwrap_or_default());
            let mut res = rt.block_on(cachecases::cache_cases(&m, &cm));
            res.extend(rt.block_on(cachecases::scan_cases(&m)));
            res.iter().for_each(print_case);
            let bad = res.iter().filter(|c| !c.ok).count();
            println!("RESULT cache+scan: {} cases, {} failed", res.len(), bad);
            std::process::exit(if bad == 0 { 0 } else { 1 });
        }
        "mutants" => {
            // every deliberate defect, each in a child process: CAUGHT = at least one case fails; SURVIVED = all pass; INVALID = the clean control failed
            let exe = std::env::current_exe().unwrap();
            let run = |a: &[&str]| -> (i32, Vec<String>) {
                let out = Command::new(&exe).args(a).env("MALLOC_ARENA_MAX", "2").stdin(Stdio::null()).output().expect("child");
                let text = String::from_utf8_lossy(&out.stdout).to_string();
                let failed: Vec<String> = text.lines().filter(|l| l.starts_with("CASE ") && l.contains(": FAIL")).map(|l| l[5..].split(':').next().unwrap_or("").to_string()).collect();
                (out.status.code().unwrap_or(-1), failed)
            };
            let mut invalid = false;
            for (label, a) in [("faults", vec!["faults"]), ("cache+scan", vec!["cache"]), ("hostile", vec!["hostile-all"])] {
                let (code, failed) = run(&a);
                println!("CONTROL {label}: {}", if code == 0 { "all pass".to_string() } else { format!("FAILED {failed:?} (exit {code})") });
                invalid |= code != 0;
            }
            if invalid {
                println!("RESULT mutants: INVALID (the unmutated control fails)");
                std::process::exit(3);
            }
            let (mut caught, mut survived) = (0, 0);
            for (name, _) in Mutants::all() {
                let mut by: Vec<String> = Vec::new();
                for a in [vec!["faults", "--mutant", name], vec!["cache", "--mutant", name], vec!["hostile-all", "--mutant", name]] {
                    let (code, failed) = run(&a);
                    if code != 0 {
                        by.push(if failed.is_empty() { format!("{} (process died, exit {code})", a[0]) } else { failed.join(",") });
                    }
                }
                if by.is_empty() { survived += 1; println!("MUTANT {name}: SURVIVED"); } else { caught += 1; println!("MUTANT {name}: CAUGHT by {}", by.join(" | ")); }
            }
            for (name, _) in CacheMutants::all() {
                let (code, failed) = run(&["cache", "--cache-mutant", name]);
                if code != 0 { caught += 1; println!("MUTANT cache:{name}: CAUGHT by {}", failed.join(",")); } else { survived += 1; println!("MUTANT cache:{name}: SURVIVED"); }
            }
            println!("RESULT mutants: {caught} caught, {survived} survived");
            std::process::exit(if survived == 0 { 0 } else { 1 });
        }
        "hostile" => {
            let m = mutant_by_name(&arg(&args, "--mutant").unwrap_or_default());
            let name = args.get(1).cloned().unwrap_or_default();
            let c = rt.block_on(faultcases::hostile(&name, &m));
            print_case(&c);
            std::process::exit(if c.ok { 0 } else { 1 });
        }
        "hostile-all" => {
            // every hostile case in a child process under `ulimit -v` (the allocation bound is the thing under test)
            let limit_kib: u64 = arg(&args, "--ulimit-mib").and_then(|s| s.parse().ok()).unwrap_or(1536) * 1024;
            let m = arg(&args, "--mutant").unwrap_or_default();
            let exe = std::env::current_exe().unwrap();
            let mut bad = 0;
            for name in faultcases::HOSTILE_CASES {
                let script = format!("ulimit -v {limit_kib}; exec \"$0\" hostile {name} --mutant '{m}'");
                let out = Command::new("sh").arg("-c").arg(&script).arg(&exe).env("MALLOC_ARENA_MAX", "2").stdin(Stdio::null()).output().expect("sh");
                let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let line = if text.is_empty() { format!("CASE {name}: FAIL process ended without a result (status {:?}: killed or allocation failure under ulimit -v {} MiB)", out.status.code(), limit_kib / 1024) } else { text };
                if !line.contains(": PASS") {
                    bad += 1;
                }
                println!("{line}");
            }
            println!("RESULT hostile: {} cases, {} failed (ulimit -v {} MiB)", faultcases::HOSTILE_CASES.len(), bad, limit_kib / 1024);
            std::process::exit(if bad == 0 { 0 } else { 1 });
        }
        _ => {
            eprintln!("usage: osb faults|hostile|hostile-all [--mutant NAME]");
            std::process::exit(2);
        }
    }
}
