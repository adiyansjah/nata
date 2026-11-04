//! Measurements for ADR 0002's hand-off (items 7a-7c of bench/results/20261104-async-bridge.md).
//!   measure latency [--rounds 15] [--chunks 200] [--producers 4]   7a + 7b control
//!   measure memory                                                  7c bound (+ unbounded control)
//!   measure idle   [--rounds 5]                                     7c reverse: idle CPU of the pool
//! Every run is checked for exact delivery before its numbers are used.
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant as StdInstant};

use spike_async_bridge::chan::{ChanSources, Chunk, Sender, Tracker, channel};
use spike_async_bridge::harness::*;
use spike_async_bridge::sched::lock;
use spike_async_bridge::*;

const POOL: usize = 2;
const TOKIO: usize = 2;

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn loadavg() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .unwrap_or_default()
        .split_whitespace()
        .take(3)
        .collect::<Vec<_>>()
        .join(" ")
}

fn load1() -> f64 {
    loadavg()
        .split(' ')
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0)
}

fn spin(d: Duration) {
    let t = StdInstant::now();
    while t.elapsed() < d {
        std::hint::spin_loop();
    }
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i]
}

fn sorted(mut v: Vec<f64>) -> Vec<f64> {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v
}

// ---------------------------------------------------------------- heavy CPU work on the pool

struct SpinSource(Arc<AtomicBool>);
struct SpinSourceFactory(Arc<AtomicBool>);
impl SourceFactory for SpinSourceFactory {
    fn make(&self) -> Box<dyn Source> {
        Box::new(SpinSource(Arc::clone(&self.0)))
    }
}
impl Source for SpinSource {
    fn next(&mut self, _w: &Waker) -> Result<SourceResult> {
        Ok(if self.0.load(SeqCst) {
            SourceResult::Finished
        } else {
            SourceResult::Batch(Batch::from_cols(vec![vec![1]]))
        })
    }
}
struct Burn(Duration);
struct BurnFactory(Duration);
impl OperatorFactory for BurnFactory {
    fn make(&self) -> Box<dyn Operator> {
        Box::new(Burn(self.0))
    }
}
impl Operator for Burn {
    fn execute(&mut self, _i: &Batch, _o: &mut Batch) -> Result<OpResult> {
        spin(self.0);
        Ok(OpResult::NeedMoreInput)
    }
}
struct NullSink;
struct NullLocal;
impl GlobalSink for NullSink {
    fn make(&self, _t: usize) -> Box<dyn LocalSink> {
        Box::new(NullLocal)
    }
    fn finalize(&self) -> Result<()> {
        Ok(())
    }
}
impl LocalSink for NullLocal {
    fn push(&mut self, _i: &mut Batch, _w: &Waker) -> Result<SinkResult> {
        Ok(SinkResult::NeedMoreInput)
    }
    fn combine(self: Box<Self>) -> Result<()> {
        Ok(())
    }
}

/// `tasks` CPU-heavy tasks that never block (each batch burns 200 us) until `stop` is set.
fn heavy_pipeline(stop: &Arc<AtomicBool>, tasks: usize) -> Pipeline {
    Pipeline {
        name: "heavy".into(),
        source: Arc::new(SpinSourceFactory(Arc::clone(stop))),
        ops: vec![Arc::new(BurnFactory(Duration::from_micros(200)))],
        sink: Arc::new(NullSink),
        deps: vec![],
        tasks,
    }
}

// ---------------------------------------------------------------- 7a / 7b: I/O latency

#[derive(Clone, Copy, PartialEq, Debug)]
enum Arm {
    /// Control for "nothing is wrong": the CPU pool is idle (it only runs the consumer).
    Idle,
    /// The CPU pool is saturated by CPU-heavy tasks; channel big enough that a send never waits.
    CpuSat,
    /// KNOWN-BAD control: the same heavy work runs on the tokio runtime, 1 ms morsels, yielding between.
    TokioSat1ms,
    /// KNOWN-BAD control: heavy work on the tokio runtime, 10 ms morsels between yields.
    TokioSat10ms,
    /// CONTROL for the cause of a tail: the pool is idle but 2 plain OS threads (not the scheduler) spin,
    /// taking the same two cores as CpuSat's heavy tasks.
    OsSpin,
    /// CPU pool saturated, channel capacity 4: a send waits because nobody drains (backpressure).
    CpuSatBackpressure,
}

