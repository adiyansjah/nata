//! Async sockets on tokio; bounded query admissions share the CPU worker budget.
use std::collections::HashMap;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nata_parquet::{ErrorKind, ScanCancel, ScanError};
use nata_sql::{ErrorClass, Pos, SourceProvider, types::SqlType};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc, oneshot, watch};

use crate::codec::{self, Message};
mod fetch;
use crate::{
    extended::{self, Portal, Statement},
    session,
};

/// Counters for resource-bound checks and embedding. No SQL, values or keys.
#[derive(Default)]
pub struct Metrics {
    pub rows_produced: AtomicU64,
    pub buffers_sent: AtomicU64,
    pub max_buffer_bytes: AtomicUsize,
    pub nodelay_sessions: AtomicUsize,
    pub running: AtomicUsize,
    pub sessions: AtomicUsize,
    pub registered: AtomicUsize,
    pub objects: nata_parquet::stats::Gauge,
    pub admitted: nata_parquet::stats::Gauge,
    /// Up to 64 completed scans; gauges remain inspectable after release.
    pub scans: Mutex<Vec<Arc<nata_parquet::ScanStats>>>,
    pub opener: Option<Arc<dyn nata_parquet::Opener>>,
    /// Completed aggregate (budget, peak, held, cancelled) samples, bounded to 64.
    pub aggregates: Mutex<Vec<(usize, usize, usize, bool)>>,
}
struct Counted {
    metrics: Option<Arc<Metrics>>,
    session: bool,
}
impl Drop for Counted {
    fn drop(&mut self) {
        if let Some(m) = &self.metrics {
            if self.session {
                m.sessions.fetch_sub(1, Ordering::SeqCst);
            } else {
                m.running.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }
}
pub struct Config {
    pub catalog: Arc<crate::catalog::Snapshot>,
    pub listen: SocketAddr,
    pub unsafe_listen: bool,
    pub max_connections: usize,
    pub threads: usize,
    /// Concurrent admissions, capped by the total CPU worker budget.
    pub max_queries: usize,
    /// Aggregate execution settings; memory is the NODE pool, divided by admissions.
    pub execution: nata_exec::ExecConfig,
    pub metrics: Option<Arc<Metrics>>,
}
impl Config {
    pub fn validate(&self) -> io::Result<()> {
        if !self.listen.ip().is_loopback() && !self.unsafe_listen {
            return Err(io::Error::other(
                "non-loopback listen requires --unsafe-listen (unsafe until TLS and authentication exist)",
            ));
        }
        if !(1..=64).contains(&self.max_connections)
            || self.threads == 0
            || self.max_queries == 0
            || self.max_queries > self.threads
        {
            return Err(io::Error::other(
                "max-connections must be 1..=64; threads/max-queries must be positive; max-queries <= threads",
            ));
        }
        Ok(())
    }
}
#[derive(Clone)]
struct Active {
    token: ScanCancel,
    stop: watch::Sender<bool>,
    cancelled: Arc<AtomicBool>,
}
impl Active {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.token.cancel();
        let _ = self.stop.send(true);
    }
}
struct Session {
    secret: i32,
    active: Mutex<Option<Active>>,
}
impl Session {
    fn cancel(&self) {
        if let Some(a) = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            a.cancel();
        }
    }
    fn finish(&self) {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}
type Registry = Arc<Mutex<HashMap<i32, Arc<Session>>>>;
struct Registered {
    id: i32,
    registry: Registry,
    session: Arc<Session>,
    metrics: Option<Arc<Metrics>>,
}
impl Drop for Registered {
    fn drop(&mut self) {
        self.session.cancel();
        let mut registry = self
            .registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        registry.remove(&self.id);
        if let Some(m) = &self.metrics {
            m.registered.store(registry.len(), Ordering::SeqCst);
        }
    }
}
struct Reader(tokio::task::JoinHandle<()>);
impl Drop for Reader {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Bind after validating the loopback policy. No process-wide signal handler is installed.
pub async fn serve(config: Config) -> io::Result<()> {
    config.validate()?;
    let listener = TcpListener::bind(config.listen).await?;
    serve_listener(listener, config).await
}
/// Serve a caller-owned listener (also checks the actual bound address).
pub async fn serve_listener(listener: TcpListener, config: Config) -> io::Result<()> {
    config.validate()?;
    if !listener.local_addr()?.ip().is_loopback() && !config.unsafe_listen {
        return Err(io::Error::other("unsafe listen address"));
    }
    let _ = writeln!(
        io::stderr(),
        "nata serve listening={} max_connections={} query_admission={} workers={}",
        listener.local_addr()?,
        config.max_connections,
        config.max_queries,
        config.threads
    );
    let mut execution = config.execution.clone();
    let system_pool = nata_exec::default_memory_pool();
    let pool = execution.aggregate_memory_bytes.unwrap_or(system_pool);
    if pool == 0 || (system_pool != 0 && pool > system_pool) {
        return Err(io::Error::other(
            "aggregate memory admission requires a positive pool no larger than the physical/cgroup pool",
        ));
    }
    execution.aggregate_memory_bytes = Some(pool / config.max_queries);
    let base = execution
        .aggregate_spill_directory
        .clone()
        .unwrap_or_else(std::env::temp_dir);
    let disk_pool = nata_exec::spill::available_bytes(&base)? / 2;
    let disk_share = disk_pool / config.max_queries as u64;
    let requested = execution.aggregate_spill_disk_bytes.unwrap_or_else(|| {
        u64::try_from(pool / config.max_queries)
            .unwrap_or(u64::MAX)
            .saturating_mul(4)
    });
    if execution.aggregate_spill_disk_bytes.is_some() && requested > disk_share {
        return Err(io::Error::other("spill disk admission exceeds node pool"));
    }
    execution.aggregate_spill_disk_bytes = Some(requested.min(disk_share));
    let _ = writeln!(
        io::stderr(),
        "aggregate_node_pool={} aggregate_query_budget={} aggregate_disk_share={} memory_source={}",
        pool,
        pool / config.max_queries,
        requested.min(disk_share),
        if config.execution.aggregate_memory_bytes.is_some() {
            "explicit"
        } else {
            "physical-cgroup"
        }
    );
    let sockets = Arc::new(Semaphore::new(config.max_connections + 8));
    let pending = Arc::new(Semaphore::new(8));
    let sessions = Arc::new(Semaphore::new(config.max_connections));
    let queries = Arc::new(Semaphore::new(config.max_queries));
    let registry: Registry = Arc::default();
    let ids = AtomicI32::new(1);
    loop {
        let (mut socket, _) = listener.accept().await?;
        socket.set_nodelay(true)?;
        if socket.nodelay()?
            && let Some(m) = &config.metrics
        {
            m.nodelay_sessions.fetch_add(1, Ordering::SeqCst);
        }
        let Ok(socket_slot) = Arc::clone(&sockets).try_acquire_owned() else {
            continue;
        };
        let Ok(startup_slot) = Arc::clone(&pending).try_acquire_owned() else {
            continue;
        };
        let (sessions, queries, registry) = (
            Arc::clone(&sessions),
            Arc::clone(&queries),
            Arc::clone(&registry),
        );
        let id = ids.fetch_add(1, Ordering::Relaxed);
        let threads = config.threads / config.max_queries;
        let metrics = config.metrics.clone();
        let mut execution = execution.clone();
        execution.threads = threads;
        execution.window = threads.saturating_mul(2);
        let catalog = Arc::clone(&config.catalog);
        tokio::spawn(async move {
            let _socket_slot = socket_slot;
            let startup = tokio::time::timeout(Duration::from_secs(5), async {
                // A single deadline covers probes plus startup; repeated probes cannot keep a slot alive.
                loop {
                    match read_frame(&mut socket, true).await? {
                        Some(Message::Encryption) => socket.write_all(b"N").await?,
                        other => return Ok::<_, io::Error>(other),
                    }
                }
            })
            .await;
            let message = match startup {
                Ok(Ok(Some(m))) => m,
                Ok(Ok(None)) => return,
                _ => {
                    send_error(&mut socket, "08P01", "malformed or timed out startup", None).await;
                    return;
                }
            };
            if let Message::Cancel(pid, secret) = message {
                let r = registry
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(s) = r.get(&pid).filter(|s| s.secret == secret) {
                    s.cancel();
                }
                return;
            }
            let Message::Startup(params) = message else {
                return;
            };
            if params.iter().any(|(k, v)| {
                k == "client_encoding"
                    && !v.eq_ignore_ascii_case("UTF8")
                    && !v.eq_ignore_ascii_case("UTF-8")
            }) {
                send_error(
                    &mut socket,
                    "0A000",
                    "not supported yet: client encoding other than UTF8",
                    None,
                )
                .await;
                return;
            }
            let mut state = session::State::default();
            if let Err(e) = state.startup(&params) {
                send_wire_error(&mut socket, &e, "").await;
                return;
            }
            let Ok(session_slot) = sessions.try_acquire_owned() else {
                send_error(&mut socket, "53300", "too many connections", None).await;
                return;
            };
            let mut random = [0; 4];
            if getrandom::fill(&mut random).is_err() {
                return;
            }
            let session = Arc::new(Session {
                secret: i32::from_ne_bytes(random),
                active: Mutex::new(None),
            });
            // IDs must never overwrite a live session, even after wrapping.
            {
                let mut r = registry
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if r.contains_key(&id) {
                    return;
                }
                r.insert(id, Arc::clone(&session));
                if let Some(m) = &metrics {
                    m.registered.store(r.len(), Ordering::SeqCst);
                }
            }
            let registration = Registered {
                id,
                registry,
                session,
                metrics: metrics.clone(),
            };
            if let Some(m) = &metrics {
                m.sessions.fetch_add(1, Ordering::SeqCst);
            }
            let _counted = Counted {
                metrics: metrics.clone(),
                session: true,
            };
            let _session_slot = session_slot;
            if handshake(&mut socket, id, registration.session.secret, &state)
                .await
                .is_err()
            {
                return;
            }
            drop(startup_slot);
            let _ = session_loop(
                socket,
                &registration,
                queries,
                execution,
                metrics,
                state,
                catalog,
            )
            .await;
        });
    }
}

/// Reads only a fixed header before checking length; EOF within a frame is an error.
async fn read_frame<R: AsyncRead + Unpin>(r: &mut R, startup: bool) -> io::Result<Option<Message>> {
    let mut first = [0; 1];
    if r.read(&mut first).await? == 0 {
        return Ok(None);
    }
    let mut h = [0; 4];
    let tag = if startup {
        h[0] = first[0];
        r.read_exact(&mut h[1..]).await?;
        None
    } else {
        r.read_exact(&mut h).await?;
        Some(first[0])
    };
    let n = codec::length(h, startup).map_err(io::Error::other)?;
    let mut b = Vec::with_capacity(n + usize::from(tag.is_some()));
    if let Some(t) = tag {
        b.push(t);
    }
    b.extend(h);
    b.resize(n + usize::from(tag.is_some()), 0);
    let start = 4 + usize::from(tag.is_some());
    r.read_exact(&mut b[start..]).await?;
    codec::decode(&b, startup)
        .map(Some)
        .map_err(io::Error::other)
}
async fn send_error<W: tokio::io::AsyncWrite + Unpin>(
    s: &mut W,
    code: &str,
    message: &str,
    position: Option<usize>,
) {
    let _ = tokio::time::timeout(
        Duration::from_secs(1),
        s.write_all(&codec::error(code, message, position)),
    )
    .await;
    let _ = s.flush().await;
}
async fn handshake(
    s: &mut TcpStream,
    id: i32,
    secret: i32,
    state: &session::State,
) -> io::Result<()> {
    s.write_all(&codec::packet(b'R', &0i32.to_be_bytes()).map_err(io::Error::other)?)
        .await?;
    for (k, v) in [
        ("server_version", "16.0"),
        ("server_encoding", "UTF8"),
        ("client_encoding", "UTF8"),
        ("DateStyle", "ISO, MDY"),
        ("TimeZone", "UTC"),
        ("application_name", ""),
        ("integer_datetimes", "on"),
        ("standard_conforming_strings", "on"),
    ] {
        let response = state
            .clone()
            .evaluate(&format!("SHOW {k}"))
            .map_err(|_| io::Error::other("startup status"))?
            .expect("SHOW");
        let value = response.rows[0][0].as_deref().unwrap_or(v);
        s.write_all(
            &codec::packet(b'S', format!("{k}\0{value}\0").as_bytes()).map_err(io::Error::other)?,
        )
        .await?;
    }
    s.write_all(
        &codec::packet(b'K', &[id.to_be_bytes(), secret.to_be_bytes()].concat())
            .map_err(io::Error::other)?,
    )
    .await?;
    s.write_all(&codec::packet(b'Z', b"I").map_err(io::Error::other)?)
        .await
}

async fn session_loop(
    socket: TcpStream,
    registered: &Registered,
    queries: Arc<Semaphore>,
    execution: nata_exec::ExecConfig,
    metrics: Option<Arc<Metrics>>,
    mut state: session::State,
    catalog: Arc<crate::catalog::Snapshot>,
) -> io::Result<()> {
    let (mut read, write) = socket.into_split();
    let mut write = tokio::io::BufWriter::with_capacity(OUTPUT_BYTES, write);
    let (tx, mut input) = mpsc::channel(1);
    let (closed, mut disconnected) = watch::channel(false);
    let session = Arc::clone(&registered.session);
    let _reader = Reader(tokio::spawn(async move {
        loop {
            match read_frame(&mut read, false).await {
                Ok(Some(m)) => {
                    if tx.send(Ok(m)).await.is_err() {
                        return;
                    }
                }
                Ok(None) => {
                    session.cancel();
                    let _ = closed.send(true);
                    return;
                }
                Err(_) => {
                    session.cancel();
                    let _ = tx.send(Err(())).await;
                    return;
                }
            }
        }
    }));
    state.catalog = Arc::clone(&catalog);
    let mut objects = Objects::new(metrics.clone());
    let mut runs: HashMap<String, fetch::Run> = HashMap::new();
    let mut failed = false;
    let mut statement_deadline: Option<Instant> = None;
    loop {
        if !failed && statement_deadline.is_some_and(|d| Instant::now() >= d) {
            registered.session.finish();
            runs.clear();
            objects.portals.clear();
            state.failed();
            failed = true;
            send_wire_error(&mut write, &WireError::timeout(), "").await;
        }
        runs.retain(|name, _| objects.portals.get(name).is_some());
        for run in runs.values_mut() {
            run.expire();
        }
        // Flush before waiting: Parse/Describe clients may wait without sending Sync.
        let m = match input.try_recv() {
            Ok(m) => m,
            Err(_) => {
                write.flush().await?;
                let lease = runs
                    .values()
                    .filter_map(fetch::Run::deadline)
                    .chain(statement_deadline.filter(|_| !failed))
                    .min();
                let received = if let Some(lease) = lease {
                    match tokio::time::timeout_at(lease.into(), input.recv()).await {
                        Ok(m) => m,
                        Err(_) => continue,
                    }
                } else {
                    input.recv().await
                };
                let Some(m) = received else { break };
                m
            }
        };
        let m = match m {
            Ok(m) => m,
            Err(()) => {
                send_error(&mut write, "08P01", "malformed protocol message", None).await;
                return Ok(());
            }
        };
        if matches!(m, Message::Terminate) {
            return Ok(());
        }
        if matches!(m, Message::Sync) {
            statement_deadline = None;
            failed = false;
            if state.transaction == b'I' {
                objects.portals.clear();
            }
            write
                .write_all(&codec::packet(b'Z', &[state.transaction]).map_err(io::Error::other)?)
                .await?;
            write.flush().await?;
            continue;
        }
        if failed {
            continue;
        }
        if matches!(m, Message::Flush) {
            write.flush().await?;
            continue;
        }
        // PostgreSQL's extended statement timer starts with the first protocol
        // message and runs through Flush and suspension until Sync (ADR 0015).
        if matches!(
            m,
            Message::Parse { .. }
                | Message::Bind { .. }
                | Message::Describe(..)
                | Message::Execute(..)
        ) && statement_deadline.is_none()
        {
            statement_deadline = state.timeout().map(|d| Instant::now() + d);
        }
        if let Message::Execute(name, limit) = &m
            && let Some(p) = objects.portals.get(name).cloned()
            && (runs.contains_key(name)
                || (*limit > 0 && column_count(&p.statement.description) > 0))
        {
            let result = if state.transaction == b'E' {
                Err(WireError::new(
                    "25P02",
                    "current transaction is aborted; use ROLLBACK",
                ))
            } else if state
                .clone()
                .evaluate(&p.statement.sql)
                .is_ok_and(|r| r.is_some())
                || crate::metadata::query_with(
                    &catalog,
                    &state.database,
                    &state.search_path(),
                    &p.statement.sql,
                    &p.params,
                )
                .is_ok_and(|r| r.is_some())
            {
                Err(WireError::unsupported(
                    "bounded fetch of session or metadata rows",
                ))
            } else if crate::copy::query(&p.statement.sql)
                .ok()
                .flatten()
                .is_some()
            {
                Err(WireError::unsupported("bounded Execute of COPY"))
            } else {
                let run = runs.entry(name.clone()).or_insert_with(fetch::Run::new);
                run.execute(
                    &mut write,
                    registered,
                    &queries,
                    &execution,
                    &metrics,
                    &catalog,
                    &state,
                    statement_deadline,
                    &p,
                    *limit,
                    &mut disconnected,
                )
                .await
            };
            // Suspension preserves the statement deadline until Sync.
            if let Err(e) = result {
                if e.code == "08006" {
                    return Ok(());
                }
                state.failed();
                failed = true;
                let e = deadline_error(e, statement_deadline);
                send_wire_error(&mut write, &e, &p.statement.sql).await;
                runs.clear();
                objects.portals.clear();
            }
            continue;
        }
        let started = Instant::now();
        let simple = matches!(m, Message::Query(_));
        let operation_deadline = if simple {
            state.timeout().map(|d| started + d)
        } else {
            statement_deadline
        };
        // Modes: 0 executes, 1 validates Parse, 2 describes a statement, 3 describes a portal.
        let mut mode = 0;
        let mut params = Vec::new();
        let mut parsed = None;
        let mut object_name = String::new();
        let mut portal: Option<Portal> = None;
        let mut error = None;
        let mut sql = String::new();
        match m {
            Message::Query(q) => {
                objects.statements.remove("");
                objects.portals.remove("");
                runs.remove("");
                sql = q;
            }
            Message::Parse {
                name,
                sql: q,
                types,
            } => {
                object_name = name.clone();
                if name.is_empty() {
                    objects.statements.remove("");
                }
                match objects
                    .check_statement(&name, &q)
                    .and_then(|()| Statement::new(&name, q, types))
                    .and_then(|s| Ok((s.dummy()?, s)))
                {
                    Ok((p, s)) => {
                        params = p;
                        sql = s.sql.clone();
                        parsed = Some(s);
                        mode = 1;
                    }
                    Err(e) => error = Some(e),
                }
            }
            Message::Bind {
                portal: name,
                statement: name_s,
                formats,
                params: p,
                results,
            } => {
                if name.is_empty() {
                    objects.portals.remove("");
                    runs.remove("");
                }
                let result = objects.check_portal(&name).and_then(|()| {
                    let s = objects
                        .statements
                        .get(&name_s)
                        .ok_or_else(|| WireError::new("26000", "unknown prepared statement"))?;
                    extended::formats(&formats, p.len())?;
                    extended::formats(&results, column_count(&s.description))?;
                    let p = crate::binary::parameters(&s.types, &formats, &p)?;
                    let mut bound = s.bind(&p)?;
                    bound.results = results;
                    objects.check_portal_bytes(&name, &bound)?;
                    Ok(bound)
                });
                match result {
                    Ok(p) => {
                        objects.portals.insert(name, p);
                        write
                            .write_all(&codec::packet(b'2', b"").map_err(io::Error::other)?)
                            .await?;
                        continue;
                    }
                    Err(e) => error = Some(e),
                }
            }
            Message::Describe(target, name) => {
                object_name = name.clone();
                let result = (|| {
                    if target == b'S' {
                        let s = objects
                            .statements
                            .get(&name)
                            .ok_or_else(|| WireError::new("26000", "unknown prepared statement"))?;
                        Ok((s.sql.clone(), s.dummy()?))
                    } else {
                        let p = objects
                            .portals
                            .get(&name)
                            .ok_or_else(|| WireError::new("34000", "unknown portal"))?;
                        portal = Some(p.clone());
                        Ok((p.statement.sql.clone(), p.params.clone()))
                    }
                })();
                match result {
                    Ok((q, p)) => {
                        sql = q;
                        params = p;
                        mode = if target == b'S' { 2 } else { 3 };
                    }
                    Err(e) => error = Some(e),
                }
            }
            Message::Close(target, name) => {
                let exists = if target == b'S' {
                    objects.statements.remove(&name).is_some()
                } else {
                    runs.remove(&name);
                    objects.portals.remove(&name).is_some()
                };
                // PostgreSQL Close is idempotent, including absent named objects.
                let _ = exists;
                write
                    .write_all(&codec::packet(b'3', b"").map_err(io::Error::other)?)
                    .await?;
                continue;
            }
            Message::Execute(name, limit) => {
                let result = (|| {
                    let p = objects
                        .portals
                        .get(&name)
                        .ok_or_else(|| WireError::new("34000", "unknown portal"))?;
                    if limit != 0 {
                        // Drivers also use max-rows=1 for ROLLBACK/SET. A command
                        // with no result columns cannot suspend or exceed this limit.
                        if column_count(&p.statement.description) != 0 {
                            return Err(WireError::unsupported(
                                "nonzero Execute max-rows; use fetch size 0",
                            ));
                        }
                    }
                    Ok(p)
                })();
                match result {
                    Ok(p) => {
                        portal = Some(p.clone());
                        sql = p.statement.sql.clone();
                        params = p.params.clone();
                    }
                    Err(e) => error = Some(e),
                }
            }
            _ => error = Some(WireError::new("08P01", "unexpected frontend message")),
        }
        let copying = crate::copy::query(&sql).ok().flatten().is_some();
        if let Some(e) = error {
            state.failed();
            failed = !simple;
            let e = deadline_error(e, operation_deadline);
            send_wire_error(&mut write, &e, &sql).await;
            let _ = writeln!(
                io::stderr(),
                "session={} duration_us={} rows=0 state={}",
                registered.id,
                started.elapsed().as_micros(),
                e.code
            );
        } else {
            let empty = sql
                .trim_matches(|c: char| c.is_whitespace() || c == ';')
                .is_empty();
            let mut next_state = state.clone();
            let mut chatter = if state.transaction == b'E' {
                next_state.evaluate(&sql)
            } else {
                crate::metadata::query_with(
                    &catalog,
                    &state.database,
                    &state.search_path(),
                    &sql,
                    &params,
                )
                .and_then(|r| {
                    if r.is_some() {
                        Ok(r)
                    } else {
                        next_state.evaluate(&sql)
                    }
                })
            };
            if operation_deadline.is_some_and(|d| Instant::now() >= d) {
                chatter = Err(WireError::new(
                    "57014",
                    "canceling statement due to statement timeout",
                ));
            }
            if mode == 2 {
                let types = &objects
                    .statements
                    .get(&object_name)
                    .expect("described statement")
                    .types;
                let mut body = (types.len() as i16).to_be_bytes().to_vec();
                for t in types {
                    body.extend((if *t == 0 { 25i32 } else { *t }).to_be_bytes());
                }
                write
                    .write_all(&codec::packet(b't', &body).map_err(io::Error::other)?)
                    .await?;
            }
            if empty || !matches!(chatter, Ok(None)) {
                let mut session_rows = 0;
                // Metadata CPU is bounded by the snapshot/query limits. Check the
                // wall deadline before output; a stalled/partial output closes the
                // socket rather than appending an error inside a frame.
                let response_budget = operation_deadline
                    .map(|d| d.saturating_duration_since(Instant::now()))
                    .unwrap_or(Duration::from_secs(2))
                    .min(Duration::from_secs(2));
                let responding = async {
                    if empty {
                        if mode == 0 {
                            write
                                .write_all(
                                    &codec::packet(b'I', b"").map_err(|_| WireError::internal())?,
                                )
                                .await
                                .map_err(|_| WireError::internal())?;
                        } else if mode >= 2 {
                            write
                                .write_all(
                                    &codec::packet(b'n', b"").map_err(|_| WireError::internal())?,
                                )
                                .await
                                .map_err(|_| WireError::internal())?;
                        }
                    } else if let Some(r) = chatter? {
                        if mode == 0 {
                            if let Some(target) = &r.deallocate {
                                if let Some(name) = target {
                                    if objects.statements.remove(name).is_none() {
                                        return Err(WireError::new(
                                            "26000",
                                            "unknown prepared statement",
                                        ));
                                    }
                                } else {
                                    objects.statements.clear();
                                }
                            }
                            state = next_state;
                            session_rows = r.rows.len();
                        }
                        if mode >= 2 || (mode == 0 && simple && !r.fields.is_empty()) {
                            let packet = if r.fields.is_empty() {
                                codec::packet(b'n', b"")
                            } else {
                                r.description()
                            };
                            let packet = packet.map_err(|_| WireError::internal())?;
                            let packet = if mode == 3 {
                                crate::binary::packet(
                                    packet,
                                    &r.description().map_err(|_| WireError::internal())?,
                                    &portal.as_ref().expect("portal").results,
                                )?
                            } else {
                                packet
                            };
                            write
                                .write_all(&packet)
                                .await
                                .map_err(|_| WireError::internal())?;
                        }
                        if mode == 0 {
                            for (k, v) in &r.status {
                                write
                                    .write_all(
                                        &codec::packet(b'S', format!("{k}\0{v}\0").as_bytes())
                                            .map_err(|_| WireError::internal())?,
                                    )
                                    .await
                                    .map_err(|_| WireError::internal())?;
                            }
                            for row in &r.rows {
                                write
                                    .write_all(&crate::binary::packet(
                                        codec::row(row).map_err(|_| WireError::internal())?,
                                        &r.description().map_err(|_| WireError::internal())?,
                                        &portal
                                            .as_ref()
                                            .map_or_else(Vec::new, |p| p.results.clone()),
                                    )?)
                                    .await
                                    .map_err(|_| WireError::internal())?;
                            }
                            write
                                .write_all(
                                    &codec::packet(b'C', format!("{}\0", r.command).as_bytes())
                                        .map_err(|_| WireError::internal())?,
                                )
                                .await
                                .map_err(|_| WireError::internal())?;
                        }
                    }
                    Ok(())
                };
                let result: Result<(), WireError> =
                    match tokio::time::timeout(response_budget, responding).await {
                        Ok(result) => result,
                        Err(_) => return Ok(()),
                    };
                let _ = writeln!(
                    io::stderr(),
                    "session={} duration_us={} rows={} state={} queue_wait_us={} worker_share={}",
                    registered.id,
                    started.elapsed().as_micros(),
                    session_rows,
                    result.as_ref().map_or_else(|e| e.code, |_| "00000"),
                    0,
                    0
                );
                if let Err(e) = result {
                    state.failed();
                    failed = !simple;
                    let e = deadline_error(e, operation_deadline);
                    send_wire_error(&mut write, &e, &sql).await;
                } else if mode == 1 {
                    if let Some(mut s) = parsed {
                        if let Ok(Some(r)) = crate::metadata::query_with(
                            &catalog,
                            &state.database,
                            &state.search_path(),
                            &sql,
                            &params,
                        )
                        .and_then(|r| {
                            if r.is_some() {
                                Ok(r)
                            } else {
                                state.clone().evaluate(&sql)
                            }
                        }) {
                            s.description = r.description().map_err(io::Error::other)?;
                        }
                        objects.statements.insert(object_name.clone(), s);
                    }
                    write
                        .write_all(&codec::packet(b'1', b"").map_err(io::Error::other)?)
                        .await?;
                }
            } else {
                let (stop, mut stopped) = watch::channel(false);
                let token = ScanCancel::new();
                let cancelled = Arc::new(AtomicBool::new(false));
                *registered
                    .session
                    .active
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Active {
                    token: token.clone(),
                    stop,
                    cancelled: Arc::clone(&cancelled),
                });
                let _deadline = fetch::Deadline::start(
                    operation_deadline.map(|d| d.saturating_duration_since(Instant::now())),
                    &registered.session,
                );
                let queued = Instant::now();
                let permit = tokio::select! {
                    p = tokio::time::timeout(Duration::from_secs(5), Arc::clone(&queries).acquire_owned()) => p.ok().and_then(Result::ok),
                    _ = stopped.changed() => None,
                    _ = disconnected.changed() => None,
                };
                let queue_wait_us = queued.elapsed().as_micros();
                let mut description = Vec::new();
                let expected = portal.as_ref().map(|p| p.statement.description.clone());
                let mut transport_failed = false;
                let result = if let Some(permit) = permit {
                    let (tx, mut rx) = mpsc::channel(BRIDGE_BUFFERS);
                    let (done, joined) = oneshot::channel();
                    let q = sql.clone();
                    let snapshot = Arc::clone(&catalog);
                    let database = state.database.clone();
                    let search_path = state.search_path();
                    let c = Arc::clone(&cancelled);
                    let progress = metrics.clone();
                    let result_formats =
                        portal.as_ref().map_or_else(Vec::new, |p| p.results.clone());
                    let execution = execution.clone();
                    let worker = std::thread::Builder::new()
                        .name("nata-query".into())
                        .stack_size(nata_sql::parse::WORKER_STACK_BYTES)
                        .spawn(move || {
                            // A single admission owns the coordinator and at most `threads` scheduler workers.
                            let _permit = permit;
                            if let Some(m) = &progress {
                                m.admitted.add(1);
                            }
                            let _admission = Admission(progress.clone());
                            if let Some(m) = &progress {
                                m.running.fetch_add(1, Ordering::SeqCst);
                            }
                            let _counted = Counted {
                                metrics: progress.clone(),
                                session: false,
                            };
                            let outcome =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    produce(
                                        &q,
                                        &params,
                                        mode,
                                        &execution,
                                        &token,
                                        &c,
                                        &tx,
                                        progress.as_deref(),
                                        &result_formats,
                                        &snapshot,
                                        &database,
                                        &search_path,
                                        None,
                                    )
                                }));
                            let _ = writeln!(
                                io::stderr(),
                                "scan_end_us={}",
                                started.elapsed().as_micros()
                            );
                            let final_result =
                                outcome.unwrap_or_else(|_| Err(WireError::internal()));
                            let _ = tx.blocking_send(Event::Finished(final_result));
                            let _ = done.send(());
                        })?;
                    let result = async {
                        let mut boundary = FrameBoundary::default();
                        loop {
                            tokio::select! {
                                biased;
                                _ = stopped.changed() => { transport_failed = !boundary.complete(); return Err(WireError::cancel()); },
                                _ = disconnected.changed() => return Err(WireError::cancel()),
                                event = rx.recv() => match event {
                                    Some(event @ (Event::Packet(_) | Event::Data(_))) => {
                                        let is_description = matches!(event, Event::Packet(_));
                                        let mut p = match event { Event::Packet(p) | Event::Data(p) => p, _ => unreachable!() };
                                        if is_description && p[0]==b'n' && mode==1 { description=p;continue; }
                                        if is_description && p[0]==b'T' {
                                            if mode==1 {description=p;continue;}
                                            if !simple && mode!=2 && expected.as_ref().is_some_and(|e| !extended::same_types(e,&p)) {
                                                return Err(WireError::unsupported("parameter changes result type; use an explicit SQL CAST"));
                                            }
                                            if mode==0 && !simple {continue;}
                                        }
                                        if is_description && p[0] == b'T' && let Some(portal) = &portal {
                                            p = crate::binary::packet(p, &portal.statement.description, &portal.results)?;
                                        }
                                        // Keep the SAME write future across cancel: restarting write_all would
                                        // duplicate a prefix, dropping it could leave half a frame on the wire.
                                        let writing = async {
                                            write.write_all(&p).await?;
                                            write.flush().await
                                        };
                                        tokio::pin!(writing);
                                        tokio::select! {
                                            biased;
                                            _ = stopped.changed() => {
                                                rx.close();
                                                registered.session.cancel();
                                                transport_failed = !matches!(tokio::time::timeout(Duration::from_secs(1), &mut writing).await, Ok(Ok(())));
                                                if !transport_failed { boundary.consume(&p); transport_failed = !boundary.complete(); }
                                                return Err(WireError::cancel());
                                            },
                                            _ = disconnected.changed() => return Err(WireError::cancel()),
                                            _ = tokio::time::sleep(Duration::from_secs(2)) => { transport_failed=true; registered.session.cancel(); return Err(WireError::new("57014","query send timeout")); },
                                            r = &mut writing => if r.is_err() { transport_failed = true; return Err(WireError::cancel()); },
                                        }
                                        boundary.consume(&p);
                                    }
                                    Some(Event::Suspended) => return Err(WireError::internal()),
                                    Some(Event::Finished(result)) => { transport_failed = !boundary.complete(); return result; },
                                    None => return Err(WireError::internal()),
                                }
                            }
                        }
                    }.await;
                    // Dropping the bridge wakes a blocked send; external cancel wakes scan claims.
                    registered.session.cancel_if_error(&result);
                    drop(rx);
                    let _ = joined.await;
                    let _ = worker.join();
                    result
                } else if cancelled.load(Ordering::SeqCst) || *disconnected.borrow() {
                    Err(WireError::cancel())
                } else {
                    Err(WireError::new("53300", "query admission timeout"))
                };
                registered.session.finish();
                let result = if operation_deadline.is_some_and(|d| Instant::now() >= d) {
                    Err(WireError::timeout())
                } else {
                    result
                };
                let rows = match &result {
                    Ok(n) => *n,
                    Err(_) => 0,
                };
                let _ = writeln!(
                    io::stderr(),
                    "session={} duration_us={} rows={} state={} queue_wait_us={} worker_share={}",
                    registered.id,
                    started.elapsed().as_micros(),
                    rows,
                    result.as_ref().map_or_else(|e| e.code, |_| "00000"),
                    queue_wait_us,
                    execution.threads
                );
                if transport_failed || *disconnected.borrow() {
                    return Ok(());
                }
                match result {
                    Ok(n) if mode == 0 => {
                        if copying {
                            write
                                .write_all(
                                    &codec::packet(b'C', format!("COPY {n}\0").as_bytes())
                                        .map_err(io::Error::other)?,
                                )
                                .await?;
                        } else {
                            write
                                .write_all(
                                    &codec::packet(b'C', format!("SELECT {n}\0").as_bytes())
                                        .map_err(io::Error::other)?,
                                )
                                .await?;
                        }
                    }
                    Ok(_) if mode == 1 => {
                        if let Some(s) = &mut parsed {
                            s.description = description;
                        }
                        if let Some(s) = parsed {
                            objects.statements.insert(object_name.clone(), s);
                        }
                        write
                            .write_all(&codec::packet(b'1', b"").map_err(io::Error::other)?)
                            .await?;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        state.failed();
                        failed = !simple;
                        let e = deadline_error(e, operation_deadline);
                        send_wire_error(&mut write, &e, &sql).await;
                    }
                }
            }
        }
        if failed || state.transaction == b'E' {
            runs.clear();
            objects.portals.clear();
        }
        if mode == 0
            && state.transaction == b'I'
            && (simple
                || sql.trim().eq_ignore_ascii_case("COMMIT")
                || sql.trim().eq_ignore_ascii_case("ROLLBACK"))
        {
            objects.portals.clear();
        }
        if simple {
            write
                .write_all(&codec::packet(b'Z', &[state.transaction]).map_err(io::Error::other)?)
                .await?;
            write.flush().await?;
        }
    }
    Ok(())
}
fn deadline_error(error: WireError, deadline: Option<Instant>) -> WireError {
    if error.code == "57014" && deadline.is_some_and(|d| Instant::now() >= d) {
        WireError::timeout()
    } else {
        error
    }
}
async fn send_wire_error<W: tokio::io::AsyncWrite + Unpin>(w: &mut W, e: &WireError, sql: &str) {
    let mut packet = codec::error(e.code, &e.message, e.pos.map(|p| codec::position(sql, p)));
    if let Some(h) = &e.hint {
        // Insert Hint before the field terminator and update the backend length.
        packet.pop();
        packet.extend(format!("H{h}\0\0").as_bytes());
        let n = (packet.len() - 1) as i32;
        packet[1..5].copy_from_slice(&n.to_be_bytes());
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        w.write_all(&packet).await?;
        w.flush().await
    })
    .await;
}
impl Session {
    fn cancel_if_error(&self, r: &Result<u64, WireError>) {
        if r.is_err() {
            self.cancel();
        }
    }
}
pub(crate) struct WireError {
    code: &'static str,
    message: String,
    pos: Option<Pos>,
    hint: Option<String>,
}
impl WireError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            pos: None,
            hint: None,
        }
    }
    pub(crate) fn unsupported(what: &str) -> Self {
        Self::new("0A000", format!("not supported yet: {what}"))
    }
    fn timeout() -> Self {
        Self::new("57014", "canceling statement due to statement timeout")
    }
    fn cancel() -> Self {
        Self::new("57014", "canceling statement due to user request")
    }
    fn internal() -> Self {
        Self::new("XX000", "internal query error")
    }
    fn sql(e: nata_sql::SqlError) -> Self {
        let code = e.sqlstate();
        // Front-end source errors contain library/IO words: never forward those.
        let message = if code == "42P01" {
            "cannot read query source".into()
        } else if e.class == ErrorClass::NotSupported && e.message.contains(": not supported yet:")
        {
            "not supported yet: source data type or encoding".into()
        } else if e.class == ErrorClass::NotSupported {
            format!("not supported yet: {}", e.message)
        } else {
            e.message
        };
        Self {
            code,
            message,
            pos: Some(e.pos),
            hint: e.suggestion.map(|s| format!("did you mean {s:?}?")),
        }
    }
    fn scan(e: ScanError) -> Self {
        match e.kind {
            ErrorKind::Cancelled(_) => Self::cancel(),
            ErrorKind::Eval {
                message,
                line,
                column,
            } => {
                let code = if message.starts_with("resource error:") {
                    "53200"
                } else if message.starts_with("date outside PostgreSQL range") {
                    "22008"
                } else if message.contains("division by zero") {
                    "22012"
                } else if message.contains("overflow") || message.contains("does not fit") {
                    "22003"
                } else if message.contains("not a valid") || message.contains("cannot be converted")
                {
                    "22P02"
                } else {
                    "22000"
                };
                Self {
                    code,
                    message,
                    pos: Some(Pos { line, col: column }),
                    hint: None,
                }
            }
            ErrorKind::NotSupported(_) => Self::unsupported("query data type or encoding"),
            ErrorKind::Internal(_) => Self::internal(),
            _ => Self::new("42P01", "cannot read query source"),
        }
    }
}
// No error frame may be appended inside a DataRow split across byte buffers.
#[derive(Default)]
struct FrameBoundary {
    header: [u8; 5],
    used: usize,
    remaining: usize,
}
impl FrameBoundary {
    fn complete(&self) -> bool {
        self.used == 0 && self.remaining == 0
    }
    fn consume(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            if self.remaining != 0 {
                let n = self.remaining.min(bytes.len());
                self.remaining -= n;
                bytes = &bytes[n..];
            } else {
                let n = (5 - self.used).min(bytes.len());
                self.header[self.used..self.used + n].copy_from_slice(&bytes[..n]);
                self.used += n;
                bytes = &bytes[n..];
                if self.used == 5 {
                    // All input is generated by the checked backend codec.
                    self.remaining =
                        u32::from_be_bytes(self.header[1..].try_into().expect("header")) as usize
                            - 4;
                    self.used = 0;
                }
            }
        }
    }
}

