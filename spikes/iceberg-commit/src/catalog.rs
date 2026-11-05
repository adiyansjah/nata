//! The catalog interface the writers and the checker use, and three implementations:
//!   RestCatalog   - the Iceberg REST catalog API over plain HTTP (the commit endpoint, requirements and updates)
//!   StoreCatalog  - no server: the next metadata version is created with a conditional put (PutMode::Create)
//!   FaultyCatalog - wraps another catalog and injects lost requests, lost responses (= CommitStateUnknown) and
//!                   duplicated requests on chosen commit calls
use crate::{Res, now_ms};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct Snap {
    pub id: i64,
    pub parent: Option<i64>,
    pub seq: i64,
    pub ts: i64,
    pub manifest_list: String,
    pub summary: BTreeMap<String, String>,
}

impl Snap {
    pub fn from_json(v: &Value) -> Res<Snap> {
        let id = v["snapshot-id"].as_i64().ok_or("snapshot without snapshot-id")?;
        let mut summary = BTreeMap::new();
        if let Some(o) = v["summary"].as_object() {
            for (k, x) in o {
                summary.insert(k.clone(), x.as_str().unwrap_or("").to_string());
            }
        }
        Ok(Snap {
            id,
            parent: v["parent-snapshot-id"].as_i64().filter(|p| *p > 0),
            seq: v["sequence-number"].as_i64().ok_or_else(|| format!("snapshot {id} without sequence-number"))?,
            ts: v["timestamp-ms"].as_i64().unwrap_or(0),
            manifest_list: v["manifest-list"].as_str().ok_or_else(|| format!("snapshot {id} without manifest-list"))?.to_string(),
            summary,
        })
    }
    pub fn op(&self) -> &str {
        self.summary.get("nata.op").map(|s| s.as_str()).unwrap_or("")
    }
    pub fn operation(&self) -> &str {
        self.summary.get("operation").map(|s| s.as_str()).unwrap_or("")
    }
}

#[derive(Clone, Debug)]
pub struct TableState {
    pub uuid: String,
    pub location: String,
    pub last_seq: i64,
    pub current: Option<i64>,
    pub snaps: Vec<Snap>,
    pub snapshot_log: Vec<i64>,
    pub metadata_log: Vec<String>,
    pub metadata_location: String,
    /// REST: unused (0). Store: the number N of the current vN.metadata.json.
    pub version: u64,
    pub raw: Value,
}

impl TableState {
    pub fn from_metadata(meta: &Value, metadata_location: String, version: u64) -> Res<TableState> {
        let mut snaps = Vec::new();
        for s in meta["snapshots"].as_array().cloned().unwrap_or_default() {
            snaps.push(Snap::from_json(&s)?);
        }
        let current = meta["refs"]["main"]["snapshot-id"].as_i64().or_else(|| meta["current-snapshot-id"].as_i64()).filter(|i| *i > 0);
        Ok(TableState {
            uuid: meta["table-uuid"].as_str().ok_or("metadata without table-uuid")?.to_string(),
            location: meta["location"].as_str().ok_or("metadata without location")?.to_string(),
            last_seq: meta["last-sequence-number"].as_i64().unwrap_or(0),
            current,
            snaps,
            snapshot_log: meta["snapshot-log"].as_array().map(|a| a.iter().filter_map(|e| e["snapshot-id"].as_i64()).collect()).unwrap_or_default(),
            metadata_log: meta["metadata-log"].as_array().map(|a| a.iter().filter_map(|e| e["metadata-file"].as_str().map(String::from)).collect()).unwrap_or_default(),
            metadata_location,
            version,
            raw: meta.clone(),
        })
    }
    pub fn snap(&self, id: i64) -> Option<&Snap> {
        self.snaps.iter().find(|s| s.id == id)
    }
    pub fn current_snap(&self) -> Option<&Snap> {
        self.current.and_then(|c| self.snap(c))
    }
    pub fn data_dir(&self) -> String {
        format!("{}/data", self.location.trim_end_matches('/'))
    }
    pub fn meta_dir(&self) -> String {
        format!("{}/metadata", self.location.trim_end_matches('/'))
    }
}

