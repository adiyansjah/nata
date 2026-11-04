//! Manifests and manifest lists (Avro, format v2): the schemas with the spec's field ids, writing, and reading files written by anyone (field values are
//! matched by name; the codec is whatever the writer used: null, deflate, snappy, zstd).
use std::collections::BTreeMap;

use apache_avro::types::Value as A;
use apache_avro::{Codec, Reader, Schema as AvroSchema, Writer};
use serde_json::{Value as J, json};

use crate::meta::{Schema, Spec};
use crate::transform::Transform;
use crate::types::{Type, Value, deserialize, serialize, vcmp};
use crate::{Res, err};

pub const ADDED: i32 = 1;
pub const EXISTING: i32 = 0;
pub const DELETED: i32 = 2;
pub const DATA: i32 = 0;
pub const POS_DELETES: i32 = 1;
pub const EQ_DELETES: i32 = 2;

#[derive(Clone, Debug, Default)]
pub struct DataFile {
    pub content: i32, pub path: String, pub format: String, pub partition: Vec<Option<Value>>, pub record_count: i64, pub size: i64,
    pub column_sizes: BTreeMap<i32, i64>, pub value_counts: BTreeMap<i32, i64>, pub null_counts: BTreeMap<i32, i64>, pub nan_counts: BTreeMap<i32, i64>,
    pub lower: BTreeMap<i32, Vec<u8>>, pub upper: BTreeMap<i32, Vec<u8>>, pub equality_ids: Vec<i32>, pub sort_order_id: Option<i32>, pub referenced: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Entry { pub status: i32, pub snapshot_id: Option<i64>, pub seq: Option<i64>, pub file_seq: Option<i64>, pub file: DataFile }

#[derive(Clone, Debug, Default)]
pub struct FieldSummary { pub contains_null: bool, pub contains_nan: Option<bool>, pub lower: Option<Vec<u8>>, pub upper: Option<Vec<u8>> }

#[derive(Clone, Debug, Default)]
pub struct ManifestFile {
    pub path: String, pub length: i64, pub spec_id: i32, pub content: i32, pub seq: i64, pub min_seq: i64, pub added_snapshot_id: i64,
    pub added_files: i32, pub existing_files: i32, pub deleted_files: i32, pub added_rows: i64, pub existing_rows: i64, pub deleted_rows: i64, pub partitions: Vec<FieldSummary>,
}

// ---------------------------------------------------------------- schemas

/// Bytes needed for a decimal of precision `p` (Iceberg's `decimalRequiredBytes`).
pub fn decimal_bytes(p: u8) -> usize { (((p as f64) * std::f64::consts::LOG2_10 + 1.0) / 8.0).ceil() as usize }

fn avro_partition_type(t: &Type, fid: i32) -> Res<J> {
    Ok(match t {
        Type::Decimal(p, s) => json!({"type": "fixed", "name": format!("decimal_{p}_{s}_{fid}"), "size": decimal_bytes(*p), "logicalType": "decimal", "precision": p, "scale": s}),
        Type::Int => json!("int"), Type::Long => json!("long"), Type::String => json!("string"), Type::Boolean => json!("boolean"), Type::Float => json!("float"), Type::Double => json!("double"),
        Type::Date => json!({"type": "int", "logicalType": "date"}), Type::Binary => json!("bytes"),
        Type::Timestamp => json!({"type": "long", "logicalType": "timestamp-micros", "adjust-to-utc": false}),
        Type::TimestampTz => json!({"type": "long", "logicalType": "timestamp-micros", "adjust-to-utc": true}),
        other => return err(format!("partition values of type {other:?} are not supported by this spike")),
    })
}

fn map_field(name: &str, fid: i32, kid: i32, vid: i32, vt: &str) -> J {
    json!({"name": name, "field-id": fid, "default": null, "type": ["null", {"type": "array", "logicalType": "map", "items": {"type": "record", "name": format!("k{kid}_v{vid}"),
        "fields": [{"name": "key", "type": "int", "field-id": kid}, {"name": "value", "type": vt, "field-id": vid}]}}]})
}

fn list_field(name: &str, fid: i32, eid: i32, et: &str) -> J {
    json!({"name": name, "field-id": fid, "default": null, "type": ["null", {"type": "array", "items": et, "element-id": eid}]})
}

pub fn manifest_schema(spec: &Spec, schema: &Schema) -> Res<J> {
    let mut pf = vec![];
    for f in &spec.fields {
        let src = schema.field(f.source_id).ok_or("partition source column not in the schema")?;
        pf.push(json!({"name": f.name, "field-id": f.field_id, "default": null, "type": ["null", avro_partition_type(&f.transform.result_type(&src.ty), f.field_id)?]}));
    }
    Ok(json!({"type": "record", "name": "manifest_entry", "fields": [
        {"name": "status", "type": "int", "field-id": 0},
        {"name": "snapshot_id", "type": ["null", "long"], "default": null, "field-id": 1},
        {"name": "sequence_number", "type": ["null", "long"], "default": null, "field-id": 3},
        {"name": "file_sequence_number", "type": ["null", "long"], "default": null, "field-id": 4},
        {"name": "data_file", "field-id": 2, "type": {"type": "record", "name": "r2", "fields": [
            {"name": "content", "type": "int", "field-id": 134},
            {"name": "file_path", "type": "string", "field-id": 100},
            {"name": "file_format", "type": "string", "field-id": 101},
            {"name": "partition", "field-id": 102, "type": {"type": "record", "name": "r102", "fields": pf}},
            {"name": "record_count", "type": "long", "field-id": 103},
            {"name": "file_size_in_bytes", "type": "long", "field-id": 104},
            map_field("column_sizes", 108, 117, 118, "long"), map_field("value_counts", 109, 119, 120, "long"),
            map_field("null_value_counts", 110, 121, 122, "long"), map_field("nan_value_counts", 137, 138, 139, "long"),
            map_field("lower_bounds", 125, 126, 127, "bytes"), map_field("upper_bounds", 128, 129, 130, "bytes"),
            list_field("equality_ids", 135, 136, "int"),
            {"name": "sort_order_id", "type": ["null", "int"], "default": null, "field-id": 140},
            {"name": "referenced_data_file", "type": ["null", "string"], "default": null, "field-id": 143},
        ]}},
    ]}))
}

pub fn manifest_list_schema() -> J {
    json!({"type": "record", "name": "manifest_file", "fields": [
        {"name": "manifest_path", "type": "string", "field-id": 500}, {"name": "manifest_length", "type": "long", "field-id": 501},
        {"name": "partition_spec_id", "type": "int", "field-id": 502}, {"name": "content", "type": "int", "field-id": 517},
        {"name": "sequence_number", "type": "long", "field-id": 515}, {"name": "min_sequence_number", "type": "long", "field-id": 516},
        {"name": "added_snapshot_id", "type": "long", "field-id": 503},
        {"name": "added_files_count", "type": "int", "field-id": 504}, {"name": "existing_files_count", "type": "int", "field-id": 505}, {"name": "deleted_files_count", "type": "int", "field-id": 506},
        {"name": "added_rows_count", "type": "long", "field-id": 512}, {"name": "existing_rows_count", "type": "long", "field-id": 513}, {"name": "deleted_rows_count", "type": "long", "field-id": 514},
        {"name": "partitions", "default": null, "field-id": 507, "type": ["null", {"type": "array", "element-id": 508, "items": {"type": "record", "name": "r508", "fields": [
            {"name": "contains_null", "type": "boolean", "field-id": 509}, {"name": "contains_nan", "type": ["null", "boolean"], "default": null, "field-id": 518},
            {"name": "lower_bound", "type": ["null", "bytes"], "default": null, "field-id": 510}, {"name": "upper_bound", "type": ["null", "bytes"], "default": null, "field-id": 511}]}}]},
    ]})
}

// ---------------------------------------------------------------- writing

fn opt<T>(v: Option<T>, f: impl FnOnce(T) -> A) -> A { match v { Some(x) => A::Union(1, Box::new(f(x))), None => A::Union(0, Box::new(A::Null)) } }

fn map_val(m: &BTreeMap<i32, i64>) -> A {
    if m.is_empty() { return A::Union(0, Box::new(A::Null)); }
    A::Union(1, Box::new(A::Array(m.iter().map(|(k, v)| A::Record(vec![("key".into(), A::Int(*k)), ("value".into(), A::Long(*v))])).collect())))
}
fn bytes_map_val(m: &BTreeMap<i32, Vec<u8>>) -> A {
    if m.is_empty() { return A::Union(0, Box::new(A::Null)); }
    A::Union(1, Box::new(A::Array(m.iter().map(|(k, v)| A::Record(vec![("key".into(), A::Int(*k)), ("value".into(), A::Bytes(v.clone()))])).collect())))
}

fn part_val(v: &Option<Value>) -> A {
    opt(v.clone(), |v| match v {
        Value::Int(x) => A::Int(x), Value::Long(x) => A::Long(x), Value::Date(x) => A::Date(x), Value::Ts(x) => A::TimestampMicros(x), Value::Str(s) => A::String(s),
        Value::Bool(b) => A::Boolean(b), Value::Float(f) => A::Float(f), Value::Double(d) => A::Double(d), Value::Bin(b) => A::Bytes(b),
        Value::Decimal(x) => A::Decimal(apache_avro::Decimal::from(x.to_be_bytes().to_vec())),   // 16 big-endian bytes; narrowed below by the writer to the fixed size
        other => panic!("partition value {other:?}"),
    })
}

fn write_avro(schema: &J, meta: &[(&str, String)], records: Vec<A>, codec: Codec) -> Res<Vec<u8>> {
    let s = AvroSchema::parse_str(&schema.to_string()).map_err(|e| format!("avro schema: {e}"))?;
    let mut w = Writer::with_codec(&s, Vec::new(), codec);
    for (k, v) in meta { w.add_user_metadata(k.to_string(), v.as_bytes()).map_err(|e| format!("avro metadata: {e}"))?; }
    for r in records { w.append_value_ref(&r).map_err(|e| format!("avro append: {e}"))?; }
    w.into_inner().map_err(|e| format!("avro finish: {e}"))
}

/// Summary of the partition values of a set of entries, per partition field (the manifest list's `field_summary`).
pub fn summarize(spec: &Spec, schema: &Schema, entries: &[Entry]) -> Vec<FieldSummary> {
    spec.fields.iter().enumerate().map(|(i, pf)| {
        let ty = pf.transform.result_type(&schema.field(pf.source_id).map(|f| f.ty.clone()).unwrap_or(Type::Int));
        let mut s = FieldSummary { contains_null: false, contains_nan: matches!(ty, Type::Float | Type::Double).then_some(false), lower: None, upper: None };
        let (mut lo, mut hi): (Option<Value>, Option<Value>) = (None, None);
        for e in entries.iter().filter(|e| e.status != DELETED) {
            match &e.file.partition[i] {
                None => s.contains_null = true,
                Some(v) if crate::types::is_nan(v) => s.contains_nan = Some(true),
                Some(v) => { if lo.as_ref().is_none_or(|l| vcmp(v, l).is_lt()) { lo = Some(v.clone()); } if hi.as_ref().is_none_or(|h| vcmp(v, h).is_gt()) { hi = Some(v.clone()); } }
            }
        }
        s.lower = lo.as_ref().map(serialize);
        s.upper = hi.as_ref().map(serialize);
        s
    }).collect()
}

pub fn write_manifest(spec: &Spec, schema: &Schema, content: i32, entries: &[Entry], codec: Codec) -> Res<(Vec<u8>, Vec<FieldSummary>)> {
    let sj = manifest_schema(spec, schema)?;
    let part_names: Vec<&str> = spec.fields.iter().map(|f| f.name.as_str()).collect();
    let records = entries.iter().map(|e| {
        let f = &e.file;
        let part = A::Record(part_names.iter().zip(&f.partition).map(|(n, v)| (n.to_string(), part_val(v))).collect());
        A::Record(vec![
            ("status".into(), A::Int(e.status)), ("snapshot_id".into(), opt(e.snapshot_id, A::Long)), ("sequence_number".into(), opt(e.seq, A::Long)), ("file_sequence_number".into(), opt(e.file_seq, A::Long)),
            ("data_file".into(), A::Record(vec![
                ("content".into(), A::Int(f.content)), ("file_path".into(), A::String(f.path.clone())), ("file_format".into(), A::String(f.format.clone())), ("partition".into(), part),
                ("record_count".into(), A::Long(f.record_count)), ("file_size_in_bytes".into(), A::Long(f.size)),
                ("column_sizes".into(), map_val(&f.column_sizes)), ("value_counts".into(), map_val(&f.value_counts)), ("null_value_counts".into(), map_val(&f.null_counts)),
                ("nan_value_counts".into(), map_val(&f.nan_counts)), ("lower_bounds".into(), bytes_map_val(&f.lower)), ("upper_bounds".into(), bytes_map_val(&f.upper)),
                ("equality_ids".into(), if f.equality_ids.is_empty() { A::Union(0, Box::new(A::Null)) } else { A::Union(1, Box::new(A::Array(f.equality_ids.iter().map(|i| A::Int(*i)).collect()))) }),
                ("sort_order_id".into(), opt(f.sort_order_id, A::Int)), ("referenced_data_file".into(), opt(f.referenced.clone(), A::String)),
            ])),
        ])
    }).collect();
    let spec_json = json!(spec.fields.iter().map(|f| json!({"source-id": f.source_id, "field-id": f.field_id, "name": f.name, "transform": f.transform.name()})).collect::<Vec<_>>());
    let meta = [("schema", schema.to_json().to_string()), ("schema-id", schema.id.to_string()), ("partition-spec", spec_json.to_string()), ("partition-spec-id", spec.id.to_string()),
                ("format-version", "2".into()), ("content", if content == DATA { "data" } else { "deletes" }.into())];
    Ok((write_avro(&sj, &meta, records, codec)?, summarize(spec, schema, entries)))
}

pub fn write_manifest_list(files: &[ManifestFile], codec: Codec) -> Res<Vec<u8>> {
    let records = files.iter().map(|m| A::Record(vec![
        ("manifest_path".into(), A::String(m.path.clone())), ("manifest_length".into(), A::Long(m.length)), ("partition_spec_id".into(), A::Int(m.spec_id)), ("content".into(), A::Int(m.content)),
        ("sequence_number".into(), A::Long(m.seq)), ("min_sequence_number".into(), A::Long(m.min_seq)), ("added_snapshot_id".into(), A::Long(m.added_snapshot_id)),
        ("added_files_count".into(), A::Int(m.added_files)), ("existing_files_count".into(), A::Int(m.existing_files)), ("deleted_files_count".into(), A::Int(m.deleted_files)),
        ("added_rows_count".into(), A::Long(m.added_rows)), ("existing_rows_count".into(), A::Long(m.existing_rows)), ("deleted_rows_count".into(), A::Long(m.deleted_rows)),
        ("partitions".into(), A::Union(1, Box::new(A::Array(m.partitions.iter().map(|p| A::Record(vec![
            ("contains_null".into(), A::Boolean(p.contains_null)), ("contains_nan".into(), opt(p.contains_nan, A::Boolean)),
            ("lower_bound".into(), opt(p.lower.clone(), A::Bytes)), ("upper_bound".into(), opt(p.upper.clone(), A::Bytes))])).collect())))),
    ])).collect();
    write_avro(&manifest_list_schema(), &[("format-version", "2".into())], records, codec)
}

// ---------------------------------------------------------------- reading

fn unwrap(v: &A) -> &A { match v { A::Union(_, b) => unwrap(b), o => o } }
fn field<'a>(rec: &'a [(String, A)], name: &str) -> Option<&'a A> { rec.iter().find(|(n, _)| n == name).map(|(_, v)| unwrap(v)).filter(|v| !matches!(v, A::Null)) }
fn long_of(v: &A) -> Option<i64> { match v { A::Long(x) => Some(*x), A::Int(x) => Some(*x as i64), A::TimestampMicros(x) => Some(*x), A::Date(x) => Some(*x as i64), _ => None } }
fn rec_of(v: &A) -> Res<&Vec<(String, A)>> { match unwrap(v) { A::Record(r) => Ok(r), o => err(format!("expected a record, got {o:?}")) } }

