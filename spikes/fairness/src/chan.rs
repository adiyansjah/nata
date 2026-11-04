//! The hand-off between the I/O runtime and the CPU pool: a bounded channel whose producer side
//! is async (a `Future` that returns `Pending` when full) and whose consumer side is synchronous
//! (`recv` never waits: it stores the scheduler's [`Waker`] and reports `Blocked`).
//!
//! Everything shared lives behind ONE mutex. The consumer's "is it empty? is it closed? then store
//! my waker" is a single critical section, and a sender's "push, then take the waker" is another,
//! so a push either happens before the consumer's check (the consumer sees the item) or after the
//! waker is stored (the push finds the waker and fires it). Wakers are fired after the lock is
//! released. This file compiles unchanged under `--cfg nata_loom` (see `tests/loom_bridge.rs`).

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use crate::api::{Result, Source, SourceFactory, SourceResult, Waker};
use crate::batch::Batch;
use crate::sched::lock;
use crate::sync::Mutex;

// ------------------------------------------------------------------ test hooks (not in loom)

/// Microseconds. When non-zero, the channel sleeps or yields for a random time below this at the
/// marked points (between releasing the lock and firing a waker, and between storing a waker and
/// returning `Blocked`), to widen the race windows. Process-wide, like `TEST_WORKER_DELAY_US`.
pub static HOOK_DELAY_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(not(nata_loom))]
fn hook() {
    use std::sync::atomic::Ordering::Relaxed;
    let max = HOOK_DELAY_US.load(Relaxed);
    if max == 0 {
        return;
    }
    thread_local! {
        static RNG: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }
    let r = RNG.with(|c| {
        let mut x = c.get();
        if x == 0 {
            x = (&c as *const _ as u64) ^ 0x9E37_79B9_7F4A_7C15 ^ max;
        }
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        c.set(x);
        x
    });
    if r & 1 == 0 {
        std::thread::yield_now();
    } else {
        std::thread::sleep(std::time::Duration::from_micros(1 + (r >> 1) % max));
    }
}

#[cfg(nata_loom)]
fn hook() {}

// ------------------------------------------------------------------ memory tracking

/// Counts live chunks and live bytes, with peaks. A chunk's payload is counted from creation to
/// drop, wherever it is (being built, waiting to send, in the channel, held by the consumer).
#[derive(Default)]
pub struct Tracker {
    live_chunks: std::sync::atomic::AtomicUsize,
    live_bytes: std::sync::atomic::AtomicUsize,
    peak_chunks: std::sync::atomic::AtomicUsize,
    peak_bytes: std::sync::atomic::AtomicUsize,
    created: std::sync::atomic::AtomicUsize,
}