#[derive(Clone, Debug)]
pub struct NewSnapshot {
    pub id: i64,
    pub parent: Option<i64>,
    pub seq: i64,
    pub manifest_list: String,
    pub summary: BTreeMap<String, String>,
}

impl NewSnapshot {
    pub fn to_json(&self) -> Value {
        let mut o = json!({
            "snapshot-id": self.id,
            "sequence-number": self.seq,
            "timestamp-ms": now_ms(),
            "manifest-list": self.manifest_list,
            "summary": self.summary,
            "schema-id": 0,
        });
        if let Some(p) = self.parent {
            o["parent-snapshot-id"] = json!(p);
        }
        o
    }
}

pub struct CommitReq<'a> {
    pub base: &'a TableState,
    pub snapshot: NewSnapshot,
    /// true: the commit asserts that main still points where the writer read it (assert-ref-snapshot-id; for the store
    /// the conditional create). false: the known-bad "no compare-and-swap" writer.
    pub require_cas: bool,
}

#[derive(Debug, Clone)]
pub enum CommitError {
    /// 409 CommitFailed / put-if-absent lost: another writer committed first. Nothing was applied.
    Conflict(String),
    /// 500, 502, 504, a timeout, a dropped connection: the commit may or may not have been applied (CommitStateUnknown).
    Unknown(String),
    /// Any other refusal (400, 404, 422, 503 ...): the request was not applied.
    Rejected(u16, String),
}

impl std::fmt::Display for CommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommitError::Conflict(m) => write!(f, "conflict: {m}"),
            CommitError::Unknown(m) => write!(f, "commit state unknown: {m}"),
            CommitError::Rejected(c, m) => write!(f, "rejected {c}: {m}"),
        }
    }
}

pub trait Catalog: Send + Sync {
    fn load(&self) -> Res<TableState>;
    fn commit(&self, req: &CommitReq) -> Result<TableState, CommitError>;
    fn describe(&self) -> String;
}

// ---------------------------------------------------------------------------------------------------- REST

pub struct RestCatalog {
    pub base_url: String,
    pub prefix: String,
    pub ns: String,
    pub table: String,
    agent: ureq::Agent,
    /// OAuth2 bearer token, when the catalog needs one (Polaris). Never printed, never part of `describe`.
    token: Option<String>,
}

/// application/x-www-form-urlencoded escaping (everything except unreserved characters).
fn form_enc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

fn new_agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(timeout)).build().into()
}