// Each queued packet is at most 64 KiB: the bridge holds at most 128 KiB.
const OUTPUT_BYTES: usize = 64 * 1024;
const BRIDGE_BUFFERS: usize = 2;
enum Event {
    Packet(Vec<u8>),
    Data(Vec<u8>),
    Finished(Result<u64, WireError>),
    Suspended,
}
struct Source<'a>(
    &'a AtomicBool,
    &'a crate::catalog::Snapshot,
    &'a str,
    &'a [String],
    Option<&'a dyn nata_parquet::RemoteIo>,
);
impl SourceProvider for Source<'_> {
    fn named(&self, name: &[String]) -> Result<nata_sql::source::SourceInfo, String> {
        self.1.resolve(name, self.2, self.3)
    }
    fn supports_remote(&self) -> bool {
        self.4.is_some()
    }
    fn expand(&self, p: &str) -> Result<Vec<PathBuf>, String> {
        if self.0.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        if let Some(remote) = self.4.filter(|_| nata_parquet::is_remote_location(p)) {
            return remote
                .expand(p, &|| self.0.load(Ordering::SeqCst))
                .map(|v| v.into_iter().map(PathBuf::from).collect())
                .map_err(|e| e.message);
        }
        nata_sql::FsSource.expand(p)
    }
    fn leaves(&self, p: &Path) -> Result<Vec<nata_parquet::schema::Leaf>, ScanError> {
        if self.0.load(Ordering::SeqCst) {
            return Err(ScanError::new(ErrorKind::Cancelled("cancelled".into())));
        }
        if let (Some(remote), Some(location)) = (
            self.4,
            p.to_str().filter(|l| nata_parquet::is_remote_location(l)),
        ) {
            let meta = remote
                .footer(location, &|| self.0.load(Ordering::SeqCst))
                .map_err(|e| nata_parquet::remote_scan_error(&e, p))?;
            return nata_parquet::schema::leaves(meta.file_metadata().schema_descr())
                .map_err(|m| ScanError::not_supported(m).in_file(p));
        }
        nata_sql::FsSource.leaves(p)
    }
}
#[allow(clippy::too_many_arguments)]
fn produce(
    sql: &str,
    params: &[nata_sql::ast::Expr],
    mode: u8,
    execution: &nata_exec::ExecConfig,
    token: &ScanCancel,
    cancelled: &AtomicBool,
    tx: &mpsc::Sender<Event>,
    metrics: Option<&Metrics>,
    result_formats: &[i16],
    catalog: &crate::catalog::Snapshot,
    database: &str,
    search_path: &[String],
    gate: Option<&fetch::Gate>,
) -> Result<u64, WireError> {
    let inner = crate::copy::query(sql)?;
    let copying = inner.is_some();
    let p = nata_sql::plan_parameters(
        inner.as_ref().map_or(sql, |c| c.sql.as_ref()),
        params,
        &Source(
            cancelled,
            catalog,
            database,
            search_path,
            execution.remote.as_deref(),
        ),
    );
    if cancelled.load(Ordering::SeqCst) {
        return Err(WireError::cancel());
    }
    let p = p.map_err(|e| {
        let mut e = WireError::sql(e);
        if let Some(offset) = inner.as_ref().and_then(|c| c.offset) {
            // Error positions remain relative to the entire COPY command.
            if let Some(pos) = &mut e.pos {
                let prefix = &sql[..offset];
                if pos.line == 1 {
                    pos.col += prefix.rsplit('\n').next().unwrap_or("").chars().count() as u32;
                }
                pos.line += prefix.bytes().filter(|b| *b == b'\n').count() as u32;
            }
        }
        if inner.as_ref().is_some_and(|c| c.offset.is_none()) {
            // The table form lowers to SELECT; do not report a generated offset
            // as though it pointed into the original COPY command.
            e.pos = None;
        }
        if !params.is_empty() && !matches!(e.code, "42703" | "42P01" | "42883" | "0A000") {
            e.message = "invalid parameterized query".into();
        }
        e
    })?;
    if mode != 0 {
        let description = if copying {
            codec::packet(b'n', b"")
        } else {
            codec::description(&extended::fields(&p))
        }
        .map_err(|_| WireError::internal())?;
        if description.len() > OUTPUT_BYTES {
            return Err(WireError::new("54000", "wire description exceeds 64 KiB"));
        }
        tx.blocking_send(Event::Packet(description))
            .map_err(|_| WireError::cancel())?;
        return Ok(0);
    }
    let send = |bytes: Vec<u8>| {
        if bytes.len() > OUTPUT_BYTES {
            return Err(WireError::new("54000", "wire metadata exceeds 64 KiB"));
        }
        tx.blocking_send(Event::Packet(bytes))
            .map_err(|_| WireError::cancel())
    };
    let encode = |r: Result<Vec<u8>, codec::Invalid>| {
        r.map_err(|_| WireError {
            code: "22000",
            message: "value cannot be represented in Postgres text".into(),
            hint: None,
            pos: None,
        })
    };
    use nata_sql::ast::ExplainMode;
    if gate.is_some() && p.mode != ExplainMode::No {
        return Err(WireError::unsupported("bounded Execute of EXPLAIN"));
    }
    if p.mode == ExplainMode::Plan || p.mode == ExplainMode::Analyze {
        let text = if p.mode == ExplainMode::Plan {
            p.plan.explain_text()
        } else {
            let plan = Arc::new(p.plan);
            let report = nata_exec::execute(Arc::clone(&plan), execution, Some(token), |_| Ok(()))
                .map_err(WireError::scan)?;
            nata_exec::explain::analyze_text(&plan, &report)
        };
        send(encode(codec::description(&[(
            "QUERY PLAN".into(),
            SqlType::Varchar,
        )]))?)?;
        let mut n = 0;
        for line in text.lines() {
            send(encode(codec::row(&[Some(line.into())]))?)?;
            n += 1;
        }
        return Ok(n);
    }
    let fields: Vec<_> = p.output.iter().map(|c| (c.name.clone(), c.ty)).collect();
    let binary_copy = inner
        .as_ref()
        .is_some_and(|c| c.format == crate::copy::Format::Binary);
    if copying {
        let mut response = vec![u8::from(binary_copy)];
        response.extend((fields.len() as i16).to_be_bytes());
        for _ in &fields {
            response.extend(i16::from(binary_copy).to_be_bytes());
        }
        send(encode(codec::packet(b'H', &response))?)?;
        if let Some(c) = &inner
            && c.header
        {
            let names = fields
                .iter()
                .map(|(n, _)| Some(n.clone()))
                .collect::<Vec<_>>();
            send(encode(codec::packet(
                b'd',
                &crate::copy::record(&names, c.format)?,
            ))?)?;
        }
    } else {
        send(encode(codec::description(&fields))?)?;
    }
    let description = codec::description(&fields).map_err(|_| WireError::internal())?;
    let mut rows = 0u64;
    let mut config = execution.clone();
    config.opener = metrics.and_then(|m| m.opener.clone());
    let mut observed = nata_exec::Observed::default();
    let mut copy_error = None;
    let result = nata_exec::execute_observed(
        Arc::new(p.plan),
        &config,
        Some(token),
        &mut observed,
        |batch| {
            let mut output = Vec::with_capacity(OUTPUT_BYTES);
            let send_buffer = |bytes: Vec<u8>| {
                if let Some(m) = metrics {
                    m.buffers_sent.fetch_add(1, Ordering::Relaxed);
                    m.max_buffer_bytes.fetch_max(bytes.len(), Ordering::Relaxed);
                }
                tx.blocking_send(Event::Data(bytes))
                    .map_err(|_| ScanError::new(ErrorKind::Cancelled("disconnected".into())))
            };
            for i in 0..batch.num_rows() {
                if cancelled.load(Ordering::SeqCst) {
                    return Err(ScanError::new(ErrorKind::Cancelled("cancelled".into())));
                }
                let values = batch
                    .columns()
                    .iter()
                    .zip(&fields)
                    .map(|(a, (_, ty))| {
                        if *ty == SqlType::Date {
                            use arrow_array::{Array, cast::AsArray};
                            let dates = a.as_primitive::<arrow_array::types::Date32Type>();
                            return if dates.is_null(i) {
                                Ok(None)
                            } else {
                                crate::date::text(dates.value(i)).map(Some).map_err(|e| {
                                    ScanError::new(ErrorKind::Eval {
                                        message: e.message,
                                        line: 1,
                                        column: 1,
                                    })
                                })
                            };
                        }
                        let value = nata_parquet::csv::cell_text(a.as_ref(), i)?;
                        value.map(|s| wire_text(*ty, s)).transpose()
                    })
                    .collect::<Result<Vec<_>, ScanError>>()?;
                let bytes = if let Some(c) = &inner
                    && !binary_copy
                {
                    let body = crate::copy::record(&values, c.format).map_err(|e| {
                        copy_error = Some(e);
                        ScanError::new(ErrorKind::Eval {
                            message: "unrepresentable COPY value".into(),
                            line: 1,
                            column: 1,
                        })
                    })?;
                    codec::packet(b'd', &body).map_err(|_| {
                        ScanError::new(ErrorKind::Eval {
                            message: "COPY row exceeds frame limit".into(),
                            line: 1,
                            column: 1,
                        })
                    })?
                } else {
                    let bytes = codec::row(&values).map_err(|_| {
                        ScanError::new(ErrorKind::Eval {
                            message: "unrepresentable text".into(),
                            line: 1,
                            column: 1,
                        })
                    })?;
                    let bytes = crate::binary::packet(
                        bytes,
                        &description,
                        if copying { &[1] } else { result_formats },
                    )
                    .map_err(|_| ScanError::new(ErrorKind::Internal("wire conversion".into())))?;
                    if copying {
                        // PostgreSQL (and ADBC's reader) puts the header and first
                        // tuple in the same CopyData; later messages carry one tuple.
                        let mut body = if rows == 0 {
                            crate::copy::HEADER.to_vec()
                        } else {
                            Vec::new()
                        };
                        body.extend_from_slice(&bytes[5..]);
                        codec::packet(b'd', &body).expect("checked row size")
                    } else {
                        bytes
                    }
                };
                // A single wide row may span buffers. Never increase the bridge byte bound.
                for chunk in bytes.chunks(OUTPUT_BYTES) {
                    if output.len() + chunk.len() > OUTPUT_BYTES {
                        send_buffer(std::mem::replace(
                            &mut output,
                            Vec::with_capacity(OUTPUT_BYTES),
                        ))?;
                    }
                    output.extend_from_slice(chunk);
                    if output.len() == OUTPUT_BYTES {
                        send_buffer(std::mem::replace(
                            &mut output,
                            Vec::with_capacity(OUTPUT_BYTES),
                        ))?;
                    }
                }
                rows += 1;
                if let Some(gate) = gate
                    && gate.row()
                {
                    if !output.is_empty() {
                        send_buffer(std::mem::replace(
                            &mut output,
                            Vec::with_capacity(OUTPUT_BYTES),
                        ))?;
                    }
                    tx.blocking_send(Event::Suspended).map_err(|_| {
                        ScanError::new(ErrorKind::Cancelled("closed portal".into()))
                    })?;
                    gate.wait().map_err(|_| {
                        ScanError::new(ErrorKind::Cancelled("closed portal".into()))
                    })?;
                }
                if let Some(m) = metrics {
                    m.rows_produced.fetch_add(1, Ordering::SeqCst);
                }
            }
            if !output.is_empty() {
                send_buffer(output)?;
            }
            Ok(())
        },
    );
    if let Some(m) = metrics
        && let Some(stats) = observed.stats
    {
        let mut scans = m.scans.lock().expect("scan metrics");
        // A bounded ring for lifecycle tests; never retain query plans or SQL.
        if scans.len() == 64 {
            scans.remove(0);
        }
        scans.push(stats);
    }
    if let Some(m) = metrics
        && let Some(manager) = observed.aggregate_memory
    {
        let s = manager.snapshot();
        let mut samples = m.aggregates.lock().expect("aggregate metrics");
        if samples.len() == 64 {
            samples.remove(0);
        }
        samples.push((s.budget, s.peak, s.held, s.cancelled));
    }
    if cancelled.load(Ordering::SeqCst) {
        return Err(WireError::cancel());
    }
    if let Some(e) = copy_error {
        return Err(e);
    }
    result.map_err(WireError::scan)?;
    if copying {
        if binary_copy {
            let mut tail = if rows == 0 {
                crate::copy::HEADER.to_vec()
            } else {
                Vec::new()
            };
            tail.extend_from_slice(crate::copy::TRAILER);
            send(encode(codec::packet(b'd', &tail))?)?;
        }
        send(encode(codec::packet(b'c', b""))?)?;
    }
    Ok(rows)
}
fn wire_text(ty: SqlType, s: String) -> Result<String, ScanError> {
    Ok(match ty {
        SqlType::Boolean => {
            if s == "true" {
                "t".into()
            } else {
                "f".into()
            }
        }
        SqlType::Float | SqlType::Double => crate::float::text(&s, ty == SqlType::Float),
        SqlType::Decimal { .. } if s.starts_with('.') => format!("0{s}"),
        SqlType::Decimal { .. } if s.starts_with("-.") => format!("-0{}", &s[1..]),
        _ => s,
    })
}

