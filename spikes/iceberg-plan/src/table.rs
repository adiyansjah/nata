//! A local Iceberg v2 table: writing (data files in Parquet with field ids, metrics with spec-style bounds, position and equality delete files,
//! manifests, manifest lists, metadata files, schema and partition evolution) and opening a table written by anyone (pyiceberg, Java).
use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use apache_avro::Codec;
use arrow::array::*;
use arrow::compute::take_record_batch;
use arrow::datatypes::{DataType, Field as AField, Schema as ASchema, SchemaRef, TimeUnit};
use parquet::arrow::PARQUET_FIELD_ID_META_KEY;
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;

use crate::manifest::*;
use crate::meta::*;
use crate::transform::Transform;
use crate::types::*;
use crate::{Res, err, local_path, read_file};

pub fn fid_meta(id: i32) -> std::collections::HashMap<String, String> { std::collections::HashMap::from([(PARQUET_FIELD_ID_META_KEY.to_string(), id.to_string())]) }

pub fn arrow_type(t: &Type) -> DataType {
    match t {
        Type::Boolean => DataType::Boolean, Type::Int => DataType::Int32, Type::Long => DataType::Int64, Type::Float => DataType::Float32, Type::Double => DataType::Float64,
        Type::Decimal(p, s) => DataType::Decimal128(*p, *s as i8), Type::Date => DataType::Date32, Type::Timestamp => DataType::Timestamp(TimeUnit::Microsecond, None),
        Type::TimestampTz => DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())), Type::String => DataType::Utf8, Type::Binary => DataType::Binary,
        Type::Uuid => DataType::FixedSizeBinary(16), Type::Fixed(n) => DataType::FixedSizeBinary(*n as i32), Type::Other(o) => panic!("no arrow type for {o}"),
    }
}

pub fn arrow_schema(s: &Schema) -> SchemaRef {
    Arc::new(ASchema::new(s.fields.iter().map(|f| AField::new(&f.name, arrow_type(&f.ty), !f.required)
        .with_metadata(fid_meta(f.id))).collect::<Vec<_>>()))
}

/// The value of row `i` of an Arrow column, as an Iceberg value of type `t` (None = NULL).
pub fn cell(a: &ArrayRef, i: usize, t: &Type) -> Option<Value> {
    if a.is_null(i) { return None; }
    Some(match (t, a.data_type()) {
        (Type::Boolean, _) => Value::Bool(a.as_boolean().value(i)),
        (Type::Int, DataType::Int32) => Value::Int(a.as_primitive::<arrow::datatypes::Int32Type>().value(i)),
        (Type::Date, DataType::Date32) => Value::Date(a.as_primitive::<arrow::datatypes::Date32Type>().value(i)),
        (Type::Long, DataType::Int64) => Value::Long(a.as_primitive::<arrow::datatypes::Int64Type>().value(i)),
        (Type::Timestamp | Type::TimestampTz, DataType::Timestamp(TimeUnit::Microsecond, _)) => Value::Ts(a.as_primitive::<arrow::datatypes::TimestampMicrosecondType>().value(i)),
        (Type::Float, _) => Value::Float(a.as_primitive::<arrow::datatypes::Float32Type>().value(i)),
        (Type::Double, _) => Value::Double(a.as_primitive::<arrow::datatypes::Float64Type>().value(i)),
        (Type::Decimal(..), _) => Value::Decimal(a.as_primitive::<arrow::datatypes::Decimal128Type>().value(i)),
        (Type::String, DataType::Utf8) => Value::Str(a.as_string::<i32>().value(i).to_string()),
        (Type::String, DataType::LargeUtf8) => Value::Str(a.as_string::<i64>().value(i).to_string()),
        (Type::Binary, DataType::Binary) => Value::Bin(a.as_binary::<i32>().value(i).to_vec()),
        (Type::Binary, DataType::LargeBinary) => Value::Bin(a.as_binary::<i64>().value(i).to_vec()),
        (Type::Uuid, _) => Value::Uuid(a.as_fixed_size_binary().value(i).try_into().unwrap()),
        (Type::Fixed(_), _) => Value::Bin(a.as_fixed_size_binary().value(i).to_vec()),
        (t, d) => panic!("cannot read {t:?} from an arrow column of {d}"),
    })
}

// ---------------------------------------------------------------- metrics

pub const TRUNC: usize = 16;

