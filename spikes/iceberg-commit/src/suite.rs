//! The scenarios `gcp-check.sh` runs against a catalog (the reference REST fixture, the object-store mode, or the mock in
//! unit tests). Two kinds:
//!   forced   - deterministic interleavings (barrier + commit order) that make a specific known-bad writer misbehave.
//!              Each is run with the correct writer (must PASS the checker) and with the bad writer it targets (the checker
//!              must report the stated invariant: otherwise the checker is blind to that bug and the run FAILS).
//!   stress   - N writer PROCESSES, random operations, injected lost responses/requests/duplicates, writers that die at a
//!              random step and writers killed with SIGKILL; the correct writer must PASS the checker on every seed.
use crate::Prng;
use crate::catalog::{Catalog, FaultPlan, FaultyCatalog, RestCatalog, StoreCatalog};
use crate::checker::{self, Report};
use crate::writer::{Isolation, Journal, Kind, Mode, Outcome, SyncCfg, Writer, WriterCfg};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

#[derive(Clone, Debug)]
pub enum Backend {
    /// REST catalog at this base URL.
    Rest(String),
    /// Conditional create in a directory under the work dir.
    Store,
}

impl Backend {
    pub fn name(&self) -> &'static str {
        match self { Backend::Rest(_) => "rest", Backend::Store => "store" }
    }
}

