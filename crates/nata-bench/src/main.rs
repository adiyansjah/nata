//! `nata-bench`: reproducible benchmarks.
//!
//! Runs every `*.sql` file in a directory several times against one engine
//! and writes a results file that records what was run and on what.
//!
//! What the numbers mean (printed in every report, because a number without
//! its limits is not a result):
//! - Time is wall-clock for the whole engine process, startup included.
//! - The first run of each query is a warm-up and is discarded, so results
//!   are "warm OS page cache", never cold.
//! - Output is discarded; result printing time is part of the number.
//!
//! Engines: `duckdb` (baseline) and `nata` (not supported yet, so it errors).

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};

#[derive(Clone, Copy, ValueEnum)]
enum Engine {
    Duckdb,
    Nata,
}

impl Engine {
    fn name(self) -> &'static str {
        match self {
            Self::Duckdb => "duckdb",
            Self::Nata => "nata",
        }
    }
}

#[derive(Parser)]
#[command(version, about = "Time queries on one engine and record the result")]
struct Args {
    /// Directory holding the data files (substituted for `{data}` in queries).
    #[arg(long)]
    data: PathBuf,
    /// Directory of `*.sql` files, one query per file.
    #[arg(long)]
    queries: PathBuf,
    #[arg(long, value_enum, default_value = "duckdb")]
    engine: Engine,
    /// Engine binary. Defaults to `duckdb`, or the `nata` next to this executable.
    #[arg(long)]
    binary: Option<PathBuf>,
    /// Timed runs per query, after one discarded warm-up run.
    #[arg(long, default_value_t = 5)]
    runs: usize,
    /// Directory the results file is written to.
    #[arg(long, default_value = "bench/results")]
    out: PathBuf,
}

fn main() -> ExitCode {
    match run(&Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("nata-bench: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &Args) -> Result<()> {
    if args.runs == 0 {
        bail!("--runs must be at least 1");
    }
    let data = args
        .data
        .canonicalize()
        .with_context(|| format!("data directory {}", args.data.display()))?;
    let binary = match (&args.binary, args.engine) {
        (Some(b), _) => b.clone(),
        (None, Engine::Duckdb) => PathBuf::from("duckdb"),
        (None, Engine::Nata) => {
            let exe = std::env::current_exe().context("locating own executable")?;
            exe.parent()
                .context("executable has no parent")?
                .join("nata")
        }
    };

    let mut queries: Vec<PathBuf> = std::fs::read_dir(&args.queries)
        .with_context(|| format!("queries directory {}", args.queries.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "sql"))
        .collect();
    queries.sort();
    if queries.is_empty() {
        bail!("no *.sql files in {}", args.queries.display());
    }

    let version = engine_version(&binary, args.engine)?;
    let mut report = header(args, &data, &version);
    report.push_str("| query | min ms | median ms | runs |\n|---|---:|---:|---:|\n");

    for path in &queries {
        let name = path
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let sql = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?
            .replace("{data}", &data.to_string_lossy());

        run_once(&binary, args.engine, &sql).with_context(|| format!("query {name} (warm-up)"))?;
        let mut times = Vec::with_capacity(args.runs);
        for _ in 0..args.runs {
            times.push(
                run_once(&binary, args.engine, &sql).with_context(|| format!("query {name}"))?,
            );
        }
        times.sort();
        let line = format!(
            "| {name} | {:.1} | {:.1} | {} |\n",
            ms(times[0]),
            ms(times[times.len() / 2]),
            times.len()
        );
        print!("{line}");
        report.push_str(&line);
    }

    std::fs::create_dir_all(&args.out)?;
    let file = args
        .out
        .join(format!("{}-{}.md", timestamp(), args.engine.name()));
    std::fs::write(&file, &report).with_context(|| format!("writing {}", file.display()))?;
    println!("\nwrote {}", file.display());
    Ok(())
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn run_once(binary: &Path, engine: Engine, sql: &str) -> Result<Duration> {
    let mut cmd = Command::new(binary);
    match engine {
        Engine::Duckdb => cmd.args(["-csv", "-c", sql]),
        Engine::Nata => cmd.args(["sql", "--csv", "-c", sql]),
    };
    let start = Instant::now();
    let out = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("running {}", binary.display()))?;
    let elapsed = start.elapsed();
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        bail!(
            "{}",
            msg.lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("engine failed")
        );
    }
    Ok(elapsed)
}

fn engine_version(binary: &Path, engine: Engine) -> Result<String> {
    let out = Command::new(binary)
        .arg("--version")
        .env_remove("AI_AGENT")
        .output()
        .with_context(|| format!("running {} --version", binary.display()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text
        .lines()
        .find(|l| !l.contains("agent mode") && !l.trim().is_empty())
        .unwrap_or("unknown");
    Ok(format!("{} ({})", line.trim(), engine.name()))
}

fn header(args: &Args, data: &Path, version: &str) -> String {
    format!(
        "# Benchmark: {engine}\n\n\
         - Engine: {version}\n\
         - Data: {data}\n\
         - Queries: {queries}\n\
         - CPU: {cpu}\n\
         - Cores: {cores}\n\
         - Memory: {mem}\n\
         - Kernel: {kernel}\n\
         - Timed runs per query: {runs} (one warm-up run discarded)\n\n\
         What limits these numbers: wall-clock of the whole engine process, \
         startup included; warm OS page cache (never cold); output discarded.\n\n",
        engine = args.engine.name(),
        data = data.display(),
        queries = args.queries.display(),
        cpu = cpu_model(),
        cores = std::thread::available_parallelism().map_or(0, std::num::NonZero::get),
        mem = mem_total(),
        kernel = read_trimmed("/proc/sys/kernel/osrelease"),
        runs = args.runs,
    )
}

fn read_trimmed(path: &str) -> String {
    std::fs::read_to_string(path).map_or_else(|_| "unknown".into(), |s| s.trim().to_string())
}

fn cpu_model() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split_once(':'))
                .map(|(_, v)| v.trim().to_string())
        })
        .unwrap_or_else(|| "unknown".into())
}

fn mem_total() -> String {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
                .map(|kb| format!("{:.1} GiB", kb as f64 / 1024.0 / 1024.0))
        })
        .unwrap_or_else(|| "unknown".into())
}

fn timestamp() -> String {
    Command::new("date")
        .args(["-u", "+%Y%m%dT%H%M%SZ"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map_or_else(|| "unknown-time".into(), |s| s.trim().to_string())
}
