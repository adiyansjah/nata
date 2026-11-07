//! A bounded query-owned disk service. Submission and completion registration
//! never wait on a CPU worker. Completion notification is sticky, including
//! completion/cancel/panic before registration. Query teardown joins the worker.
use nata_sched::memory::{Grant, MemoryManager, Reservation, Wake};
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub type DiskJob<T> = Box<dyn FnOnce() -> io::Result<T> + Send + 'static>;
struct Admission {
    live: AtomicUsize,
    limit: usize,
    _grant: Grant,
}
struct State<T> {
    admission: Arc<Admission>,
    result: Option<io::Result<T>>,
    wake: Option<Box<dyn Wake>>,
}
impl<T> Drop for State<T> {
    fn drop(&mut self) {
        drop(self.result.take());
        drop(self.wake.take());
        self.admission.live.fetch_sub(1, Ordering::SeqCst);
    }
}
/// One pending result. Dropping the ticket discards the eventual result, whose
/// RAII owners still release all files/reservations when the job completes.
pub struct Ticket<T>(Arc<Mutex<State<T>>>);
impl<T> Ticket<T> {
    /// Register/re-register a scheduler callback, or take a completed result.
    /// A callback only requeues; it never executes an operator or disk job.
    pub fn poll(&self, wake: impl Wake + 'static) -> Option<io::Result<T>> {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(result) = state.result.take() {
            return Some(result);
        }
        let old = state.wake.replace(Box::new(wake));
        drop(state);
        drop(old);
        None
    }
}
struct Work<T> {
    job: DiskJob<T>,
    completion: Arc<Mutex<State<T>>>,
}
pub enum Submit<T> {
    Queued(Ticket<T>),
    /// The task retains its job and yields until its previous completion frees
    /// a queue slot. No thread waits for capacity in submit.
    Full(DiskJob<T>),
    Closed(DiskJob<T>),
}
pub struct IoService<T: Send + 'static> {
    sender: Option<SyncSender<Work<T>>>,
    worker: Option<JoinHandle<()>>,
    cancelled: Arc<AtomicBool>,
    admission: Arc<Admission>,
}
impl<T: Send + 'static> IoService<T> {
    /// `capacity` is bounded by the query's up-front workspace/task allowance.
    /// Exactly one native I/O worker, independent of the CPU task count.
    /// Jobs/results must carry grants for their separately allocated buffers;
    /// this admission charge covers queue/ticket metadata and inline `T`.
    pub fn new(capacity: usize, memory: &Arc<MemoryManager>) -> io::Result<Self> {
        if capacity == 0 || capacity > 32 {
            return Err(io::Error::other("invalid spill I/O queue capacity"));
        }
        let per_slot = std::mem::size_of::<State<T>>()
            .checked_add(std::mem::size_of::<Work<T>>())
            .and_then(|n| n.checked_add(512))
            .ok_or_else(|| io::Error::other("spill I/O metadata size"))?;
        let charge = (capacity + 1)
            .checked_mul(per_slot)
            .and_then(|n| n.checked_add(4096))
            .ok_or_else(|| io::Error::other("spill I/O metadata size"))?;
        let grant = match MemoryManager::request(memory, 0, charge) {
            Ok(Reservation::Granted(grant)) => grant,
            _ => return Err(io::Error::other("spill I/O queue memory limit")),
        };
        let (sender, receiver) = sync_channel::<Work<T>>(capacity);
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&cancelled);
        let worker = std::thread::Builder::new()
            .name("nata-spill-io".into())
            .spawn(move || {
                for Work { job, completion } in receiver {
                    let result = if stop.load(Ordering::SeqCst) {
                        drop(job);
                        Err(io::Error::new(
                            io::ErrorKind::Interrupted,
                            "spill cancelled",
                        ))
                    } else {
                        match catch_unwind(AssertUnwindSafe(job)) {
                            Ok(result) => result,
                            Err(_) => Err(io::Error::other("spill I/O task panic")),
                        }
                    };
                    // Dropping a cancelled job also drops all the files it owns,
                    // before publishing completion or letting query teardown join.
                    let mut state = completion
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    state.result = Some(result);
                    let wake = state.wake.take();
                    drop(state);
                    if let Some(wake) = wake {
                        wake.wake();
                    }
                }
            })?;
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
            cancelled,
            admission: Arc::new(Admission {
                live: AtomicUsize::new(0),
                limit: capacity + 1,
                _grant: grant,
            }),
        })
    }
    pub fn submit(&self, job: DiskJob<T>) -> Submit<T> {
        let Some(sender) = &self.sender else {
            return Submit::Closed(job);
        };
        if self
            .admission
            .live
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                (live < self.admission.limit).then_some(live + 1)
            })
            .is_err()
        {
            return Submit::Full(job);
        }
        let completion = Arc::new(Mutex::new(State {
            admission: Arc::clone(&self.admission),
            result: None,
            wake: None,
        }));
        match sender.try_send(Work {
            job,
            completion: Arc::clone(&completion),
        }) {
            Ok(()) => Submit::Queued(Ticket(completion)),
            Err(TrySendError::Full(work)) => Submit::Full(work.job),
            Err(TrySendError::Disconnected(work)) => Submit::Closed(work.job),
        }
    }
    /// In-flight syscalls finish; queued jobs are dropped without running.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}
impl<T: Send + 'static> Drop for IoService<T> {
    fn drop(&mut self) {
        self.cancel();
        drop(self.sender.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
