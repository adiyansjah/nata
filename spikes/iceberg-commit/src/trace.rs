//! Abstract-event tracing for the TRACE VALIDATION check of verification-plan item 24 (the "code gate": the Rust writers
//! against the TLA+ model). Off unless `IC_TRACE=<dir>` is set, so none of the existing suites changes.
//!
//! What it records is NOT a log of the implementation: it is the sequence of abstract actions of `model/IcebergCommit.tla`
//! that the code performed (plan, upload, build the manifest list, send, deliver, receive, time out, recover, crash,
//! drop), each with the observable numbers the model also has (attempt, base and expected version, the version the
//! catalog was at). `model/trace_to_tla.py` turns a trace into a TLC run that constrains the model to that sequence.
//!
//! One trace file per table, `<IC_TRACE>/<table>.jsonl`. Every event has a global sequence number `seq` (and `sub` to
//! order events that share one). The sequence is allocated under an exclusive `flock` on `<table>.lock`, which is held
//! across a catalog call and the event(s) that describe it, so that
//!   * a commit is recorded at the instant the catalog's conditional create happened (for the object store that create IS
//!     the linearization point), and no other catalog call or read lands between the call and its record;
//!   * a read of the table (`load`) gets its sequence number under the same lock; the writer's DECISION that follows the
//!     read (plan, or recover-done / -abort / -retry / -fail) is recorded AT that number (`emit_at`), because the model
//!     takes such a decision atomically with the read of the current metadata.
//! The lock is released by the kernel when a process dies, so a SIGKILLed writer cannot wedge the others. A commit writes
//! a `deliver-intent` line before the catalog call and the `deliver` line (with the outcome) after it; a process killed in
//! between leaves an intent without an outcome, which the translator resolves from the next version anyone observes.
//!
//! At the end of a run `final_state` records the table's version and live rows. Only the object-store backend is traced: the REST backend has no version number the writers can see (and the reference
//! fixture needs Java, which this check does not use).
use crate::catalog::{Catalog, CommitError, CommitReq, TableState};
use crate::Res;
use serde_json::{Value, json};
use std::cell::Cell;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The trace directory, when tracing is on.
pub fn dir() -> Option<PathBuf> {
    std::env::var("IC_TRACE").ok().filter(|v| !v.is_empty()).map(PathBuf::from)
}

thread_local! {
    /// (sequence number, version) of the last `load` made on this thread through a traced catalog.
    static LAST_LOAD: Cell<Option<(u64, u64)>> = const { Cell::new(None) };
    /// 0: an ordinary delivery; 1: the first of two deliveries of a duplicated request; 2: the second.
    static DELIVERY: Cell<u8> = const { Cell::new(0) };
}

/// The sequence number and version of this thread's last read of the table.
pub fn last_load() -> Option<(u64, u64)> {
    LAST_LOAD.with(|c| c.get())
}

/// Used by the fault injector around the two deliveries of a duplicated request.
pub fn set_delivery(kind: u8) {
    DELIVERY.with(|c| c.set(kind));
}

pub struct Sink {
    lock: PathBuf,
    seq: PathBuf,
    events: PathBuf,
}

pub struct Locked<'a> {
    sink: &'a Sink,
    _lock: std::fs::File,
}

fn io<T, E: std::fmt::Display>(what: &str, r: Result<T, E>) -> T {
    // A trace that silently lost an event could hide a bug, so a failure to write one stops the run.
    match r {
        Ok(v) => v,
        Err(e) => panic!("IC_TRACE: {what}: {e}"),
    }
}

impl Sink {
    pub fn open(dir: &Path, table: &str) -> Sink {
        io("create the trace directory", std::fs::create_dir_all(dir));
        Sink { lock: dir.join(format!("{table}.lock")), seq: dir.join(format!("{table}.seq")), events: dir.join(format!("{table}.jsonl")) }
    }

    /// Runs `f` holding the exclusive inter-process lock of this trace.
    pub fn locked<R>(&self, f: impl FnOnce(&Locked) -> R) -> R {
        let file = io("open the lock file", OpenOptions::new().create(true).truncate(false).write(true).open(&self.lock));
        io("take the trace lock", file.lock());
        let guard = Locked { sink: self, _lock: file };
        f(&guard)
    }

    /// A fresh event with its own sequence number. Returns the number.
    pub fn emit(&self, v: Value) -> u64 {
        self.locked(|g| {
            let seq = g.alloc_seq();
            g.append(seq, 1, v);
            seq
        })
    }

    /// An event placed at an earlier position (`seq`, from `last_load`), after the events already there.
    pub fn emit_at(&self, seq: u64, sub: u32, v: Value) {
        self.locked(|g| g.append(seq, sub, v));
    }
}

