//! Test and measurement support: CPU-burning sources, a completion-signalling sink, and the
//! starvation scenario (K heavy tasks and one small task on a pool of W threads).
//! Not built under `--cfg nata_loom`.

use std::hint::black_box;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::*;

/// Real CPU work for (at least) `d` of wall time. Wall-bounded so that a "morsel" has the same
/// length on every machine; under CPU contention it does less work in that time, which only makes
/// the scenario harder.
pub fn spin(d: Duration) {
    let end = Instant::now() + d;
    let mut x = 1u64;
    while Instant::now() < end {
        for _ in 0..64 {
            x = black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407));
        }
    }
    black_box(x);
}

/// A fixed amount of CPU work (not wall-bounded): `iters` dependent multiply-adds.
pub fn work(iters: u64) -> u64 {
    let mut x = 1u64;
    for _ in 0..iters {
        x = black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407));
    }
    x
}

/// Source: `morsels` batches of one row, each produced after burning `morsel` of CPU.
pub struct Burn {
    pub morsels: u64,
    pub morsel: Duration,
}

impl SourceFactory for Burn {
    fn make(&self) -> Box<dyn Source> {
        Box::new(BurnSource {
            left: self.morsels,
            morsel: self.morsel,
        })
    }
}

struct BurnSource {
    left: u64,
    morsel: Duration,
}

impl Source for BurnSource {
    fn next(&mut self, _w: &Waker) -> Result<SourceResult> {
        if self.left == 0 {
            return Ok(SourceResult::Finished);
        }
        self.left -= 1;
        spin(self.morsel);
        Ok(SourceResult::Batch(Batch::from_cols(vec![vec![1]])))
    }
}

/// Sink that discards its input (after burning `push_cost` of CPU per batch) and records the time at
/// which the pipeline is finalized.
#[derive(Clone, Default)]
pub struct Signal {
    pub done: Arc<Mutex<Option<Instant>>>,
    pub push_cost: Duration,
}

struct SignalLocal(Duration);

impl GlobalSink for Signal {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        Box::new(SignalLocal(self.push_cost))
    }
    fn finalize(&self) -> Result<()> {
        *self.done.lock().unwrap() = Some(Instant::now());
        Ok(())
    }
}

impl LocalSink for SignalLocal {
    fn push(&mut self, _input: &mut Batch, _w: &Waker) -> Result<SinkResult> {
        if !self.0.is_zero() {
            spin(self.0);
        }
        Ok(SinkResult::NeedMoreInput)
    }
    fn combine(self: Box<Self>) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Scenario {
    pub workers: usize,
    /// Number of CPU-heavy tasks (one pipeline with this many tasks).
    pub heavy: usize,
    pub heavy_morsel: Duration,
    /// Total heavy runtime per task is far beyond any test budget.
    pub heavy_total: Duration,
    pub small_morsels: u64,
    pub small_morsel: Duration,
    /// Give up waiting for the small task after this long.
    pub timeout: Duration,
}

#[derive(Debug)]
pub struct Outcome {
    /// Time from `start` until the small pipeline was finalized; `None` if it did not within `timeout`.
    pub latency: Option<Duration>,
    pub result: std::result::Result<(), QueryError>,
}

/// K heavy tasks start first (so they hold every worker), the small task is enqueued last.
pub fn run_starvation(s: &Scenario, start: impl FnOnce(Query, usize) -> RunningQuery) -> Outcome {
    let heavy_morsels = (s.heavy_total.as_nanos() / s.heavy_morsel.as_nanos().max(1)) as u64;
    let done = Signal::default();
    let heavy = Pipeline {
        name: "heavy".into(),
        source: Arc::new(Burn {
            morsels: heavy_morsels,
            morsel: s.heavy_morsel,
        }),
        ops: vec![],
        sink: Arc::new(Signal::default()),
        deps: vec![],
        tasks: s.heavy,
    };
    let small = Pipeline {
        name: "small".into(),
        source: Arc::new(Burn {
            morsels: s.small_morsels,
            morsel: s.small_morsel,
        }),
        ops: vec![],
        sink: Arc::new(done.clone()),
        deps: vec![],
        tasks: 1,
    };
    let t0 = Instant::now();
    let rq = start(Query::new(vec![heavy, small]), s.workers);
    let mut latency = None;
    while t0.elapsed() < s.timeout {
        if let Some(t) = *done.done.lock().unwrap() {
            latency = Some(t - t0);
            break;
        }
        std::thread::sleep(Duration::from_micros(500));
    }
    rq.cancel();
    let result = rq.wait();
    Outcome { latency, result }
}

/// Minimal `block_on` for the channel's send future (a stand-in for a tokio task).
pub fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
    use std::task::{Context, Poll, Wake};
    struct Unpark(std::thread::Thread, AtomicBool);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.1.store(true, SeqCst);
            self.0.unpark();
        }
    }
    let u = Arc::new(Unpark(std::thread::current(), AtomicBool::new(false)));
    let waker = std::task::Waker::from(Arc::clone(&u));
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        while !u.1.swap(false, SeqCst) {
            std::thread::park();
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ConsumerScenario {
    pub workers: usize,
    pub heavy: usize,
    pub heavy_morsel: Duration,
    pub chunks: u64,
    /// A producer thread (standing in for an I/O task) sends one chunk per `gap`.
    pub gap: Duration,
    /// CPU the consumer spends per chunk.
    pub per_chunk: Duration,
    pub cap: usize,
    pub timeout: Duration,
}

/// Heavy tasks hold the pool; a consumer task is fed through the bounded channel of the bridge.
/// Returns the time until the consumer has taken all chunks (`None` on timeout).
pub fn run_consumer(s: &ConsumerScenario, slice: SliceConfig) -> Option<Duration> {
    use crate::chan::{ChanSources, Chunk, channel};
    let (tx, rx) = channel::<Chunk>(s.cap);
    let done = Signal {
        done: Arc::default(),
        push_cost: s.per_chunk,
    };
    let heavy = Pipeline {
        name: "heavy".into(),
        source: Arc::new(Burn {
            morsels: 60_000_000 / (s.heavy_morsel.as_micros().max(1) as u64),
            morsel: s.heavy_morsel,
        }),
        ops: vec![],
        sink: Arc::new(Signal::default()),
        deps: vec![],
        tasks: s.heavy,
    };
    let consume = Pipeline {
        name: "consume".into(),
        source: Arc::new(ChanSources::new(vec![rx])),
        ops: vec![],
        sink: Arc::new(done.clone()),
        deps: vec![],
        tasks: 1,
    };
    let t0 = Instant::now();
    let rq = Query::new(vec![heavy, consume]).with_slice(slice).start(s.workers).unwrap();
    let (chunks, gap) = (s.chunks, s.gap);
    let producer = std::thread::spawn(move || {
        for i in 0..chunks {
            std::thread::sleep(gap);
            if block_on(tx.send(Chunk::bare(i))).is_err() {
                break;
            }
        }
    });
    let mut latency = None;
    while t0.elapsed() < s.timeout {
        if let Some(t) = *done.done.lock().unwrap() {
            latency = Some(t - t0);
            break;
        }
        std::thread::sleep(Duration::from_micros(500));
    }
    rq.cancel();
    let _ = rq.wait();
    let _ = producer.join();
    latency
}