pub struct Table {
    pub catalog: Arc<dyn Catalog>,
    pub journal_dir: PathBuf,
    pub backend: Backend,
    pub name: String,
    pub dir: PathBuf,
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A fresh table (unique name) and an empty journal directory.
pub fn new_table(backend: &Backend, work: &Path, label: &str) -> Result<Table, String> {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let name = format!("t{}_{}_{}", std::process::id(), n, label.replace(['-', ' '], "_"));
    let journal_dir = work.join("journals").join(&name);
    std::fs::create_dir_all(&journal_dir).map_err(|e| e.to_string())?;
    match backend {
        Backend::Rest(url) => {
            let c = RestCatalog::connect(url, "nata", &name)?;
            c.ensure_namespace()?;
            c.create_table()?;
            Ok(Table { catalog: Arc::new(c), journal_dir, backend: backend.clone(), name, dir: PathBuf::new() })
        }
        Backend::Store => {
            let dir = work.join("store").join(&name);
            let c = StoreCatalog::new(dir.clone())?;
            c.create_table()?;
            Ok(Table { catalog: Arc::new(c), journal_dir, backend: backend.clone(), name, dir })
        }
    }
}

fn open_for_child(backend: &Backend, name: &str, dir: &Path) -> Result<Arc<dyn Catalog>, String> {
    match backend {
        Backend::Rest(url) => Ok(Arc::new(RestCatalog::connect(url, "nata", name)?)),
        Backend::Store => Ok(Arc::new(StoreCatalog::new(dir.to_path_buf())?)),
    }
}

pub fn writer_cfg(id: u32, ordinal: u32, kind: Kind, mode: Mode, iso: Isolation, keys: &[&str], rows: usize, seed: u64) -> WriterCfg {
    WriterCfg {
        id, ordinal, kind, mode, isolation: iso, keys: keys.iter().map(|k| k.to_string()).collect(), rows,
        max_attempts: 8, seed, crash: None, hard_exit: false, sync: None,
    }
}

/// The first commit of every scenario: some rows with keys a and b.
pub fn seed_table(t: &Table, rows: usize) -> Result<(), String> {
    let j = Arc::new(Journal::open(&t.journal_dir, "w0")?);
    let w = Writer::new(writer_cfg(0, 0, Kind::Append, Mode::Correct, Isolation::Serializable, &["a", "b"], rows, 7), t.catalog.clone(), j);
    match w.run() {
        Outcome::Acked(_) => Ok(()),
        other => Err(format!("seed failed: {other:?}")),
    }
}

// ---------------------------------------------------------------------------------------------------- forced scenarios

#[derive(Clone, Debug)]
pub enum Expect {
    Pass,
    /// The checker must report all of these invariants.
    Fail(Vec<&'static str>),
}

pub struct WriterSpec {
    pub kind: Kind,
    pub keys: Vec<&'static str>,
    pub rows: usize,
    pub faults: &'static str,
}

pub struct Forced {
    pub name: &'static str,
    pub isolation: Isolation,
    pub ordered: bool,
    pub writers: Vec<WriterSpec>,
    /// For each bad mode that this scenario targets: what the checker must find. Mode::Correct is always run and must pass.
    pub bad: Vec<(Mode, Vec<&'static str>)>,
    /// Modes that must also PASS (a bad writer that happens not to misbehave here).
    pub also_pass: Vec<Mode>,
}

pub fn forced_scenarios() -> Vec<Forced> {
    let w = |kind, keys: &[&'static str], rows, faults| WriterSpec { kind, keys: keys.to_vec(), rows, faults };
    vec![
        Forced {
            name: "lost-update", isolation: Isolation::Serializable, ordered: true,
            writers: vec![w(Kind::Append, &["a", "b"], 2, ""), w(Kind::Append, &["a", "b"], 2, ""), w(Kind::Append, &["a", "b"], 2, "")],
            bad: vec![(Mode::BadNoCas, vec!["I2"])], also_pass: vec![],
        },
        Forced {
            name: "unknown-outcome", isolation: Isolation::Serializable, ordered: false,
            writers: vec![w(Kind::Append, &["a"], 3, "drop-response@1")],
            bad: vec![(Mode::BadBlindRetry, vec!["I5"])], also_pass: vec![],
        },
        Forced {
            name: "lost-request", isolation: Isolation::Serializable, ordered: false,
            writers: vec![w(Kind::Append, &["a"], 3, "drop-request@1")],
            bad: vec![], also_pass: vec![Mode::BadBlindRetry],
        },
        Forced {
            name: "duplicate-request", isolation: Isolation::Serializable, ordered: false,
            writers: vec![w(Kind::Append, &["a"], 3, "dup@1")],
            bad: vec![], also_pass: vec![],
        },
        Forced {
            name: "delete-then-compact", isolation: Isolation::Serializable, ordered: true,
            writers: vec![w(Kind::Delete, &["a"], 0, ""), w(Kind::Compact, &["a"], 0, "")],
            bad: vec![(Mode::BadSkipValidation, vec!["I2"])], also_pass: vec![],
        },
        Forced {
            name: "compact-then-delete", isolation: Isolation::Serializable, ordered: true,
            writers: vec![w(Kind::Compact, &["a"], 0, ""), w(Kind::Delete, &["a"], 0, "")],
            bad: vec![(Mode::BadSkipValidation, vec!["I2"])], also_pass: vec![],
        },
        Forced {
            name: "phantom-serializable", isolation: Isolation::Serializable, ordered: true,
            writers: vec![w(Kind::Append, &["a"], 2, ""), w(Kind::Delete, &["a"], 0, "")],
            bad: vec![(Mode::BadSkipValidation, vec!["I4"])], also_pass: vec![],
        },
        Forced {
            name: "phantom-snapshot-isolation", isolation: Isolation::Snapshot, ordered: true,
            writers: vec![w(Kind::Append, &["a"], 2, ""), w(Kind::Delete, &["a"], 0, "")],
            bad: vec![], also_pass: vec![Mode::BadSkipValidation],
        },
        Forced {
            name: "append-then-compact", isolation: Isolation::Serializable, ordered: true,
            writers: vec![w(Kind::Append, &["a", "b"], 2, ""), w(Kind::Compact, &["a"], 0, "")],
            bad: vec![], also_pass: vec![Mode::BadSkipValidation],
        },
        Forced {
            name: "unknown-then-conflict", isolation: Isolation::Serializable, ordered: true,
            writers: vec![w(Kind::Append, &["a"], 2, "drop-response@1"), w(Kind::Append, &["b"], 2, "")],
            bad: vec![], also_pass: vec![],
        },
    ]
}

pub struct ScenarioResult {
    pub name: String,
    pub backend: &'static str,
    pub mode: Mode,
    pub expect: Expect,
    pub report: Report,
    pub outcomes: Vec<Outcome>,
    pub error: Option<String>,
}

impl ScenarioResult {
    pub fn ok(&self) -> bool {
        if self.error.is_some() {
            return false;
        }
        match &self.expect {
            Expect::Pass => self.report.ok(),
            Expect::Fail(inv) => inv.iter().all(|i| self.report.has(i)),
        }
    }
    pub fn line(&self) -> String {
        let want = match &self.expect { Expect::Pass => "PASS".to_string(), Expect::Fail(v) => format!("DETECT {}", v.join("+")) };
        let got = match &self.error {
            Some(e) => format!("ERROR {e}"),
            None if self.report.ok() => "checker: no violation".to_string(),
            None => format!("checker: {}", self.report.invariants().into_iter().collect::<Vec<_>>().join("+")),
        };
        format!("{:<8} {:<28} {:<20} expect {:<14} got {:<28} {}", self.backend, self.name, self.mode.name(), want, got, if self.ok() { "OK" } else { "MISMATCH" })
    }
}

pub fn run_forced(backend: &Backend, work: &Path, sc: &Forced, mode: Mode, expect: Expect) -> ScenarioResult {
    let mut res = ScenarioResult { name: sc.name.to_string(), backend: backend.name(), mode, expect, report: Report::default(), outcomes: vec![], error: None };
    let go = || -> Result<(Report, Vec<Outcome>), String> {
        let t = new_table(backend, work, &format!("{}-{}", sc.name, mode.name()))?;
        seed_table(&t, 6)?;
        let n = sc.writers.len() as u32;
        let sync_dir = work.join("sync").join(&t.name);
        let mut handles = Vec::new();
        for (i, ws) in sc.writers.iter().enumerate() {
            let id = i as u32 + 1;
            let mut cfg = writer_cfg(id, 0, ws.kind, mode, sc.isolation, &ws.keys, ws.rows, 100 + id as u64);
            if sc.ordered {
                cfg.sync = Some(SyncCfg { dir: sync_dir.clone(), rank: i as u32, n, timeout: Duration::from_secs(20) });
            }
            let plan = FaultPlan::parse(ws.faults)?;
            let cat: Arc<dyn Catalog> = if plan.is_empty() { t.catalog.clone() } else { Arc::new(FaultyCatalog::new(t.catalog.clone(), plan)) };
            let journal = Arc::new(Journal::open(&t.journal_dir, &format!("w{id}"))?);
            handles.push(std::thread::spawn(move || Writer::new(cfg, cat, journal).run()));
        }
        let outcomes: Vec<Outcome> = handles.into_iter().map(|h| h.join().unwrap_or(Outcome::Failed("writer thread panicked".into()))).collect();
        Ok((checker::check(t.catalog.as_ref(), &t.journal_dir), outcomes))
    };
    match go() {
        Ok((r, o)) => {
            res.report = r;
            res.outcomes = o;
        }
        Err(e) => res.error = Some(e),
    }
    res
}

/// Every forced scenario with the correct writer (PASS), with each bad writer it targets (DETECT) and with the bad writers
/// that must not misbehave there (PASS).
pub fn run_all_forced(backend: &Backend, work: &Path) -> Vec<ScenarioResult> {
    let mut out = Vec::new();
    for sc in forced_scenarios() {
        out.push(run_forced(backend, work, &sc, Mode::Correct, Expect::Pass));
        for (mode, inv) in &sc.bad {
            out.push(run_forced(backend, work, &sc, *mode, Expect::Fail(inv.clone())));
        }
        for mode in &sc.also_pass {
            out.push(run_forced(backend, work, &sc, *mode, Expect::Pass));
        }
    }
    out
}

/// A writer that dies at each step of the commit path (the process exits as after SIGKILL when `hard_exit`), then other
/// writers carry on: the table must stay consistent and nothing acknowledged may be lost.
pub fn run_crash_matrix(backend: &Backend, work: &Path) -> Vec<ScenarioResult> {
    let mut out = Vec::new();
    for kind in [Kind::Append, Kind::Delete, Kind::Compact] {
        for step in ["after-plan", "after-files", "after-list", "after-commit"] {
            let name = format!("crash-{}-{}", kind.name(), step);
            let mut res = ScenarioResult { name: name.clone(), backend: backend.name(), mode: Mode::Correct, expect: Expect::Pass, report: Report::default(), outcomes: vec![], error: None };
            let go = || -> Result<(Report, Vec<Outcome>), String> {
                let t = new_table(backend, work, &name)?;
                seed_table(&t, 6)?;
                let j1 = Arc::new(Journal::open(&t.journal_dir, "w1")?);
                let mut cfg = writer_cfg(1, 0, kind, Mode::Correct, Isolation::Serializable, &["a"], 2, 11);
                cfg.crash = Some((step.to_string(), 0));
                let o1 = Writer::new(cfg, t.catalog.clone(), j1).run();
                // the survivors: an append, a delete, a compaction, one after the other
                let mut outcomes = vec![o1];
                for (i, k) in [Kind::Append, Kind::Delete, Kind::Compact].into_iter().enumerate() {
                    let id = 2 + i as u32;
                    let j = Arc::new(Journal::open(&t.journal_dir, &format!("w{id}"))?);
                    let cfg = writer_cfg(id, 0, k, Mode::Correct, Isolation::Serializable, &["b"], 2, 20 + id as u64);
                    outcomes.push(Writer::new(cfg, t.catalog.clone(), j).run());
                }
                Ok((checker::check(t.catalog.as_ref(), &t.journal_dir), outcomes))
            };
            match go() {
                Ok((r, o)) => {
                    res.report = r;
                    res.outcomes = o;
                }
                Err(e) => res.error = Some(e),
            }
            out.push(res);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------------- stress

pub struct StressCfg {
    pub writers: u32,
    pub ops_per_writer: u32,
    pub seed: u64,
    pub mode: Mode,
    pub isolation: Isolation,
    /// SIGKILL this many writer processes at a random moment.
    pub kills: u32,
    /// Probability that a writer gets an injected fault / a crash point.
    pub p_fault: f64,
    pub p_crash: f64,
}

pub struct StressResult {
    pub report: Report,
    pub exits: Vec<(u32, String)>,
    pub error: Option<String>,
}

/// N writer processes (`exe writer ...`), each doing a few operations.
pub fn run_stress(backend: &Backend, work: &Path, exe: &Path, cfg: &StressCfg) -> StressResult {
    let go = || -> Result<StressResult, String> {
        let t = new_table(backend, work, &format!("stress-{}-{}", cfg.mode.name(), cfg.seed))?;
        seed_table(&t, 12)?;
        let mut rng = Prng::new(cfg.seed);
        let mut children = Vec::new();
        let compactor = rng.below(cfg.writers as u64) as u32;
        for i in 0..cfg.writers {
            let id = i + 1;
            let mut ops = Vec::new();
            for o in 0..cfg.ops_per_writer {
                let op = if id - 1 == compactor && o == 0 {
                    "compact".to_string()
                } else {
                    let r = rng.below(100);
                    let key = ["a", "b", "c"][rng.below(3) as usize];
                    if r < 55 { format!("append:{key}:{}", 1 + rng.below(3)) } else if r < 92 { format!("delete:{key}") } else { "compact".to_string() }
                };
                ops.push(op);
            }
            let mut faults = Vec::new();
            for n in 1..=3u32 {
                if rng.chance(cfg.p_fault) {
                    faults.push(match rng.below(3) { 0 => format!("drop-response@{n}"), 1 => format!("drop-request@{n}"), _ => format!("dup@{n}") });
                }
            }
            let mut c = Command::new(exe);
            c.arg("writer").args(["--backend", backend.name()]).args(["--table", &t.name]).args(["--dir", &t.dir.display().to_string()])
                .args(["--journal", &t.journal_dir.display().to_string()]).args(["--id", &id.to_string()])
                .args(["--ops", &ops.join(";")]).args(["--mode", cfg.mode.name()]).args(["--isolation", cfg.isolation.name()])
                .args(["--seed", &(cfg.seed * 1000 + id as u64).to_string()]).args(["--faults", &faults.join(",")]);
            if let Backend::Rest(u) = backend {
                c.args(["--url", u]);
            }
            if rng.chance(cfg.p_crash) {
                let step = ["after-plan", "after-files", "after-list", "after-commit"][rng.below(4) as usize];
                c.args(["--crash", &format!("{step}:0:{}", rng.below(cfg.ops_per_writer as u64))]);
            }
            c.stdout(Stdio::null()).stderr(Stdio::piped());
            children.push((id, c.spawn().map_err(|e| format!("spawn {}: {e}", exe.display()))?));
        }
        // SIGKILL a few of them at random moments
        let mut killed = Vec::new();
        for _ in 0..cfg.kills.min(cfg.writers) {
            std::thread::sleep(Duration::from_millis(20 + rng.below(250)));
            let idx = rng.below(children.len() as u64) as usize;
            if !killed.contains(&idx) {
                let _ = children[idx].1.kill();
                killed.push(idx);
            }
        }
        let mut exits = Vec::new();
        for (id, mut ch) in children {
            let st = ch.wait().map_err(|e| e.to_string())?;
            let mut err = String::new();
            if let Some(mut e) = ch.stderr.take() {
                use std::io::Read;
                let _ = e.read_to_string(&mut err);
            }
            let tail: String = err.lines().rev().take(2).collect::<Vec<_>>().join(" | ");
            exits.push((id, format!("{st} {tail}")));
        }
        Ok(StressResult { report: checker::check(t.catalog.as_ref(), &t.journal_dir), exits, error: None })
    };
    go().unwrap_or_else(|e| StressResult { report: Report::default(), exits: vec![], error: Some(e) })
}

/// Entry point of the `writer` subcommand: runs the listed operations one after the other.
pub fn writer_process(args: &std::collections::BTreeMap<String, String>) -> Result<(), String> {
    let get = |k: &str| args.get(k).cloned().ok_or_else(|| format!("missing --{k}"));
    let backend = match get("backend")?.as_str() {
        "rest" => Backend::Rest(get("url")?),
        "store" => Backend::Store,
        other => return Err(format!("unknown backend {other}")),
    };
    let table = get("table")?;
    let dir = PathBuf::from(args.get("dir").cloned().unwrap_or_default());
    let base = open_for_child(&backend, &table, &dir)?;
    let faults = FaultPlan::parse(args.get("faults").map(|s| s.as_str()).unwrap_or(""))?;
    let cat: Arc<dyn Catalog> = if faults.is_empty() { base } else { Arc::new(FaultyCatalog::new(base, faults)) };
    let id: u32 = get("id")?.parse().map_err(|_| "bad --id")?;
    let mode = Mode::parse(&get("mode")?)?;
    let iso = Isolation::parse(&get("isolation")?)?;
    let seed: u64 = get("seed")?.parse().map_err(|_| "bad --seed")?;
    let journal = Arc::new(Journal::open(Path::new(&get("journal")?), &format!("w{id}"))?);
    let crash = match args.get("crash") {
        Some(c) => {
            let p: Vec<&str> = c.split(':').collect();
            if p.len() != 3 {
                return Err(format!("bad --crash {c} (step:attempt:ordinal)"));
            }
            Some((p[0].to_string(), p[1].parse::<u32>().map_err(|_| "bad crash attempt")?, p[2].parse::<u32>().map_err(|_| "bad crash ordinal")?))
        }
        None => None,
    };
    for (ordinal, op) in get("ops")?.split(';').enumerate() {
        let parts: Vec<&str> = op.split(':').collect();
        let kind = Kind::parse(parts[0])?;
        let key = parts.get(1).copied().unwrap_or("a");
        let rows: usize = parts.get(2).and_then(|r| r.parse().ok()).unwrap_or(2);
        let mut cfg = writer_cfg(id, ordinal as u32, kind, mode, iso, &[key], rows, seed);
        cfg.hard_exit = true;
        if let Some((step, att, ord)) = &crash {
            if *ord == ordinal as u32 {
                cfg.crash = Some((step.clone(), *att));
            }
        }
        let _ = Writer::new(cfg, cat.clone(), journal.clone()).run();
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------- interop table

/// Builds one table with every kind of operation (append, delete, compaction), one writer at a time, and describes what a
/// reader must find in each snapshot: the data and delete files. An independent reader (pyiceberg, see
/// scripts/interop_check.py) must list the same files from the same manifests.
pub fn interop_table(backend: &Backend, work: &Path) -> Result<serde_json::Value, String> {
    let t = new_table(backend, work, "interop")?;
    seed_table(&t, 6)?;
    let steps = [(Kind::Append, "a", 2usize), (Kind::Delete, "a", 0), (Kind::Append, "b", 2), (Kind::Compact, "a", 0), (Kind::Append, "a", 1)];
    for (i, (k, key, rows)) in steps.into_iter().enumerate() {
        let id = 1 + i as u32;
        let j = Arc::new(Journal::open(&t.journal_dir, &format!("w{id}"))?);
        let cfg = writer_cfg(id, 0, k, Mode::Correct, Isolation::Serializable, &[key], rows, id as u64);
        match Writer::new(cfg, t.catalog.clone(), j).run() {
            Outcome::Acked(_) => {}
            other => return Err(format!("interop step {i} ({k:?}): {other:?}")),
        }
    }
    let state = t.catalog.load()?;
    let mut chain = Vec::new();
    let mut cur = state.current;
    while let Some(id) = cur {
        let s = state.snap(id).ok_or("dangling parent")?;
        chain.push(s.clone());
        cur = s.parent;
    }
    chain.reverse();
    let mut snaps = Vec::new();
    for s in &chain {
        let v = crate::ice::load_view(&s.manifest_list, false)?;
        snaps.push(serde_json::json!({"id": s.id, "data": v.data_paths(), "deletes": v.delete_paths(), "live_rows": v.live_rows().len()}));
    }
    Ok(serde_json::json!({"table": t.name, "snapshots": snaps}))
}