impl RestCatalog {
    /// Reads /v1/config for the path prefix. Does not create anything.
    ///
    /// Product catalogs (Apache Polaris) need two things the reference fixture does not, both taken from the environment so that the suite and its
    /// call sites are unchanged: `IC_REST_CLIENT_ID` and `IC_REST_CLIENT_SECRET` (OAuth2 client credentials, scope `IC_REST_SCOPE`, default
    /// `PRINCIPAL_ROLE:ALL`; the token is fetched from `<base>/v1/oauth/tokens` once and sent as a bearer token), and `IC_REST_WAREHOUSE` (the catalog's
    /// name, sent as `?warehouse=` to /v1/config, whose answer carries the path prefix). Without them nothing changes.
    pub fn connect(base_url: &str, ns: &str, table: &str) -> Res<RestCatalog> {
        let agent = new_agent(Duration::from_secs(60));
        let base_url = base_url.trim_end_matches('/').to_string();
        let token = match (std::env::var("IC_REST_CLIENT_ID").ok().filter(|v| !v.is_empty()), std::env::var("IC_REST_CLIENT_SECRET").ok().filter(|v| !v.is_empty())) {
            (Some(id), Some(secret)) => {
                let scope = std::env::var("IC_REST_SCOPE").unwrap_or_else(|_| "PRINCIPAL_ROLE:ALL".into());
                let form = format!("grant_type=client_credentials&client_id={}&client_secret={}&scope={}", form_enc(&id), form_enc(&secret), form_enc(&scope));
                let mut r = agent.post(&format!("{base_url}/v1/oauth/tokens")).header("Content-Type", "application/x-www-form-urlencoded").send(form).map_err(|e| format!("POST /v1/oauth/tokens: {e}"))?;
                let st = r.status().as_u16();
                let text = r.body_mut().read_to_string().map_err(|e| e.to_string())?;
                if st != 200 {
                    return Err(format!("the token request returned {st} (credentials not shown)"));
                }
                let v: Value = serde_json::from_str(&text).map_err(|e| format!("token json: {e}"))?;
                Some(v["access_token"].as_str().ok_or("the token response has no access_token")?.to_string())
            }
            _ => None,
        };
        let cfg_url = match std::env::var("IC_REST_WAREHOUSE").ok().filter(|v| !v.is_empty()) {
            Some(w) => format!("{base_url}/v1/config?warehouse={}", form_enc(&w)),
            None => format!("{base_url}/v1/config"),
        };
        let mut req = agent.get(&cfg_url);
        if let Some(t) = &token {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        let mut resp = req.call().map_err(|e| format!("GET /v1/config: {e}"))?;
        let status = resp.status().as_u16();
        let body = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
        if status != 200 {
            return Err(format!("GET /v1/config returned {status}: {body}"));
        }
        let cfg: Value = serde_json::from_str(&body).map_err(|e| format!("config json: {e}"))?;
        let prefix = cfg["overrides"]["prefix"].as_str().or_else(|| cfg["defaults"]["prefix"].as_str()).unwrap_or("").to_string();
        Ok(RestCatalog { base_url, prefix, ns: ns.into(), table: table.into(), agent, token })
    }

    fn root(&self) -> String {
        if self.prefix.is_empty() { format!("{}/v1", self.base_url) } else { format!("{}/v1/{}", self.base_url, self.prefix) }
    }
    fn table_url(&self) -> String {
        format!("{}/namespaces/{}/tables/{}", self.root(), self.ns, self.table)
    }

    fn send(&self, method: &str, url: &str, body: Option<&Value>) -> Result<(u16, String), String> {
        let auth = self.token.as_ref().map(|t| format!("Bearer {t}"));
        let r = match (method, body) {
            ("GET", _) => {
                let mut q = self.agent.get(url);
                if let Some(a) = &auth {
                    q = q.header("Authorization", a);
                }
                q.call()
            }
            ("POST", Some(b)) => {
                let mut q = self.agent.post(url).header("Content-Type", "application/json");
                if let Some(a) = &auth {
                    q = q.header("Authorization", a);
                }
                q.send(b.to_string())
            }
            (m, _) => return Err(format!("unsupported request {m}")),
        };
        let mut resp = r.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
        Ok((status, text))
    }

    pub fn ensure_namespace(&self) -> Res<()> {
        let (st, body) = self.send("POST", &format!("{}/namespaces", self.root()), Some(&json!({"namespace": [self.ns], "properties": {}})))?;
        match st {
            200 | 201 | 409 => Ok(()),
            _ => Err(format!("create namespace: {st} {body}")),
        }
    }

    /// Creates the table (format version 2, schema id/long and k/string, unpartitioned) and returns its state.
    pub fn create_table(&self) -> Res<TableState> {
        let body = json!({
            "name": self.table,
            "schema": {"type": "struct", "schema-id": 0, "fields": [
                {"id": 1, "name": "id", "required": true, "type": "long"},
                {"id": 2, "name": "k", "required": true, "type": "string"}]},
            "properties": {"format-version": "2"},
        });
        let (st, text) = self.send("POST", &format!("{}/namespaces/{}/tables", self.root(), self.ns), Some(&body))?;
        if st != 200 {
            return Err(format!("create table: {st} {text}"));
        }
        let v: Value = serde_json::from_str(&text).map_err(|e| format!("create table json: {e}"))?;
        let state = TableState::from_metadata(&v["metadata"], v["metadata-location"].as_str().unwrap_or("").to_string(), 0)?;
        if v["metadata"]["format-version"].as_i64() != Some(2) {
            return Err(format!("the catalog created format version {} instead of 2", v["metadata"]["format-version"]));
        }
        Ok(state)
    }

    fn parse_load(text: &str) -> Res<TableState> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("table json: {e}"))?;
        TableState::from_metadata(&v["metadata"], v["metadata-location"].as_str().unwrap_or("").to_string(), 0)
    }
}