const ARMS: [Arm; 6] = [
    Arm::Idle,
    Arm::CpuSat,
    Arm::OsSpin,
    Arm::TokioSat1ms,
    Arm::TokioSat10ms,
    Arm::CpuSatBackpressure,
];

#[derive(Default)]
struct Samples {
    total: Vec<f64>,
    lag: Vec<f64>,
    send: Vec<f64>,
    consumed_before_stop: usize,
    unfinished_at_window_end: usize,
}

/// The "object store": a plain OS thread that delivers a chunk every `period` and timestamps the moment
/// it is ready (`t_ready`), then hands the timestamp to the tokio task through an unbounded channel,
/// like a network completion waking an I/O task. (The first version used tokio's own timer, whose 1 ms
/// tick put a ~1.1 ms floor under every arm and hid small differences.)
fn device(
    period: Duration,
    n: usize,
    offset: Duration,
    out: tokio::sync::mpsc::UnboundedSender<StdInstant>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let t0 = StdInstant::now() + Duration::from_millis(5) + offset;
        for i in 0..n {
            let target = t0 + period * (i as u32 + 1);
            while let Some(left) = target.checked_duration_since(StdInstant::now()) {
                if left.is_zero() {
                    break;
                }
                std::thread::sleep(left);
            }
            if out.send(StdInstant::now()).is_err() {
                return;
            }
        }
    })
}

async fn timed_producer(
    tx: Sender<Chunk>,
    base: u64,
    mut ready: tokio::sync::mpsc::UnboundedReceiver<StdInstant>,
) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let (mut total, mut lag, mut send) = (Vec::new(), Vec::new(), Vec::new());
    let mut i = 0u64;
    while let Some(t_ready) = ready.recv().await {
        let t_poll = StdInstant::now();
        let chunk = Chunk::bare(base + i);
        i += 1;
        assert!(tx.send(chunk).await.is_ok(), "receiver closed");
        let t_done = StdInstant::now();
        total.push((t_done - t_ready).as_secs_f64() * 1e6);
        lag.push((t_poll - t_ready).as_secs_f64() * 1e6);
        send.push((t_done - t_poll).as_secs_f64() * 1e6);
    }
    (total, lag, send)
}

