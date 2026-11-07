//! One retained producer per portal, bounded by the existing admission and byte bridge.
use super::lifecycle::{Credit, Enter, Once, Phase};
use super::*;
use std::cell::Cell;
pub(super) struct Deadline(tokio::task::JoinHandle<()>);
impl Deadline {
    pub(super) fn start(duration: Option<Duration>, session: &Arc<Session>) -> Option<Self> {
        duration.map(|d| {
            // Pin the token, never cancel whichever query replaces it later.
            let active = session.active.current();
            Self(tokio::spawn(async move {
                tokio::time::sleep(d).await;
                if let Some(active) = active {
                    active.cancel();
                }
            }))
        })
    }
}
impl Drop for Deadline {
    fn drop(&mut self) {
        self.0.abort();
    }
}
pub(super) struct Gate {
    remaining: Cell<usize>,
    resume: Arc<Credit>,
}
impl Gate {
    pub(super) fn row(&self) -> bool {
        let mut n = self.remaining.get();
        let suspend = lifecycle::row(&mut n);
        self.remaining.set(n);
        suspend
    }
    pub(super) fn wait(&self) -> Result<(), ()> {
        self.remaining.set(self.resume.wait()?);
        Ok(())
    }
}
struct Execution {
    rx: mpsc::Receiver<Event>,
    resume: Arc<Credit>,
    token: ScanCancel,
    cancelled: Arc<AtomicBool>,
    join: JoinCell,
}
type JoinCell = Arc<Once<(oneshot::Receiver<()>, std::thread::JoinHandle<()>)>>;
fn reap(join: &JoinCell) {
    let joined = join.take();
    if let Some((done, worker)) = joined {
        tokio::spawn(async move {
            let _ = done.await;
            let _ = worker.join();
        });
    }
}
impl Execution {
    fn lease(&self, duration: Duration) -> Deadline {
        let token = self.token.clone();
        let cancelled = Arc::clone(&self.cancelled);
        let resume = Arc::clone(&self.resume);
        let join = Arc::clone(&self.join);
        Deadline(tokio::spawn(async move {
            tokio::time::sleep(duration).await;
            cancelled.store(true, Ordering::SeqCst);
            token.cancel();
            // A parked producer waits on the credit channel, not on the active
            // statement token. Wake it so it can observe cancellation and exit.
            let _ = resume.send(0);
            reap(&join);
        }))
    }
}
impl Drop for Execution {
    fn drop(&mut self) {
        self.token.cancel();
        self.cancelled.store(true, Ordering::SeqCst);
        self.resume.close();
        self.rx.close();
        // Expiry and cleanup race through one join owner; never block a socket
        // worker waiting for a producer, and never join the same thread twice.
        reap(&self.join);
    }
}
pub(super) struct Run {
    execution: Option<Execution>,
    phase: Phase<Instant>,
    expiry: Option<Deadline>,
}
impl Run {
    pub(super) fn new() -> Self {
        Self {
            execution: None,
            phase: Phase::Bound,
            expiry: None,
        }
    }
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.phase.suspended().map(|at| at + Duration::from_secs(5))
    }
    pub(super) fn expire(&mut self) {
        if self.deadline().is_some_and(|d| Instant::now() >= d) {
            self.expiry.take();
            self.execution.take();
            self.phase.expire();
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn execute<W: tokio::io::AsyncWrite + Unpin>(
        &mut self,
        write: &mut W,
        registered: &Registered,
        queries: &Arc<Semaphore>,
        execution: &nata_exec::ExecConfig,
        metrics: &Option<Arc<Metrics>>,
        snapshot: &Arc<crate::catalog::Snapshot>,
        state: &session::State,
        deadline: Option<Instant>,
        portal: &Portal,
        limit: i32,
        disconnected: &mut watch::Receiver<bool>,
    ) -> Result<(), WireError> {
        self.expire();
        self.expiry.take();
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(WireError::timeout());
        }
        let enter = self.phase.enter();
        if enter == Enter::Expired {
            return Err(WireError::new("34000", "suspended portal expired"));
        }
        if enter == Enter::Complete {
            let packet = codec::packet(b'C', b"SELECT 0\0").map_err(|_| WireError::internal())?;
            let writing = async {
                write.write_all(&packet).await?;
                write.flush().await
            };
            return match tokio::time::timeout(
                deadline
                    .map(|d| d.saturating_duration_since(Instant::now()))
                    .unwrap_or(Duration::from_secs(2))
                    .min(Duration::from_secs(2)),
                writing,
            )
            .await
            {
                Ok(Ok(())) => Ok(()),
                _ => Err(WireError::new("08006", "query send timeout")),
            };
        }
        let (stop, mut stopped) = watch::channel(false);
        let token = self
            .execution
            .as_ref()
            .map_or_else(ScanCancel::new, |e| e.token.clone());
        let cancelled = self.execution.as_ref().map_or_else(
            || Arc::new(AtomicBool::new(false)),
            |e| Arc::clone(&e.cancelled),
        );
        registered.session.active.install(Active {
            token: token.clone(),
            stop,
            cancelled: Arc::clone(&cancelled),
        });
        let _deadline = Deadline::start(
            deadline.map(|d| d.saturating_duration_since(Instant::now())),
            &registered.session,
        );
        let result = async {
            if self.execution.is_none() {
                let permit = tokio::select! {
                    _ = stopped.changed() => return Err(WireError::cancel()),
                    _ = disconnected.changed() => return Err(WireError::cancel()),
                    p = tokio::time::timeout(Duration::from_secs(5), Arc::clone(queries).acquire_owned()) => p.ok().and_then(Result::ok).ok_or_else(|| WireError::new("53300", "query admission timeout"))?,
                };
                let (tx, rx) = mpsc::channel(BRIDGE_BUFFERS);
                let resume = Arc::new(Credit::new());
                let resumed = Arc::clone(&resume);
                let (done, joined) = oneshot::channel();
                let p = portal.clone();
                let execution = execution.clone();
                let snapshot = Arc::clone(snapshot);
                let database = state.database.clone();
                let search_path = state.search_path();
                let progress = metrics.clone();
                let c = Arc::clone(&cancelled);
                let t = token.clone();
                let worker = std::thread::Builder::new().name("nata-portal".into()).stack_size(nata_sql::parse::WORKER_STACK_BYTES).spawn(move || {
                    let _permit = permit;
                    if let Some(m) = &progress { m.admitted.add(1); m.running.fetch_add(1, Ordering::SeqCst); }
                    let _admission = Admission(progress.clone());
                    let _counted = Counted { metrics: progress.clone(), session: false };
                    let gate = Gate { remaining: Cell::new(limit as usize), resume: resumed };
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| produce(&p.statement.sql, &p.params, 0, &execution, &t, &c, &tx, progress.as_deref(), &p.results, &snapshot, &database, &search_path, Some(&gate))));
                    let _ = tx.blocking_send(Event::Finished(outcome.unwrap_or_else(|_| Err(WireError::internal()))));
                    let _ = done.send(());
                }).map_err(|_| WireError::internal())?;
                self.execution = Some(Execution { rx, resume, token, cancelled, join: Arc::new(Once::new((joined, worker))) });
            } else if let Some(execution) = &self.execution {
                execution.resume.send(limit as usize).map_err(|_| WireError::internal())?;
            }
            let e = self.execution.as_mut().expect("execution");
            let mut rows = 0;
            let mut boundary = FrameBoundary::default();
            loop {
                let event = tokio::select! {
                    biased;
                    _ = stopped.changed() => { if !boundary.complete() { return Err(WireError::new("08006", "incomplete output frame")); } return Err(WireError::cancel()); },
                    _ = disconnected.changed() => return Err(WireError::cancel()),
                    e = e.rx.recv() => e.ok_or_else(WireError::internal)?,
                };
                match event {
                    Event::Packet(p) if p[0] == b'T' => {
                        if !extended::same_types(&portal.statement.description, &p) { return Err(WireError::unsupported("parameter changes result type")); }
                    }
                    Event::Packet(p) | Event::Data(p) => {
                        // Count complete DataRows across bounded chunks without collecting them.
                        for byte in &p {
                            let header = boundary.remaining == 0 && boundary.used == 4;
                            boundary.consume(std::slice::from_ref(byte));
                            if header && boundary.header[0] == b'D' { rows += 1; }
                        }
                        let writing = async { write.write_all(&p).await?; write.flush().await };
                        tokio::pin!(writing);
                        tokio::select! {
                            biased;
                            _ = stopped.changed() => {
                                if !matches!(tokio::time::timeout(Duration::from_secs(1), &mut writing).await, Ok(Ok(()))) || !boundary.complete() { return Err(WireError::new("08006", "incomplete output frame")); }
                                return Err(WireError::cancel());
                            }
                            _ = disconnected.changed() => return Err(WireError::cancel()),
                            r = tokio::time::timeout(Duration::from_secs(2), &mut writing) => { if !matches!(r, Ok(Ok(()))) { return Err(WireError::new("08006", "query send timeout")); } },
                        }
                    }
                    Event::Suspended => {
                        if deadline.is_some_and(|d| Instant::now() >= d) { return Err(WireError::timeout()); }
                        // Remove the cancel target BEFORE publishing suspension. Keep
                        // its sender alive until the final write completes so receiver
                        // closure cannot masquerade as a CancelRequest.
                        let _parked = registered.session.active.take(); // suspension transition
                        // Exact-end fetches suspend too: exhaustion is only observed on resume.
                        terminal(write, &codec::packet(b's', b"").map_err(|_| WireError::internal())?, &mut stopped, disconnected).await?;
                        self.phase.suspend(Instant::now());
                        self.expiry = Some(e.lease(Duration::from_secs(5)));
                        return Ok(());
                    }
                    Event::Finished(result) => {
                        result?;
                        if deadline.is_some_and(|d| Instant::now() >= d) { return Err(WireError::timeout()); }
                        terminal(write, &codec::packet(b'C', format!("SELECT {rows}\0").as_bytes()).map_err(|_| WireError::internal())?, &mut stopped, disconnected).await?;
                        self.execution.take(); self.phase.complete();
                        return Ok(());
                    }
                }
            }
        }.await;
        if self.phase.suspended().is_none() {
            registered.session.finish();
        }
        if result.is_err() {
            self.execution.take();
        }
        result
    }
}

