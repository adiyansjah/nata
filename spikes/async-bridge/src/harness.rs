//! Test and measurement support for the real bridge: a tokio runtime, mock object store,
//! producers, a collecting sink, CPU contention, and the stress scenario runner.
//! Not built under `--cfg nata_loom`.

use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::runtime::{Builder, Runtime};

use crate::chan::{ChanSources, Chunk, Sender, channel};
use crate::sched::lock;
use crate::*;

pub fn runtime(workers: usize) -> Runtime {
    Builder::new_multi_thread()
        .worker_threads(workers)
        .thread_name("nata-io")
        .enable_time()
        .build()
        .expect("tokio runtime")
}

pub fn xorshift(x: &mut u64) -> u64 {
    if *x == 0 {
        *x = 0x2545_F491_4F6C_DD1D;
    }
    *x ^= *x << 13;
    *x ^= *x >> 7;
    *x ^= *x << 17;
    *x
}

/// Mock object store latency per chunk.
#[derive(Clone, Copy, Debug)]
pub enum Latency {
    /// No await at all: the producer never gives the runtime a chance to run something else.
    None,
    /// One `yield_now`.
    Yield,
    Sleep(Duration),
    /// Sleep for a random time in `0..max_us` microseconds.
    Random {
        max_us: u64,
    },
}

impl Latency {
    pub async fn wait(&self, rng: &mut u64) {
        match *self {
            Latency::None => {}
            Latency::Yield => tokio::task::yield_now().await,
            Latency::Sleep(d) => tokio::time::sleep(d).await,
            Latency::Random { max_us } => {
                let us = xorshift(rng) % max_us.max(1);
                if us == 0 {
                    tokio::task::yield_now().await
                } else {
                    tokio::time::sleep(Duration::from_micros(us)).await
                }
            }
        }
    }
}

/// Per-batch work done by the sink (the "consumer").
#[derive(Clone, Copy, Debug)]
pub enum Work {
    None,
    Sleep(Duration),
}

/// Records `(task, id)` of every batch the moment it is pushed.
pub struct Collect {
    pub got: Arc<Mutex<Vec<(usize, u64)>>>,
    pub work: Work,
}

struct CollectLocal {
    task: usize,
    got: Arc<Mutex<Vec<(usize, u64)>>>,
    work: Work,
}

impl GlobalSink for Collect {
    fn make(&self, task: usize) -> Box<dyn LocalSink> {
        Box::new(CollectLocal {
            task,
            got: Arc::clone(&self.got),
            work: self.work,
        })
    }
    fn finalize(&self) -> Result<()> {
        Ok(())
    }
}

impl LocalSink for CollectLocal {
    fn push(&mut self, input: &mut Batch, _w: &Waker) -> Result<SinkResult> {
        if let Work::Sleep(d) = self.work {
            std::thread::sleep(d);
        }
        let mut g = lock(&self.got);
        for &id in &input.cols[0] {
            g.push((self.task, id as u64));
        }
        Ok(SinkResult::NeedMoreInput)
    }
    fn combine(self: Box<Self>) -> Result<()> {
        Ok(())
    }
}

/// Busy-loop threads that compete for the CPUs for as long as the guard lives.
pub struct Busy {
    stop: Arc<AtomicBool>,
    handles: Vec<std::thread::JoinHandle<()>>,
}

