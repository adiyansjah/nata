#![cfg(not(nata_loom))]
//! Does a finished query give back its memory? (std threads only; compiled out under `--cfg nata_loom`.)
//!
//! The exchange keeps the wakers of blocked tasks in `consumer_wakers` / `producer_wakers`. A waker
//! owns its task, which owns the scheduler state (`Shared`), which owns the pipelines, which own
//! the exchange: a reference cycle. The lists are drained only when the other side acts (a push or
//! `producer_done` for consumers, a pop or `close_consumers` for producers). A task that is woken
//! by something else (cancel; hypothesis: also a sibling's early finish) and ends leaves its stale
//! waker behind. f2 showed it for cancel (failed 20 of 20 before the fix in `close_consumers`, which now
//! drops the stored wakers); f1 probes the sibling path and never leaked.
//!
//! Each test holds only a `Weak<Channel>` and fails if the channel is still alive after the query
//! has ended and every handle has been dropped.

use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use spike_exchange_loom::*;

/// Emits `items` one-row batches, `pause` apart; if `stuck`, never produces anything and never wakes.
struct SlowInner {
    items: usize,
    pause: Duration,
    stuck: bool,
    next: AtomicUsize,
}

struct Slow(Arc<SlowInner>);

struct SlowLocal(Arc<SlowInner>);

impl SourceFactory for Slow {
    fn make(&self) -> Box<dyn Source> {
        Box::new(SlowLocal(Arc::clone(&self.0)))
    }
}

impl Source for SlowLocal {
    fn next(&mut self, _w: &Waker) -> Result<SourceResult> {
        if self.0.stuck {
            return Ok(SourceResult::Blocked);
        }
        std::thread::sleep(self.0.pause);
        let m = self.0.next.fetch_add(1, SeqCst);
        if m >= self.0.items {
            return Ok(SourceResult::Finished);
        }
        Ok(SourceResult::Batch(Batch::from_cols(vec![vec![m as i64]])))
    }
}

struct Count(Arc<Mutex<usize>>);

impl GlobalSink for Count {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        Box::new(CountLocal(0, Arc::clone(&self.0)))
    }
    fn finalize(&self) -> Result<()> {
        Ok(())
    }
}

struct CountLocal(usize, Arc<Mutex<usize>>);

impl LocalSink for CountLocal {
    fn push(&mut self, input: &mut Batch, _w: &Waker) -> Result<SinkResult> {
        self.0 += input.len();
        Ok(SinkResult::NeedMoreInput)
    }
    fn combine(self: Box<Self>) -> Result<()> {
        *self.1.lock().unwrap() += self.0;
        Ok(())
    }
}

struct Setup {
    query: Query,
    chan: Weak<Channel>,
    received: Arc<Mutex<usize>>,
}

fn setup(items: usize, stuck_producer: bool, cons_tasks: usize, limit: Option<usize>) -> Setup {
    let (xsink, xsource, chan) = exchange(1, 1, cons_tasks);
    let weak = Arc::downgrade(&chan);
    drop(chan);
    let received = Arc::new(Mutex::new(0));
    let producer = Pipeline {
        name: "producer".into(),
        source: Arc::new(Slow(Arc::new(SlowInner {
            items,
            pause: Duration::from_millis(2),
            stuck: stuck_producer,
            next: AtomicUsize::new(0),
        }))),
        ops: vec![],
        sink: xsink,
        deps: vec![],
        tasks: 1,
    };
    let ops: Vec<Arc<dyn OperatorFactory>> = limit
        .map(|n| Arc::new(Limit::new(n)) as Arc<dyn OperatorFactory>)
        .into_iter()
        .collect();
    let consumer = Pipeline {
        name: "consumer".into(),
        source: xsource,
        ops,
        sink: Arc::new(Count(Arc::clone(&received))),
        deps: vec![],
        tasks: cons_tasks,
    };
    Setup {
        query: Query::new(vec![producer, consumer]),
        chan: weak,
        received,
    }
}

/// Control: a plain query that runs to the end releases the channel.
#[test]
fn f0_control_plain_query_releases_the_channel() {
    let s = setup(5, false, 1, None);
    s.query.run(4).unwrap();
    assert_eq!(*s.received.lock().unwrap(), 5);
    assert!(s.chan.upgrade().is_none(), "channel still alive after a plain query");
}

/// `LIMIT 1` over two consumer tasks: probes the sibling-stop path (`wake_pipeline`). Passed in
/// 20 of 20 runs, so no leak is claimed for it.
#[test]
fn f1_limit_with_two_consumer_tasks_releases_the_channel() {
    let s = setup(50, false, 2, Some(1));
    s.query.run(4).unwrap();
    assert_eq!(*s.received.lock().unwrap(), 1);
    assert!(
        s.chan.upgrade().is_none(),
        "LEAK: the channel is still alive after the query ended (a stale consumer waker keeps the scheduler state alive)"
    );
}

/// Cancel while a consumer is blocked on the empty exchange.
#[test]
fn f2_cancel_with_blocked_consumer_releases_the_channel() {
    let s = setup(0, true, 1, None);
    let running = s.query.start(2).unwrap();
    std::thread::sleep(Duration::from_millis(50)); // let the consumer block
    running.cancel();
    assert_eq!(running.wait(), Err(QueryError("cancelled".into())));
    assert!(
        s.chan.upgrade().is_none(),
        "LEAK: the channel is still alive after cancel (a stale consumer waker keeps the scheduler state alive)"
    );
}
