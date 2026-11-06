//! The invariant checker. It looks only at what the catalog and the files say after (or during) a run, plus the writers'
//! journals, and replays EVERY snapshot of the lineage, not just the last one:
//!   I1  every file a snapshot names exists (manifest list, manifests, data and delete files), and every metadata file
//!       the catalog names exists and parses
//!   I2  no lost update, nothing resurrected: the rows of snapshot n equal the rows of snapshot n-1 plus what the
//!       operation of n added minus what it deleted; every acknowledged commit is in the table
//!   I3  a reader of snapshot S sees exactly S: the snapshot record is identical in every metadata version that has it
//!   I4  a delete-where that ran under serializable isolation leaves no matching live row in its own snapshot
//!   I5  an operation commits at most once (also: no live row id appears twice)
//!   I6  sequence numbers strictly increase along the lineage and the table's last-sequence-number is the largest
//!   I7  the lineage is one line: every snapshot is on main's ancestry, no metadata version drops a snapshot, the
//!       snapshot log matches
use crate::catalog::{Catalog, Snap, TableState};
use crate::ice::{self, Row};
use crate::to_local;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct Violation {
    pub inv: &'static str,
    pub msg: String,
}

#[derive(Debug, Default)]
pub struct Report {
    pub violations: Vec<Violation>,
    pub notes: Vec<String>,
    pub snapshots_checked: usize,
    pub metadata_versions_checked: usize,
    pub ops_in_journal: usize,
    pub orphans: Vec<String>,
}

impl Report {
    fn v(&mut self, inv: &'static str, msg: String) {
        self.violations.push(Violation { inv, msg });
    }
    pub fn ok(&self) -> bool {
        self.violations.is_empty()
    }
    pub fn has(&self, inv: &str) -> bool {
        self.violations.iter().any(|v| v.inv == inv)
    }
    pub fn invariants(&self) -> BTreeSet<&'static str> {
        self.violations.iter().map(|v| v.inv).collect()
    }
    pub fn render(&self) -> String {
        let mut s = format!(
            "checked {} snapshots, {} metadata versions, {} journalled operations; orphan files: {}\n",
            self.snapshots_checked, self.metadata_versions_checked, self.ops_in_journal, self.orphans.len()
        );
        if self.violations.is_empty() {
            s.push_str("  no violation\n");
        }
        for v in &self.violations {
            s.push_str(&format!("  VIOLATION {}: {}\n", v.inv, v.msg));
        }
        for n in &self.notes {
            s.push_str(&format!("  note: {n}\n"));
        }
        s
    }
}

#[derive(Debug, Clone, Default)]
pub struct JournalOp {
    pub token: String,
    pub result: Option<String>,
    pub snapshot: Option<i64>,
    pub attempts: Vec<i64>,
    pub unknown_seen: bool,
    pub kind: String,
}

/// Reads every *.jsonl in the directory; a torn last line (writer killed mid-write) is ignored.
pub fn read_journals(dir: &Path) -> BTreeMap<String, JournalOp> {
    let mut ops: BTreeMap<String, JournalOp> = BTreeMap::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return ops };
    for e in rd.filter_map(|e| e.ok()) {
        let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            let Some(tok) = v["op"].as_str() else { continue };
            let op = ops.entry(tok.to_string()).or_insert_with(|| JournalOp { token: tok.to_string(), ..Default::default() });
            match v["ev"].as_str() {
                Some("intent") => op.kind = v["kind"].as_str().unwrap_or("").to_string(),
                Some("attempt") => {
                    if let Some(s) = v["snapshot"].as_i64() {
                        op.attempts.push(s)
                    }
                }
                Some("answer") => {
                    if v["answer"].as_str().map(|a| a.contains("unknown")).unwrap_or(false) {
                        op.unknown_seen = true
                    }
                }
                Some("result") => {
                    op.result = v["res"].as_str().map(String::from);
                    op.snapshot = v["snapshot"].as_i64();
                }
                _ => {}
            }
        }
    }
    ops
}