impl Tracker {
    pub fn peak_chunks(&self) -> usize {
        self.peak_chunks.load(std::sync::atomic::Ordering::SeqCst)
    }
    pub fn peak_bytes(&self) -> usize {
        self.peak_bytes.load(std::sync::atomic::Ordering::SeqCst)
    }
    pub fn live_chunks(&self) -> usize {
        self.live_chunks.load(std::sync::atomic::Ordering::SeqCst)
    }
    pub fn created(&self) -> usize {
        self.created.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// One prefetched piece of an object. `id` identifies it; the payload is real memory.
pub struct Chunk {
    pub id: u64,
    data: Vec<u8>,
    tracker: Option<Arc<Tracker>>,
}

impl Chunk {
    /// A chunk with no payload and no tracking (used by the model checker).
    pub fn bare(id: u64) -> Self {
        Chunk {
            id,
            data: Vec::new(),
            tracker: None,
        }
    }

    pub fn tracked(id: u64, bytes: usize, tracker: &Arc<Tracker>) -> Self {
        use std::sync::atomic::Ordering::SeqCst;
        // Touch every page so the memory is really resident.
        let data = vec![(id & 0xff) as u8 | 1; bytes];
        let chunks = tracker.live_chunks.fetch_add(1, SeqCst) + 1;
        let b = tracker.live_bytes.fetch_add(bytes, SeqCst) + bytes;
        tracker.peak_chunks.fetch_max(chunks, SeqCst);
        tracker.peak_bytes.fetch_max(b, SeqCst);
        tracker.created.fetch_add(1, SeqCst);
        Chunk {
            id,
            data,
            tracker: Some(Arc::clone(tracker)),
        }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

impl Drop for Chunk {
    fn drop(&mut self) {
        if let Some(t) = &self.tracker {
            use std::sync::atomic::Ordering::SeqCst;
            t.live_chunks.fetch_sub(1, SeqCst);
            t.live_bytes.fetch_sub(self.data.len(), SeqCst);
        }
    }
}

// ------------------------------------------------------------------ the channel

struct State<T> {
    queue: VecDeque<T>,
    senders: usize,
    rx_closed: bool,
    rx_waker: Option<Waker>,
    tx_wakers: Vec<std::task::Waker>,
}

struct Inner<T> {
    cap: usize,
    state: Mutex<State<T>>,
}

/// Create a bounded channel holding at most `cap` queued items (`cap >= 1`).
pub fn channel<T>(cap: usize) -> (Sender<T>, Receiver<T>) {
    assert!(cap >= 1);
    let inner = Arc::new(Inner {
        cap,
        state: Mutex::new(State {
            queue: VecDeque::new(),
            senders: 1,
            rx_closed: false,
            rx_waker: None,
            tx_wakers: Vec::new(),
        }),
    });
    (
        Sender {
            inner: Arc::clone(&inner),
        },
        Receiver { inner },
    )
}

pub struct Sender<T> {
    inner: Arc<Inner<T>>,
}

pub struct Receiver<T> {
    inner: Arc<Inner<T>>,
}

/// The receiver is gone (closed by the consumer, or the query was cancelled). Gives the item back.
#[derive(Debug)]
pub struct Closed<T>(pub T);

pub enum Recv<T> {
    Item(T),
    /// Nothing now; the waker was stored and will be fired by a send or by the last sender's drop.
    Blocked,
    /// Empty and every sender is gone.
    Closed,
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        lock(&self.inner.state).senders += 1;
        Sender {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let waker = {
            let mut s = lock(&self.inner.state);
            s.senders -= 1;
            if s.senders == 0 {
                s.rx_waker.take()
            } else {
                None
            }
        };
        hook();
        if let Some(w) = waker {
            w.wake();
        }
    }
}

impl<T> Sender<T> {
    /// Send one item; waits (asynchronously) while the channel is full.
    pub fn send(&self, item: T) -> SendFuture<T> {
        SendFuture {
            inner: Arc::clone(&self.inner),
            item: Some(item),
        }
    }
}

pub struct SendFuture<T> {
    inner: Arc<Inner<T>>,
    item: Option<T>,
}

impl<T> Unpin for SendFuture<T> {}

impl<T> Future for SendFuture<T> {
    type Output = std::result::Result<(), Closed<T>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        hook();
        let mut s = lock(&this.inner.state);
        if s.rx_closed {
            drop(s);
            return Poll::Ready(Err(Closed(
                this.item.take().expect("polled after completion"),
            )));
        }
        if s.queue.len() < this.inner.cap {
            s.queue
                .push_back(this.item.take().expect("polled after completion"));
            let waker = s.rx_waker.take();
            drop(s);
            hook();
            if let Some(w) = waker {
                w.wake();
            }
            Poll::Ready(Ok(()))
        } else {
            if !s.tx_wakers.iter().any(|w| w.will_wake(cx.waker())) {
                s.tx_wakers.push(cx.waker().clone());
            }
            Poll::Pending
        }
    }
}

impl<T> Receiver<T> {
    /// Never waits. Either returns an item, reports that the channel is finished, or stores
    /// `waker` and returns `Blocked`.
    pub fn recv(&self, waker: &Waker) -> Recv<T> {
        let mut s = lock(&self.inner.state);
        if let Some(item) = s.queue.pop_front() {
            let senders = std::mem::take(&mut s.tx_wakers);
            drop(s);
            hook();
            senders.into_iter().for_each(std::task::Waker::wake);
            return Recv::Item(item);
        }
        if s.senders == 0 {
            return Recv::Closed;
        }
        s.rx_waker = Some(waker.clone());
        drop(s);
        // Window: the waker is stored, the task has not reported `Blocked` yet.
        hook();
        Recv::Blocked
    }

    /// Tell the senders nobody is reading: pending and future sends fail, queued items are freed.
    pub fn close(&self) {
        let (senders, items) = {
            let mut s = lock(&self.inner.state);
            s.rx_closed = true;
            (
                std::mem::take(&mut s.tx_wakers),
                std::mem::take(&mut s.queue),
            )
        };
        drop(items);
        hook();
        senders.into_iter().for_each(std::task::Waker::wake);
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        self.close();
    }
}

// ------------------------------------------------------------------ the scheduler-facing source

/// A [`SourceFactory`] that hands receiver `i` to task `i` of its pipeline.
pub struct ChanSources {
    receivers: Mutex<Vec<Option<Receiver<Chunk>>>>,
}

impl ChanSources {
    pub fn new(receivers: Vec<Receiver<Chunk>>) -> Self {
        ChanSources {
            receivers: Mutex::new(receivers.into_iter().map(Some).collect()),
        }
    }
}

impl SourceFactory for ChanSources {
    fn make(&self) -> Box<dyn Source> {
        let rx = lock(&self.receivers)
            .iter_mut()
            .find_map(Option::take)
            .expect("more tasks than receivers");
        Box::new(ChanSource { rx, held: None })
    }
}

/// Source over one receiver. The chunk handed to the pipeline stays counted as "held" until the
/// next `next()` call or `finish()`, so the memory bound includes the chunk being processed.
pub struct ChanSource {
    rx: Receiver<Chunk>,
    held: Option<Chunk>,
}

impl Source for ChanSource {
    fn next(&mut self, waker: &Waker) -> Result<SourceResult> {
        self.held = None;
        Ok(match self.rx.recv(waker) {
            Recv::Item(c) => {
                let b = Batch::from_cols(vec![vec![c.id as i64]]);
                self.held = Some(c);
                SourceResult::Batch(b)
            }
            Recv::Blocked => SourceResult::Blocked,
            Recv::Closed => SourceResult::Finished,
        })
    }

    fn finish(&mut self) {
        self.held = None;
        self.rx.close();
    }
}
