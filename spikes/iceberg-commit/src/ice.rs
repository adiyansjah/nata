//! Iceberg v2 files we write ourselves: manifests, manifest lists, tiny data files and position-delete files, all Avro
//! (Iceberg allows Avro as a data file format). The table has two required columns: `id` long (field id 1) and `k`
//! string (field id 2), is unpartitioned and has format version 2.
use crate::avro::{self, Sch, Val, opt_long};
use crate::{Res, to_local, write_atomic};
use std::collections::{BTreeMap, BTreeSet};

pub const TABLE_SCHEMA_JSON: &str = r#"{"type":"struct","schema-id":0,"fields":[{"id":1,"name":"id","required":true,"type":"long"},{"id":2,"name":"k","required":true,"type":"string"}]}"#;

pub const STATUS_EXISTING: i32 = 0;
pub const STATUS_ADDED: i32 = 1;
pub const STATUS_DELETED: i32 = 2;
pub const CONTENT_DATA: i32 = 0;
pub const CONTENT_POS_DELETES: i32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Row {
    pub id: i64,
    pub key: String,
}

fn f(name: &str, id: i64, s: Sch) -> (String, Option<i64>, Sch) {
    (name.to_string(), Some(id), s)
}

pub fn data_schema() -> Sch {
    Sch::Rec("nata_row".into(), vec![f("id", 1, Sch::Long), f("k", 2, Sch::Str)])
}

pub fn posdel_schema() -> Sch {
    Sch::Rec(
        "pos_delete".into(),
        vec![f("file_path", 2147483546, Sch::Str), f("pos", 2147483545, Sch::Long)],
    )
}

pub fn manifest_entry_schema() -> Sch {
    let data_file = Sch::Rec(
        "r2".into(),
        vec![
            f("content", 134, Sch::Int),
            f("file_path", 100, Sch::Str),
            f("file_format", 101, Sch::Str),
            f("partition", 102, Sch::Rec("r102".into(), vec![])),
            f("record_count", 103, Sch::Long),
            f("file_size_in_bytes", 104, Sch::Long),
        ],
    );
    Sch::Rec(
        "manifest_entry".into(),
        vec![
            f("status", 0, Sch::Int),
            f("snapshot_id", 1, Sch::Opt(Box::new(Sch::Long))),
            f("sequence_number", 3, Sch::Opt(Box::new(Sch::Long))),
            f("file_sequence_number", 4, Sch::Opt(Box::new(Sch::Long))),
            f("data_file", 2, data_file),
        ],
    )
}

pub fn manifest_list_schema() -> Sch {
    Sch::Rec(
        "manifest_file".into(),
        vec![
            f("manifest_path", 500, Sch::Str),
            f("manifest_length", 501, Sch::Long),
            f("partition_spec_id", 502, Sch::Int),
            f("content", 517, Sch::Int),
            f("sequence_number", 515, Sch::Long),
            f("min_sequence_number", 516, Sch::Long),
            f("added_snapshot_id", 503, Sch::Long),
            f("added_files_count", 504, Sch::Int),
            f("existing_files_count", 505, Sch::Int),
            f("deleted_files_count", 506, Sch::Int),
            f("added_rows_count", 512, Sch::Long),
            f("existing_rows_count", 513, Sch::Long),
            f("deleted_rows_count", 514, Sch::Long),
        ],
    )
}

// ---------------------------------------------------------------------------------------------------- data files

pub fn write_data_file(path: &str, rows: &[Row], seed: u64) -> Res<i64> {
    let recs: Vec<Val> = rows.iter().map(|r| Val::Rec(vec![Val::Long(r.id), Val::Str(r.key.clone())])).collect();
    let bytes = avro::write_file(&data_schema(), &[("format-version", "2".into())], &recs, seed);
    write_atomic(&to_local(path), &bytes)?;
    Ok(bytes.len() as i64)
}

pub fn read_data_file(path: &str) -> Res<Vec<Row>> {
    let bytes = std::fs::read(to_local(path)).map_err(|e| format!("read {path}: {e}"))?;
    let f = avro::read_file(&bytes, &data_schema()).map_err(|e| format!("{path}: {e}"))?;
    Ok(f.records.iter().map(|r| Row { id: r.rec()[0].long(), key: r.rec()[1].str().to_string() }).collect())
}

