//! The writers. One operation per `Writer::run`: an append, a delete-where (position deletes), or a compaction
//! (replace). The CORRECT protocol is what the model describes; three KNOWN-BAD variants each remove one protection
//! and must be caught by the checker:
//!   BadNoCas          no assert-ref-snapshot-id (REST) / overwrite instead of create-if-absent (store); it refreshes the
//!                     sequence number and parent right before committing, so the server accepts a commit whose manifest
//!                     list was built from a stale table (lost update)
//!   BadBlindRetry     after CommitStateUnknown it retries on the latest table without looking for its own snapshot
//!                     (duplicate commit)
//!   BadSkipValidation after a conflict it rebuilds on the latest table without validating (compaction resurrects
//!                     deleted rows, a delete misses files that were rewritten or rows that were added)
use crate::Res;
use crate::catalog::{Catalog, CommitError, CommitReq, NewSnapshot, TableState};
use crate::ice::{self, Entry, MFile, Row, View};
use crate::{Prng, now_ms};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Append,
    Delete,
    Compact,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self { Kind::Append => "append", Kind::Delete => "delete", Kind::Compact => "compact" }
    }
    pub fn parse(s: &str) -> Res<Kind> {
        match s { "append" => Ok(Kind::Append), "delete" => Ok(Kind::Delete), "compact" => Ok(Kind::Compact), _ => Err(format!("unknown kind {s}")) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Correct,
    BadNoCas,
    BadBlindRetry,
    BadSkipValidation,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Correct => "correct",
            Mode::BadNoCas => "bad-no-cas",
            Mode::BadBlindRetry => "bad-blind-retry",
            Mode::BadSkipValidation => "bad-skip-validation",
        }
    }
    pub fn parse(s: &str) -> Res<Mode> {
        match s {
            "correct" => Ok(Mode::Correct),
            "bad-no-cas" => Ok(Mode::BadNoCas),
            "bad-blind-retry" => Ok(Mode::BadBlindRetry),
            "bad-skip-validation" => Ok(Mode::BadSkipValidation),
            _ => Err(format!("unknown mode {s}")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Isolation {
    Serializable,
    Snapshot,
}

impl Isolation {
    pub fn name(self) -> &'static str {
        match self { Isolation::Serializable => "serializable", Isolation::Snapshot => "snapshot" }
    }
    pub fn parse(s: &str) -> Res<Isolation> {
        match s { "serializable" => Ok(Isolation::Serializable), "snapshot" => Ok(Isolation::Snapshot), _ => Err(format!("unknown isolation {s}")) }
    }
}

/// Deterministic ordering of commits for the scenarios that need a particular interleaving: every writer reads the table
/// and writes its files, waits at a barrier until all `n` have, then commits in rank order.
#[derive(Clone, Debug)]
pub struct SyncCfg {
    pub dir: PathBuf,
    pub rank: u32,
    pub n: u32,
    pub timeout: Duration,
}

impl SyncCfg {
    fn touch(&self, name: &str) {
        let _ = std::fs::create_dir_all(&self.dir);
        let _ = std::fs::write(self.dir.join(name), b"");
    }
    fn wait_for(&self, ok: impl Fn() -> bool) {
        let t0 = Instant::now();
        while !ok() && t0.elapsed() < self.timeout {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    pub fn barrier(&self) {
        self.touch(&format!("ready-{}", self.rank));
        let (dir, n) = (self.dir.clone(), self.n as usize);
        self.wait_for(|| std::fs::read_dir(&dir).map(|d| d.filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().starts_with("ready-")).count() >= n).unwrap_or(false));
    }
    pub fn wait_turn(&self) {
        if self.rank > 0 {
            let prev = self.dir.join(format!("done-{}", self.rank - 1));
            self.wait_for(|| prev.exists());
        }
    }
    pub fn done(&self) {
        self.touch(&format!("done-{}", self.rank));
    }
}

/// Append-only, fsynced journal of what a writer intended and what it was told. Written BEFORE the step it describes,
/// so a writer killed at any point leaves a truthful record.
pub struct Journal {
    file: std::sync::Mutex<std::fs::File>,
}

impl Journal {
    pub fn open(dir: &Path, name: &str) -> Res<Journal> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(format!("{name}.jsonl"))).map_err(|e| e.to_string())?;
        Ok(Journal { file: std::sync::Mutex::new(f) })
    }
    pub fn log(&self, v: Value) {
        if let Ok(mut f) = self.file.lock() {
            let mut line = v.to_string();
            line.push('\n');
            let _ = f.write_all(line.as_bytes());
            let _ = f.sync_data();
        }
    }
}

#[derive(Clone, Debug)]
pub struct WriterCfg {
    pub id: u32,
    pub ordinal: u32,
    pub kind: Kind,
    pub mode: Mode,
    pub isolation: Isolation,
    /// Keys for appended rows (row i gets keys[i % len]); for a delete, keys[0] is the predicate.
    pub keys: Vec<String>,
    pub rows: usize,
    pub max_attempts: u32,
    pub seed: u64,
    /// (step, attempt; 0 = any attempt): die there.
    pub crash: Option<(String, u32)>,
    /// true: std::process::exit(137) (what a SIGKILL looks like to everyone else); false: return Outcome::Crashed (tests).
    pub hard_exit: bool,
    pub sync: Option<SyncCfg>,
}

impl WriterCfg {
    pub fn token(&self) -> String {
        format!("w{}o{}", self.id, self.ordinal)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The catalog said OK.
    Acked(i64),
    /// After a conflict or an unknown outcome the writer found its own snapshot in the table.
    Recovered(i64),
    /// Validation failed on retry (a conflicting commit): nothing committed, files left as orphans.
    Aborted(String),
    Rejected(u16, String),
    GaveUp,
    Crashed(String),
    Failed(String),
}

struct Plan {
    /// Delete: (data file, position) pairs and the data files that hold matching rows.
    targets: Vec<(String, i64)>,
    target_files: BTreeSet<String>,
    /// Compaction: the data and delete files rewritten.
    in_data: Vec<(String, i64, i64)>,
    in_deletes: Vec<(String, i64, i64)>,
    /// Manifests of the snapshot the plan was made on (a compaction replaces exactly these).
    base_manifests: BTreeSet<String>,
    new_data_file: Option<(String, i64, i64)>,
    new_delete_file: Option<(String, i64, i64)>,
    /// New manifests of this operation, without the sequence number of any attempt.
    new_manifests: Vec<MFile>,
    rows_added: Vec<Row>,
}

pub struct Writer {
    pub cfg: WriterCfg,
    pub catalog: Arc<dyn Catalog>,
    pub journal: Arc<Journal>,
    prng: Prng,
}

fn size_of(path: &str) -> i64 {
    std::fs::metadata(crate::to_local(path)).map(|m| m.len() as i64).unwrap_or(0)
}

fn view_of(state: &TableState) -> Res<View> {
    match state.current_snap() {
        Some(s) => ice::load_view(&s.manifest_list, false),
        None => Ok(View::default()),
    }
}

impl Writer {
    pub fn new(cfg: WriterCfg, catalog: Arc<dyn Catalog>, journal: Arc<Journal>) -> Writer {
        let prng = Prng::new(cfg.seed ^ ((cfg.id as u64) << 32) ^ cfg.ordinal as u64);
        Writer { cfg, catalog, journal, prng }
    }

    fn log(&self, ev: &str, extra: Value) {
        let mut v = json!({"ev": ev, "op": self.cfg.token(), "writer": self.cfg.id, "kind": self.cfg.kind.name(), "mode": self.cfg.mode.name(), "ts": now_ms()});
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            for (k, x) in e {
                o.insert(k.clone(), x.clone());
            }
        }
        self.journal.log(v);
    }

    /// A crash point. Logged first, then the process dies (or the caller is told to stop).
    fn crash_point(&self, step: &str, attempt: u32) -> Result<(), Outcome> {
        if let Some((s, a)) = &self.cfg.crash {
            if s == step && (*a == 0 || *a == attempt) {
                self.log("crash", json!({"step": step, "attempt": attempt}));
                if self.cfg.hard_exit {
                    std::process::exit(137);
                }
                return Err(Outcome::Crashed(step.to_string()));
            }
        }
        Ok(())
    }

    pub fn run(mut self) -> Outcome {
        let out = self.run_inner();
        let (res, extra) = match &out {
            Outcome::Acked(s) => ("acked", json!({"snapshot": s})),
            Outcome::Recovered(s) => ("recovered", json!({"snapshot": s})),
            Outcome::Aborted(m) => ("aborted", json!({"detail": m})),
            Outcome::Rejected(c, m) => ("rejected", json!({"code": c, "detail": m})),
            Outcome::GaveUp => ("gave_up", json!({})),
            Outcome::Crashed(s) => ("crashed", json!({"step": s})),
            Outcome::Failed(m) => ("failed", json!({"detail": m})),
        };
        self.log("result", merge(json!({"res": res}), extra));
        out
    }

    fn run_inner(&mut self) -> Outcome {
        let token = self.cfg.token();
        let base0 = match self.catalog.load() {
            Ok(s) => s,
            Err(e) => return Outcome::Failed(format!("load: {e}")),
        };
        let view = match view_of(&base0) {
            Ok(v) => v,
            Err(e) => return Outcome::Failed(format!("read base snapshot: {e}")),
        };
        self.log("intent", json!({"keys": self.cfg.keys, "rows": self.cfg.rows, "isolation": self.cfg.isolation.name(),
                                  "base_snapshot": base0.current, "base_seq": base0.last_seq}));
        if let Err(o) = self.crash_point("after-plan", 0) {
            return o;
        }
        // ---- plan and write the files of this operation (done once; a retry reuses them)
        let mut plan = match self.make_plan(&base0, &view) {
            Ok(p) => p,
            Err(e) => return Outcome::Failed(format!("plan: {e}")),
        };
        self.log("planned", json!({"rows_added": plan.rows_added.iter().map(|r| r.id).collect::<Vec<_>>(),
                                   "delete_targets": plan.targets.len(), "compact_inputs": plan.in_data.len()}));
        if let Err(o) = self.write_files(&base0, &mut plan) {
            return o;
        }
        if let Err(o) = self.crash_point("after-files", 0) {
            return o;
        }

        let mut base = base0;
        for attempt in 1..=self.cfg.max_attempts {
            // Scenarios that need a fixed commit order: everybody has read the table and written its files; now wait for
            // the turn. The sequence number, the parent and (for the bad writer) the refresh come after this wait.
            if attempt == 1 {
                if let Some(s) = &self.cfg.sync {
                    s.barrier();
                    s.wait_turn();
                }
            }
            let snapshot_id = self.prng.snapshot_id();
            let mut seq = base.last_seq + 1;
            let mut parent = base.current;
            let carry_from = base.clone();
            // The known-bad no-CAS writer refreshes the sequence number and the parent right before committing (so
            // the catalog accepts it) but keeps the manifest list it built from the stale table.
            if self.cfg.mode == Mode::BadNoCas {
                if let Ok(fresh) = self.catalog.load() {
                    seq = fresh.last_seq + 1;
                    parent = fresh.current;
                    base = fresh;
                }
            }
            let list_path = format!("{}/snap-{}-{}.avro", base.meta_dir(), snapshot_id, token);
            let list = match self.build_list(&carry_from, &plan, seq, snapshot_id) {
                Ok(l) => l,
                Err(e) => return Outcome::Failed(format!("build manifest list: {e}")),
            };
            if let Err(e) = ice::write_manifest_list(&list_path, snapshot_id, parent, seq, &list, self.prng.next()) {
                return Outcome::Failed(format!("write manifest list: {e}"));
            }
            if let Err(o) = self.crash_point("after-list", attempt) {
                return o;
            }
            let mut summary = BTreeMap::new();
            summary.insert("operation".to_string(), match self.cfg.kind { Kind::Append => "append", Kind::Delete => "delete", Kind::Compact => "replace" }.to_string());
            summary.insert("nata.op".to_string(), token.clone());
            summary.insert("nata.kind".to_string(), self.cfg.kind.name().to_string());
            summary.insert("nata.isolation".to_string(), self.cfg.isolation.name().to_string());
            summary.insert("nata.attempt".to_string(), attempt.to_string());
            if self.cfg.kind == Kind::Delete {
                summary.insert("nata.delete-key".to_string(), self.cfg.keys[0].clone());
            }
            let snap = NewSnapshot { id: snapshot_id, parent, seq, manifest_list: list_path, summary };
            self.log("attempt", json!({"attempt": attempt, "snapshot": snapshot_id, "seq": seq, "parent": parent}));
            let req = CommitReq { base: &base, snapshot: snap, require_cas: self.cfg.mode != Mode::BadNoCas };
            let result = self.catalog.commit(&req);
            self.log("answer", json!({"attempt": attempt, "snapshot": snapshot_id, "answer": match &result { Ok(_) => "ok".to_string(), Err(e) => e.to_string() }}));
            if let Err(o) = self.crash_point("after-commit", attempt) {
                return o;
            }
            if attempt == 1 {
                if let Some(s) = &self.cfg.sync {
                    s.done();
                }
            }
            match result {
                Ok(_) => return Outcome::Acked(snapshot_id),
                Err(CommitError::Rejected(code, msg)) => return Outcome::Rejected(code, msg),
                Err(err) => {
                    let unknown = matches!(err, CommitError::Unknown(_));
                    let fresh = match self.catalog.load() {
                        Ok(s) => s,
                        Err(e) => return Outcome::Failed(format!("reload after {err}: {e}")),
                    };
                    let blind = unknown && self.cfg.mode == Mode::BadBlindRetry;
                    if !blind {
                        // Did one of my attempts commit (the answer was lost, or a late request landed)?
                        if let Some(s) = fresh.snaps.iter().find(|s| s.op() == token) {
                            return Outcome::Recovered(s.id);
                        }
                    }
                    if !blind && self.cfg.mode != Mode::BadSkipValidation {
                        match self.validate(&plan, &base, &fresh) {
                            Ok(None) => {}
                            Ok(Some(reason)) => return Outcome::Aborted(reason),
                            Err(e) => return Outcome::Failed(format!("validate: {e}")),
                        }
                    }
                    // The plan is kept: it is re-applied to the new base (Iceberg's retry), not recomputed. The files and
                    // manifests are reused; only the manifest list is rewritten on the next attempt.
                    base = fresh;
                }
            }
        }
        Outcome::GaveUp
    }

    fn make_plan(&mut self, base: &TableState, view: &View) -> Res<Plan> {
        let mut plan = Plan {
            targets: vec![],
            target_files: BTreeSet::new(),
            in_data: vec![],
            in_deletes: vec![],
            base_manifests: view.manifests.iter().map(|m| m.path.clone()).collect(),
            new_data_file: None,
            new_delete_file: None,
            new_manifests: vec![],
            rows_added: vec![],
        };
        let _ = base;
        match self.cfg.kind {
            Kind::Append => {
                for i in 0..self.cfg.rows {
                    plan.rows_added.push(Row { id: self.cfg.id as i64 * 1_000_000 + self.cfg.ordinal as i64 * 1000 + i as i64, key: self.cfg.keys[i % self.cfg.keys.len()].clone() });
                }
            }
            Kind::Delete => {
                let key = &self.cfg.keys[0];
                for df in &view.data {
                    let mut dead: BTreeSet<i64> = BTreeSet::new();
                    for d in view.deletes.iter().filter(|d| d.seq >= df.seq) {
                        for (p, pos) in &d.pairs {
                            if *p == df.path {
                                dead.insert(*pos);
                            }
                        }
                    }
                    for (i, r) in df.rows.iter().enumerate() {
                        if &r.key == key && !dead.contains(&(i as i64)) {
                            plan.targets.push((df.path.clone(), i as i64));
                            plan.target_files.insert(df.path.clone());
                        }
                    }
                }
            }
            Kind::Compact => {
                for df in &view.data {
                    plan.in_data.push((df.path.clone(), df.rows.len() as i64, size_of(&df.path)));
                }
                for d in &view.deletes {
                    plan.in_deletes.push((d.path.clone(), d.pairs.len() as i64, size_of(&d.path)));
                }
                plan.rows_added = view.live_rows();
            }
        }
        Ok(plan)
    }

    fn write_files(&mut self, base: &TableState, plan: &mut Plan) -> Result<(), Outcome> {
        let fail = |e: String| Outcome::Failed(format!("write files: {e}"));
        let token = self.cfg.token();
        let data_dir = base.data_dir();
        let meta_dir = base.meta_dir();
        match self.cfg.kind {
            Kind::Append | Kind::Compact => {
                let path = format!("{data_dir}/{token}.data.avro");
                let size = ice::write_data_file(&path, &plan.rows_added, self.prng.next()).map_err(fail)?;
                plan.new_data_file = Some((path.clone(), plan.rows_added.len() as i64, size));
                self.log("file", json!({"step": "data", "path": path}));
                self.crash_point("after-data", 0)?;
                let mut entries = vec![Entry::added(ice::CONTENT_DATA, &path, plan.rows_added.len() as i64, size)];
                for (p, n, s) in &plan.in_data {
                    entries.push(Entry::deleted(ice::CONTENT_DATA, p, *n, *s));
                }
                let m = ice::write_manifest(&format!("{meta_dir}/{token}-data.avro"), ice::CONTENT_DATA, &entries, self.prng.next()).map_err(fail)?;
                plan.new_manifests.push(m);
                if !plan.in_deletes.is_empty() {
                    let del_entries: Vec<Entry> = plan.in_deletes.iter().map(|(p, n, s)| Entry::deleted(ice::CONTENT_POS_DELETES, p, *n, *s)).collect();
                    let m = ice::write_manifest(&format!("{meta_dir}/{token}-del.avro"), ice::CONTENT_POS_DELETES, &del_entries, self.prng.next()).map_err(fail)?;
                    plan.new_manifests.push(m);
                }
            }
            Kind::Delete => {
                let path = format!("{data_dir}/{token}.deletes.avro");
                let size = ice::write_posdel_file(&path, &plan.targets, self.prng.next()).map_err(fail)?;
                plan.new_delete_file = Some((path.clone(), plan.targets.len() as i64, size));
                self.log("file", json!({"step": "deletes", "path": path}));
                let entries = vec![Entry::added(ice::CONTENT_POS_DELETES, &path, plan.targets.len() as i64, size)];
                let m = ice::write_manifest(&format!("{meta_dir}/{token}-del.avro"), ice::CONTENT_POS_DELETES, &entries, self.prng.next()).map_err(fail)?;
                plan.new_manifests.push(m);
            }
        }
        Ok(())
    }

    /// The manifest list of one attempt: the manifests that stay (from `carry_from`, the table the attempt is based on)
    /// plus this operation's new manifests carrying the attempt's sequence number.
    fn build_list(&self, carry_from: &TableState, plan: &Plan, seq: i64, snapshot_id: i64) -> Res<Vec<MFile>> {
        let mut list: Vec<MFile> = match carry_from.current_snap() {
            Some(s) => ice::read_manifest_list(&s.manifest_list)?,
            None => vec![],
        };
        if self.cfg.kind == Kind::Compact {
            list.retain(|m| !plan.base_manifests.contains(&m.path));
        }
        for m in &plan.new_manifests {
            let mut m = m.clone();
            m.seq = seq;
            m.min_seq = seq;
            m.added_snapshot = snapshot_id;
            list.push(m);
        }
        Ok(list)
    }

    /// Iceberg's rules for re-applying a change to a newer table. Ok(None): valid. Ok(Some(reason)): conflict.
    fn validate(&self, plan: &Plan, base: &TableState, cur: &TableState) -> Res<Option<String>> {
        let cur_view = view_of(cur)?;
        let known: BTreeSet<i64> = base.snaps.iter().map(|s| s.id).collect();
        let intervening: Vec<_> = cur.snaps.iter().filter(|s| !known.contains(&s.id)).collect();
        match self.cfg.kind {
            Kind::Append => Ok(None),
            Kind::Delete => {
                let live = cur_view.data_paths();
                if let Some(missing) = plan.target_files.iter().find(|p| !live.contains(*p)) {
                    return Ok(Some(format!("a data file this delete targets was rewritten or removed: {missing}")));
                }
                if self.cfg.isolation == Isolation::Serializable {
                    for s in intervening.iter().filter(|s| s.operation() != "replace") {
                        for e in ice::added_entries(&s.manifest_list, s.id)?.iter().filter(|e| e.content == ice::CONTENT_DATA) {
                            if ice::read_data_file(&e.path)?.iter().any(|r| r.key == self.cfg.keys[0]) {
                                return Ok(Some(format!("snapshot {} added rows matching the delete predicate (serializable)", s.id)));
                            }
                        }
                    }
                }
                Ok(None)
            }
            Kind::Compact => {
                let live = cur_view.data_paths();
                if let Some((missing, _, _)) = plan.in_data.iter().find(|(p, _, _)| !live.contains(p)) {
                    return Ok(Some(format!("an input data file was rewritten or removed: {missing}")));
                }
                let live_del = cur_view.delete_paths();
                if let Some((missing, _, _)) = plan.in_deletes.iter().find(|(p, _, _)| !live_del.contains(p)) {
                    return Ok(Some(format!("an input delete file was removed: {missing}")));
                }
                let inputs: BTreeSet<&String> = plan.in_data.iter().map(|(p, _, _)| p).collect();
                for s in &intervening {
                    for e in ice::added_entries(&s.manifest_list, s.id)?.iter().filter(|e| e.content == ice::CONTENT_POS_DELETES) {
                        if ice::read_posdel_file(&e.path)?.iter().any(|(p, _)| inputs.contains(p)) {
                            return Ok(Some(format!("snapshot {} added deletes on files this compaction rewrites", s.id)));
                        }
                    }
                }
                Ok(None)
            }
        }
    }
}

fn merge(mut a: Value, b: Value) -> Value {
    if let (Some(x), Some(y)) = (a.as_object_mut(), b.as_object()) {
        for (k, v) in y {
            x.insert(k.clone(), v.clone());
        }
    }
    a
}
