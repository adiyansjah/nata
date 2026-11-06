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
use crate::{
    extended::{self, Portal, Statement},
    session,
};

/// Counters for resource-bound checks and embedding. No SQL, values or keys.
#[derive(Default)]
pub struct Metrics {
    pub rows_produced: AtomicU64,
    pub running: AtomicUsize,
    pub sessions: AtomicUsize,
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
    pub listen: SocketAddr,
    pub unsafe_listen: bool,
    pub max_connections: usize,
    pub threads: usize,
    /// Concurrent admissions, capped by the total CPU worker budget.
    pub max_queries: usize,
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
}
impl Drop for Registered {
    fn drop(&mut self) {
        self.session.cancel();
        self.registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.id);
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
    let sockets = Arc::new(Semaphore::new(config.max_connections + 8));
    let pending = Arc::new(Semaphore::new(8));
    let sessions = Arc::new(Semaphore::new(config.max_connections));
    let queries = Arc::new(Semaphore::new(config.max_queries));
    let registry: Registry = Arc::default();
    let ids = AtomicI32::new(1);
    loop {
        let (mut socket, _) = listener.accept().await?;
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
            }
            let registration = Registered {
                id,
                registry,
                session,
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
            let _ = session_loop(socket, &registration, queries, threads, metrics, state).await;
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
    threads: usize,
    metrics: Option<Arc<Metrics>>,
    mut state: session::State,
) -> io::Result<()> {
    let (mut read, mut write) = socket.into_split();
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
    let mut statement: Option<Statement> = None;
    let mut portal: Option<Portal> = None;
    let mut failed = false;
    while let Some(m) = input.recv().await {
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
            failed = false;
            portal = None;
            write
                .write_all(&codec::packet(b'Z', &[state.transaction]).map_err(io::Error::other)?)
                .await?;
            continue;
        }
        if failed {
            continue;
        }
        if matches!(m, Message::Flush) {
            write.flush().await?;
            continue;
        }
        let started = Instant::now();
        let simple = matches!(m, Message::Query(_));
        // Modes: 0 executes, 1 validates Parse, 2 describes a statement, 3 describes a portal.
        let mut mode = 0;
        let mut params = Vec::new();
        let mut parsed = None;
        let mut error = None;
        let mut sql = String::new();
        match m {
            Message::Query(q) => {
                statement = None;
                portal = None;
                sql = q;
            }
            Message::Parse {
                name,
                sql: q,
                types,
            } => {
                statement = None;
                match Statement::new(&name, q, types).and_then(|s| Ok((s.dummy()?, s))) {
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
                let result = extended::unnamed(&name)
                    .and_then(|()| extended::unnamed(&name_s))
                    .and_then(|()| extended::formats(&formats, p.len()))
                    .and_then(|()| {
                        extended::formats(
                            &results,
                            statement.as_ref().map_or(0, |s| {
                                if s.description.len() >= 7 {
                                    i16::from_be_bytes([s.description[5], s.description[6]])
                                        as usize
                                } else {
                                    0
                                }
                            }),
                        )
                    })
                    .and_then(|()| {
                        statement
                            .as_ref()
                            .ok_or_else(|| WireError::new("26000", "no unnamed statement"))
                    })
                    .and_then(|s| s.bind(&p));
                match result {
                    Ok(p) => {
                        portal = Some(p);
                        write
                            .write_all(&codec::packet(b'2', b"").map_err(io::Error::other)?)
                            .await?;
                        continue;
                    }
                    Err(e) => error = Some(e),
                }
            }
            Message::Describe(target, name) => {
                let result = extended::unnamed(&name).and_then(|()| {
                    if target == b'S' {
                        let s = statement
                            .as_ref()
                            .ok_or_else(|| WireError::new("26000", "no unnamed statement"))?;
                        Ok((s.sql.clone(), s.dummy()?))
                    } else {
                        let p = portal
                            .as_ref()
                            .ok_or_else(|| WireError::new("34000", "no unnamed portal"))?;
                        Ok((p.statement.sql.clone(), p.params.clone()))
                    }
                });
                match result {
                    Ok((q, p)) => {
                        sql = q;
                        params = p;
                        mode = if target == b'S' { 2 } else { 3 };
                    }
                    Err(e) => error = Some(e),
                }
            }
            Message::Close(target, name) => match extended::unnamed(&name) {
                Ok(()) => {
                    if target == b'S' {
                        statement = None;
                    } else {
                        portal = None;
                    }
                    write
                        .write_all(&codec::packet(b'3', b"").map_err(io::Error::other)?)
                        .await?;
                    continue;
                }
                Err(e) => error = Some(e),
            },
            Message::Execute(name, limit) => {
                let result = extended::unnamed(&name)
                    .and_then(|()| {
                        if limit != 0 {
                            Err(WireError::unsupported(
                                "nonzero Execute max-rows; use fetch size 0",
                            ))
                        } else {
                            Ok(())
                        }
                    })
                    .and_then(|()| {
                        portal
                            .as_ref()
                            .ok_or_else(|| WireError::new("34000", "no unnamed portal"))
                    });
                match result {
                    Ok(p) => {
                        sql = p.statement.sql.clone();
                        params = p.params.clone();
                    }
                    Err(e) => error = Some(e),
                }
            }
            _ => error = Some(WireError::new("08P01", "unexpected frontend message")),
        }
        if let Some(e) = error {
            state.failed();
            failed = !simple;
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
            let chatter = next_state.evaluate(&sql);
            if mode == 2 {
                let types = &statement.as_ref().expect("described statement").types;
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
                let result: Result<(), WireError> = async {
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
                            state = next_state;
                            session_rows = r.rows.len();
                        }
                        if mode >= 2 || (mode == 0 && simple && !r.fields.is_empty()) {
                            let packet = if r.fields.is_empty() {
                                codec::packet(b'n', b"")
                            } else {
                                codec::description(&r.fields)
                            };
                            write
                                .write_all(&packet.map_err(|_| WireError::internal())?)
                                .await
                                .map_err(|_| WireError::internal())?;
                        }
                        if mode == 0 {
                            for (k, v) in r.status {
                                write
                                    .write_all(
                                        &codec::packet(b'S', format!("{k}\0{v}\0").as_bytes())
                                            .map_err(|_| WireError::internal())?,
                                    )
                                    .await
                                    .map_err(|_| WireError::internal())?;
                            }
                            for row in r.rows {
                                write
                                    .write_all(
                                        &codec::row(&row).map_err(|_| WireError::internal())?,
                                    )
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
                }
                .await;
                let _ = writeln!(
                    io::stderr(),
                    "session={} duration_us={} rows={} state={}",
                    registered.id,
                    started.elapsed().as_micros(),
                    session_rows,
                    result.as_ref().map_or_else(|e| e.code, |_| "00000")
                );
                if let Err(e) = result {
                    state.failed();
                    failed = !simple;
                    send_wire_error(&mut write, &e, &sql).await;
                } else if mode == 1 {
                    statement = parsed;
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
                let permit = tokio::select! {
                    p = tokio::time::timeout(Duration::from_secs(5), Arc::clone(&queries).acquire_owned()) => p.ok().and_then(Result::ok),
                    _ = stopped.changed() => None,
                    _ = disconnected.changed() => None,
                };
                let mut description = Vec::new();
                let expected = portal.as_ref().map(|p| p.statement.description.clone());
                let mut transport_failed = false;
                let result = if let Some(permit) = permit {
                    let (tx, mut rx) = mpsc::channel(32);
                    let (done, joined) = oneshot::channel();
                    let q = sql.clone();
                    let c = Arc::clone(&cancelled);
                    let progress = metrics.clone();
                    let worker = std::thread::Builder::new()
                        .name("nata-query".into())
                        .stack_size(nata_sql::parse::WORKER_STACK_BYTES)
                        .spawn(move || {
                            // A single admission owns the coordinator and at most `threads` scheduler workers.
                            let _permit = permit;
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
                                        threads,
                                        &token,
                                        &c,
                                        &tx,
                                        progress.as_deref(),
                                    )
                                }));
                            let final_result =
                                outcome.unwrap_or_else(|_| Err(WireError::internal()));
                            let _ = tx.blocking_send(Event::Finished(final_result));
                            let _ = done.send(());
                        })?;
                    let result = async {
                        loop {
                            tokio::select! {
                                biased;
                                _ = stopped.changed() => return Err(WireError::cancel()),
                                _ = disconnected.changed() => return Err(WireError::cancel()),
                                event = rx.recv() => match event {
                                    Some(Event::Packet(p)) => {
                                        if p[0]==b'T' {
                                            if mode==1 {description=p;continue;}
                                            if !simple && mode!=2 && expected.as_ref().is_some_and(|e| !extended::same_types(e,&p)) {
                                                return Err(WireError::unsupported("parameter changes result type; use an explicit SQL CAST"));
                                            }
                                            if mode==0 && !simple {continue;}
                                        }
                                        // Keep the SAME write future across cancel: restarting write_all would
                                        // duplicate a prefix, dropping it could leave half a frame on the wire.
                                        let writing = write.write_all(&p);
                                        tokio::pin!(writing);
                                        tokio::select! {
                                            biased;
                                            _ = stopped.changed() => {
                                                rx.close();
                                                registered.session.cancel();
                                                transport_failed = !matches!(tokio::time::timeout(Duration::from_secs(1), &mut writing).await, Ok(Ok(())));
                                                return Err(WireError::cancel());
                                            },
                                            _ = disconnected.changed() => return Err(WireError::cancel()),
                                            _ = tokio::time::sleep(Duration::from_secs(2)) => { transport_failed=true; registered.session.cancel(); return Err(WireError::new("57014","query send timeout")); },
                                            r = &mut writing => if r.is_err() { transport_failed = true; return Err(WireError::cancel()); },
                                        }
                                    }
                                    Some(Event::Finished(result)) => return result,
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
                let rows = match &result {
                    Ok(n) => *n,
                    Err(_) => 0,
                };
                let _ = writeln!(
                    io::stderr(),
                    "session={} duration_us={} rows={} state={}",
                    registered.id,
                    started.elapsed().as_micros(),
                    rows,
                    result.as_ref().map_or_else(|e| e.code, |_| "00000")
                );
                if transport_failed || *disconnected.borrow() {
                    return Ok(());
                }
                match result {
                    Ok(n) if mode == 0 => {
                        write
                            .write_all(
                                &codec::packet(b'C', format!("SELECT {n}\0").as_bytes())
                                    .map_err(io::Error::other)?,
                            )
                            .await?
                    }
                    Ok(_) if mode == 1 => {
                        if let Some(s) = &mut parsed {
                            s.description = description;
                        }
                        statement = parsed;
                        write
                            .write_all(&codec::packet(b'1', b"").map_err(io::Error::other)?)
                            .await?;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        state.failed();
                        failed = !simple;
                        send_wire_error(&mut write, &e, &sql).await;
                    }
                }
            }
        }
        if simple {
            write
                .write_all(&codec::packet(b'Z', &[state.transaction]).map_err(io::Error::other)?)
                .await?;
        }
    }
    Ok(())
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
    let _ = tokio::time::timeout(Duration::from_secs(1), w.write_all(&packet)).await;
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
                let code = if message.contains("division by zero") {
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
enum Event {
    Packet(Vec<u8>),
    Finished(Result<u64, WireError>),
}
struct Source<'a>(&'a AtomicBool);
impl SourceProvider for Source<'_> {
    fn expand(&self, p: &str) -> Result<Vec<PathBuf>, String> {
        if self.0.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        nata_sql::FsSource.expand(p)
    }
    fn leaves(&self, p: &Path) -> Result<Vec<nata_parquet::schema::Leaf>, ScanError> {
        if self.0.load(Ordering::SeqCst) {
            return Err(ScanError::new(ErrorKind::Cancelled("cancelled".into())));
        }
        nata_sql::FsSource.leaves(p)
    }
}
#[allow(clippy::too_many_arguments)]
fn produce(
    sql: &str,
    params: &[nata_sql::ast::Expr],
    mode: u8,
    threads: usize,
    token: &ScanCancel,
    cancelled: &AtomicBool,
    tx: &mpsc::Sender<Event>,
    metrics: Option<&Metrics>,
) -> Result<u64, WireError> {
    let p = nata_sql::plan_parameters(sql, params, &Source(cancelled));
    if cancelled.load(Ordering::SeqCst) {
        return Err(WireError::cancel());
    }
    let p = p.map_err(|e| {
        let mut e = WireError::sql(e);
        if !params.is_empty() && !matches!(e.code, "42703" | "42P01" | "42883" | "0A000") {
            e.message = "invalid parameterized query".into();
        }
        e
    })?;
    if mode != 0 {
        tx.blocking_send(Event::Packet(
            codec::description(&extended::fields(&p)).map_err(|_| WireError::internal())?,
        ))
        .map_err(|_| WireError::cancel())?;
        return Ok(0);
    }
    let send = |bytes: Vec<u8>| {
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
    if p.mode == ExplainMode::Plan || p.mode == ExplainMode::Analyze {
        let text = if p.mode == ExplainMode::Plan {
            p.plan.explain_text()
        } else {
            let plan = Arc::new(p.plan);
            let report = nata_exec::execute(
                Arc::clone(&plan),
                &nata_exec::ExecConfig::new(threads),
                Some(token),
                |_| Ok(()),
            )
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
    send(encode(codec::description(&fields))?)?;
    let mut rows = 0u64;
    let result = nata_exec::execute(
        Arc::new(p.plan),
        &nata_exec::ExecConfig::new(threads),
        Some(token),
        |batch| {
            for i in 0..batch.num_rows() {
                if cancelled.load(Ordering::SeqCst) {
                    return Err(ScanError::new(ErrorKind::Cancelled("cancelled".into())));
                }
                let values = batch
                    .columns()
                    .iter()
                    .zip(&fields)
                    .map(|(a, (_, ty))| {
                        let value = nata_parquet::csv::cell_text(a.as_ref(), i)?;
                        value.map(|s| wire_text(*ty, s)).transpose()
                    })
                    .collect::<Result<Vec<_>, ScanError>>()?;
                let bytes = codec::row(&values).map_err(|_| {
                    ScanError::new(ErrorKind::Eval {
                        message: "unrepresentable text".into(),
                        line: 1,
                        column: 1,
                    })
                })?;
                tx.blocking_send(Event::Packet(bytes))
                    .map_err(|_| ScanError::new(ErrorKind::Cancelled("disconnected".into())))?;
                rows += 1;
                if let Some(m) = metrics {
                    m.rows_produced.fetch_add(1, Ordering::SeqCst);
                }
            }
            Ok(())
        },
    );
    if cancelled.load(Ordering::SeqCst) {
        return Err(WireError::cancel());
    }
    result.map_err(WireError::scan)?;
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
        SqlType::Float | SqlType::Double => match s.as_str() {
            "nan" | "-nan" => "NaN".into(),
            "inf" => "Infinity".into(),
            "-inf" => "-Infinity".into(),
            _ => s,
        },
        SqlType::Date if s.len() != 10 || s.starts_with('-') || s.starts_with("0000") => {
            return Err(ScanError::new(ErrorKind::Eval {
                message: "date outside supported wire domain".into(),
                line: 1,
                column: 1,
            }));
        }
        _ => s,
    })
}
