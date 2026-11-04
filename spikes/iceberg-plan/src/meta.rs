//! Table metadata (format v2): schemas, partition specs, snapshots, and their JSON.
use serde_json::{Value as J, json};

use crate::transform::Transform;
use crate::types::Type;
use crate::{Res, err};

#[derive(Clone, Debug)]
pub struct Field { pub id: i32, pub name: String, pub required: bool, pub ty: Type }

#[derive(Clone, Debug)]
pub struct Schema { pub id: i32, pub fields: Vec<Field> }

impl Schema {
    pub fn field(&self, id: i32) -> Option<&Field> { self.fields.iter().find(|f| f.id == id) }
    pub fn by_name(&self, name: &str) -> Option<&Field> { self.fields.iter().find(|f| f.name == name) }
    pub fn to_json(&self) -> J {
        json!({"type": "struct", "schema-id": self.id, "fields": self.fields.iter().map(|f| json!({"id": f.id, "name": f.name, "required": f.required, "type": f.ty.to_json()})).collect::<Vec<_>>()})
    }
    pub fn from_json(j: &J) -> Res<Schema> {
        let fields = j["fields"].as_array().ok_or("schema without fields")?.iter().map(|f| {
            Ok(Field { id: f["id"].as_i64().ok_or("field id")? as i32, name: f["name"].as_str().ok_or("field name")?.to_string(), required: f["required"].as_bool().unwrap_or(false), ty: Type::parse_lenient(&f["type"]) })
        }).collect::<Res<Vec<_>>>()?;
        Ok(Schema { id: j["schema-id"].as_i64().unwrap_or(0) as i32, fields })
    }
}

#[derive(Clone, Debug)]
pub struct PartField { pub source_id: i32, pub field_id: i32, pub name: String, pub transform: Transform }

#[derive(Clone, Debug)]
pub struct Spec { pub id: i32, pub fields: Vec<PartField> }

impl Spec {
    pub fn unpartitioned(id: i32) -> Spec { Spec { id, fields: vec![] } }
    pub fn to_json(&self) -> J {
        json!({"spec-id": self.id, "fields": self.fields.iter().map(|f| json!({"source-id": f.source_id, "field-id": f.field_id, "name": f.name, "transform": f.transform.name()})).collect::<Vec<_>>()})
    }
    pub fn from_json(j: &J) -> Res<Spec> {
        let fields = j["fields"].as_array().ok_or("spec without fields")?.iter().map(|f| {
            Ok(PartField { source_id: f["source-id"].as_i64().ok_or("source-id")? as i32, field_id: f["field-id"].as_i64().ok_or("field-id")? as i32, name: f["name"].as_str().ok_or("name")?.to_string(),
                           transform: Transform::parse(f["transform"].as_str().ok_or("transform")?).unwrap_or(Transform::Void) })
        }).collect::<Res<Vec<_>>>()?;
        Ok(Spec { id: j["spec-id"].as_i64().unwrap_or(0) as i32, fields })
    }
    /// Does this spec (field list) equal another, ignoring the id? Writers must not create a spec equivalent to an existing one.
    pub fn same_fields(&self, o: &Spec) -> bool {
        self.fields.len() == o.fields.len() && self.fields.iter().zip(&o.fields).all(|(a, b)| a.source_id == b.source_id && a.transform == b.transform && a.name == b.name)
    }
}

#[derive(Clone, Debug)]
pub struct Snapshot { pub id: i64, pub parent: Option<i64>, pub seq: i64, pub ts_ms: i64, pub manifest_list: String, pub operation: String, pub schema_id: Option<i32> }

#[derive(Clone, Debug)]
pub struct TableMeta {
    pub location: String, pub uuid: String, pub last_seq: i64, pub last_column_id: i32, pub schemas: Vec<Schema>, pub current_schema_id: i32,
    pub specs: Vec<Spec>, pub default_spec_id: i32, pub last_partition_id: i32, pub snapshots: Vec<Snapshot>, pub current_snapshot: Option<i64>,
}