/// Smallest string greater than every string that starts with `s` truncated: increment the last code point that can be incremented.
fn increment_str(s: &str) -> Option<String> {
    let mut cs: Vec<char> = s.chars().collect();
    while let Some(last) = cs.pop() {
        let mut n = last as u32 + 1;
        if (0xD800..0xE000).contains(&n) { n = 0xE000; }
        if let Some(c) = char::from_u32(n) { cs.push(c); return Some(cs.into_iter().collect()); }
    }
    None
}

fn increment_bytes(b: &[u8]) -> Option<Vec<u8>> {
    let mut v = b.to_vec();
    while let Some(last) = v.pop() { if last < 0xFF { v.push(last + 1); return Some(v); } }
    None
}

/// Lower and upper bound of a column for the manifest: strings and binaries longer than TRUNC are truncated; the upper bound is incremented so that it
/// stays an upper bound (and is omitted if it cannot be).
pub fn bounds_for(min: &Value, max: &Value) -> (Value, Option<Value>) {
    match (min, max) {
        (Value::Str(lo), Value::Str(hi)) => {
            let lo2: String = lo.chars().take(TRUNC).collect();
            let hi2 = if hi.chars().count() > TRUNC { increment_str(&hi.chars().take(TRUNC).collect::<String>()).map(Value::Str) } else { Some(Value::Str(hi.clone())) };
            (Value::Str(lo2), hi2)
        }
        (Value::Bin(lo), Value::Bin(hi)) => {
            let lo2: Vec<u8> = lo.iter().take(TRUNC).copied().collect();
            let hi2 = if hi.len() > TRUNC { increment_bytes(&hi[..TRUNC]).map(Value::Bin) } else { Some(Value::Bin(hi.clone())) };
            (Value::Bin(lo2), hi2)
        }
        _ => (min.clone(), Some(max.clone())),
    }
}

pub fn metrics(schema: &Schema, batch: &RecordBatch, file: &mut DataFile) {
    file.record_count = batch.num_rows() as i64;
    for (ci, f) in schema.fields.iter().enumerate() {
        if matches!(f.ty, Type::Other(_)) { continue; }
        let a = batch.column(ci);
        let (mut nulls, mut nans) = (0i64, 0i64);
        let (mut lo, mut hi): (Option<Value>, Option<Value>) = (None, None);
        for i in 0..a.len() {
            match cell(a, i, &f.ty) {
                None => nulls += 1,
                Some(v) if is_nan(&v) => nans += 1,
                Some(v) => {
                    if lo.as_ref().is_none_or(|l| vcmp(&v, l).is_lt()) { lo = Some(v.clone()); }
                    if hi.as_ref().is_none_or(|h| vcmp(&v, h).is_gt()) { hi = Some(v); }
                }
            }
        }
        file.value_counts.insert(f.id, a.len() as i64);
        file.null_counts.insert(f.id, nulls);
        if matches!(f.ty, Type::Float | Type::Double) { file.nan_counts.insert(f.id, nans); }
        if let (Some(l), Some(h)) = (&lo, &hi) {
            let (l2, h2) = bounds_for(l, h);
            file.lower.insert(f.id, serialize(&l2));
            if let Some(h2) = h2 { file.upper.insert(f.id, serialize(&h2)); }
        }
    }
}

// ---------------------------------------------------------------- the table

pub struct Table {
    pub dir: PathBuf,
    pub meta: TableMeta,
    version: u32,
    pub codec: Codec,
    uid: u64,
}

pub fn uuid_string(seed: u64) -> String {
    let h = |x: u64| { let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15); z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9); z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB); z ^ (z >> 31) };
    let (a, b) = (h(seed), h(seed ^ 0xABCD));
    format!("{:08x}-{:04x}-4{:03x}-a{:03x}-{:012x}", (a >> 32) as u32, (a >> 16) as u16, a & 0xFFF, (b >> 52) & 0xFFF, b & 0xFFFF_FFFF_FFFF)
}

impl Table {
    pub fn create(dir: &Path, schema: Schema, spec: Spec, seed: u64) -> Table {
        std::fs::create_dir_all(dir.join("metadata")).unwrap();
        std::fs::create_dir_all(dir.join("data")).unwrap();
        let dir = dir.canonicalize().unwrap();
        let last_col = schema.fields.iter().map(|f| f.id).max().unwrap_or(0);
        let last_part = spec.fields.iter().map(|f| f.field_id).max().unwrap_or(999);
        let meta = TableMeta { location: dir.to_string_lossy().to_string(), uuid: uuid_string(seed), last_seq: 0, last_column_id: last_col, schemas: vec![schema], current_schema_id: 0,
                               specs: vec![spec], default_spec_id: 0, last_partition_id: last_part, snapshots: vec![], current_snapshot: None };
        let mut t = Table { dir, meta, version: 0, codec: Codec::Null, uid: seed };
        t.commit_metadata();
        t
    }