impl Catalog for RestCatalog {
    fn load(&self) -> Res<TableState> {
        let (st, text) = self.send("GET", &self.table_url(), None)?;
        if st != 200 {
            return Err(format!("load table: {st} {text}"));
        }
        Self::parse_load(&text)
    }

    fn commit(&self, req: &CommitReq) -> Result<TableState, CommitError> {
        let mut requirements = vec![json!({"type": "assert-table-uuid", "uuid": req.base.uuid})];
        if req.require_cas {
            requirements.push(json!({"type": "assert-ref-snapshot-id", "ref": "main", "snapshot-id": req.base.current}));
        }
        let body = json!({
            "requirements": requirements,
            "updates": [
                {"action": "add-snapshot", "snapshot": req.snapshot.to_json()},
                {"action": "set-snapshot-ref", "ref-name": "main", "type": "branch", "snapshot-id": req.snapshot.id},
            ],
        });
        // A transport error after the request left is a timeout from the writer's point of view: the commit state is unknown.
        let (st, text) = self.send("POST", &self.table_url(), Some(&body)).map_err(CommitError::Unknown)?;
        match st {
            200 => Self::parse_load(&text).map_err(CommitError::Unknown),
            409 => Err(CommitError::Conflict(text)),
            500 | 502 | 504 => Err(CommitError::Unknown(format!("{st} {text}"))),
            _ => Err(CommitError::Rejected(st, text)),
        }
    }

    fn describe(&self) -> String {
        format!("REST {} (prefix '{}') {}.{}", self.base_url, self.prefix, self.ns, self.table)
    }
}

// ---------------------------------------------------------------------------------------------------- object store

/// The catalog-less table: metadata/vN.metadata.json, created with PutMode::Create (the operation S3 calls
/// If-None-Match: *, GCS ifGenerationMatch=0, Azure If-None-Match: *). The "current" version is the highest N.
/// Backed by object_store's LocalFileSystem, whose Create is an atomic hard link: faithful for this one operation;
/// NOT faithful for network failures (the FaultyCatalog adds those) or for PutMode::Update (not implemented locally).
pub struct StoreCatalog {
    pub root: PathBuf,
    store: object_store::local::LocalFileSystem,
}

fn vpath(n: u64) -> object_store::path::Path {
    object_store::path::Path::from(format!("metadata/v{n}.metadata.json"))
}

impl StoreCatalog {
    pub fn new(root: PathBuf) -> Res<StoreCatalog> {
        std::fs::create_dir_all(root.join("metadata")).map_err(|e| e.to_string())?;
        let store = object_store::local::LocalFileSystem::new_with_prefix(&root).map_err(|e| e.to_string())?;
        Ok(StoreCatalog { root, store })
    }