impl Locked<'_> {
    pub fn alloc_seq(&self) -> u64 {
        let cur = std::fs::read_to_string(&self.sink.seq).ok().and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0);
        let next = cur + 1;
        // Replace the file atomically: a writer SIGKILLed between truncating and writing it would reset the counter to 0
        // (seen in a 40-seed stress run: two operations then shared sequence numbers).
        let tmp = self.sink.seq.with_extension("seq.tmp");
        io("write the sequence file", std::fs::write(&tmp, next.to_string()));
        io("replace the sequence file", std::fs::rename(&tmp, &self.sink.seq));
        next
    }

    pub fn append(&self, seq: u64, sub: u32, mut v: Value) {
        if let Some(o) = v.as_object_mut() {
            o.insert("seq".into(), json!(seq));
            o.insert("sub".into(), json!(sub));
        }
        let mut line = v.to_string();
        line.push('\n');
        let mut f = io("open the trace file", OpenOptions::new().create(true).append(true).open(&self.sink.events));
        io("write the trace file", f.write_all(line.as_bytes()));
    }
}

/// The end of a run: the table as it is once every writer has finished (version and the ids of the live rows). The model
/// must be at the same version with the same live rows. Nothing unless the catalog is traced.
pub fn final_state(catalog: &dyn Catalog) {
    let Some(sink) = catalog.tracer() else { return };
    let state = match catalog.load() {
        Ok(s) => s,
        Err(e) => {
            sink.emit(json!({"a": "final", "error": format!("load: {e}")}));
            return;
        }
    };
    let rows = match state.current_snap() {
        Some(s) => crate::ice::load_view(&s.manifest_list, false).map(|v| v.live_rows().iter().map(|r| r.id).collect::<Vec<_>>()),
        None => Ok(vec![]),
    };
    match rows {
        Ok(mut r) => {
            r.sort();
            sink.emit(json!({"a": "final", "ver": state.version, "rows": r}));
        }
        Err(e) => {
            sink.emit(json!({"a": "final", "error": format!("rows: {e}")}));
        }
    }
}

/// Wraps a catalog and records its linearization points. Sits BELOW the fault injector, so it sees what reached the
/// catalog (a dropped request never does; a duplicated one arrives twice).
pub struct TracedCatalog {
    inner: Arc<dyn Catalog>,
    sink: Arc<Sink>,
}

impl TracedCatalog {
    /// `inner` unchanged when tracing is off. Tracing is for the object-store backend only: asking for it with another
    /// backend is an error, never a silently empty trace.
    pub fn wrap(inner: Arc<dyn Catalog>, table: &str, store_backend: bool) -> Res<Arc<dyn Catalog>> {
        let Some(d) = dir() else { return Ok(inner) };
        if !store_backend {
            static WARN: std::sync::Once = std::sync::Once::new();
            WARN.call_once(|| eprintln!("IC_TRACE is set but the REST backend is not traced (object-store backend only): no trace is written for it"));
            return Ok(inner);
        }
        Ok(Arc::new(TracedCatalog { inner, sink: Arc::new(Sink::open(&d, table)) }))
    }
}

fn str_of<'a>(req: &'a CommitReq, key: &str) -> &'a str {
    req.snapshot.summary.get(key).map(|s| s.as_str()).unwrap_or("")
}

impl Catalog for TracedCatalog {
    fn load(&self) -> Res<TableState> {
        self.sink.locked(|g| {
            let r = self.inner.load();
            if let Ok(s) = &r {
                LAST_LOAD.with(|c| c.set(Some((g.alloc_seq(), s.version))));
            }
            r
        })
    }

    fn commit(&self, req: &CommitReq) -> Result<TableState, CommitError> {
        let delivery = DELIVERY.with(|c| c.get());
        self.sink.locked(|g| {
            let cur = self.inner.load().map(|s| s.version as i64).unwrap_or(-1);
            let seq = g.alloc_seq();
            let head = json!({"op": str_of(req, "nata.op"), "att": str_of(req, "nata.attempt").parse::<u32>().unwrap_or(0),
                              "exp": req.base.version, "cur": cur, "cas": req.require_cas, "dup": delivery, "keep": delivery == 1});
            let mut intent = head.clone();
            intent["a"] = json!("deliver-intent");
            g.append(seq, 0, intent);
            let r = self.inner.commit(req);
            let mut done = head;
            done["a"] = json!("deliver");
            match &r {
                Ok(s) => {
                    done["res"] = json!("ok");
                    done["new"] = json!(s.version);
                }
                Err(CommitError::Conflict(_)) => done["res"] = json!("conflict"),
                Err(e) => {
                    done["res"] = json!("error");
                    done["err"] = json!(e.to_string());
                }
            }
            g.append(seq, 0, done);
            r
        })
    }

    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn prepare_check(&self) -> Res<()> {
        self.inner.prepare_check()
    }

    fn tracer(&self) -> Option<Arc<Sink>> {
        Some(self.sink.clone())
    }
}
