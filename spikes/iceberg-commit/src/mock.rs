//! A test double of the Iceberg REST catalog: just enough of the API for one table (config, namespace, create table,
//! load table, commit with assert-table-uuid / assert-ref-snapshot-id and add-snapshot / set-snapshot-ref), with an
//! atomic commit under one lock and a metadata file per version, like the reference server.
//! It is written from the spec by the same author as the writers: it lets the unit tests and the harness checks run
//! without Java, and it is NOT evidence about Iceberg. The evidence is the run against the reference fixture.
use crate::now_ms;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Method, Response, Server};

struct State {
    warehouse: PathBuf,
    tables: std::collections::BTreeMap<String, Table>,
}

struct Table {
    uuid: String,
    location: String,
    version: u64,
    metadata_path: String,
    last_seq: i64,
    current: Option<i64>,
    snaps: Vec<Value>,
    snapshot_log: Vec<Value>,
    metadata_log: Vec<Value>,
}

impl Table {
    fn json(&self) -> Value {
        let mut m = json!({
            "format-version": 2, "table-uuid": self.uuid, "location": self.location,
            "last-sequence-number": self.last_seq, "last-updated-ms": now_ms(), "last-column-id": 2,
            "schemas": [serde_json::from_str::<Value>(crate::ice::TABLE_SCHEMA_JSON).unwrap_or(Value::Null)],
            "current-schema-id": 0, "partition-specs": [{"spec-id": 0, "fields": []}], "default-spec-id": 0,
            "last-partition-id": 999, "properties": {"format-version": "2"},
            "snapshots": self.snaps, "snapshot-log": self.snapshot_log, "metadata-log": self.metadata_log,
            "sort-orders": [{"order-id": 0, "fields": []}], "default-sort-order-id": 0, "refs": {},
        });
        match self.current {
            Some(c) => {
                m["current-snapshot-id"] = json!(c);
                m["refs"] = json!({"main": {"snapshot-id": c, "type": "branch"}});
            }
            None => m["current-snapshot-id"] = json!(-1),
        }
        m
    }
    fn write_metadata(&mut self) -> Result<(), String> {
        self.version += 1;
        let path = format!("{}/metadata/{:05}-{}.metadata.json", self.location, self.version, self.uuid);
        self.metadata_log.push(json!({"timestamp-ms": now_ms(), "metadata-file": self.metadata_path}));
        let text = serde_json::to_string_pretty(&self.json()).map_err(|e| e.to_string())?;
        crate::write_atomic(&crate::to_local(&path), text.as_bytes())?;
        self.metadata_path = path;
        Ok(())
    }
    fn load_response(&self) -> Value {
        // the metadata-log entry of the file being returned is not yet in the list (it is the current file)
        let mut m = self.json();
        let mut log = self.metadata_log.clone();
        log.retain(|e| e["metadata-file"].as_str() != Some(&self.metadata_path));
        m["metadata-log"] = Value::Array(log);
        json!({"metadata-location": self.metadata_path, "metadata": m, "config": {}})
    }
}

pub struct MockServer {
    pub url: String,
    server: Arc<Server>,
}

impl MockServer {
    pub fn start(warehouse: PathBuf) -> Result<MockServer, String> {
        let server = Arc::new(Server::http("127.0.0.1:0").map_err(|e| e.to_string())?);
        let url = format!("http://{}", server.server_addr().to_ip().ok_or("no ip address")?);
        let state = Arc::new(Mutex::new(State { warehouse, tables: std::collections::BTreeMap::new() }));
        let srv = server.clone();
        std::thread::spawn(move || {
            for request in srv.incoming_requests() {
                let state = state.clone();
                std::thread::spawn(move || handle(request, &state));
            }
        });
        Ok(MockServer { url, server })
    }
    pub fn stop(&self) {
        self.server.unblock();
    }
}

fn reply(request: tiny_http::Request, code: u16, body: Value) {
    let h = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("header");
    let _ = request.respond(Response::from_string(body.to_string()).with_status_code(code).with_header(h));
}

fn err(code: u16, kind: &str, msg: &str) -> (u16, Value) {
    (code, json!({"error": {"message": msg, "type": kind, "code": code}}))
}

fn handle(mut request: tiny_http::Request, state: &Mutex<State>) {
    let method = request.method().clone();
    let url = request.url().split('?').next().unwrap_or("").to_string();
    let mut body = String::new();
    let _ = request.as_reader().read_to_string(&mut body);
    let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let segs: Vec<&str> = url.trim_matches('/').split('/').collect();
    let (code, out) = {
        let mut st = match state.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        route(&method, &segs, &parsed, &mut st)
    };
    reply(request, code, out);
}