    /// Creates v1 with an empty table (no snapshots). Safe to call twice.
    pub fn create_table(&self) -> Res<TableState> {
        use object_store::{ObjectStore, PutMode, PutPayload};
        let location = self.root.display().to_string();
        let meta = json!({
            "format-version": 2, "table-uuid": "00000000-0000-4000-8000-000000000001", "location": location,
            "last-sequence-number": 0, "last-updated-ms": now_ms(), "last-column-id": 2,
            "schemas": [serde_json::from_str::<Value>(crate::ice::TABLE_SCHEMA_JSON).map_err(|e| e.to_string())?],
            "current-schema-id": 0,
            "partition-specs": [{"spec-id": 0, "fields": []}], "default-spec-id": 0, "last-partition-id": 999,
            "properties": {}, "snapshots": [], "snapshot-log": [], "metadata-log": [],
            "sort-orders": [{"order-id": 0, "fields": []}], "default-sort-order-id": 0, "refs": {},
        });
        let payload = PutPayload::from(serde_json::to_vec_pretty(&meta).map_err(|e| e.to_string())?);
        match futures::executor::block_on(self.store.put_opts(&vpath(1), payload, PutMode::Create.into())) {
            Ok(_) | Err(object_store::Error::AlreadyExists { .. }) => {}
            Err(e) => return Err(format!("create v1: {e}")),
        }
        self.load()
    }

    fn latest_version(&self) -> Res<u64> {
        use futures::TryStreamExt;
        use object_store::ObjectStore;
        let prefix = object_store::path::Path::from("metadata");
        let metas: Vec<_> = futures::executor::block_on(self.store.list(Some(&prefix)).try_collect::<Vec<_>>()).map_err(|e| e.to_string())?;
        metas
            .iter()
            .filter_map(|m| {
                let name = m.location.filename()?;
                name.strip_prefix('v')?.strip_suffix(".metadata.json")?.parse::<u64>().ok()
            })
            .max()
            .ok_or_else(|| "no metadata version found".to_string())
    }

    fn read_version(&self, n: u64) -> Res<TableState> {
        use object_store::ObjectStoreExt;
        let bytes = futures::executor::block_on(async {
            let r = self.store.get(&vpath(n)).await.map_err(|e| e.to_string())?;
            r.bytes().await.map_err(|e| e.to_string())
        })?;
        let meta: Value = serde_json::from_slice(&bytes).map_err(|e| format!("v{n}: {e}"))?;
        TableState::from_metadata(&meta, self.root.join(format!("metadata/v{n}.metadata.json")).display().to_string(), n)
    }
}

impl Catalog for StoreCatalog {
    fn load(&self) -> Res<TableState> {
        let n = self.latest_version()?;
        self.read_version(n)
    }

    fn commit(&self, req: &CommitReq) -> Result<TableState, CommitError> {
        use object_store::{ObjectStore, PutMode, PutPayload};
        let base = req.base;
        let mut m = base.raw.clone();
        let ts = now_ms();
        let snap = req.snapshot.to_json();
        let mut snaps = m["snapshots"].as_array().cloned().unwrap_or_default();
        snaps.push(snap);
        m["snapshots"] = Value::Array(snaps);
        m["current-snapshot-id"] = json!(req.snapshot.id);
        m["refs"] = json!({"main": {"snapshot-id": req.snapshot.id, "type": "branch"}});
        m["last-sequence-number"] = json!(req.snapshot.seq);
        m["last-updated-ms"] = json!(ts);
        let mut slog = m["snapshot-log"].as_array().cloned().unwrap_or_default();
        slog.push(json!({"timestamp-ms": ts, "snapshot-id": req.snapshot.id}));
        m["snapshot-log"] = Value::Array(slog);
        let mut mlog = m["metadata-log"].as_array().cloned().unwrap_or_default();
        mlog.push(json!({"timestamp-ms": ts, "metadata-file": base.metadata_location}));
        m["metadata-log"] = Value::Array(mlog);
        let bytes = serde_json::to_vec_pretty(&m).map_err(|e| CommitError::Rejected(0, e.to_string()))?;
        let next = base.version + 1;
        // require_cas: create-if-absent. The known-bad writer overwrites.
        let mode = if req.require_cas { PutMode::Create } else { PutMode::Overwrite };
        match futures::executor::block_on(self.store.put_opts(&vpath(next), PutPayload::from(bytes), mode.into())) {
            Ok(_) => self.read_version(next).map_err(CommitError::Unknown),
            Err(object_store::Error::AlreadyExists { .. }) | Err(object_store::Error::Precondition { .. }) => {
                Err(CommitError::Conflict(format!("v{next}.metadata.json already exists")))
            }
            Err(e) => Err(CommitError::Unknown(e.to_string())),
        }
    }