fn ids(rows: &[Row]) -> BTreeSet<i64> {
    rows.iter().map(|r| r.id).collect()
}

fn short(set: &BTreeSet<i64>) -> String {
    let v: Vec<String> = set.iter().take(8).map(|i| i.to_string()).collect();
    format!("[{}{}] ({} rows)", v.join(","), if set.len() > 8 { ",..." } else { "" }, set.len())
}

pub fn check(catalog: &dyn Catalog, journal_dir: &Path) -> Report {
    let mut rep = Report::default();
    if let Err(e) = catalog.prepare_check() {
        rep.v("I1", format!("the catalog's metadata files could not be copied for the check: {e}"));
        return rep;
    }
    let state = match catalog.load() {
        Ok(s) => s,
        Err(e) => {
            rep.v("I1", format!("cannot load the table from the catalog: {e}"));
            return rep;
        }
    };
    let by_id: BTreeMap<i64, &Snap> = state.snaps.iter().map(|s| (s.id, s)).collect();

    // ---- I7 lineage: main's ancestry, oldest first
    let mut chain: Vec<&Snap> = Vec::new();
    let mut cur = state.current;
    let mut guard = 0;
    while let Some(id) = cur {
        guard += 1;
        if guard > by_id.len() + 1 {
            rep.v("I7", "the parent chain has a cycle".into());
            break;
        }
        match by_id.get(&id) {
            Some(s) => {
                chain.push(s);
                cur = s.parent;
            }
            None => {
                rep.v("I7", format!("snapshot {id} is named as current or parent but is not in the table metadata"));
                break;
            }
        }
    }
    chain.reverse();
    let chain_ids: BTreeSet<i64> = chain.iter().map(|s| s.id).collect();
    for s in &state.snaps {
        if !chain_ids.contains(&s.id) {
            rep.v("I7", format!("snapshot {} (op {}) is in the metadata but not on main's ancestry: history is not linear", s.id, s.op()));
        }
    }
    let log_ids: Vec<i64> = state.snapshot_log.clone();
    let chain_list: Vec<i64> = chain.iter().map(|s| s.id).collect();
    if !log_ids.is_empty() && log_ids != chain_list {
        rep.v("I7", format!("snapshot-log {:?} differs from main's ancestry {:?}", log_ids, chain_list));
    }

    // ---- I6 sequence numbers
    let mut prev_seq = 0;
    for s in &chain {
        if s.seq <= prev_seq {
            rep.v("I6", format!("snapshot {} has sequence number {} which is not greater than its predecessor's {}", s.id, s.seq, prev_seq));
        }
        prev_seq = prev_seq.max(s.seq);
    }
    if let Some(last) = chain.last() {
        if state.last_seq != last.seq {
            rep.v("I6", format!("last-sequence-number is {} but the current snapshot has {}", state.last_seq, last.seq));
        }
    }
    let mut seqs: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for s in &state.snaps {
        seqs.entry(s.seq).or_default().push(s.id);
    }
    for (q, v) in seqs.iter().filter(|(_, v)| v.len() > 1) {
        rep.v("I6", format!("sequence number {q} is used by {} snapshots {:?}", v.len(), v));
    }

    // ---- I1 / I2 / I4 / I5 by replaying every snapshot
    let mut expected: BTreeSet<i64> = BTreeSet::new();
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for s in &chain {
        rep.snapshots_checked += 1;
        referenced.insert(norm(&s.manifest_list));
        let view = match ice::load_view(&s.manifest_list, true) {
            Ok(v) => v,
            Err(e) => {
                rep.v("I1", format!("snapshot {}: {e}", s.id));
                continue;
            }
        };
        for m in &view.missing {
            rep.v("I1", format!("snapshot {} (op {}) names a file that is missing or unreadable: {m}", s.id, s.op()));
        }
        for m in &view.manifests {
            referenced.insert(norm(&m.path));
        }
        for d in &view.data {
            referenced.insert(norm(&d.path));
        }
        for d in &view.deletes {
            referenced.insert(norm(&d.path));
        }
        let rows = view.live_rows();
        let actual = ids(&rows);
        if actual.len() != rows.len() {
            let mut seen = BTreeSet::new();
            let dups: BTreeSet<i64> = rows.iter().filter(|r| !seen.insert(r.id)).map(|r| r.id).collect();
            rep.v("I5", format!("snapshot {} has duplicate live rows {}: an append was committed twice", s.id, short(&dups)));
        }
        // what this operation changed
        let kind = s.summary.get("nata.kind").map(|x| x.as_str()).unwrap_or("");
        let mut next = expected.clone();
        if view.missing.is_empty() {
            match kind {
                "append" => match ice::added_entries(&s.manifest_list, s.id) {
                    Ok(es) => {
                        for e in es.iter().filter(|e| e.content == ice::CONTENT_DATA) {
                            if let Ok(rs) = ice::read_data_file(&e.path) {
                                next.extend(ids(&rs));
                            }
                        }
                    }
                    Err(e) => rep.v("I1", format!("snapshot {}: {e}", s.id)),
                },
                "delete" => match ice::added_entries(&s.manifest_list, s.id) {
                    Ok(es) => {
                        for e in es.iter().filter(|e| e.content == ice::CONTENT_POS_DELETES) {
                            match ice::read_posdel_file(&e.path).and_then(|pairs| ice::rows_at(&BTreeSet::new(), &pairs)) {
                                Ok(rows_deleted) => {
                                    for id in rows_deleted.keys() {
                                        next.remove(id);
                                    }
                                }
                                Err(err) => rep.v("I1", format!("snapshot {}: {err}", s.id)),
                            }
                        }
                    }
                    Err(e) => rep.v("I1", format!("snapshot {}: {e}", s.id)),
                },
                "compact" => {}
                other => rep.notes.push(format!("snapshot {} has no nata.kind ({other:?}); its rows are taken as given", s.id)),
            }
            if kind.is_empty() {
                next = actual.clone();
            }
            let missing: BTreeSet<i64> = next.difference(&actual).cloned().collect();
            let extra: BTreeSet<i64> = actual.difference(&next).cloned().collect();
            if !missing.is_empty() {
                rep.v("I2", format!("snapshot {} ({} by {}): rows that must be live are not: {} (lost update)", s.id, kind, s.op(), short(&missing)));
            }
            if !extra.is_empty() {
                rep.v("I2", format!("snapshot {} ({} by {}): rows are live that should not be: {} (deleted rows came back, or rows that nothing committed)", s.id, kind, s.op(), short(&extra)));
            }
            expected = actual.clone(); // resynchronise: report each divergence once, where it appears
        } else {
            expected = actual.clone();
        }
        // I4
        if kind == "delete" && s.summary.get("nata.isolation").map(|x| x.as_str()) == Some("serializable") {
            if let Some(key) = s.summary.get("nata.delete-key") {
                let matching: BTreeSet<i64> = rows.iter().filter(|r| &r.key == key).map(|r| r.id).collect();
                if !matching.is_empty() {
                    rep.v("I4", format!("snapshot {} (delete where k={key} by {}, serializable) still has matching live rows {}", s.id, s.op(), short(&matching)));
                }
            }
        }
    }

    // ---- I5 each operation at most once
    let mut by_op: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for s in &state.snaps {
        if !s.op().is_empty() {
            by_op.entry(s.op().to_string()).or_default().push(s.id);
        }
    }
    for (op, v) in by_op.iter().filter(|(_, v)| v.len() > 1) {
        rep.v("I5", format!("operation {op} committed {} times (snapshots {:?})", v.len(), v));
    }

    // ---- journals: what writers were told versus what the table holds
    let ops = read_journals(journal_dir);
    rep.ops_in_journal = ops.len();
    for op in ops.values() {
        let present: Vec<i64> = by_op.get(&op.token).cloned().unwrap_or_default();
        match op.result.as_deref() {
            Some("acked") | Some("recovered") => {
                if present.is_empty() {
                    rep.v("I2", format!("operation {} was acknowledged as committed (snapshot {:?}) but no snapshot of it is in the table: lost commit", op.token, op.snapshot));
                }
            }
            Some("aborted") | Some("rejected") => {
                if !present.is_empty() {
                    rep.v("I2", format!("operation {} reported {} but its snapshot(s) {:?} are in the table", op.token, op.result.as_deref().unwrap_or(""), present));
                }
            }
            _ => {
                if !present.is_empty() && !op.attempts.iter().any(|a| present.contains(a)) {
                    rep.v("I2", format!("operation {} has snapshot(s) {:?} that none of its attempts {:?} wrote", op.token, present, op.attempts));
                }
            }
        }
    }
    for t in by_op.keys().filter(|t| !ops.contains_key(*t)) {
        rep.notes.push(format!("operation {t} is in the table but not in any journal"));
    }

    // ---- metadata history: I1 (files exist), I3 (snapshots never change), I7 (never dropped), I6
    check_metadata_history(&state, &mut rep, &mut referenced);

    // ---- orphans
    let root = to_local(&state.location);
    let mut orphans = Vec::new();
    for dir in ["data", "metadata"] {
        if let Ok(rd) = std::fs::read_dir(root.join(dir)) {
            for e in rd.filter_map(|e| e.ok()) {
                let p = e.path();
                let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if name.contains(".tmp-") {
                    continue;
                }
                if !referenced.contains(&norm(&p.display().to_string())) {
                    orphans.push(p.display().to_string());
                }
            }
        }
    }
    orphans.sort();
    rep.orphans = orphans;
    rep
}

