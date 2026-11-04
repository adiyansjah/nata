//! The REAL exchange of the operator-trait prototype: `git show HEAD:spikes/operator-trait/src/ops.rs`,
//! lines 237-390 (section "exchange"), copied verbatim. Differences (see SNAPSHOT.md): the imports,
//! and ONE marked fix in `close_consumers` (finding F1: the stale-waker leak). Imports:
//! `Mutex`/`AtomicUsize` come from `crate::sync` (loom under `--cfg nata_loom`); `Arc` stays std's
//! because the trait objects it holds (`Arc<dyn GlobalSink>`) cannot be loom `Arc`s.

use std::collections::VecDeque;
use std::sync::Arc;

use crate::api::*;
use crate::batch::Batch;
use crate::sched::lock;
use crate::sync::Ordering::SeqCst;
use crate::sync::{AtomicUsize, Mutex};

struct ChanState {
    queue: VecDeque<Batch>,
    producers: usize,
    consumers_closed: bool,
    consumer_wakers: Vec<Waker>,
    producer_wakers: Vec<Waker>,
}

/// A bounded queue of batches between two pipelines that run at the same time.
pub struct Channel {
    cap: usize,
    state: Mutex<ChanState>,
    pub pushed: AtomicUsize,
}

fn wake_all(wakers: Vec<Waker>) {
    wakers.iter().for_each(Waker::wake);
}

impl Channel {
    fn push(&self, batch: &mut Batch, waker: &Waker) -> SinkResult {
        let mut s = lock(&self.state);
        if s.consumers_closed {
            return SinkResult::Finished;
        }
        if s.queue.len() >= self.cap {
            s.producer_wakers.push(waker.clone());
            return SinkResult::Blocked;
        }
        s.queue.push_back(std::mem::take(batch));
        self.pushed.fetch_add(1, SeqCst);
        let to_wake = std::mem::take(&mut s.consumer_wakers);
        drop(s);
        wake_all(to_wake);
        SinkResult::NeedMoreInput
    }

    fn pop(&self, waker: &Waker) -> SourceResult {
        let mut s = lock(&self.state);
        if let Some(b) = s.queue.pop_front() {
            let to_wake = std::mem::take(&mut s.producer_wakers);
            drop(s);
            wake_all(to_wake);
            return SourceResult::Batch(b);
        }
        if s.producers == 0 {
            return SourceResult::Finished;
        }
        s.consumer_wakers.push(waker.clone());
        SourceResult::Blocked
    }

    fn producer_done(&self) {
        let mut s = lock(&self.state);
        s.producers -= 1;
        let to_wake = if s.producers == 0 {
            std::mem::take(&mut s.consumer_wakers)
        } else {
            Vec::new()
        };
        drop(s);
        wake_all(to_wake);
    }

    fn close_consumers(&self) {
        let mut s = lock(&self.state);
        s.consumers_closed = true;
        let to_wake = std::mem::take(&mut s.producer_wakers);
        // FIX for finding F1 (not in ops.rs): every consumer task has ended when the channel is
        // closed, so the consumer wakers still stored are stale. A waker owns its task, which owns
        // the scheduler state, which owns this channel: keeping it closes a reference cycle that
        // leaks the whole query after a cancel. Take them and drop them outside the lock.
        let stale = std::mem::take(&mut s.consumer_wakers);
        drop(s);
        wake_all(to_wake);
        drop(stale);
    }
}

/// Make a channel plus its sink (producer side) and source (consumer side). `producers` and
/// `consumers` must equal the `tasks` of the two pipelines.
pub fn exchange(
    cap: usize,
    producers: usize,
    consumers: usize,
) -> (Arc<dyn GlobalSink>, Arc<dyn SourceFactory>, Arc<Channel>) {
    let chan = Arc::new(Channel {
        cap,
        state: Mutex::new(ChanState {
            queue: VecDeque::new(),
            producers,
            consumers_closed: false,
            consumer_wakers: Vec::new(),
            producer_wakers: Vec::new(),
        }),
        pushed: AtomicUsize::new(0),
    });
    let sink = Arc::new(ExchangeSink(Arc::clone(&chan)));
    let source = Arc::new(ExchangeSource {
        chan: Arc::clone(&chan),
        consumers: Arc::new(AtomicUsize::new(consumers)),
    });
    (sink, source, chan)
}

struct ExchangeSink(Arc<Channel>);

struct ExchangeSinkLocal(Arc<Channel>);

impl GlobalSink for ExchangeSink {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        Box::new(ExchangeSinkLocal(Arc::clone(&self.0)))
    }

    fn finalize(&self) -> Result<()> {
        Ok(())
    }
}

impl LocalSink for ExchangeSinkLocal {
    fn push(&mut self, input: &mut Batch, waker: &Waker) -> Result<SinkResult> {
        Ok(self.0.push(input, waker))
    }

    fn combine(self: Box<Self>) -> Result<()> {
        self.0.producer_done();
        Ok(())
    }
}

struct ExchangeSource {
    chan: Arc<Channel>,
    consumers: Arc<AtomicUsize>,
}

struct ExchangeSourceLocal {
    chan: Arc<Channel>,
    consumers: Arc<AtomicUsize>,
}

impl SourceFactory for ExchangeSource {
    fn make(&self) -> Box<dyn Source> {
        Box::new(ExchangeSourceLocal {
            chan: Arc::clone(&self.chan),
            consumers: Arc::clone(&self.consumers),
        })
    }
}

impl Source for ExchangeSourceLocal {
    fn next(&mut self, waker: &Waker) -> Result<SourceResult> {
        Ok(self.chan.pop(waker))
    }

    fn finish(&mut self) {
        if self.consumers.fetch_sub(1, SeqCst) == 1 {
            self.chan.close_consumers();
        }
    }
}

// ---------------------------------------------------------------- ADDED for the loom tests
// (not in ops.rs) A read-only probe so the tests can check the bound on the queue.
impl Channel {
    /// Number of batches queued right now (taken under the channel lock).
    pub fn queue_len(&self) -> usize {
        lock(&self.state).queue.len()
    }

    /// The configured capacity.
    pub fn capacity(&self) -> usize {
        self.cap
    }
}

impl Channel {
    /// (consumer wakers, producer wakers) still stored, i.e. tasks the channel could still wake.
    pub fn stored_wakers(&self) -> (usize, usize) {
        let s = lock(&self.state);
        (s.consumer_wakers.len(), s.producer_wakers.len())
    }
}