    fn describe(&self) -> String {
        format!("object store (conditional create) at {}", self.root.display())
    }
}

// ---------------------------------------------------------------------------------------------------- fault injection

/// Faults keyed by the number of the commit call (1-based, per FaultyCatalog, i.e. per writer process).
#[derive(Clone, Debug, Default)]
pub struct FaultPlan {
    /// The commit is applied by the catalog, the answer never reaches the writer (timeout): CommitStateUnknown.
    pub drop_response: BTreeSet<u32>,
    /// The request never reaches the catalog (timeout): CommitStateUnknown, nothing applied.
    pub drop_request: BTreeSet<u32>,
    /// The network delivers the request twice.
    pub duplicate: BTreeSet<u32>,
    /// Sleep before the request is sent (ms).
    pub delay_ms: BTreeMap<u32, u64>,
}

impl FaultPlan {
    /// "drop-response@1,drop-request@3,dup@2,delay@1=200"
    pub fn parse(s: &str) -> Res<FaultPlan> {
        let mut p = FaultPlan::default();
        for item in s.split(',').map(str::trim).filter(|x| !x.is_empty()) {
            let (kind, rest) = item.split_once('@').ok_or_else(|| format!("bad fault '{item}'"))?;
            match kind {
                "delay" => {
                    let (n, ms) = rest.split_once('=').ok_or_else(|| format!("bad fault '{item}'"))?;
                    p.delay_ms.insert(n.parse().map_err(|_| format!("bad fault '{item}'"))?, ms.parse().map_err(|_| format!("bad fault '{item}'"))?);
                }
                "drop-response" | "drop-request" | "dup" => {
                    let n: u32 = rest.parse().map_err(|_| format!("bad fault '{item}'"))?;
                    match kind {
                        "drop-response" => p.drop_response.insert(n),
                        "drop-request" => p.drop_request.insert(n),
                        _ => p.duplicate.insert(n),
                    };
                }
                _ => return Err(format!("unknown fault kind '{kind}' in '{item}'")),
            }
        }
        Ok(p)
    }
    pub fn is_empty(&self) -> bool {
        self.drop_response.is_empty() && self.drop_request.is_empty() && self.duplicate.is_empty() && self.delay_ms.is_empty()
    }
}

pub struct FaultyCatalog {
    pub inner: Arc<dyn Catalog>,
    pub plan: FaultPlan,
    calls: AtomicU32,
}

impl FaultyCatalog {
    pub fn new(inner: Arc<dyn Catalog>, plan: FaultPlan) -> FaultyCatalog {
        FaultyCatalog { inner, plan, calls: AtomicU32::new(0) }
    }
}

impl Catalog for FaultyCatalog {
    fn load(&self) -> Res<TableState> {
        self.inner.load()
    }
    fn commit(&self, req: &CommitReq) -> Result<TableState, CommitError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(ms) = self.plan.delay_ms.get(&n) {
            std::thread::sleep(Duration::from_millis(*ms));
        }
        if self.plan.drop_request.contains(&n) {
            return Err(CommitError::Unknown(format!("injected: request {n} lost before reaching the catalog")));
        }
        let first = self.inner.commit(req);
        if self.plan.duplicate.contains(&n) {
            let _second = self.inner.commit(req);
        }
        if self.plan.drop_response.contains(&n) {
            return Err(CommitError::Unknown(format!("injected: response of commit {n} lost after the catalog handled it ({})", match &first {
                Ok(_) => "applied".to_string(),
                Err(e) => e.to_string(),
            })));
        }
        first
    }
    fn describe(&self) -> String {
        format!("{} + faults", self.inner.describe())
    }
}