fn read_map(v: Option<&A>) -> BTreeMap<i32, i64> {
    let mut m = BTreeMap::new();
    if let Some(A::Array(items)) = v { for it in items { if let A::Record(r) = unwrap(it) { if let (Some(k), Some(val)) = (field(r, "key").and_then(long_of), field(r, "value").and_then(long_of)) { m.insert(k as i32, val); } } } }
    m
}
fn read_bytes_map(v: Option<&A>) -> BTreeMap<i32, Vec<u8>> {
    let mut m = BTreeMap::new();
    if let Some(A::Array(items)) = v { for it in items { if let A::Record(r) = unwrap(it) { if let (Some(k), Some(A::Bytes(b))) = (field(r, "key").and_then(long_of), field(r, "value")) { m.insert(k as i32, b.clone()); } } } }
    m
}

fn to_value(v: &A, ty: &Type) -> Option<Value> {
    let v = unwrap(v);
    Some(match (v, ty) {
        (A::Null, _) => return None,
        (A::Int(x) | A::Date(x), Type::Date) => Value::Date(*x), (A::Int(x), _) => Value::Int(*x),
        (A::Long(x) | A::TimestampMicros(x), Type::Timestamp | Type::TimestampTz) => Value::Ts(*x), (A::Long(x), _) => Value::Long(*x),
        (A::String(s), _) => Value::Str(s.clone()), (A::Boolean(b), _) => Value::Bool(*b), (A::Float(f), _) => Value::Float(*f), (A::Double(d), _) => Value::Double(*d),
        (A::Decimal(d), _) => Value::Decimal(crate::types::dec_from_be(&Vec::<u8>::try_from(d).ok()?)),
        (A::Bytes(b), _) => Value::Bin(b.clone()), (A::Fixed(_, b), Type::Decimal(..)) => Value::Decimal(crate::types::dec_from_be(b)), (A::Fixed(_, b), _) => Value::Bin(b.clone()),
        _ => return None,
    })
}