fn route(method: &Method, segs: &[&str], body: &Value, st: &mut State) -> (u16, Value) {
    match (method, segs) {
        (Method::Get, ["v1", "config"]) => (200, json!({"defaults": {}, "overrides": {}})),
        (Method::Post, ["v1", "namespaces"]) => (200, json!({"namespace": body["namespace"], "properties": {}})),
        (Method::Post, ["v1", "namespaces", ns, "tables"]) => {
            let name = body["name"].as_str().unwrap_or("t").to_string();
            let name = name.as_str();
            if st.tables.contains_key(name) {
                return err(409, "AlreadyExistsException", "table exists");
            }
            let location = format!("{}/{}/{}", st.warehouse.display(), ns, name);
            let mut t = Table {
                uuid: format!("{:08x}-0000-4000-8000-{:012x}", now_ms() as u32, now_ms() as u64 & 0xFFFF_FFFF_FFFF),
                location, version: 0, metadata_path: String::new(), last_seq: 0, current: None,
                snaps: vec![], snapshot_log: vec![], metadata_log: vec![],
            };
            t.metadata_path = format!("{}/metadata/{:05}-{}.metadata.json", t.location, 0, t.uuid);
            let text = serde_json::to_string_pretty(&t.json()).unwrap_or_default();
            if let Err(e) = crate::write_atomic(&crate::to_local(&t.metadata_path), text.as_bytes()) {
                return err(500, "InternalServerError", &e);
            }
            let resp = t.load_response();
            st.tables.insert(name.to_string(), t);
            (200, resp)
        }
        (Method::Get, ["v1", "namespaces", _ns, "tables", name]) => match st.tables.get(*name) {
            Some(t) => (200, t.load_response()),
            None => err(404, "NoSuchTableException", "no such table"),
        },
        (Method::Post, ["v1", "namespaces", _ns, "tables", name]) => commit(body, st, name),
        _ => err(404, "NotFound", "unknown route"),
    }
}

fn commit(body: &Value, st: &mut State, name: &str) -> (u16, Value) {
    let Some(t) = st.tables.get_mut(name) else { return err(404, "NoSuchTableException", "no such table") };
    for r in body["requirements"].as_array().cloned().unwrap_or_default() {
        match r["type"].as_str() {
            Some("assert-table-uuid") => {
                if r["uuid"].as_str() != Some(&t.uuid) {
                    return err(409, "CommitFailedException", "table uuid does not match");
                }
            }
            Some("assert-ref-snapshot-id") => {
                if r["ref"].as_str() == Some("main") && r["snapshot-id"].as_i64() != t.current {
                    return err(409, "CommitFailedException", &format!("branch main was created or updated concurrently: expected {:?}, is {:?}", r["snapshot-id"].as_i64(), t.current));
                }
            }
            other => return err(400, "BadRequestException", &format!("unknown requirement {other:?}")),
        }
    }
    // apply on a copy: all updates or none
    let mut snaps = t.snaps.clone();
    let mut last_seq = t.last_seq;
    let mut current = t.current;
    let mut slog = t.snapshot_log.clone();
    for u in body["updates"].as_array().cloned().unwrap_or_default() {
        match u["action"].as_str() {
            Some("add-snapshot") => {
                let s = &u["snapshot"];
                let (Some(id), Some(seq)) = (s["snapshot-id"].as_i64(), s["sequence-number"].as_i64()) else {
                    return err(400, "BadRequestException", "snapshot without id or sequence number");
                };
                if snaps.iter().any(|x| x["snapshot-id"].as_i64() == Some(id)) {
                    return err(400, "ValidationException", &format!("Snapshot already exists for id: {id}"));
                }
                // the reference builder (TableMetadata.Builder.addSnapshot) requires seq > last-sequence-number unless the snapshot has no parent
                if !(seq > last_seq || s["parent-snapshot-id"].is_null()) {
                    return err(400, "ValidationException", &format!("Cannot add snapshot with sequence number {seq} older than last sequence number {last_seq}"));
                }
                last_seq = last_seq.max(seq);
                snaps.push(s.clone());
            }
            Some("set-snapshot-ref") => {
                let Some(id) = u["snapshot-id"].as_i64() else { return err(400, "BadRequestException", "set-snapshot-ref without snapshot-id") };
                if u["ref-name"].as_str() != Some("main") {
                    return err(400, "BadRequestException", "only main is supported by this mock");
                }
                if !snaps.iter().any(|x| x["snapshot-id"].as_i64() == Some(id)) {
                    return err(400, "ValidationException", "unknown snapshot");
                }
                current = Some(id);
                slog.push(json!({"timestamp-ms": now_ms(), "snapshot-id": id}));
            }
            other => return err(400, "BadRequestException", &format!("unknown update {other:?}")),
        }
    }
    t.snaps = snaps;
    t.last_seq = last_seq;
    t.current = current;
    t.snapshot_log = slog;
    if let Err(e) = t.write_metadata() {
        return err(500, "InternalServerError", &e);
    }
    (200, t.load_response())
}