impl Busy {
    pub fn start(n: usize) -> Busy {
        let stop = Arc::new(AtomicBool::new(false));
        let handles = (0..n)
            .map(|_| {
                let s = Arc::clone(&stop);
                std::thread::spawn(move || {
                    let mut x = 1u64;
                    while !s.load(SeqCst) {
                        for _ in 0..1000 {
                            x = std::hint::black_box(
                                x.wrapping_mul(6364136223846793005).wrapping_add(1),
                            );
                        }
                    }
                })
            })
            .collect();
        Busy { stop, handles }
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.stop.store(true, SeqCst);
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

pub async fn produce(tx: Sender<Chunk>, base: u64, n: usize, lat: Latency, mut rng: u64) {
    for i in 0..n as u64 {
        lat.wait(&mut rng).await;
        if tx.send(Chunk::bare(base + i)).await.is_err() {
            break; // receiver closed: nobody is reading
        }
    }
}

// ------------------------------------------------------------------ the stress scenario

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Every producer sends all its chunks, then drops its sender.
    Normal,
    /// Producers send a random number of chunks (possibly zero) and close early.
    EarlyClose,
    /// Some producer tasks are aborted from outside after a random delay.
    Abort,
    /// The query is cancelled after a random delay.
    Cancel,
}

#[derive(Clone, Debug)]
pub struct Scenario {
    pub pool: usize,
    pub channels: usize,
    pub producers: usize,
    pub cap: usize,
    pub chunks: usize,
    pub latency: Latency,
    pub kind: Kind,
    pub seed: u64,
}

#[derive(Debug)]
pub struct Outcome {
    pub delivered: usize,
    pub planned: usize,
    pub query_err: bool,
}

const ID_K: u32 = 40;
const ID_J: u32 = 20;

/// Run one scenario to completion and check the result. `Err` describes a wrong result.
/// (A hang never returns: callers use [`run_timeboxed`].)
pub fn run(sc: &Scenario) -> std::result::Result<Outcome, String> {
    let mut rng = sc.seed ^ 0xA5A5_5A5A_1234_5678;
    let rt = runtime(2);
    let mut senders: Vec<Vec<Sender<Chunk>>> = Vec::new();
    let mut receivers = Vec::new();
    for _ in 0..sc.channels {
        let (tx, rx) = channel::<Chunk>(sc.cap);
        let mut v = vec![tx];
        for _ in 1..sc.producers {
            let c = v[0].clone();
            v.push(c);
        }
        senders.push(v);
        receivers.push(rx);
    }
    let got = Arc::new(Mutex::new(Vec::new()));
    let pipe = Pipeline {
        name: "consume".into(),
        source: Arc::new(ChanSources::new(receivers)),
        ops: vec![],
        sink: Arc::new(Collect {
            got: Arc::clone(&got),
            work: Work::None,
        }),
        deps: vec![],
        tasks: sc.channels,
    };
    let rq = Query::new(vec![pipe]).start(sc.pool).map_err(|e| e.0)?;

    // planned[k][j] = number of chunks producer (k, j) will try to send
    let mut planned = vec![vec![0usize; sc.producers]; sc.channels];
    let mut handles = Vec::new();
    let mut abortable = Vec::new();
    for (k, txs) in senders.into_iter().enumerate() {
        for (j, tx) in txs.into_iter().enumerate() {
            let n = match sc.kind {
                Kind::EarlyClose => (xorshift(&mut rng) as usize) % (sc.chunks + 1),
                _ => sc.chunks,
            };
            planned[k][j] = n;
            let base = ((k as u64) << ID_K) | ((j as u64) << ID_J);
            let h = rt.spawn(produce(tx, base, n, sc.latency, xorshift(&mut rng)));
            if sc.kind == Kind::Abort && xorshift(&mut rng) % 3 == 0 {
                abortable.push(h.abort_handle());
            }
            handles.push(h);
        }
    }
    // 1 us .. 2 ms, log-uniform, so cancel/abort land in every phase of the run
    let delay = Duration::from_micros(1u64 << (xorshift(&mut rng) % 12));
    match sc.kind {
        Kind::Abort => {
            std::thread::sleep(delay);
            abortable.iter().for_each(|a| a.abort());
        }
        Kind::Cancel => {
            std::thread::sleep(delay);
            rq.cancel();
        }
        _ => {}
    }
    let result = rq.wait();
    // Producers must all end: normally because they finished, after cancel/abort because the
    // channel is closed or their task is gone.
    let joined = rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            for h in handles {
                let _ = h.await;
            }
        })
        .await
    });
    if joined.is_err() {
        return Err("producers did not terminate after the query ended".into());
    }
    rt.shutdown_timeout(Duration::from_secs(2));

    let got = lock(&got).clone();
    let mut per_producer = std::collections::HashMap::<(u64, u64), Vec<u64>>::new();
    for &(task, id) in &got {
        let (k, j, i) = (id >> ID_K, (id >> ID_J) & 0xFFFFF, id & 0xFFFFF);
        if k as usize != task {
            return Err(format!(
                "chunk {id:#x} of channel {k} arrived in task {task}"
            ));
        }
        per_producer.entry((k, j)).or_default().push(i);
    }
    let mut delivered = 0;
    let mut planned_total = 0;
    for (k, row) in planned.iter().enumerate() {
        for (j, &n) in row.iter().enumerate() {
            planned_total += n;
            let d = per_producer
                .remove(&(k as u64, j as u64))
                .unwrap_or_default();
            delivered += d.len();
            // FIFO per sender, so what arrived must be exactly the first `m` chunks, in order.
            if !d.iter().copied().eq(0..d.len() as u64) {
                return Err(format!(
                    "producer ({k},{j}) delivered {d:?}: not a prefix in order"
                ));
            }
            if d.len() > n {
                return Err(format!(
                    "producer ({k},{j}) delivered {} > planned {n}",
                    d.len()
                ));
            }
            let must_be_complete = matches!(sc.kind, Kind::Normal | Kind::EarlyClose);
            if must_be_complete && d.len() != n {
                return Err(format!(
                    "producer ({k},{j}) delivered {} of {n}: chunks lost",
                    d.len()
                ));
            }
        }
    }
    if !per_producer.is_empty() {
        return Err(format!(
            "chunks from unknown producers: {:?}",
            per_producer.keys()
        ));
    }
    let query_err = match (&result, sc.kind) {
        (Ok(()), _) => false,
        (Err(e), Kind::Cancel) if e.0 == "cancelled" => true,
        (Err(e), _) => return Err(format!("query failed: {e}")),
    };
    Ok(Outcome {
        delivered,
        planned: planned_total,
        query_err,
    })
}