fn norm(p: &str) -> String {
    to_local(p).display().to_string()
}

fn check_metadata_history(state: &TableState, rep: &mut Report, referenced: &mut BTreeSet<String>) {
    let mut files: Vec<String> = state.metadata_log.clone();
    if !state.metadata_location.is_empty() {
        files.push(state.metadata_location.clone());
    }
    if files.len() < 2 {
        rep.notes.push("fewer than two metadata versions are listed in the metadata log: the metadata history replay was skipped".into());
        for f in &files {
            referenced.insert(norm(f));
        }
        return;
    }
    let mut prev: Option<(String, BTreeMap<i64, Value>, i64)> = None;
    for f in &files {
        referenced.insert(norm(f));
        let text = match std::fs::read_to_string(to_local(f)) {
            Ok(t) => t,
            Err(e) => {
                rep.v("I1", format!("metadata file {f} named by the catalog cannot be read: {e}"));
                continue;
            }
        };
        let meta: Value = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                rep.v("I1", format!("metadata file {f} does not parse: {e}"));
                continue;
            }
        };
        rep.metadata_versions_checked += 1;
        let mut snaps: BTreeMap<i64, Value> = BTreeMap::new();
        for s in meta["snapshots"].as_array().cloned().unwrap_or_default() {
            if let Some(id) = s["snapshot-id"].as_i64() {
                snaps.insert(id, s);
            }
        }
        let last_seq = meta["last-sequence-number"].as_i64().unwrap_or(0);
        if let Some((pf, psnaps, pseq)) = &prev {
            for (id, rec) in psnaps {
                match snaps.get(id) {
                    None => rep.v("I7", format!("metadata {f} dropped snapshot {id} that {pf} had (a committed snapshot was lost)")),
                    Some(now) if now != rec => rep.v("I3", format!("snapshot {id} differs between metadata {pf} and {f}")),
                    _ => {}
                }
            }
            if last_seq < *pseq {
                rep.v("I6", format!("last-sequence-number went down from {pseq} to {last_seq} between {pf} and {f}"));
            }
        }
        prev = Some((f.clone(), snaps, last_seq));
    }
}