    /// Open the newest metadata file of a table directory (version-hint, else the highest vN / numbered file), or a metadata file given directly.
    pub fn open(path: &Path) -> Res<Table> {
        let (dir, file) = if path.is_file() { (path.parent().unwrap().parent().unwrap().to_path_buf(), path.to_path_buf()) } else {
            let md = path.join("metadata");
            let mut best: Option<(u32, PathBuf)> = None;
            for e in std::fs::read_dir(&md).map_err(|e| format!("{}: {e}", md.display()))? {
                let p = e.map_err(|e| e.to_string())?.path();
                let n = p.file_name().unwrap().to_string_lossy().to_string();
                if n.ends_with(".metadata.json") {
                    let num: u32 = n.trim_start_matches('v').split(|c: char| !c.is_ascii_digit()).next().and_then(|s| s.parse().ok()).unwrap_or(0);
                    if best.as_ref().is_none_or(|b| num >= b.0) { best = Some((num, p)); }
                }
            }
            (path.to_path_buf(), best.ok_or("no metadata file")?.1)
        };
        let j: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).map_err(|e| e.to_string())?).map_err(|e| format!("metadata json: {e}"))?;
        let meta = TableMeta::from_json(&j)?;
        Ok(Table { dir, meta, version: 1000, codec: Codec::Null, uid: 99 })
    }

    fn next_uid(&mut self) -> u64 { self.uid = self.uid.wrapping_add(1); self.uid }

    fn commit_metadata(&mut self) {
        self.version += 1;
        let p = self.dir.join("metadata").join(format!("v{}.metadata.json", self.version));
        std::fs::write(&p, serde_json::to_vec_pretty(&self.meta.to_json()).unwrap()).unwrap();
        std::fs::write(self.dir.join("metadata").join("version-hint.text"), self.version.to_string()).unwrap();
    }

    pub fn metadata_path(&self) -> PathBuf { self.dir.join("metadata").join(format!("v{}.metadata.json", self.version)) }

    fn live_manifests(&self) -> Res<Vec<ManifestFile>> {
        match self.meta.current_snapshot.and_then(|s| self.meta.snapshot(s)) { Some(s) => read_manifest_list(&read_file(&s.manifest_list)?), None => Ok(vec![]) }
    }

    pub fn part_types(&self, spec: &Spec) -> Vec<Type> {
        let schema = self.meta.current_schema();
        spec.fields.iter().map(|pf| pf.transform.result_type(&self.meta.schemas.iter().find_map(|s| s.field(pf.source_id)).map(|f| f.ty.clone()).unwrap_or(Type::Int))).collect::<Vec<_>>().into_iter().chain(std::iter::empty()).map(|t| { let _ = schema; t }).collect()
    }

    pub fn entries(&self, m: &ManifestFile) -> Res<(ManifestHeader, Vec<Entry>)> {
        let spec = self.meta.spec(m.spec_id).ok_or("manifest with an unknown spec id")?;
        read_manifest(&read_file(&m.path)?, &self.part_types(spec))
    }

    fn write_data_file(&mut self, schema: &Schema, batch: &RecordBatch, subdir: &str) -> (String, i64) {
        let name = format!("{}/{:016x}.parquet", subdir, self.next_uid());
        let path = self.dir.join("data").join(&name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let f = File::create(&path).unwrap();
        let mut w = ArrowWriter::try_new(f, arrow_schema(schema), Some(WriterProperties::builder().build())).unwrap();
        w.write(batch).unwrap();
        w.close().unwrap();
        (path.to_string_lossy().to_string(), std::fs::metadata(&path).unwrap().len() as i64)
    }

    fn snapshot_commit(&mut self, op: &str, manifests: Vec<ManifestFile>) -> i64 {
        self.meta.last_seq += 1;
        let id = 1_000_000 + self.meta.snapshots.len() as i64 * 7 + 3;
        let ml = self.dir.join("metadata").join(format!("snap-{id}.avro"));
        std::fs::write(&ml, write_manifest_list(&manifests, self.codec.clone()).unwrap()).unwrap();
        let parent = self.meta.current_snapshot;
        self.meta.snapshots.push(Snapshot { id, parent, seq: self.meta.last_seq, ts_ms: 1_700_000_000_000 + self.meta.snapshots.len() as i64 * 1000, manifest_list: ml.to_string_lossy().to_string(),
                                            operation: op.to_string(), schema_id: Some(self.meta.current_schema_id) });
        self.meta.current_snapshot = Some(id);
        self.commit_metadata();
        id
    }

    fn write_manifest_file(&mut self, spec: &Spec, content: i32, entries: &[Entry], snap_id: i64, seq: i64) -> ManifestFile {
        let schema = self.meta.current_schema().clone();
        let schema_for_spec = self.meta.schemas.iter().find(|s| spec.fields.iter().all(|f| s.field(f.source_id).is_some())).unwrap_or(&schema).clone();
        let (bytes, partitions) = write_manifest(spec, &schema_for_spec, content, entries, self.codec.clone()).unwrap();
        let path = self.dir.join("metadata").join(format!("{:016x}-m.avro", self.next_uid()));
        std::fs::write(&path, &bytes).unwrap();
        let live = |e: &&Entry| e.status != DELETED;
        let min_seq = entries.iter().filter(live).map(|e| e.seq.unwrap_or(seq)).min().unwrap_or(seq);
        let sum = |st: i32, rows: bool| -> i64 { entries.iter().filter(|e| e.status == st).map(|e| if rows { e.file.record_count } else { 1 }).sum() };
        ManifestFile { path: path.to_string_lossy().to_string(), length: bytes.len() as i64, spec_id: spec.id, content, seq, min_seq, added_snapshot_id: snap_id,
                       added_files: sum(ADDED, false) as i32, existing_files: sum(EXISTING, false) as i32, deleted_files: sum(DELETED, false) as i32,
                       added_rows: sum(ADDED, true), existing_rows: sum(EXISTING, true), deleted_rows: sum(DELETED, true), partitions }
    }

    /// Partition tuple of one row under `spec`.
    fn tuple(&self, spec: &Spec, schema: &Schema, batch: &RecordBatch, row: usize) -> Vec<Option<Value>> {
        spec.fields.iter().map(|pf| {
            let (ci, f) = schema.fields.iter().enumerate().find(|(_, f)| f.id == pf.source_id).expect("partition source column");
            cell(batch.column(ci), row, &f.ty).and_then(|v| pf.transform.apply(&v))
        }).collect()
    }

    /// Append `batch` (columns in the CURRENT schema's order). One data file per partition tuple; one manifest.
    pub fn append(&mut self, batch: &RecordBatch) -> i64 {
        let schema = self.meta.current_schema().clone();
        let spec = self.meta.spec(self.meta.default_spec_id).unwrap().clone();
        let mut groups: BTreeMap<String, (Vec<Option<Value>>, Vec<u32>)> = BTreeMap::new();
        for r in 0..batch.num_rows() {
            let t = self.tuple(&spec, &schema, batch, r);
            groups.entry(format!("{t:?}")).or_insert_with(|| (t.clone(), vec![])).1.push(r as u32);
        }
        let seq = self.meta.last_seq + 1;
        let snap_id = 1_000_000 + self.meta.snapshots.len() as i64 * 7 + 3;
        let mut entries = vec![];
        for (_, (tuple, rows)) in groups {
            let sub = take_record_batch(batch, &UInt32Array::from(rows)).unwrap();
            let label: String = tuple.iter().map(|v| v.as_ref().map_or("null".into(), |v| format!("{v:?}").replace(['"', ' ', '(', ')', '/'], ""))).collect::<Vec<_>>().join("_");
            let (path, size) = self.write_data_file(&schema, &sub, &format!("s{}/{}", spec.id, label));
            let mut f = DataFile { content: DATA, path, format: "parquet".into(), partition: tuple, size, sort_order_id: Some(0), ..Default::default() };
            metrics(&schema, &sub, &mut f);
            entries.push(Entry { status: ADDED, snapshot_id: Some(snap_id), seq: None, file_seq: None, file: f });
        }
        let mut list = self.live_manifests().unwrap();
        let mf = self.write_manifest_file(&spec, DATA, &entries, snap_id, seq);
        list.push(mf);
        self.snapshot_commit("append", list)
    }

    /// All live entries of the current snapshot, with the sequence numbers resolved (inheritance applied), tagged with their manifest.
    pub fn live_entries(&self) -> Res<Vec<(ManifestFile, Entry)>> {
        let mut out = vec![];
        for m in self.live_manifests()? {
            let (_, es) = self.entries(&m)?;
            for mut e in es {
                if e.status == DELETED { continue; }
                if e.seq.is_none() { e.seq = Some(m.seq); }
                if e.file_seq.is_none() { e.file_seq = Some(m.seq); }
                out.push((m.clone(), e));
            }
        }
        Ok(out)
    }

    /// Remove data files (an overwrite / compaction commit): the manifests that hold them are rewritten with those entries marked DELETED.
    pub fn remove_files(&mut self, paths: &[String]) -> i64 {
        let snap_id = 1_000_000 + self.meta.snapshots.len() as i64 * 7 + 3;
        let mut list = vec![];
        for m in self.live_manifests().unwrap() {
            let (_, es) = self.entries(&m).unwrap();
            if !es.iter().any(|e| paths.contains(&e.file.path) && e.status != DELETED) { list.push(m); continue; }
            let spec = self.meta.spec(m.spec_id).unwrap().clone();
            let rewritten: Vec<Entry> = es.into_iter().filter(|e| e.status != DELETED).map(|mut e| {
                if e.seq.is_none() { e.seq = Some(m.seq); }
                if e.file_seq.is_none() { e.file_seq = Some(m.seq); }
                e.status = if paths.contains(&e.file.path) { DELETED } else { EXISTING };
                e.snapshot_id = Some(if e.status == DELETED { snap_id } else { e.snapshot_id.unwrap_or(m.added_snapshot_id) });
                e
            }).collect();
            let mut nm = self.write_manifest_file(&spec, m.content, &rewritten, snap_id, m.seq);
            nm.added_snapshot_id = snap_id;
            list.push(nm);
        }
        self.snapshot_commit("overwrite", list)
    }

    /// A position delete file for `data_path` (deleting the given row positions), in the same partition and spec as that data file.
    pub fn delete_positions(&mut self, data_path: &str, positions: &[i64]) -> i64 {
        let (m, e) = self.live_entries().unwrap().into_iter().find(|(_, e)| e.file.path == data_path).expect("data file");
        let spec = self.meta.spec(m.spec_id).unwrap().clone();
        let mut pos = positions.to_vec();
        pos.sort();
        let aschema = Arc::new(ASchema::new(vec![AField::new("file_path", DataType::Utf8, false).with_metadata(fid_meta(2147483546)),
                                                 AField::new("pos", DataType::Int64, false).with_metadata(fid_meta(2147483545))]));
        let batch = RecordBatch::try_new(aschema.clone(), vec![Arc::new(StringArray::from(vec![data_path; pos.len()])), Arc::new(Int64Array::from(pos.clone()))]).unwrap();
        let name = format!("deletes/{:016x}-pos.parquet", self.next_uid());
        let path = self.dir.join("data").join(&name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut w = ArrowWriter::try_new(File::create(&path).unwrap(), aschema, None).unwrap();
        w.write(&batch).unwrap();
        w.close().unwrap();
        let seq = self.meta.last_seq + 1;
        let snap_id = 1_000_000 + self.meta.snapshots.len() as i64 * 7 + 3;
        let f = DataFile { content: POS_DELETES, path: path.to_string_lossy().to_string(), format: "parquet".into(), partition: e.file.partition.clone(), record_count: pos.len() as i64,
                           size: std::fs::metadata(&path).unwrap().len() as i64, referenced: Some(data_path.to_string()), ..Default::default() };
        let entry = Entry { status: ADDED, snapshot_id: Some(snap_id), seq: None, file_seq: None, file: f };
        let mut list = self.live_manifests().unwrap();
        list.push(self.write_manifest_file(&spec, POS_DELETES, &[entry], snap_id, seq));
        self.snapshot_commit("delete", list)
    }

    /// An equality delete file on the columns `ids` with the given rows (all columns of `rows` are the delete columns, in the order of `ids`).
    /// `partition`: the partition it belongs to under the default spec; ignored (global) when the default spec is unpartitioned.
    pub fn delete_equality(&mut self, ids: &[i32], rows: &RecordBatch, partition: Vec<Option<Value>>) -> i64 {
        let schema = self.meta.current_schema().clone();
        let spec = self.meta.spec(self.meta.default_spec_id).unwrap().clone();
        assert!(spec.fields.is_empty() || partition.len() == spec.fields.len(), "the partition tuple must belong to the default spec");
        let fields: Vec<Field> = ids.iter().map(|i| schema.field(*i).expect("delete column").clone()).collect();
        let dschema = Schema { id: 0, fields: fields.clone() };
        let name = format!("deletes/{:016x}-eq.parquet", self.next_uid());
        let path = self.dir.join("data").join(&name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut w = ArrowWriter::try_new(File::create(&path).unwrap(), arrow_schema(&dschema), None).unwrap();
        w.write(rows).unwrap();
        w.close().unwrap();
        let seq = self.meta.last_seq + 1;
        let snap_id = 1_000_000 + self.meta.snapshots.len() as i64 * 7 + 3;
        let mut f = DataFile { content: EQ_DELETES, path: path.to_string_lossy().to_string(), format: "parquet".into(), partition: if spec.fields.is_empty() { vec![] } else { partition },
                               size: std::fs::metadata(&path).unwrap().len() as i64, equality_ids: ids.to_vec(), ..Default::default() };
        metrics(&dschema, rows, &mut f);
        let entry = Entry { status: ADDED, snapshot_id: Some(snap_id), seq: None, file_seq: None, file: f };
        let mut list = self.live_manifests().unwrap();
        list.push(self.write_manifest_file(&spec, EQ_DELETES, &[entry], snap_id, seq));
        self.snapshot_commit("delete", list)
    }

    // ---- evolution (metadata-only commits)
    pub fn add_column(&mut self, name: &str, ty: Type) -> i32 {
        self.meta.last_column_id += 1;
        let id = self.meta.last_column_id;
        let mut s = self.meta.current_schema().clone();
        s.id = self.meta.schemas.iter().map(|s| s.id).max().unwrap() + 1;
        s.fields.push(Field { id, name: name.to_string(), required: false, ty });
        self.meta.current_schema_id = s.id;
        self.meta.schemas.push(s);
        self.commit_metadata();
        id
    }

    pub fn rename_column(&mut self, old: &str, new: &str) {
        let mut s = self.meta.current_schema().clone();
        s.id = self.meta.schemas.iter().map(|s| s.id).max().unwrap() + 1;
        s.fields.iter_mut().find(|f| f.name == old).expect("column").name = new.to_string();
        self.meta.current_schema_id = s.id;
        self.meta.schemas.push(s);
        self.commit_metadata();
    }

    pub fn drop_column(&mut self, name: &str) {
        let mut s = self.meta.current_schema().clone();
        s.id = self.meta.schemas.iter().map(|s| s.id).max().unwrap() + 1;
        s.fields.retain(|f| f.name != name);
        self.meta.current_schema_id = s.id;
        self.meta.schemas.push(s);
        self.commit_metadata();
    }

    /// New default partition spec from `(source column name, transform)` pairs. Field ids are reused for equivalent fields.
    pub fn evolve_spec(&mut self, fields: &[(&str, Transform)]) -> i32 {
        let schema = self.meta.current_schema().clone();
        let id = self.meta.specs.iter().map(|s| s.id).max().unwrap() + 1;
        let mut pf = vec![];
        for (col, tr) in fields {
            let src = schema.by_name(col).expect("partition column");
            let name = if *tr == Transform::Identity { col.to_string() } else { format!("{}_{}", col, tr.name().split('[').next().unwrap()) };
            let reuse = self.meta.specs.iter().flat_map(|s| s.fields.iter()).find(|f| f.source_id == src.id && f.transform == *tr).map(|f| f.field_id);
            let fid = reuse.unwrap_or_else(|| { self.meta.last_partition_id += 1; self.meta.last_partition_id });
            pf.push(PartField { source_id: src.id, field_id: fid, name, transform: tr.clone() });
        }
        self.meta.specs.push(Spec { id, fields: pf });
        self.meta.default_spec_id = id;
        self.commit_metadata();
        id
    }
}

pub fn simple_schema(cols: &[(&str, Type)]) -> Schema {
    Schema { id: 0, fields: cols.iter().enumerate().map(|(i, (n, t))| Field { id: i as i32 + 1, name: n.to_string(), required: false, ty: t.clone() }).collect() }
}

pub fn _paths(_: &str) -> &str { local_path("") }
pub fn _e() -> Res<()> { err("") }