pub enum Verdict {
    Done(std::result::Result<Outcome, String>),
    Hang,
}

/// Run a scenario on its own thread with a watchdog. A hung run leaks its threads.
pub fn run_timeboxed(sc: &Scenario, timeout: Duration) -> Verdict {
    let (tx, rx) = std::sync::mpsc::channel();
    let sc2 = sc.clone();
    std::thread::spawn(move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&sc2)));
        let _ = tx.send(match r {
            Ok(r) => r,
            Err(p) => Err(format!(
                "panic: {}",
                p.downcast_ref::<String>()
                    .cloned()
                    .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_default()
            )),
        });
    });
    match rx.recv_timeout(timeout) {
        Ok(r) => Verdict::Done(r),
        Err(_) => Verdict::Hang,
    }
}

/// The `i`-th configuration of the matrix (deterministic): cycles through every dimension.
pub fn config(i: usize, seed: u64) -> Scenario {
    let pools = [1usize, 2];
    let channels = [1usize, 3, 8];
    let producers = [1usize, 3];
    let caps = [1usize, 2, 8];
    let chunks = [0usize, 1, 5, 40, 200];
    let lats = [
        Latency::None,
        Latency::Yield,
        Latency::Sleep(Duration::from_micros(50)),
        Latency::Random { max_us: 400 },
    ];
    let kinds = [Kind::Normal, Kind::EarlyClose, Kind::Abort, Kind::Cancel];
    let total = kinds.len()
        * pools.len()
        * channels.len()
        * producers.len()
        * caps.len()
        * chunks.len()
        * lats.len();
    // Spread consecutive indexes over the whole matrix (1409 is prime, so coprime to `total`):
    // any prefix of the run covers every dimension, and `i >= total` repeats with a new seed.
    let mut r = (i * 1409) % total;
    let mut pick = |n: usize| {
        let v = r % n;
        r /= n;
        v
    };
    // kinds and pools vary fastest so that any short prefix of the matrix covers all of them
    let kind = kinds[pick(kinds.len())];
    let pool = pools[pick(pools.len())];
    let ch = channels[pick(channels.len())];
    let pr = producers[pick(producers.len())];
    let cap = caps[pick(caps.len())];
    let ck = chunks[pick(chunks.len())];
    let lat = lats[pick(lats.len())];
    Scenario {
        pool,
        channels: ch,
        producers: pr,
        cap,
        chunks: ck,
        latency: lat,
        kind,
        seed: seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(i as u64),
    }
}

pub fn elapsed_ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}