struct Admission(Option<Arc<Metrics>>);
impl Drop for Admission {
    fn drop(&mut self) {
        if let Some(m) = &self.0 {
            m.admitted.sub(1);
        }
    }
}
struct ObjectMap<T> {
    map: HashMap<String, T>,
    metrics: Option<Arc<Metrics>>,
}
impl<T> ObjectMap<T> {
    fn get(&self, name: &str) -> Option<&T> {
        self.map.get(name)
    }
    fn insert(&mut self, name: String, value: T) {
        if self.map.insert(name, value).is_none()
            && let Some(m) = &self.metrics
        {
            m.objects.add(1);
        }
    }
    fn remove(&mut self, name: &str) -> Option<T> {
        let value = self.map.remove(name);
        if value.is_some()
            && let Some(m) = &self.metrics
        {
            m.objects.sub(1);
        }
        value
    }
    fn clear(&mut self) {
        if let Some(m) = &self.metrics {
            m.objects.sub(self.map.len() as u64);
        }
        self.map.clear();
    }
}
impl<T> Drop for ObjectMap<T> {
    fn drop(&mut self) {
        self.clear();
    }
}
struct Objects {
    statements: ObjectMap<Statement>,
    portals: ObjectMap<Portal>,
}
impl Objects {
    fn new(metrics: Option<Arc<Metrics>>) -> Self {
        Self {
            statements: ObjectMap {
                map: HashMap::new(),
                metrics: metrics.clone(),
            },
            portals: ObjectMap {
                map: HashMap::new(),
                metrics,
            },
        }
    }
    fn check_statement(&self, name: &str, sql: &str) -> Result<(), WireError> {
        if !name.is_empty() && self.statements.get(name).is_some() {
            return Err(WireError::new("42P05", "prepared statement already exists"));
        }
        let bytes: usize = self
            .statements
            .map
            .iter()
            .map(|(n, s)| n.len() + s.sql.len())
            .sum();
        if self.statements.map.len() >= 256 || bytes + name.len() + sql.len() > 4 * codec::MAX_SQL {
            return Err(WireError::new(
                "54000",
                "prepared statement budget exceeded",
            ));
        }
        Ok(())
    }
    fn check_portal(&self, name: &str) -> Result<(), WireError> {
        if !name.is_empty() && self.portals.get(name).is_some() {
            return Err(WireError::new("42P03", "portal already exists"));
        }
        if self.portals.map.len() >= 256 {
            return Err(WireError::new("54000", "portal count exceeded"));
        }
        Ok(())
    }
    fn check_portal_bytes(&self, name: &str, portal: &Portal) -> Result<(), WireError> {
        fn size(p: &Portal) -> usize {
            // Parameter AST source expansion is bounded to MAX_SQL at Bind. Charge the
            // full allowance (including node overhead), rather than undercount literals.
            p.statement.sql.len() + p.statement.description.len() + codec::MAX_SQL * 4
        }
        let bytes: usize = self
            .portals
            .map
            .iter()
            .map(|(n, p)| n.len() + size(p))
            .sum();
        if bytes + name.len() + size(portal) > 16 * codec::MAX_SQL {
            return Err(WireError::new("54000", "portal byte budget exceeded"));
        }
        Ok(())
    }
}
pub(crate) fn column_count(description: &[u8]) -> usize {
    description
        .get(5..7)
        .map_or(0, |n| i16::from_be_bytes([n[0], n[1]]) as usize)
}

#[cfg(test)]
mod resource_error_tests {
    use super::*;
    #[test]
    fn memory_limit_preserves_sqlstate_message_and_position() {
        let message =
            "resource error: aggregate minimum working set is 123456 bytes, budget 8192 bytes";
        let error = WireError::scan(ScanError::new(ErrorKind::Eval {
            message: message.into(),
            line: 2,
            column: 8,
        }));
        assert_eq!(error.code, "53200");
        assert_eq!(error.message, message);
        assert_eq!(error.pos, Some(Pos { line: 2, col: 8 }));
    }
}
