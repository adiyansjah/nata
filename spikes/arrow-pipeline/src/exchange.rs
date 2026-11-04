//! An exchange for Arrow batches (port of the operator-trait prototype's channel). The producer side COMPACTS the
//! selection before a batch enters the channel (ADR 0001 / 0003). Wakers are stored under the channel lock and
//! called after it is released; the consumer's `Source::finish` closes the channel for the producers.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};

use crate::batch::Batch;
use crate::proto::api::*;
use crate::proto::sched::lock;

struct ChanState {
    queue: VecDeque<Batch>,
    producers: usize,
    consumers_closed: bool,
    consumer_wakers: Vec<Waker>,
    producer_wakers: Vec<Waker>,
}

pub struct Channel {
    cap: usize,
    state: Mutex<ChanState>,
    pub pushed: AtomicUsize,
    /// Batches that entered the channel still carrying a selection vector (must stay 0: compaction is the rule).
    pub uncompacted: AtomicUsize,
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
        if batch.sel().is_some() {
            self.uncompacted.fetch_add(1, SeqCst);
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
        let to_wake = if s.producers == 0 { std::mem::take(&mut s.consumer_wakers) } else { Vec::new() };
        drop(s);
        wake_all(to_wake);
    }

    fn close_consumers(&self) {
        let mut s = lock(&self.state);
        s.consumers_closed = true;
        let to_wake = std::mem::take(&mut s.producer_wakers);
        drop(s);
        wake_all(to_wake);
    }
}

/// `producers` and `consumers` must equal the `tasks` of the two pipelines.
pub fn exchange(cap: usize, producers: usize, consumers: usize) -> (Arc<dyn GlobalSink>, Arc<dyn SourceFactory>, Arc<Channel>) {
    let chan = Arc::new(Channel {
        cap,
        state: Mutex::new(ChanState { queue: VecDeque::new(), producers, consumers_closed: false, consumer_wakers: Vec::new(), producer_wakers: Vec::new() }),
        pushed: AtomicUsize::new(0),
        uncompacted: AtomicUsize::new(0),
    });
    (Arc::new(ExchangeSink(Arc::clone(&chan))), Arc::new(ExchangeSource { chan: Arc::clone(&chan), consumers: Arc::new(AtomicUsize::new(consumers)) }), chan)
}

struct ExchangeSink(Arc<Channel>);

struct ExchangeSinkLocal {
    chan: Arc<Channel>,
    /// The compacted copy of a batch the channel did not accept yet (compact once, however often it is retried).
    pending: Option<Batch>,
}

impl GlobalSink for ExchangeSink {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        Box::new(ExchangeSinkLocal { chan: Arc::clone(&self.0), pending: None })
    }

    fn finalize(&self) -> Result<()> {
        Ok(())
    }
}

impl LocalSink for ExchangeSinkLocal {
    fn push(&mut self, input: &mut Batch, waker: &Waker) -> Result<SinkResult> {
        if self.pending.is_none() {
            self.pending = Some(input.compact().map_err(|e| QueryError(format!("compact: {e}")))?);
        }
        let mut b = self.pending.take().expect("pending");
        match self.chan.push(&mut b, waker) {
            SinkResult::Blocked => {
                self.pending = Some(b); // `input` is untouched, as the protocol requires
                Ok(SinkResult::Blocked)
            }
            r => Ok(r),
        }
    }

    fn combine(self: Box<Self>) -> Result<()> {
        self.chan.producer_done();
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
        Box::new(ExchangeSourceLocal { chan: Arc::clone(&self.chan), consumers: Arc::clone(&self.consumers) })
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
