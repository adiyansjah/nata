//! The measuring harness of the bake-off. Output is JSON lines for `corpus/report.py`, which makes the tables.
//!   bake try SQL...                      both arms on each argument, printed
//!   bake run RECORDS OUT [--mutants]     every arm on every record (id \x1f query, records separated by \x1e) -> one JSON line per record
//!   bake time RECORDS REPS PASSES        parse time per query and arm, interleaved
//!   bake robust                          deep, long and huge queries in child processes (crash, time, memory)
//!   bake robust-child CASE ARM STACK_KB  (internal)
use spike_sql_parser::sqlp::{self, Dial, Opts};
use spike_sql_parser::*;
use std::fmt::Write as _;
use std::hint::black_box;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Arm {
    Hand,
    AGen,
    APg,
    AGenRaw,
    AGenNoFix,
    AGenNoGuard,
    MutOr,
    MutNot,
    MutF64,
}

const ARMS: [(Arm, &str); 6] = [
    (Arm::Hand, "hand"),
    (Arm::AGen, "a-generic"),
    (Arm::APg, "a-postgres"),
    (Arm::AGenRaw, "a-generic-raw"),
    (Arm::AGenNoFix, "a-generic-nofix"),
    (Arm::AGenNoGuard, "a-generic-noguard"),
];
const MUTANTS: [(Arm, &str); 3] = [
    (Arm::MutOr, "mut-or-before-and"),
    (Arm::MutNot, "mut-lose-not"),
    (Arm::MutF64, "mut-number-via-f64"),
];

fn run_arm(arm: Arm, q: &str) -> Result<Query, ParseError> {
    match arm {
        Arm::Hand => hand::parse(q),
        Arm::AGen => sqlp::parse_opts(q, Opts::new(Dial::Generic)),
        Arm::APg => sqlp::parse_opts(q, Opts::new(Dial::Postgres)),
        // the crate alone: parse errors are mapped, a successful parse is not converted (the AST is a placeholder)
        Arm::AGenRaw => match sqlp::parse_raw(q, Opts::new(Dial::Generic)) {
            Ok(_) => Err(ParseError {
                kind: ErrorKind::NotSupported,
                message: "raw-ok".into(),
                line: None,
                col: None,
                offset: None,
            }),
            Err(e) => Err(ParseError {
                kind: ErrorKind::Syntax,
                message: e.to_string(),
                line: None,
                col: None,
                offset: None,
            }),
        },
        Arm::AGenNoFix => sqlp::parse_opts(
            q,
            Opts {
                eof_position_fix: false,
                ..Opts::new(Dial::Generic)
            },
        ),
        Arm::AGenNoGuard => sqlp::parse_opts(
            q,
            Opts {
                depth_guard: false,
                ..Opts::new(Dial::Generic)
            },
        ),
        Arm::MutOr => hand::parse_with(q, hand::Mutant::OrBeforeAnd),
        Arm::MutNot => hand::parse_with(q, hand::Mutant::LoseNot),
        Arm::MutF64 => hand::parse_with(q, hand::Mutant::NumberViaF64),
    }
}

fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn outcome(arm: Arm, q: &str) -> String {
    match catch_unwind(AssertUnwindSafe(|| run_arm(arm, q))) {
        Ok(Ok(ast)) => format!("{{\"ok\":true,\"ast\":{}}}", json_str(&ast.sexp())),
        Ok(Err(e)) => {
            let kind = match e.kind {
                ErrorKind::Syntax => "syntax",
                ErrorKind::NotSupported => "unsupported",
                ErrorKind::Limit => "limit",
            };
            let num = |x: Option<usize>| x.map_or("null".to_string(), |v| v.to_string());
            format!(
                "{{\"ok\":false,\"kind\":\"{kind}\",\"msg\":{},\"line\":{},\"col\":{},\"off\":{}}}",
                json_str(&e.message),
                num(e.line),
                num(e.col),
                num(e.offset)
            )
        }
        Err(_) => "{\"ok\":false,\"kind\":\"panic\"}".to_string(),
    }
}