fn read_avro(bytes: &[u8]) -> Res<(Vec<A>, std::collections::HashMap<String, Vec<u8>>)> {
    let r = Reader::new(bytes).map_err(|e| format!("avro open: {e}"))?;
    let meta = r.user_metadata().clone();
    let recs = r.collect::<Result<Vec<_>, _>>().map_err(|e| format!("avro read: {e}"))?;
    Ok((recs, meta))
}

pub struct ManifestHeader { pub spec_id: i32, pub content: i32 }

/// `part_types` = the result types of the partition fields of the spec the manifest was written with (to rebuild typed partition values).
pub fn read_manifest(bytes: &[u8], part_types: &[Type]) -> Res<(ManifestHeader, Vec<Entry>)> {
    let (recs, meta) = read_avro(bytes)?;
    let spec_id = meta.get("partition-spec-id").and_then(|b| String::from_utf8_lossy(b).parse().ok()).unwrap_or(0);
    let content = if meta.get("content").map(|b| b.as_slice()) == Some(b"deletes") { 1 } else { 0 };
    let mut out = vec![];
    for r in &recs {
        let r = rec_of(r)?;
        let df = rec_of(field(r, "data_file").ok_or("entry without data_file")?)?;
        let part = match field(df, "partition") { Some(A::Record(p)) => p.iter().enumerate().map(|(i, (_, v))| to_value(v, part_types.get(i).unwrap_or(&Type::Long))).collect(), _ => vec![] };
        let ints = |n: &str| -> Vec<i32> { match field(df, n) { Some(A::Array(a)) => a.iter().filter_map(|x| long_of(unwrap(x)).map(|v| v as i32)).collect(), _ => vec![] } };
        out.push(Entry {
            status: field(r, "status").and_then(long_of).ok_or("entry without status")? as i32, snapshot_id: field(r, "snapshot_id").and_then(long_of), seq: field(r, "sequence_number").and_then(long_of),
            file_seq: field(r, "file_sequence_number").and_then(long_of),
            file: DataFile {
                content: field(df, "content").and_then(long_of).unwrap_or(0) as i32, path: match field(df, "file_path") { Some(A::String(s)) => s.clone(), _ => return err("data file without path") },
                format: match field(df, "file_format") { Some(A::String(s)) => s.to_lowercase(), _ => "parquet".into() }, partition: part,
                record_count: field(df, "record_count").and_then(long_of).unwrap_or(0), size: field(df, "file_size_in_bytes").and_then(long_of).unwrap_or(0),
                column_sizes: read_map(field(df, "column_sizes")), value_counts: read_map(field(df, "value_counts")), null_counts: read_map(field(df, "null_value_counts")),
                nan_counts: read_map(field(df, "nan_value_counts")), lower: read_bytes_map(field(df, "lower_bounds")), upper: read_bytes_map(field(df, "upper_bounds")),
                equality_ids: ints("equality_ids"), sort_order_id: field(df, "sort_order_id").and_then(long_of).map(|v| v as i32),
                referenced: match field(df, "referenced_data_file") { Some(A::String(s)) => Some(s.clone()), _ => None },
            },
        });
    }
    Ok((ManifestHeader { spec_id, content }, out))
}