pub fn write_posdel_file(path: &str, pairs: &[(String, i64)], seed: u64) -> Res<i64> {
    let recs: Vec<Val> = pairs.iter().map(|(p, n)| Val::Rec(vec![Val::Str(p.clone()), Val::Long(*n)])).collect();
    let bytes = avro::write_file(&posdel_schema(), &[("format-version", "2".into())], &recs, seed);
    write_atomic(&to_local(path), &bytes)?;
    Ok(bytes.len() as i64)
}

pub fn read_posdel_file(path: &str) -> Res<Vec<(String, i64)>> {
    let bytes = std::fs::read(to_local(path)).map_err(|e| format!("read {path}: {e}"))?;
    let f = avro::read_file(&bytes, &posdel_schema()).map_err(|e| format!("{path}: {e}"))?;
    Ok(f.records.iter().map(|r| (r.rec()[0].str().to_string(), r.rec()[1].long())).collect())
}

// ---------------------------------------------------------------------------------------------------- manifests

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub status: i32,
    pub snapshot_id: Option<i64>,
    pub seq: Option<i64>,
    pub file_seq: Option<i64>,
    pub content: i32,
    pub path: String,
    pub records: i64,
    pub size: i64,
}

impl Entry {
    /// A new file: snapshot id and sequence numbers are inherited from the manifest list (null), as the spec allows
    /// and requires for reuse of a manifest across commit attempts.
    pub fn added(content: i32, path: &str, records: i64, size: i64) -> Entry {
        Entry { status: STATUS_ADDED, snapshot_id: None, seq: None, file_seq: None, content, path: path.into(), records, size }
    }
    pub fn deleted(content: i32, path: &str, records: i64, size: i64) -> Entry {
        Entry { status: STATUS_DELETED, snapshot_id: None, seq: None, file_seq: None, content, path: path.into(), records, size }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MFile {
    pub path: String,
    pub length: i64,
    pub spec_id: i32,
    pub content: i32,
    pub seq: i64,
    pub min_seq: i64,
    pub added_snapshot: i64,
    pub added_files: i32,
    pub existing_files: i32,
    pub deleted_files: i32,
    pub added_rows: i64,
    pub existing_rows: i64,
    pub deleted_rows: i64,
}

/// Write a manifest; returns the list entry for it with sequence number 0 (the caller sets the sequence number of the
/// commit attempt when it writes the manifest list, because only the list is rewritten on a retry).
pub fn write_manifest(path: &str, content: i32, entries: &[Entry], seed: u64) -> Res<MFile> {
    let recs: Vec<Val> = entries
        .iter()
        .map(|e| {
            Val::Rec(vec![
                Val::Int(e.status),
                opt_long(e.snapshot_id),
                opt_long(e.seq),
                opt_long(e.file_seq),
                Val::Rec(vec![
                    Val::Int(e.content),
                    Val::Str(e.path.clone()),
                    Val::Str("AVRO".into()),
                    Val::Rec(vec![]),
                    Val::Long(e.records),
                    Val::Long(e.size),
                ]),
            ])
        })
        .collect();
    let meta = [
        ("schema", TABLE_SCHEMA_JSON.to_string()),
        ("schema-id", "0".to_string()),
        ("partition-spec", "[]".to_string()),
        ("partition-spec-id", "0".to_string()),
        ("format-version", "2".to_string()),
        ("content", if content == CONTENT_DATA { "data" } else { "deletes" }.to_string()),
    ];
    let bytes = avro::write_file(&manifest_entry_schema(), &meta, &recs, seed);
    write_atomic(&to_local(path), &bytes)?;
    let count = |st: i32| entries.iter().filter(|e| e.status == st).count() as i32;
    let rows = |st: i32| entries.iter().filter(|e| e.status == st).map(|e| e.records).sum::<i64>();
    Ok(MFile {
        path: path.into(),
        length: bytes.len() as i64,
        spec_id: 0,
        content,
        seq: 0,
        min_seq: 0,
        added_snapshot: 0,
        added_files: count(STATUS_ADDED),
        existing_files: count(STATUS_EXISTING),
        deleted_files: count(STATUS_DELETED),
        added_rows: rows(STATUS_ADDED),
        existing_rows: rows(STATUS_EXISTING),
        deleted_rows: rows(STATUS_DELETED),
    })
}

pub fn read_manifest(path: &str) -> Res<Vec<Entry>> {
    let bytes = std::fs::read(to_local(path)).map_err(|e| format!("read {path}: {e}"))?;
    let f = avro::read_file(&bytes, &manifest_entry_schema()).map_err(|e| format!("{path}: {e}"))?;
    for key in ["schema", "schema-id", "partition-spec", "partition-spec-id", "format-version", "content"] {
        if f.meta(key).is_none() {
            return Err(format!("{path}: manifest header lacks required key {key}"));
        }
    }
    Ok(f.records
        .iter()
        .map(|r| {
            let r = r.rec();
            let d = r[4].rec();
            Entry {
                status: r[0].int(),
                snapshot_id: r[1].opt_long(),
                seq: r[2].opt_long(),
                file_seq: r[3].opt_long(),
                content: d[0].int(),
                path: d[1].str().to_string(),
                records: d[4].long(),
                size: d[5].long(),
            }
        })
        .collect())
}

pub fn write_manifest_list(path: &str, snapshot_id: i64, parent: Option<i64>, seq: i64, list: &[MFile], seed: u64) -> Res<()> {
    let recs: Vec<Val> = list
        .iter()
        .map(|m| {
            Val::Rec(vec![
                Val::Str(m.path.clone()),
                Val::Long(m.length),
                Val::Int(m.spec_id),
                Val::Int(m.content),
                Val::Long(m.seq),
                Val::Long(m.min_seq),
                Val::Long(m.added_snapshot),
                Val::Int(m.added_files),
                Val::Int(m.existing_files),
                Val::Int(m.deleted_files),
                Val::Long(m.added_rows),
                Val::Long(m.existing_rows),
                Val::Long(m.deleted_rows),
            ])
        })
        .collect();
    let meta = [
        ("snapshot-id", snapshot_id.to_string()),
        ("parent-snapshot-id", parent.map(|p| p.to_string()).unwrap_or_else(|| "null".into())),
        ("sequence-number", seq.to_string()),
        ("format-version", "2".to_string()),
    ];
    let bytes = avro::write_file(&manifest_list_schema(), &meta, &recs, seed);
    write_atomic(&to_local(path), &bytes)
}

pub fn read_manifest_list(path: &str) -> Res<Vec<MFile>> {
    let bytes = std::fs::read(to_local(path)).map_err(|e| format!("read {path}: {e}"))?;
    let f = avro::read_file(&bytes, &manifest_list_schema()).map_err(|e| format!("{path}: {e}"))?;
    Ok(f.records
        .iter()
        .map(|r| {
            let r = r.rec();
            MFile {
                path: r[0].str().to_string(),
                length: r[1].long(),
                spec_id: r[2].int(),
                content: r[3].int(),
                seq: r[4].long(),
                min_seq: r[5].long(),
                added_snapshot: r[6].long(),
                added_files: r[7].int(),
                existing_files: r[8].int(),
                deleted_files: r[9].int(),
                added_rows: r[10].long(),
                existing_rows: r[11].long(),
                deleted_rows: r[12].long(),
            }
        })
        .collect())
}

// ---------------------------------------------------------------------------------------------------- snapshot view

#[derive(Clone, Debug)]
pub struct DataFile {
    pub path: String,
    pub seq: i64,
    pub rows: Vec<Row>,
}

#[derive(Clone, Debug)]
pub struct DeleteFile {
    pub path: String,
    pub seq: i64,
    pub pairs: Vec<(String, i64)>,
}

/// Everything a reader of one snapshot sees. `missing` lists files that a manifest list or manifest names and that do
/// not exist or cannot be read (only filled in lenient mode; strict mode fails at the first one).
#[derive(Clone, Debug, Default)]
pub struct View {
    pub manifests: Vec<MFile>,
    pub data: Vec<DataFile>,
    pub deletes: Vec<DeleteFile>,
    pub missing: Vec<String>,
}

impl View {
    /// The live rows (a multiset, sorted): data rows minus the positions that an applicable position delete removes.
    /// A position delete applies to a data file when its sequence number is >= the data file's (spec, Scan Planning).
    pub fn live_rows(&self) -> Vec<Row> {
        let mut out = Vec::new();
        for df in &self.data {
            let mut dead: BTreeSet<i64> = BTreeSet::new();
            for d in self.deletes.iter().filter(|d| d.seq >= df.seq) {
                for (p, pos) in &d.pairs {
                    if *p == df.path {
                        dead.insert(*pos);
                    }
                }
            }
            for (i, r) in df.rows.iter().enumerate() {
                if !dead.contains(&(i as i64)) {
                    out.push(r.clone());
                }
            }
        }
        out.sort();
        out
    }
    pub fn data_paths(&self) -> BTreeSet<String> {
        self.data.iter().map(|d| d.path.clone()).collect()
    }
    pub fn delete_paths(&self) -> BTreeSet<String> {
        self.deletes.iter().map(|d| d.path.clone()).collect()
    }
}

pub fn load_view(manifest_list: &str, lenient: bool) -> Res<View> {
    let mut v = View::default();
    let list = match read_manifest_list(manifest_list) {
        Ok(l) => l,
        Err(e) => {
            if lenient {
                v.missing.push(format!("{manifest_list} ({e})"));
                return Ok(v);
            }
            return Err(e);
        }
    };
    for m in &list {
        let entries = match read_manifest(&m.path) {
            Ok(e) => e,
            Err(e) => {
                if lenient {
                    v.missing.push(format!("{} ({e})", m.path));
                    continue;
                }
                return Err(e);
            }
        };
        for e in entries.iter().filter(|e| e.status != STATUS_DELETED) {
            let seq = e.seq.unwrap_or(m.seq);
            if e.content == CONTENT_DATA {
                match read_data_file(&e.path) {
                    Ok(rows) => v.data.push(DataFile { path: e.path.clone(), seq, rows }),
                    Err(err) if lenient => v.missing.push(format!("{} ({err})", e.path)),
                    Err(err) => return Err(err),
                }
            } else if e.content == CONTENT_POS_DELETES {
                match read_posdel_file(&e.path) {
                    Ok(pairs) => v.deletes.push(DeleteFile { path: e.path.clone(), seq, pairs }),
                    Err(err) if lenient => v.missing.push(format!("{} ({err})", e.path)),
                    Err(err) => return Err(err),
                }
            }
        }
    }
    v.manifests = list;
    Ok(v)
}

/// The files a snapshot ADDED: the entries with status ADDED in the manifests that were added by that snapshot.
pub fn added_entries(manifest_list: &str, snapshot_id: i64) -> Res<Vec<Entry>> {
    let mut out = Vec::new();
    for m in read_manifest_list(manifest_list)?.iter().filter(|m| m.added_snapshot == snapshot_id) {
        out.extend(read_manifest(&m.path)?.into_iter().filter(|e| e.status == STATUS_ADDED));
    }
    Ok(out)
}

/// Row id -> row for every row in the given data file paths (used to turn (path, position) deletes into row ids).
pub fn rows_at(paths: &BTreeSet<String>, pairs: &[(String, i64)]) -> Res<BTreeMap<i64, Row>> {
    let mut cache: BTreeMap<String, Vec<Row>> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for (p, pos) in pairs {
        if !paths.is_empty() && !paths.contains(p) {
            continue;
        }
        if !cache.contains_key(p) {
            cache.insert(p.clone(), read_data_file(p)?);
        }
        let rows = &cache[p];
        let row = rows.get(*pos as usize).ok_or_else(|| format!("{p}: position {pos} out of range"))?;
        out.insert(row.id, row.clone());
    }
    Ok(out)
}