impl TableMeta {
    pub fn schema(&self, id: i32) -> Option<&Schema> { self.schemas.iter().find(|s| s.id == id) }
    pub fn current_schema(&self) -> &Schema { self.schema(self.current_schema_id).expect("current schema") }
    pub fn spec(&self, id: i32) -> Option<&Spec> { self.specs.iter().find(|s| s.id == id) }
    pub fn snapshot(&self, id: i64) -> Option<&Snapshot> { self.snapshots.iter().find(|s| s.id == id) }

    pub fn to_json(&self) -> J {
        let mut j = json!({
            "format-version": 2, "table-uuid": self.uuid, "location": self.location, "last-sequence-number": self.last_seq, "last-updated-ms": self.snapshots.last().map_or(0, |s| s.ts_ms),
            "last-column-id": self.last_column_id, "schemas": self.schemas.iter().map(|s| s.to_json()).collect::<Vec<_>>(), "current-schema-id": self.current_schema_id,
            "partition-specs": self.specs.iter().map(|s| s.to_json()).collect::<Vec<_>>(), "default-spec-id": self.default_spec_id, "last-partition-id": self.last_partition_id,
            "properties": {}, "sort-orders": [{"order-id": 0, "fields": []}], "default-sort-order-id": 0,
            "snapshots": self.snapshots.iter().map(|s| { let mut o = json!({"snapshot-id": s.id, "sequence-number": s.seq, "timestamp-ms": s.ts_ms, "manifest-list": s.manifest_list, "summary": {"operation": s.operation}});
                if let Some(p) = s.parent { o["parent-snapshot-id"] = json!(p); } if let Some(sc) = s.schema_id { o["schema-id"] = json!(sc); } o }).collect::<Vec<_>>(),
            "snapshot-log": self.snapshots.iter().map(|s| json!({"timestamp-ms": s.ts_ms, "snapshot-id": s.id})).collect::<Vec<_>>(), "metadata-log": [],
        });
        if let Some(c) = self.current_snapshot { j["current-snapshot-id"] = json!(c); j["refs"] = json!({"main": {"snapshot-id": c, "type": "branch"}}); } else { j["current-snapshot-id"] = json!(-1); }
        j
    }

    pub fn from_json(j: &J) -> Res<TableMeta> {
        if j["format-version"].as_i64() != Some(2) { return err(format!("format version {} is not supported (this spike reads v2)", j["format-version"])); }
        let schemas = j["schemas"].as_array().ok_or("no schemas")?.iter().map(Schema::from_json).collect::<Res<Vec<_>>>()?;
        let specs = j["partition-specs"].as_array().ok_or("no partition-specs")?.iter().map(Spec::from_json).collect::<Res<Vec<_>>>()?;
        let snapshots = j["snapshots"].as_array().cloned().unwrap_or_default().iter().map(|s| Ok(Snapshot {
            id: s["snapshot-id"].as_i64().ok_or("snapshot-id")?, parent: s["parent-snapshot-id"].as_i64(), seq: s["sequence-number"].as_i64().unwrap_or(0), ts_ms: s["timestamp-ms"].as_i64().unwrap_or(0),
            manifest_list: s["manifest-list"].as_str().ok_or("manifest-list")?.to_string(), operation: s["summary"]["operation"].as_str().unwrap_or("").to_string(), schema_id: s["schema-id"].as_i64().map(|v| v as i32) })).collect::<Res<Vec<_>>>()?;
        let cur = j["current-snapshot-id"].as_i64().filter(|v| *v >= 0);
        Ok(TableMeta { location: j["location"].as_str().unwrap_or("").to_string(), uuid: j["table-uuid"].as_str().unwrap_or("").to_string(), last_seq: j["last-sequence-number"].as_i64().unwrap_or(0),
                       last_column_id: j["last-column-id"].as_i64().unwrap_or(0) as i32, schemas, current_schema_id: j["current-schema-id"].as_i64().unwrap_or(0) as i32, specs,
                       default_spec_id: j["default-spec-id"].as_i64().unwrap_or(0) as i32, last_partition_id: j["last-partition-id"].as_i64().unwrap_or(999) as i32, snapshots, current_snapshot: cur })
    }
}