fn records(path: &str) -> Vec<(String, String)> {
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| fatal(&format!("cannot read {path}: {e}")));
    raw.split('\x1e')
        .filter(|r| !r.is_empty())
        .map(|r| {
            let (id, q) = r.split_once('\x1f').unwrap_or((r, ""));
            (id.to_string(), q.to_string())
        })
        .collect()
}

fn fatal(m: &str) -> ! {
    eprintln!("{m}");
    std::process::exit(2)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("try") => {
            for q in &args[2..] {
                println!("--- {q}");
                for (a, n) in ARMS.iter().take(3) {
                    match run_arm(*a, q) {
                        Ok(x) => println!("{n:>12}: {}", x.sexp()),
                        Err(e) => println!("{n:>12}: ERR {e}"),
                    }
                }
            }
        }
        Some("run") => cmd_run(&args),
        Some("time") => cmd_time(&args),
        Some("robust") => cmd_robust(),
        Some("robust-child") => cmd_robust_child(&args),
        Some("stack-need") => cmd_stack_need(&args),
        _ => fatal("usage: bake try|run|time|robust ..."),
    }
}

// ---------------------------------------------------------------- run

fn cmd_run(args: &[String]) {
    use std::io::Write;
    let (inp, out) = (
        args.get(2)
            .map_or_else(|| fatal("run RECORDS OUT"), |s| s.as_str()),
        args.get(3)
            .map_or_else(|| fatal("run RECORDS OUT"), |s| s.as_str()),
    );
    let mutants = args.iter().any(|a| a == "--mutants");
    std::panic::set_hook(Box::new(|_| {}));
    let mut w = std::io::BufWriter::new(
        std::fs::File::create(out).unwrap_or_else(|e| fatal(&format!("{out}: {e}"))),
    );
    for (id, q) in records(inp) {
        let mut line = format!("{{\"id\":{},\"arms\":{{", json_str(&id));
        let mut first = true;
        for (a, n) in ARMS
            .iter()
            .chain(if mutants { MUTANTS.iter() } else { [].iter() })
        {
            if !first {
                line.push(',');
            }
            first = false;
            let _ = write!(line, "\"{n}\":{}", outcome(*a, &q));
        }
        line.push_str("}}\n");
        let _ = w.write_all(line.as_bytes());
    }
}

// ---------------------------------------------------------------- time

fn pctl(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    if v.is_empty() {
        return 0.0;
    }
    v[(((v.len() - 1) as f64) * p).round() as usize]
}

