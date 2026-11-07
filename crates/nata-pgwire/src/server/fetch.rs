//! One retained producer per portal, bounded by the existing admission and byte bridge.
use super::lifecycle::{Credit, Enter, Lease, Once};
use super::*;
use std::cell::Cell;
use std::sync::Weak;
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
    idle: Arc<Idle>,
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
/// A candidate is tied to one retained producer, never to a registry's current token.
struct Idle {
    phase: Arc<Lease<Instant>>,
    token: ScanCancel,
    cancelled: Arc<AtomicBool>,
    resume: Arc<Credit>,
    join: JoinCell,
}
impl Idle {
    fn expire(&self, at: Instant) -> bool {
        self.phase.expire(at, || {
            self.cancelled.store(true, Ordering::SeqCst);
            self.token.cancel();
            let _ = self.resume.send(0);
            reap(&self.join);
        })
    }
    fn lease(self: &Arc<Self>, at: Instant) -> Deadline {
        let idle = Arc::clone(self);
        Deadline(tokio::spawn(async move {
            tokio::time::sleep_until((at + Duration::from_secs(5)).into()).await;
            idle.expire(at);
        }))
    }
}

/// Only suspended producers are listed. Weak entries neither retain work nor
/// outlive its object cap; publication prunes to at most the admission count.
pub(super) struct Admissions {
    permits: Arc<Semaphore>,
    limit: usize,
    idle: Mutex<Vec<Weak<Idle>>>,
    changed: tokio::sync::Notify,
}
impl Admissions {
    pub(super) fn new(count: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(count)),
            limit: count,
            idle: Mutex::new(Vec::new()),
            changed: tokio::sync::Notify::new(),
        }
    }
    fn publish(&self, idle: &Arc<Idle>) {
        let mut entries = self.idle.lock().expect("idle portals");
        entries.retain(|w| {
            w.upgrade()
                .is_some_and(|p| !Arc::ptr_eq(&p, idle) && p.phase.suspended().is_some())
        });
        // Timer expiry can release an admission during pruning. Recheck until
        // that stale entry is gone; the publishing producer owns one admission.
        while entries.len() >= self.limit {
            entries.retain(|w| w.upgrade().is_some_and(|p| p.phase.suspended().is_some()));
        }
        entries.push(Arc::downgrade(idle));
        self.changed.notify_waiters();
    }
    // Selection serializes waiters; the lease lock independently arbitrates resume.
    fn yield_oldest(&self) -> (bool, Option<Instant>) {
        let mut entries = self.idle.lock().expect("idle portals");
        loop {
            if self.permits.available_permits() != 0 {
                return (false, None);
            }
            let candidate = entries
                .iter()
                .filter_map(Weak::upgrade)
                .filter_map(|p| p.phase.suspended().map(|at| (p, at)))
                .min_by_key(|(_, at)| *at);
            let Some((idle, at)) = candidate else {
                return (false, None);
            };
            let eligible = at + Duration::from_millis(250);
            if Instant::now() < eligible {
                return (false, Some(eligible));
            }
            let expired = idle.expire(at);
            entries.retain(|w| w.upgrade().is_some_and(|p| !Arc::ptr_eq(&p, &idle)));
            if expired {
                return (true, None);
            }
            // Resume won. Re-select rather than cancelling that executing query.
        }
    }
    pub(super) async fn acquire(
        &self,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, tokio::sync::AcquireError> {
        let admission = Arc::clone(&self.permits).acquire_owned();
        tokio::pin!(admission);
        let pressure = async {
            loop {
                let notified = self.changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable(); // register before inspecting publication
                let (yielded, next) = self.yield_oldest();
                if yielded {
                    break;
                } // one successful expiry per waiting query
                if let Some(next) = next {
                    tokio::select! { _ = &mut notified => {}, _ = tokio::time::sleep_until(next.into()) => {} }
                } else {
                    notified.await;
                }
            }
            std::future::pending::<()>().await;
        };
        tokio::select! {
            biased;
            permit = &mut admission => permit,
            _ = pressure => unreachable!(),
        }
    }
}
impl Drop for Execution {
    fn drop(&mut self) {
        self.idle.phase.close();
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
    phase: Arc<Lease<Instant>>,
    expiry: Option<Deadline>,
}
impl Run {
    pub(super) fn new() -> Self {
        Self {
            execution: None,
            phase: Arc::new(Lease::new()),
            expiry: None,
        }
    }
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.phase.suspended().map(|at| at + Duration::from_secs(5))
    }
    pub(super) fn expire(&mut self) {
        if let Some(at) = self.phase.suspended()
            && Instant::now() >= at + Duration::from_secs(5)
            && let Some(e) = &self.execution
        {
            e.idle.expire(at);
        }
        if self.phase.expired() {
            self.expiry.take();
            self.execution.take();
        }
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn execute<W: tokio::io::AsyncWrite + Unpin>(
        &mut self,
        write: &mut W,
        registered: &Registered,
        queries: &Arc<Admissions>,
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
                    p = tokio::time::timeout(Duration::from_secs(5), queries.acquire()) => p.ok().and_then(Result::ok).ok_or_else(|| WireError::new("53300", "query admission timeout"))?,
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
                let join = Arc::new(Once::new((joined, worker)));
                let idle = Arc::new(Idle { phase: Arc::clone(&self.phase), token: token.clone(), cancelled: Arc::clone(&cancelled), resume: Arc::clone(&resume), join: Arc::clone(&join) });
                self.execution = Some(Execution { rx, resume, token, cancelled, join, idle });
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
                        let at = Instant::now();
                        self.phase.suspend(at);
                        self.expiry = Some(e.idle.lease(at));
                        queries.publish(&e.idle);
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
        let phase = Arc::new(Lease::new());
        phase.enter();
        let at = Instant::now() - Duration::from_secs(5) + Duration::from_millis(1);
        phase.suspend(at);
        let idle = Arc::new(Idle {
            phase,
            token: ScanCancel::new(),
            cancelled,
            resume,
            join: Arc::new(Once::new((joined, worker))),
        });
        let _lease = idle.lease(at);
        tokio::time::timeout(Duration::from_secs(1), exited)
            .await
            .expect("lease deadline")
            .expect("producer exit");
        assert!(idle.join.take().is_none());
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