pub fn read_manifest_list(bytes: &[u8]) -> Res<Vec<ManifestFile>> {
    let (recs, _) = read_avro(bytes)?;
    recs.iter().map(|r| {
        let r = rec_of(r)?;
        let l = |n: &str| field(r, n).and_then(long_of);
        let parts = match field(r, "partitions") { Some(A::Array(a)) => a.iter().map(|p| { let p = rec_of(p)?; Ok(FieldSummary {
            contains_null: matches!(field(p, "contains_null"), Some(A::Boolean(true))), contains_nan: match field(p, "contains_nan") { Some(A::Boolean(b)) => Some(*b), _ => None },
            lower: match field(p, "lower_bound") { Some(A::Bytes(b)) => Some(b.clone()), _ => None }, upper: match field(p, "upper_bound") { Some(A::Bytes(b)) => Some(b.clone()), _ => None } }) }).collect::<Res<Vec<_>>>()?, _ => vec![] };
        Ok(ManifestFile {
            path: match field(r, "manifest_path") { Some(A::String(s)) => s.clone(), _ => return err("manifest without path") }, length: l("manifest_length").unwrap_or(0), spec_id: l("partition_spec_id").unwrap_or(0) as i32,
            content: l("content").unwrap_or(0) as i32, seq: l("sequence_number").unwrap_or(0), min_seq: l("min_sequence_number").unwrap_or(0), added_snapshot_id: l("added_snapshot_id").unwrap_or(0),
            added_files: l("added_files_count").unwrap_or(1) as i32, existing_files: l("existing_files_count").unwrap_or(1) as i32, deleted_files: l("deleted_files_count").unwrap_or(0) as i32,
            added_rows: l("added_rows_count").unwrap_or(0), existing_rows: l("existing_rows_count").unwrap_or(0), deleted_rows: l("deleted_rows_count").unwrap_or(0), partitions: parts })
    }).collect()
}

pub fn deserialize_bound(t: &Type, b: &[u8]) -> Option<Value> { deserialize(t, b).ok() }
pub fn _unused(_: &Transform) {}