fn cmd_time(args: &[String]) {
    let path = args
        .get(2)
        .map_or_else(|| fatal("time RECORDS REPS PASSES"), |s| s.as_str());
    let reps: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(20);
    let passes: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(5);
    const K: usize = 5; // parses per timed sample, to keep the clock's own cost small
    let qs: Vec<String> = records(path).into_iter().map(|(_, q)| q).collect();
    let arms = [
        (Arm::Hand, "hand"),
        (Arm::AGen, "a-generic"),
        (Arm::APg, "a-postgres"),
        (Arm::AGenRaw, "a-generic-parse-only"),
    ];
    // the clock: cost of one Instant::now() pair
    let t = Instant::now();
    for _ in 0..1_000_000 {
        black_box(Instant::now());
    }
    println!(
        "{{\"clock_ns\":{:.1}}}",
        t.elapsed().as_nanos() as f64 / 1e6
    );
    let bytes: usize = qs.iter().map(String::len).sum();
    println!(
        "{{\"queries\":{},\"mean_bytes\":{:.1}}}",
        qs.len(),
        bytes as f64 / qs.len() as f64
    );
    for pass in 0..passes {
        let mut per: Vec<Vec<f64>> = vec![Vec::with_capacity(qs.len()); arms.len()];
        let mut samples: Vec<Vec<f64>> = vec![Vec::with_capacity(reps); arms.len()];
        for q in &qs {
            for s in &mut samples {
                s.clear();
            }
            for r in 0..reps {
                // interleaved: the order of the arms rotates with every repetition
                for j in 0..arms.len() {
                    let ai = (j + r) % arms.len();
                    let arm = arms[ai].0;
                    let t = Instant::now();
                    for _ in 0..K {
                        match arm {
                            Arm::AGenRaw => {
                                black_box(
                                    sqlp::parse_raw(black_box(q), Opts::new(Dial::Generic)).is_ok(),
                                );
                            }
                            _ => {
                                black_box(run_arm(arm, black_box(q)).is_ok());
                            }
                        }
                    }
                    samples[ai].push(t.elapsed().as_nanos() as f64 / K as f64);
                }
            }
            for (ai, s) in samples.iter_mut().enumerate() {
                per[ai].push(pctl(s, 0.5));
            }
        }
        let mut line = format!("{{\"pass\":{pass},\"arms\":{{");
        for (ai, (_, n)) in arms.iter().enumerate() {
            let mut v = per[ai].clone();
            let (med, p99, p999) = (pctl(&mut v, 0.5), pctl(&mut v, 0.99), pctl(&mut v, 0.999));
            let max = v.last().copied().unwrap_or(0.0);
            let _ = write!(
                line,
                "{}\"{n}\":{{\"median_ns\":{med:.0},\"p99_ns\":{p99:.0},\"p999_ns\":{p999:.0},\"max_ns\":{max:.0}}}",
                if ai > 0 { "," } else { "" }
            );
        }
        line.push_str("}}");
        println!("{line}");
    }
}

// ---------------------------------------------------------------- robustness

fn robust_case(name: &str) -> String {
    let n_of = |s: &str| {
        s.rsplit('-')
            .next()
            .and_then(|x| x.parse::<usize>().ok())
            .unwrap_or(0)
    };
    let head = "SELECT a FROM 'f.parquet' WHERE ";
    match name.split('-').next().unwrap_or("") {
        "parens" => format!(
            "{head}{}a{}",
            "(".repeat(n_of(name)),
            ")".repeat(n_of(name))
        ),
        "ands" => format!("{head}a{}", " AND a".repeat(n_of(name))),
        "nots" => format!("{head}{}a", "NOT ".repeat(n_of(name))),
        "negs" => format!("SELECT {}a FROM 'f.parquet'", "- ".repeat(n_of(name))),
        "cases" => format!(
            "SELECT {}1{} FROM 'f.parquet'",
            "CASE WHEN a THEN ".repeat(n_of(name)),
            " END".repeat(n_of(name))
        ),
        "inlist" => format!("{head}a IN (1{})", ",1".repeat(n_of(name))),
        "selectlist" => format!("SELECT a{} FROM 'f.parquet'", ", a".repeat(n_of(name))),
        "string" => format!("SELECT '{}' FROM 'f.parquet'", "x".repeat(n_of(name))),
        "comment" => format!("SELECT a FROM 'f.parquet' -- {}", "x".repeat(n_of(name))),
        "ident" => format!("SELECT {} FROM 'f.parquet'", "x".repeat(n_of(name))),
        "sums" => format!("{head}a{}", " + a".repeat(n_of(name))),
        _ => fatal("unknown robust case"),
    }
}

const ROBUST_CASES: &[&str] = &[
    "parens-1000",
    "parens-1500",
    "parens-500000",
    "ands-1000",
    "ands-1500",
    "ands-250000",
    "sums-1000",
    "sums-250000",
    "nots-1000",
    "nots-250000",
    "negs-1000",
    "negs-500000",
    "cases-1000",
    "cases-60000",
    "inlist-500000",
    "selectlist-350000",
    "string-1000000",
    "comment-1000000",
    "ident-1000000",
];