fn run_latency(arm: Arm, producers: usize, n: usize) -> Samples {
    let period = Duration::from_millis(1);
    let rt = runtime(TOKIO);
    let cap = if arm == Arm::CpuSatBackpressure {
        4
    } else {
        producers * n
    };
    let (tx, rx) = channel::<Chunk>(cap);
    let got = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let mut pipes = Vec::new();
    if matches!(arm, Arm::CpuSat | Arm::CpuSatBackpressure) {
        pipes.push(heavy_pipeline(&stop, POOL * 2));
    }
    pipes.push(Pipeline {
        name: "consume".into(),
        source: Arc::new(ChanSources::new(vec![rx])),
        ops: vec![],
        sink: Arc::new(Collect {
            got: Arc::clone(&got),
            work: Work::None,
        }),
        deps: vec![],
        tasks: 1,
    });
    let rq = Query::new(pipes).start(POOL).unwrap();
    if matches!(arm, Arm::TokioSat1ms | Arm::TokioSat10ms) {
        let morsel = Duration::from_millis(if arm == Arm::TokioSat1ms { 1 } else { 10 });
        for _ in 0..TOKIO * 2 {
            let stop = Arc::clone(&stop);
            rt.spawn(async move {
                while !stop.load(SeqCst) {
                    spin(morsel);
                    tokio::task::yield_now().await;
                }
            });
        }
    }
    let os_spinners: Vec<_> = if arm == Arm::OsSpin {
        (0..POOL)
            .map(|_| {
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || {
                    while !stop.load(SeqCst) {
                        spin(Duration::from_micros(200));
                    }
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    std::thread::sleep(Duration::from_millis(30)); // let the saturation reach steady state
    let mut handles = Vec::new();
    let mut devices = Vec::new();
    for j in 0..producers {
        let tx = tx.clone();
        let base = (j as u64) << 20;
        let (dtx, drx) = tokio::sync::mpsc::unbounded_channel();
        devices.push(device(period, n, period * j as u32 / producers as u32, dtx));
        handles.push(rt.spawn(timed_producer(tx, base, drx)));
    }
    drop(tx);
    // Wait for the producers, but not forever: with capacity 4 and a starved consumer they cannot finish.
    let mut results: Vec<Option<_>> = Vec::new();
    let window = period * n as u32 + Duration::from_millis(400);
    let deadline = std::time::Instant::now() + window;
    let mut handles: Vec<_> = handles.into_iter().map(Some).collect();
    loop {
        for h in handles.iter_mut() {
            if h.as_ref().is_some_and(|h| h.is_finished()) {
                results.push(Some(rt.block_on(h.take().unwrap()).unwrap()));
            }
        }
        if handles.iter().all(Option::is_none) || std::time::Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let consumed_before_stop = lock(&got).len();
    let unfinished = handles.iter().filter(|h| h.is_some()).count();
    stop.store(true, SeqCst); // heavy tasks end; the consumer can drain
    for h in handles.into_iter().flatten() {
        results.push(Some(
            rt.block_on(async { tokio::time::timeout(Duration::from_secs(20), h).await })
                .expect("producer hung")
                .unwrap(),
        ));
    }
    rq.wait().expect("query failed");
    devices.into_iter().for_each(|d| d.join().unwrap());
    os_spinners.into_iter().for_each(|d| d.join().unwrap());
    rt.shutdown_timeout(Duration::from_secs(2));
    // Result check: every chunk arrived, in order per producer.
    let got = lock(&got).clone();
    assert_eq!(got.len(), producers * n, "{arm:?}: chunks lost");
    let mut next = vec![0u64; producers];
    for &(_, id) in &got {
        let (j, i) = ((id >> 20) as usize, id & 0xFFFFF);
        assert_eq!(i, next[j], "{arm:?}: producer {j} out of order");
        next[j] += 1;
    }
    let mut s = Samples {
        consumed_before_stop,
        unfinished_at_window_end: unfinished,
        ..Default::default()
    };
    for (t, l, d) in results.into_iter().flatten() {
        s.total.extend(t);
        s.lag.extend(l);
        s.send.extend(d);
    }
    s
}

struct ArmStats {
    p50: Vec<f64>,
    p99: Vec<f64>,
    max: Vec<f64>,
    pooled_total: Vec<f64>,
    pooled_lag: Vec<f64>,
    pooled_send: Vec<f64>,
    consumed: Vec<usize>,
    unfinished: Vec<usize>,
    loads: Vec<f64>,
}

fn range(v: &[f64]) -> String {
    let s = sorted(v.to_vec());
    format!("{:.0} / {:.0} / {:.0}", s[0], pct(&s, 0.5), s[s.len() - 1])
}

fn latency(args: &[String]) {
    let rounds: usize = arg(args, "--rounds").map_or(15, |v| v.parse().unwrap());
    let n: usize = arg(args, "--chunks").map_or(200, |v| v.parse().unwrap());
    let producers: usize = arg(args, "--producers").map_or(4, |v| v.parse().unwrap());
    let cores = std::thread::available_parallelism().map_or(8, |n| n.get());
    println!(
        "## latency: {rounds} rounds x {} arms interleaved, {producers} producers x {n} chunks, one chunk per producer per ms from an OS-thread \"store\", tokio={TOKIO} workers, cpu pool={POOL} threads, cores={cores}",
        ARMS.len()
    );
    println!("load average at start: {}", loadavg());
    // Check results before timing: one untimed run of every arm.
    for arm in ARMS {
        let s = run_latency(arm, producers, n.min(50));
        println!(
            "preflight {arm:?}: delivered {} chunks in order; consumed before stop: {}",
            s.total.len(),
            s.consumed_before_stop
        );
    }
    let mut stats: Vec<ArmStats> = ARMS
        .iter()
        .map(|_| ArmStats {
            p50: vec![],
            p99: vec![],
            max: vec![],
            pooled_total: vec![],
            pooled_lag: vec![],
            pooled_send: vec![],
            consumed: vec![],
            unfinished: vec![],
            loads: vec![],
        })
        .collect();
    for r in 0..rounds {
        for k in 0..ARMS.len() {
            let a = (k + r) % ARMS.len(); // rotate the starting arm so no arm always follows the same one
            let load = load1();
            let s = run_latency(ARMS[a], producers, n);
            let st = &mut stats[a];
            let tot = sorted(s.total.clone());
            st.p50.push(pct(&tot, 0.5));
            st.p99.push(pct(&tot, 0.99));
            st.max.push(tot[tot.len() - 1]);
            st.pooled_total.extend(s.total);
            st.pooled_lag.extend(s.lag);
            st.pooled_send.extend(s.send);
            st.consumed.push(s.consumed_before_stop);
            st.unfinished.push(s.unfinished_at_window_end);
            st.loads.push(load);
        }
    }
    println!("load average at end: {}", loadavg());
    println!(
        "\nLatency from \"chunk ready\" (the store thread timestamps the chunk as ready) to \"send completed\", microseconds."
    );
    println!(
        "Per-round p50 and p99 are shown as min / median / max over {rounds} rounds (the median-to-min/max spread is the run-to-run noise).\n"
    );
    println!(
        "| arm | p50 of rounds (min/med/max) | p99 of rounds (min/med/max) | worst sample of rounds (min/med/max) | pooled p50 | pooled p99 | pooled p99.9 | load1 before each run (min/med/max) |"
    );
    println!("|---|---|---|---|---:|---:|---:|---|");
    for (a, st) in stats.iter().enumerate() {
        let pooled = sorted(st.pooled_total.clone());
        let ld = sorted(st.loads.clone());
        println!(
            "| {:?} | {} | {} | {} | {:.0} | {:.0} | {:.0} | {:.1} / {:.1} / {:.1} |",
            ARMS[a],
            range(&st.p50),
            range(&st.p99),
            range(&st.max),
            pct(&pooled, 0.5),
            pct(&pooled, 0.99),
            pct(&pooled, 0.999),
            ld[0],
            pct(&ld, 0.5),
            ld[ld.len() - 1]
        );
    }
    println!(
        "\nSplit of the pooled samples (microseconds): `lag` = ready to the producer task being polled (the I/O task waited for a tokio thread), `send` = time inside send().await (lock + wake, or waiting for room).\n"
    );
    println!(
        "| arm | lag p50 | lag p99 | send p50 | send p99 | chunks consumed by the CPU pool before the window ended (min/med/max) | producers unfinished at window end (max) |"
    );
    println!("|---|---:|---:|---:|---:|---|---:|");
    for (a, st) in stats.iter().enumerate() {
        let lag = sorted(st.pooled_lag.clone());
        let snd = sorted(st.pooled_send.clone());
        let c = sorted(st.consumed.iter().map(|&x| x as f64).collect());
        println!(
            "| {:?} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} / {:.0} / {:.0} | {} |",
            ARMS[a],
            pct(&lag, 0.5),
            pct(&lag, 0.99),
            pct(&snd, 0.5),
            pct(&snd, 0.99),
            c[0],
            pct(&c, 0.5),
            c[c.len() - 1],
            st.unfinished.iter().max().unwrap()
        );
    }
}

// ---------------------------------------------------------------- 7c: memory bound

struct MemCase {
    producers: usize,
    consumers: usize, // channels = consumer tasks
    cap: usize,
    chunk_bytes: usize,
    chunks: usize, // per producer
    consume_ms: u64,
}

fn vm_hwm_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with("VmHWM"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Returns (peak_chunks, peak_bytes, created, vm_hwm_kb_delta).
fn run_memory(c: &MemCase) -> (usize, usize, usize, u64) {
    let tracker = Arc::new(Tracker::default());
    let rt = runtime(TOKIO);
    let mut rxs = Vec::new();
    let mut handles = Vec::new();
    let hwm0 = vm_hwm_kb();
    for k in 0..c.consumers {
        let (tx, rx) = channel::<Chunk>(c.cap);
        rxs.push(rx);
        for j in 0..c.producers {
            let tx = tx.clone();
            let tr = Arc::clone(&tracker);
            let (bytes, n) = (c.chunk_bytes, c.chunks);
            let base = ((k as u64) << 40) | ((j as u64) << 20);
            handles.push(rt.spawn(async move {
                // fast producer: no latency at all; it only waits when the channel is full
                for i in 0..n as u64 {
                    let chunk = Chunk::tracked(base + i, bytes, &tr);
                    if tx.send(chunk).await.is_err() {
                        break;
                    }
                }
            }));
        }
    }
    let got = Arc::new(Mutex::new(Vec::new()));
    let pipe = Pipeline {
        name: "slow-consume".into(),
        source: Arc::new(ChanSources::new(rxs)),
        ops: vec![],
        sink: Arc::new(Collect {
            got: Arc::clone(&got),
            work: Work::Sleep(Duration::from_millis(c.consume_ms)),
        }),
        deps: vec![],
        tasks: c.consumers,
    };
    Query::new(vec![pipe])
        .run(POOL.max(c.consumers))
        .expect("query failed");
    for h in handles {
        rt.block_on(h).unwrap();
    }
    let total = c.consumers * c.producers * c.chunks;
    assert_eq!(lock(&got).len(), total, "chunks lost");
    assert_eq!(tracker.created(), total);
    assert_eq!(tracker.live_chunks(), 0, "chunks leaked");
    rt.shutdown_timeout(Duration::from_secs(2));
    (
        tracker.peak_chunks(),
        tracker.peak_bytes(),
        tracker.created(),
        vm_hwm_kb().saturating_sub(hwm0),
    )
}

fn memory(args: &[String]) {
    if let Some(i) = args.iter().position(|a| a == "--one") {
        // child mode: one case in a fresh process so VmHWM is not polluted by earlier cases
        let v: Vec<usize> = args[i + 1].split(',').map(|x| x.parse().unwrap()).collect();
        let c = MemCase {
            producers: v[0],
            consumers: v[1],
            cap: v[2],
            chunk_bytes: v[3],
            chunks: v[4],
            consume_ms: v[5] as u64,
        };
        let (pc, pb, created, hwm) = run_memory(&c);
        println!("{pc} {pb} {created} {hwm}");
        return;
    }
    println!(
        "## memory bound: fast producers (no latency), slow consumer (1 ms per chunk), 1 MiB chunks, 240 chunks per producer"
    );
    println!("load average at start: {}", loadavg());
    println!(
        "Documented bound per channel: capacity N (queued) + P (one built chunk per producer waiting inside send) + 1 (the chunk the consumer task still holds). Total bound = consumers x (N + P + 1).\n"
    );
    println!(
        "| producers P | consumer tasks | capacity N | bound (chunks) | peak chunks | peak bytes (MiB) | bound (MiB) | within bound | process VmHWM growth (MiB) | load1 |"
    );
    println!("|---:|---:|---:|---:|---:|---:|---:|---|---:|---|");
    let mb = 1usize << 20;
    let exe = std::env::current_exe().unwrap();
    let mut all_ok = true;
    // (P, C, N); the last rows are the unbounded control (capacity 1,000,000)
    let cases = [
        (1, 1, 1),
        (1, 1, 4),
        (1, 1, 16),
        (4, 1, 1),
        (4, 1, 4),
        (4, 1, 16),
        (4, 2, 4),
        (8, 2, 16),
        (4, 1, 1_000_000),
    ];
    for (p, c, n) in cases {
        let chunks = 240;
        let load = load1();
        let out = std::process::Command::new(&exe)
            .args(["memory", "--one", &format!("{p},{c},{n},{mb},{chunks},1")])
            .output()
            .expect("child");
        assert!(
            out.status.success(),
            "child failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v: Vec<usize> = String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .map(|x| x.parse().unwrap())
            .collect();
        let bound = c * (n.min(usize::MAX / 4) + p + 1);
        let within = v[0] <= bound && v[1] <= bound * mb;
        let control = n >= 1_000_000;
        if !control {
            all_ok &= within;
        }
        println!(
            "| {p} | {c} | {} | {} | {} | {} | {} | {}{} | {:.0} | {load:.1} |",
            if control {
                "unbounded (1e6)".into()
            } else {
                n.to_string()
            },
            if control {
                "n/a".into()
            } else {
                bound.to_string()
            },
            v[0],
            v[1] / mb,
            if control {
                "n/a".into()
            } else {
                (bound).to_string()
            },
            if control {
                "n/a (control)"
            } else if within {
                "yes"
            } else {
                "NO"
            },
            if control {
                format!(", total chunks {}", p * c * chunks)
            } else {
                String::new()
            },
            v[3] as f64 / 1024.0,
        );
    }
    println!("\nbound respected in every bounded row: {all_ok}");
    println!("load average at end: {}", loadavg());
    if !all_ok {
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------- 7c reverse: idle workers

/// KNOWN-BAD control: when the channel is empty the source wakes itself at once instead of waiting for the producer.
struct BusyPollFactory(ChanSources);
impl SourceFactory for BusyPollFactory {
    fn make(&self) -> Box<dyn Source> {
        // ChanSources::make hands out a boxed `dyn Source`; wrap it with a self-waking shim.
        struct Shim(Box<dyn Source>);
        impl Source for Shim {
            fn next(&mut self, w: &Waker) -> Result<SourceResult> {
                match self.0.next(w)? {
                    SourceResult::Blocked => {
                        w.wake();
                        Ok(SourceResult::Blocked)
                    }
                    r => Ok(r),
                }
            }
            fn finish(&mut self) {
                self.0.finish();
            }
        }
        Box::new(Shim(self.0.make()))
    }
}

fn clk_tck() -> f64 {
    let o = std::process::Command::new("getconf")
        .arg("CLK_TCK")
        .output()
        .ok();
    o.and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(100.0)
}

/// (tid, comm, utime+stime in ticks) of every thread of this process.
fn threads() -> Vec<(u64, String, u64)> {
    let mut v = Vec::new();
    for e in std::fs::read_dir("/proc/self/task").unwrap().flatten() {
        let tid: u64 = e.file_name().to_string_lossy().parse().unwrap();
        if let Ok(s) = std::fs::read_to_string(e.path().join("stat")) {
            let (l, r) = (s.find('(').unwrap(), s.rfind(')').unwrap());
            let comm = s[l + 1..r].to_string();
            let f: Vec<&str> = s[r + 2..].split_whitespace().collect();
            let (ut, st): (u64, u64) = (f[11].parse().unwrap(), f[12].parse().unwrap());
            v.push((tid, comm, ut + st));
        }
    }
    v
}

/// Run slow producers (5 ms per chunk) into a fast consumer for `window`, then cancel; return the CPU
/// seconds used by the pool's worker threads and by the tokio threads during the window.
fn run_idle(busy_poll: bool, window: Duration, check_complete: bool) -> (f64, f64, f64) {
    let rt = runtime(TOKIO);
    // let the tokio threads exist before the baseline
    rt.block_on(async { tokio::time::sleep(Duration::from_millis(5)).await });
    let before: std::collections::HashSet<u64> = threads().into_iter().map(|t| t.0).collect();
    let (tx, rx) = channel::<Chunk>(4);
    let producers = 2;
    let n = if check_complete { 20 } else { 100_000 };
    let mut hs = Vec::new();
    for j in 0..producers {
        let tx = tx.clone();
        hs.push(rt.spawn(produce(
            tx,
            (j as u64) << 20,
            n,
            Latency::Sleep(Duration::from_millis(5)),
            1,
        )));
    }
    drop(tx);
    let got = Arc::new(Mutex::new(Vec::new()));
    let sources = ChanSources::new(vec![rx]);
    let source: Arc<dyn SourceFactory> = if busy_poll {
        Arc::new(BusyPollFactory(sources))
    } else {
        Arc::new(sources)
    };
    let pipe = Pipeline {
        name: "fast-consume".into(),
        source,
        ops: vec![],
        sink: Arc::new(Collect {
            got: Arc::clone(&got),
            work: Work::None,
        }),
        deps: vec![],
        tasks: 1,
    };
    let rq = Query::new(vec![pipe]).start(POOL).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let t0 = threads();
    let w0 = StdInstant::now();
    if check_complete {
        rq.wait().expect("query failed");
        for h in hs {
            rt.block_on(h).unwrap();
        }
        assert_eq!(lock(&got).len(), producers * n, "chunks lost");
        return (0.0, 0.0, 0.0);
    }
    std::thread::sleep(window);
    let wall = w0.elapsed().as_secs_f64();
    let t1 = threads();
    let tck = clk_tck();
    let ticks =
        |set: &[(u64, String, u64)], tid: u64| set.iter().find(|t| t.0 == tid).map_or(0, |t| t.2);
    let (mut pool, mut tokio_cpu) = (0.0, 0.0);
    let mut pool_threads = 0;
    for (tid, comm, _) in &t1 {
        let d = (ticks(&t1, *tid) - ticks(&t0, *tid)) as f64 / tck;
        if comm.starts_with("nata-io") {
            tokio_cpu += d;
        } else if !before.contains(tid) {
            pool += d;
            pool_threads += 1;
        }
    }
    assert_eq!(
        pool_threads, POOL,
        "expected {POOL} pool threads, saw {pool_threads}"
    );
    rq.cancel();
    let _ = rq.wait();
    rt.shutdown_timeout(Duration::from_secs(2));
    (pool, tokio_cpu, wall)
}

fn idle(args: &[String]) {
    let rounds: usize = arg(args, "--rounds").map_or(5, |v| v.parse().unwrap());
    let window = Duration::from_millis(1500);
    println!(
        "## idle CPU of the pool: 2 producers at 5 ms per chunk (400 chunks/s total), channel capacity 4, fast consumer; {POOL} pool threads, window {} ms, {rounds} rounds interleaved",
        window.as_millis()
    );
    println!("load average at start: {}", loadavg());
    for bp in [false, true] {
        run_idle(bp, window, true);
        println!("preflight busy_poll={bp}: all 40 chunks delivered");
    }
    let mut res: [Vec<(f64, f64, f64, f64)>; 2] = [vec![], vec![]];
    for r in 0..rounds {
        for k in 0..2 {
            let bp = (k + r) % 2 == 1;
            let load = load1();
            let (pool, tok, wall) = run_idle(bp, window, false);
            res[bp as usize].push((pool / wall, tok / wall, wall, load));
        }
    }
    println!(
        "\nCPU seconds used per wall second (1.0 = one core fully busy). Clock tick = 10 ms, so each reading is accurate to about +-0.01 CPU s over a window of ~{:.1} s.\n",
        window.as_secs_f64()
    );
    println!(
        "| design | pool workers' CPU (min / median / max), 2 threads | tokio threads' CPU (min / median / max) | load1 before each run (median) |"
    );
    println!("|---|---|---|---|");
    for (i, name) in [
        "real bridge: Blocked + waker stored, re-queued by the producer's send",
        "KNOWN-BAD control: source re-wakes itself while empty (busy re-queue)",
    ]
    .iter()
    .enumerate()
    {
        let p = sorted(res[i].iter().map(|x| x.0).collect());
        let t = sorted(res[i].iter().map(|x| x.1).collect());
        let l = sorted(res[i].iter().map(|x| x.3).collect());
        println!(
            "| {name} | {:.3} / {:.3} / {:.3} | {:.3} / {:.3} / {:.3} | {:.1} |",
            p[0],
            pct(&p, 0.5),
            p[p.len() - 1],
            t[0],
            pct(&t, 0.5),
            t[t.len() - 1],
            pct(&l, 0.5)
        );
    }
    println!("load average at end: {}", loadavg());
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("latency") => latency(&args),
        Some("memory") => memory(&args),
        Some("idle") => idle(&args),
        _ => eprintln!("usage: measure latency|memory|idle"),
    }
}
