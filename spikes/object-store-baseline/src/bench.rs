//! The reference measurement: DuckDB (httpfs) and our arms over the same Parquet objects in a bucket, alternating, with twins, one CSV row per
//! run. Correctness against DuckDB-on-local-files is checked on every run before it counts. The bucket, endpoint and prefix come ONLY from
//! the environment and never reach a file (the redactor guards every message; the job scans the results for them).

use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use object_store::ObjectStore;
use object_store::path::Path;

use crate::cache::{CacheMutants, MetaCache, MetaService};
use crate::engine::{Burner, Exec, ExecKind, PolicyKind, QueryKind, ScanCfg, scan};
use crate::policy::{Config, Mutants};
use crate::remote::{Kind, Remote, S3Spec, build_gcs, build_s3};
use crate::util::{Redactor, env_nonempty, median};

#[derive(Clone)]
pub enum StoreKind {
    Gcs { bucket: String },
    S3 { spec: S3Spec },
    /// A local directory (tests of upload/list/delete only; no DuckDB arm).
    Fs { dir: String },
}

#[derive(Clone)]
pub struct StoreCfg {
    pub kind: StoreKind,
    pub prefix: String,
}

impl StoreCfg {
    pub fn from_env() -> Result<StoreCfg, String> {
        let prefix = env_nonempty("NATA_OBJ_PREFIX").ok_or("NATA_OBJ_PREFIX is not set")?;
        let bucket = env_nonempty("NATA_OBJ_BUCKET").ok_or("NATA_OBJ_BUCKET is not set")?;
        match env_nonempty("NATA_OBJ_KIND").as_deref() {
            Some("fs") => Ok(StoreCfg { kind: StoreKind::Fs { dir: bucket }, prefix }),
            Some("s3") => Ok(StoreCfg {
                kind: StoreKind::S3 {
                    spec: S3Spec {
                        endpoint: env_nonempty("NATA_OBJ_ENDPOINT").ok_or("NATA_OBJ_ENDPOINT is not set")?,
                        bucket,
                        region: env_nonempty("NATA_OBJ_REGION").unwrap_or("us-east-1".into()),
                        key: env_nonempty("NATA_OBJ_ACCESS_KEY"),
                        secret: env_nonempty("NATA_OBJ_SECRET_KEY"),
                        allow_http: env_nonempty("NATA_OBJ_ENDPOINT").is_some_and(|e| e.starts_with("http://")),
                    },
                },
                prefix,
            }),
            _ => Ok(StoreCfg { kind: StoreKind::Gcs { bucket }, prefix }),
        }
    }
    pub fn build(&self, cfg: &Config, m: &Mutants) -> Result<Arc<dyn ObjectStore>, String> {
        match &self.kind {
            StoreKind::Gcs { bucket } => build_gcs(bucket, cfg, m),
            StoreKind::S3 { spec } => build_s3(spec, cfg, m),
            StoreKind::Fs { dir } => object_store::local::LocalFileSystem::new_with_prefix(dir).map(|s| Arc::new(s) as Arc<dyn ObjectStore>).map_err(|e| e.to_string()),
        }
    }
    pub fn redactor(&self) -> Redactor {
        let mut v = vec![self.prefix.clone()];
        match &self.kind {
            StoreKind::Gcs { bucket } => v.push(bucket.clone()),
            StoreKind::Fs { dir } => v.push(dir.clone()),
            StoreKind::S3 { spec } => {
                v.push(spec.bucket.clone());
                v.push(spec.endpoint.clone());
                v.extend(spec.key.clone());
                v.extend(spec.secret.clone());
            }
        }
        Redactor::new(v)
    }
    pub fn path(&self, rel: &str) -> Path {
        Path::from(format!("{}/{}", self.prefix, rel))
    }
    /// The URL DuckDB reads for `rel`.
    fn duck_url(&self, rel: &str) -> String {
        match &self.kind {
            StoreKind::Gcs { bucket } => format!("https://storage.googleapis.com/{bucket}/{}/{rel}", self.prefix),
            StoreKind::S3 { spec } => format!("s3://{}/{}/{rel}", spec.bucket, self.prefix),
            StoreKind::Fs { .. } => String::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Cell {
    pub dir: String,   // e.g. sf1/small1
    pub files: usize,
    pub sf: f64,
    pub query: String, // full | proj | sel | meta
}

impl Cell {
    pub fn paths(&self, sc: &StoreCfg) -> Vec<Path> {
        (0..self.files).map(|i| sc.path(&format!("{}/part-{i:04}.parquet", self.dir))).collect()
    }
    pub fn rels(&self) -> Vec<String> {
        (0..self.files).map(|i| format!("{}/part-{i:04}.parquet", self.dir)).collect()
    }
    pub fn id(&self) -> String {
        format!("{}/{}", self.dir, self.query)
    }
}

// ------------------------------------------------------------------------------------------------ process accounting

fn proc_stat_ticks(children: bool) -> f64 {
    let s = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    // fields after the ")" of the comm: state is field 3; utime=14 stime=15 cutime=16 cstime=17 (1-based)
    let rest = s.rsplit_once(')').map(|x| x.1).unwrap_or("");
    let f: Vec<&str> = rest.split_whitespace().collect();
    let g = |i: usize| f.get(i - 3).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
    (if children { g(16) + g(17) } else { g(14) + g(15) }) / 100.0
}

fn vm_hwm_mb(pid: Option<u32>) -> f64 {
    let p = match pid {
        Some(p) => format!("/proc/{p}/status"),
        None => "/proc/self/status".to_string(),
    };
    std::fs::read_to_string(p)
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmHWM:")).and_then(|l| l.split_whitespace().nth(1).and_then(|x| x.parse::<f64>().ok())))
        .map(|kb| kb / 1024.0)
        .unwrap_or(0.0)
}

fn reset_hwm() {
    let _ = std::fs::write("/proc/self/clear_refs", "5");
}

fn load1() -> f64 {
    std::fs::read_to_string("/proc/loadavg").ok().and_then(|s| s.split_whitespace().next().and_then(|x| x.parse().ok())).unwrap_or(0.0)
}

// ------------------------------------------------------------------------------------------------ DuckDB

#[derive(Clone)]
pub enum DuckAuth {
    /// HMAC / anonymous S3 API (MinIO, the fake endpoint).
    S3,
    /// A bearer token for https (GCS XML API with the VM's service-account token).
    Bearer,
}

pub struct DuckRun {
    pub texts: Vec<String>,
    pub timers: Vec<f64>,
    pub wall: f64,
    pub cpu: f64,
    pub rss_mb: f64,
}

fn metadata_token() -> Result<String, String> {
    let out = Command::new("curl")
        .args(["-s", "-m", "5", "-H", "Metadata-Flavor: Google", "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token"])
        .output()
        .map_err(|e| e.to_string())?;
    let t = String::from_utf8_lossy(&out.stdout).to_string();
    let i = t.find("\"access_token\":\"").ok_or("no token from the metadata server")? + 16;
    Ok(t[i..].split('"').next().unwrap_or("").to_string())
}

pub fn duck_script(sc: &StoreCfg, cell: &Cell, q: &QueryKind, threads: usize, reps: usize, auth: &DuckAuth, token: Option<&str>) -> String {
    let urls: Vec<String> = cell.rels().iter().map(|r| format!("'{}'", sc.duck_url(r))).collect();
    let table = format!("read_parquet([{}])", urls.join(", "));
    // CREATE SECRET prints a result row ("true"); its output is discarded so that the first statement's text is the query's alone
    let mut s = String::from("LOAD httpfs;\n.output /dev/null\n");
    s += &format!("SET threads={threads};\n");
    match (&sc.kind, auth) {
        (StoreKind::S3 { spec }, _) => {
            let (ssl, host) = match spec.endpoint.split_once("://") {
                Some(("http", r)) => (false, r),
                Some((_, r)) => (true, r),
                None => (true, spec.endpoint.as_str()),
            };
            s += &format!("SET s3_endpoint='{}';\nSET s3_url_style='path';\nSET s3_use_ssl={ssl};\nSET s3_region='{}';\n", host.trim_end_matches('/'), spec.region);
            if let (Some(k), Some(sec)) = (&spec.key, &spec.secret) {
                s += &format!("CREATE OR REPLACE SECRET osb_s3 (TYPE S3, KEY_ID '{k}', SECRET '{sec}', ENDPOINT '{}', URL_STYLE 'path', USE_SSL {ssl}, REGION '{}');\n", host.trim_end_matches('/'), spec.region);
            }
        }
        (StoreKind::Gcs { .. } | StoreKind::Fs { .. }, _) => {
            if let Some(t) = token {
                s += &format!("CREATE OR REPLACE SECRET osb_http (TYPE http, BEARER_TOKEN '{t}', SCOPE 'https://storage.googleapis.com/');\n");
            }
        }
    }
    s += ".output\n.timer on\n";
    let sql = q.sql().replace("{T}", &table);
    for _ in 0..reps {
        s += &sql;
        s += ";\n";
    }
    s
}

pub fn run_duck(script: &str, redact: &Redactor) -> Result<DuckRun, String> {
    let c0 = proc_stat_ticks(true);
    let t0 = Instant::now();
    let mut child = Command::new("duckdb")
        .args(["-bail", "-list", "-noheader", "-separator", "|", "-nullvalue", "NULL"])
        .env_remove("AI_AGENT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start duckdb: {e}"))?;
    child.stdin.take().ok_or("no stdin")?.write_all(script.as_bytes()).map_err(|e| e.to_string())?;
    let pid = child.id();
    let mut rss = 0f64;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                rss = rss.max(vm_hwm_mb(Some(pid)));
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let wall = t0.elapsed().as_secs_f64();
    let cpu = proc_stat_ticks(true) - c0;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() || stderr.contains("Error") {
        return Err(redact.apply(&format!("duckdb failed ({}): {} {}", out.status, stderr.trim(), stdout.trim())).chars().take(600).collect());
    }
    let (mut texts, mut timers) = (Vec::new(), Vec::new());
    let mut cur: Vec<&str> = Vec::new();
    for l in stdout.lines() {
        if let Some(rest) = l.strip_prefix("Run Time (s):") {
            let mut it = rest.split_whitespace();
            while let Some(w) = it.next() {
                if w == "real" {
                    timers.push(it.next().and_then(|x| x.parse::<f64>().ok()).unwrap_or(f64::NAN));
                }
            }
            texts.push(cur.join("\n"));
            cur.clear();
        } else if !l.is_empty() {
            cur.push(l);
        }
    }
    Ok(DuckRun { texts, timers, wall, cpu, rss_mb: rss })
}

// ------------------------------------------------------------------------------------------------ runs

#[derive(Clone, Debug)]
pub struct Row {
    pub cell: String,
    pub arm: String,
    pub round: usize,
    pub rep: usize,
    pub workers: usize,
    pub ok: bool,
    pub wall: f64,
    pub first: f64,
    pub needed: u64,
    pub fetched: u64,
    pub heads: u64,
    pub footer_gets: u64,
    pub data_gets: u64,
    pub retries: u64,
    pub wire: u64,
    pub cpu: f64,
    pub rss_mb: f64,
    pub fb50: f64,
    pub fb99: f64,
    pub note: String,
}

impl Row {
    pub fn csv_header() -> &'static str {
        "cell,arm,round,rep,workers,ok,wall_s,first_unit_s,needed_bytes,fetched_bytes,heads,footer_gets,data_gets,retries,wire_bytes,cpu_s,rss_mb,fb_p50_ms,fb_p99_ms,note"
    }
    pub fn csv(&self) -> String {
        format!(
            "{},{},{},{},{},{},{:.4},{:.4},{},{},{},{},{},{},{},{:.3},{:.1},{:.1},{:.1},{}",
            self.cell, self.arm, self.round, self.rep, self.workers, self.ok as u8, self.wall, self.first, self.needed, self.fetched, self.heads, self.footer_gets, self.data_gets, self.retries, self.wire, self.cpu, self.rss_mb, self.fb50, self.fb99, self.note.replace(',', ";")
        )
    }
}

pub struct Bench {
    pub sc: StoreCfg,
    pub cfg: Config,
    pub io: tokio::runtime::Runtime,
    pub execs: HashMap<&'static str, Arc<Exec>>,
    pub workers: usize,
    pub expected: HashMap<String, String>,
    pub redact: Redactor,
    pub saturate: bool,
}

/// Arms: `duck`, `ctl` (unmerged parallel), `wrap` (ADR 0011 merging, per-row-group), `wrapx8` (cross-row-group merging, card rule 2),
/// `wrapc` (wrap + bounded metadata cache, shared by the reps), `wrapT` (wrap with the decode on a separate tokio runtime).
/// A trailing `2` is the twin (the same configuration again).
pub fn base_arm(arm: &str) -> &str {
    arm.strip_suffix('2').unwrap_or(arm)
}

impl Bench {
    pub fn run_arm(&self, cell: &Cell, arm: &str, round: usize) -> Vec<Row> {
        let q = QueryKind::parse(&cell.query, cell.sf).expect("query");
        let want = self.expected.get(&format!("{}|{}", cell.dir.split('/').next().unwrap_or(""), cell.query)).cloned().unwrap_or_default();
        let reps = if cell.query == "meta" { 5 } else { 1 };
        let mk = |rep: usize, wall: f64, text: &str| Row {
            // note filled below when the answer differs
            cell: cell.id(),
            arm: arm.to_string(),
            round,
            rep,
            workers: self.workers,
            ok: !want.is_empty() && text == want,
            // (the answer text is numbers only: on a mismatch it is the first thing to look at)
            wall,
            first: 0.0,
            needed: 0,
            fetched: 0,
            heads: 0,
            footer_gets: 0,
            data_gets: 0,
            retries: 0,
            wire: 0,
            cpu: 0.0,
            rss_mb: 0.0,
            fb50: 0.0,
            fb99: 0.0,
            note: if !want.is_empty() && text != want { format!("MISMATCH got={}", text.chars().take(80).collect::<String>().replace('\n', "/")) } else { String::new() },
        };
        if base_arm(arm) == "duck" {
            let token = if matches!(self.sc.kind, StoreKind::Gcs { .. }) { metadata_token().ok() } else { None };
            let script = duck_script(&self.sc, cell, &q, self.workers, reps, &DuckAuth::Bearer, token.as_deref());
            return match run_duck(&script, &self.redact) {
                Ok(d) => (0..d.timers.len())
                    .map(|i| {
                        let mut r = mk(i, d.timers[i], d.texts.get(i).map(|s| s.as_str()).unwrap_or(""));
                        // the process-level numbers belong to the whole session: on the first rep only
                        if i == 0 {
                            r.cpu = d.cpu;
                            r.rss_mb = d.rss_mb;
                            r.note = format!("process_wall={:.3}", d.wall);
                        }
                        r
                    })
                    .collect(),
                Err(e) => {
                    let mut r = mk(0, f64::NAN, "");
                    r.note = e;
                    vec![r]
                }
            };
        }
        let b = base_arm(arm).to_string();
        let (policy, ek, cache_on) = match b.as_str() {
            "ctl" => (PolicyKind::Unmerged, ExecKind::Split, false),
            "wrap" => (PolicyKind::Merged { rg_per_fetch: 1 }, ExecKind::Split, false),
            "wrapx8" => (PolicyKind::Merged { rg_per_fetch: 8 }, ExecKind::Split, false),
            "wrapc" => (PolicyKind::Merged { rg_per_fetch: 1 }, ExecKind::Split, true),
            "wrapT" => (PolicyKind::Merged { rg_per_fetch: 1 }, ExecKind::TokioCpu, false),
            _ => panic!("unknown arm {arm}"),
        };
        let exec = self.execs[if ek == ExecKind::Split { "split" } else { "tokio" }].clone();
        let cache = if cache_on { Some(MetaCache::adr(CacheMutants::default())) } else { None };
        let paths = cell.paths(&self.sc);
        let mut rows = Vec::new();
        reset_hwm();
        let cpu0 = proc_stat_ticks(false);
        let burner = if self.saturate { Some(Burner::start(&exec, self.io.handle(), self.workers)) } else { None };
        // the store is built per run: its connections and credentials are cold, like DuckDB's fresh process
        for rep in 0..reps {
            let (sc, cfg, exec2, cache2, policy2, q2, paths2) = (self.sc.clone(), self.cfg.clone(), exec.clone(), cache.clone(), policy.clone(), q.clone(), paths.clone());
            let red = self.redact.clone();
            let _ = rep;
            let t0 = Instant::now();
            // one store (and its connection pool) per run, shared by the reps of a run: rep 0 is cold, later reps are warm connections
            let out = self.io.block_on(async {
                let store = sc.build(&cfg, &Mutants::default()).map_err(|e| red.apply(&e))?;
                let remote = Remote::new(store, cfg, Mutants::default(), 1);
                let query = remote.query();
                let svc = MetaService::new(cache2, "gcs", "bench");
                let scfg = ScanCfg { policy: policy2, exec: exec2, files_ahead: 8, units_ahead: (self.workers * 2).max(4) };
                let r = scan(&query, &svc, &paths2, &q2, &scfg).await.map_err(|e| e.to_string());
                let st = remote.stats.snap();
                let fb: Vec<f64> = remote.stats.recs.lock().unwrap().iter().filter(|x| x.2 != 0 && x.3).map(|x| x.0).collect();
                r.map(|o| (o, st, crate::util::percentile(&fb, 50.0), crate::util::percentile(&fb, 99.0)))
            });
            let wall = t0.elapsed().as_secs_f64();
            match out {
                Ok((o, st, p50, p99)) => {
                    let mut r = mk(rep, wall, &o.text);
                    r.fb50 = p50;
                    r.fb99 = p99;
                    r.first = o.first_unit.as_secs_f64();
                    r.needed = o.needed_bytes;
                    r.fetched = o.fetched_bytes;
                    r.heads = st.requests[Kind::Head as usize];
                    r.footer_gets = st.requests[Kind::Footer as usize];
                    r.data_gets = st.requests[Kind::Data as usize];
                    r.retries = st.retries;
                    r.wire = st.wire_bytes.iter().sum();
                    rows.push(r);
                }
                Err(e) => {
                    let mut r = mk(rep, wall, "");
                    r.note = e;
                    rows.push(r);
                }
            }
            if reps > 1 {
                // the store is dropped between reps of this loop, so every rep is cold at the connection level; the CACHE (if any) persists
            }
        }
        if let Some(bn) = burner {
            let done = self.io.block_on(bn.stop());
            if let Some(r) = rows.first_mut() {
                r.note = format!("burner_morsels={done}");
            }
        }
        let cpu = proc_stat_ticks(false) - cpu0;
        let rss = vm_hwm_mb(None);
        if let Some(r) = rows.first_mut() {
            r.cpu = cpu;
            r.rss_mb = rss;
        }
        rows
    }
}

/// Auth + first request cost: N fresh stores doing one HEAD, then N HEADs on one warm store.
pub fn startup_probe(sc: &StoreCfg, cfg: &Config, io: &tokio::runtime::Runtime, rel: &str, n: usize) -> (Vec<f64>, Vec<f64>) {
    let path = sc.path(rel);
    let mut cold = Vec::new();
    for _ in 0..n {
        let t = Instant::now();
        let r = io.block_on(async {
            let store = sc.build(cfg, &Mutants::default())?;
            let remote = Remote::new(store, cfg.clone(), Mutants::default(), 1);
            remote.query().head(&path).await.map(|_| ()).map_err(|e| e.to_string())
        });
        if r.is_ok() {
            cold.push(t.elapsed().as_secs_f64() * 1e3);
        }
    }
    let mut warm = Vec::new();
    let store = sc.build(cfg, &Mutants::default()).unwrap();
    let remote = Remote::new(store, cfg.clone(), Mutants::default(), 1);
    let q = remote.query();
    for i in 0..=n {
        let t = Instant::now();
        let r = io.block_on(q.head(&path));
        if r.is_ok() && i > 0 {
            warm.push(t.elapsed().as_secs_f64() * 1e3);
        }
    }
    (cold, warm)
}

pub fn settle(label: &str) -> String {
    match env_nonempty("NATA_SETTLE") {
        Some(script) => {
            let out = Command::new("bash").args([&script, label, "0.5", "300"]).output();
            out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
        }
        None => format!("SETTLE {label}: not requested, load {:.2}", load1()),
    }
}

pub fn median_of(v: &[f64]) -> f64 {
    median(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gcs_script_discards_the_secret_row_and_carries_the_token_only_in_the_secret() {
        // found by the first GCP attempt: CREATE SECRET prints a "true" row that polluted the first answer; no local trial had used a secret
        let sc = StoreCfg { kind: StoreKind::Gcs { bucket: "bkt".into() }, prefix: "p".into() };
        let cell = Cell { dir: "sf1/small1".into(), files: 1, sf: 1.0, query: "full".into() };
        let s = duck_script(&sc, &cell, &QueryKind::Full, 4, 1, &DuckAuth::Bearer, Some("TOK"));
        let i_out = s.find(".output /dev/null").unwrap();
        let i_secret = s.find("BEARER_TOKEN 'TOK'").unwrap();
        let i_back = s.find("\n.output\n").unwrap();
        let i_timer = s.find(".timer on").unwrap();
        assert!(i_out < i_secret && i_secret < i_back && i_back < i_timer);
        assert!(s.contains("https://storage.googleapis.com/bkt/p/sf1/small1/part-0000.parquet"));
        assert_eq!(s.matches("TOK").count(), 1);
    }
}