fn vm_hwm_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1).and_then(|x| x.parse().ok()))
        })
        .unwrap_or(0)
}

fn cmd_robust_child(args: &[String]) {
    let (case, arm_name, stack_kb) = (&args[2], &args[3], args[4].parse::<usize>().unwrap_or(2048));
    let arm = ARMS
        .iter()
        .chain(MUTANTS.iter())
        .find(|(_, n)| n == arm_name)
        .map_or_else(|| fatal("unknown arm"), |(a, _)| *a);
    let q = robust_case(case);
    let before = vm_hwm_kb();
    let h = std::thread::Builder::new()
        .stack_size(stack_kb * 1024)
        .spawn(move || {
            let t = Instant::now();
            let r = run_arm(arm, &q);
            let ms = t.elapsed().as_secs_f64() * 1e3;
            let s = match r {
                Ok(_) => "ok".to_string(),
                Err(e) => format!(
                    "error:{:?}:{}",
                    e.kind,
                    e.message.chars().take(60).collect::<String>()
                ),
            };
            (s, ms)
        });
    let (s, ms) = h
        .unwrap_or_else(|e| fatal(&format!("thread: {e}")))
        .join()
        .unwrap_or_else(|_| ("panic".to_string(), 0.0));
    println!(
        "{{\"status\":{},\"ms\":{ms:.2},\"hwm_before_kb\":{before},\"hwm_after_kb\":{}}}",
        json_str(&s),
        vm_hwm_kb()
    );
}

fn cmd_robust() {
    let exe = std::env::current_exe().unwrap_or_else(|e| fatal(&format!("{e}")));
    for case in ROBUST_CASES {
        for arm in [
            "hand",
            "a-generic",
            "a-postgres",
            "a-generic-raw",
            "a-generic-noguard",
        ] {
            for stack_kb in [2048usize, 8192] {
                let t = Instant::now();
                let out = std::process::Command::new(&exe)
                    .args(["robust-child", case, arm, &stack_kb.to_string()])
                    .output()
                    .unwrap_or_else(|e| fatal(&format!("{e}")));
                let wall = t.elapsed().as_secs_f64() * 1e3;
                let stdout = String::from_utf8_lossy(&out.stdout);
                let child = stdout.trim();
                let status = {
                    use std::os::unix::process::ExitStatusExt;
                    match (out.status.code(), out.status.signal()) {
                        (_, Some(sig)) => format!("\"crash: signal {sig}\""),
                        (Some(0), _) => "null".to_string(),
                        (c, _) => format!("\"exit {c:?}\""),
                    }
                };
                let child = if child.is_empty() { "{}" } else { child };
                println!(
                    "{{\"case\":\"{case}\",\"arm\":\"{arm}\",\"stack_kb\":{stack_kb},\"crash\":{status},\"wall_ms\":{wall:.1},\"child\":{child}}}"
                );
            }
        }
    }
}

/// The smallest thread stack (KB) at which a case of `robust` completes without crashing, by trying a ladder of sizes in child processes.
fn cmd_stack_need(args: &[String]) {
    let exe = std::env::current_exe().unwrap_or_else(|e| fatal(&format!("{e}")));
    let (arm, case) = (&args[2], &args[3]);
    for kb in [
        64usize, 96, 128, 192, 256, 384, 512, 768, 1024, 1536, 2048, 3072, 4096, 6144, 8192, 12288,
        16384, 32768, 65536,
    ] {
        let out = std::process::Command::new(&exe)
            .args(["robust-child", case, arm, &kb.to_string()])
            .output()
            .unwrap_or_else(|e| fatal(&format!("{e}")));
        if out.status.success() {
            println!("{{\"arm\":\"{arm}\",\"case\":\"{case}\",\"min_stack_kb\":{kb}}}");
            return;
        }
    }
    println!("{{\"arm\":\"{arm}\",\"case\":\"{case}\",\"min_stack_kb\":null}}");
}