async fn terminal<W: tokio::io::AsyncWrite + Unpin>(
    write: &mut W,
    packet: &[u8],
    stopped: &mut watch::Receiver<bool>,
    disconnected: &mut watch::Receiver<bool>,
) -> Result<(), WireError> {
    let writing = async {
        write.write_all(packet).await?;
        write.flush().await
    };
    tokio::pin!(writing);
    tokio::select! {
        biased;
        _ = stopped.changed() => {
            if !matches!(tokio::time::timeout(Duration::from_secs(1), &mut writing).await, Ok(Ok(()))) { return Err(WireError::new("08006", "incomplete output frame")); }
            Err(WireError::cancel())
        },
        _ = disconnected.changed() => Err(WireError::new("08006", "disconnected")),
        r = tokio::time::timeout(Duration::from_secs(2), &mut writing) => if matches!(r, Ok(Ok(()))) { Ok(()) } else { Err(WireError::new("08006", "query send timeout")) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn active() -> Active {
        let (stop, _) = watch::channel(false);
        Active {
            token: ScanCancel::new(),
            stop,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
    #[tokio::test]
    async fn idle_lease_wakes_and_reaps_without_session_loop_polling() {
        let resume = Arc::new(Credit::new());
        let resumed = Arc::clone(&resume);
        let (done, joined) = oneshot::channel();
        let (observed, exited) = oneshot::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let c = Arc::clone(&cancelled);
        let worker = std::thread::spawn(move || {
            let gate = Gate {
                remaining: Cell::new(0),
                resume: resumed,
            };
            gate.wait().expect("lease wakes credit wait");
            assert!(c.load(Ordering::SeqCst));
            let _ = observed.send(());
            let _ = done.send(());
        });
        let (_, rx) = mpsc::channel(BRIDGE_BUFFERS);
        let execution = Execution {
            rx,
            resume,
            token: ScanCancel::new(),
            cancelled,
            join: Arc::new(Once::new((joined, worker))),
        };
        let _lease = execution.lease(Duration::from_millis(1));
        tokio::time::timeout(Duration::from_secs(1), exited)
            .await
            .expect("lease deadline")
            .expect("producer exit");
        assert!(execution.join.take().is_none());
    }
    #[tokio::test]
    async fn deadline_is_pinned_to_its_execution_not_a_replacement() {
        let first = active();
        let next = active();
        let session = Arc::new(Session {
            secret: 0,
            active: lifecycle::Slot::new(Some(first.clone())),
        });
        let _deadline = Deadline::start(Some(Duration::from_millis(1)), &session);
        session.active.install(next.clone());
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(first.token.is_cancelled());
        assert!(!next.token.is_cancelled());
    }
}
