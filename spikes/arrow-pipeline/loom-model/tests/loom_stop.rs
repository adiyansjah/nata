//! Model checks of the early-finish protocol of `spikes/arrow-pipeline`: the REAL scheduler (`src/proto/sched.rs`, with
//! its per-stage `stop_depth`), the REAL exchange (`src/exchange.rs`) and the REAL LIMIT (`src/limit.rs`), included by
//! path into this crate (`src/lib.rs`), explored with `loom`. Only the batch type is a stand-in (`src/batch.rs`).
//!
//! Run: `RUSTFLAGS="--cfg nata_loom" cargo test --release --test loom_stop -- --nocapture --test-threads=1`
//! (or `../gcp-check-loom.sh`, which also runs the unmutated control and every mutant).
//! Without `--cfg nata_loom` the same tests run as a plain stress loop on std threads (`LS_STRESS=<n>` repetitions each,
//! default 200): same fixtures, same assertions, no exploration. A deadlock (every thread blocked) is a failure under
//! loom: it is how a lost wake-up, or a stop that is not propagated, shows up. The scheduler's own loom-only
//! assertions are on too: a task starts only from `Queued` (never runs twice, never after `Done`), and when `wait()`
//! returns the queue is empty and every task is `Done`; loom itself fails an execution that ends with a live `Arc`.
//!
//! Properties, by test prefix (bounds and execution counts: bench/results/20261104-stop-depth-loom.md):
//!  (1) after a stage reached its limit it produces nothing more, the task does not run its upstream stages again nor
//!      read the source again, and every upstream stage across the exchanges is told to stop and terminates:
//!      s1 (single task, LIMIT behind an expanding operator), x1 (producer), x3 (two exchanges, three pipelines).
//!  (2) the per-stage rule: stopping stage i drops what is pending BEFORE i and nothing after it, and stops nothing
//!      that still has consumers: s2, s3 (a blocked sink holds rows the LIMIT admitted while a sibling stops the
//!      pipeline), s4 (two pipelines of one query, one stops, the other must deliver everything), x2 (an exchange with
//!      two consumer tasks is closed only by the last one).
//!  (3) no lost wake-up (s5: a sibling parked on a source that never wakes it must be woken by the stop), no task run
//!      twice (scheduler assertion), and nothing leaks at shutdown, also when `cancel()` races with the early finish
//!      (x4, x5): every morsel created is dropped (live-batch counter) and loom finds no leaked `Arc`.
//!  (4) the result has exactly min(limit, available) rows in every interleaving, no duplicate, every row from the
//!      source: every test; for one task end to end also the exact sequence.

use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicBool as StdBool, AtomicUsize as StdAtomic, Ordering::Relaxed};

use spike_ap_loom_model::batch::{Batch, Live};
use spike_ap_loom_model::exchange::exchange;
use spike_ap_loom_model::limit::Limit;
use spike_ap_loom_model::proto::sync::Ordering::SeqCst;
use spike_ap_loom_model::proto::sync::{Arc, AtomicUsize, Mutex};
use spike_ap_loom_model::proto::*;

// ---------------------------------------------------------------- driver

#[cfg(nata_loom)]
fn model(name: &str, default_bound: Option<usize>, f: impl Fn() + Sync + Send + 'static) {
    let runs = StdArc::new(StdAtomic::new(0));
    let r = StdArc::clone(&runs);
    let mut b = loom::model::Builder::new(); // reads LOOM_MAX_PREEMPTIONS etc. from the environment
    if b.preemption_bound.is_none() {
        b.preemption_bound = default_bound;
    }
    b.max_branches = 200_000;
    b.check(move || {
        r.fetch_add(1, Relaxed);
        f();
    });
    eprintln!("[loom] {name}: {} executions, preemption bound {:?}", runs.load(Relaxed), b.preemption_bound);
}

#[cfg(not(nata_loom))]
fn model(name: &str, _bound: Option<usize>, f: impl Fn() + Sync + Send + 'static) {
    let n: usize = std::env::var("LS_STRESS").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
    for _ in 0..n {
        f();
    }
    eprintln!("[stress] {name}: {n} runs with std threads");
}

// ---------------------------------------------------------------- configuration and coverage

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    /// One pipeline of `tasks` tasks: source -> ops -> collecting sink.
    Single,
    /// producer (`prod_tasks`) -> exchange -> consumer (`tasks`, with LIMIT) -> collecting sink.
    Chain2,
    /// producer -> exchange -> middle (1 task) -> exchange -> consumer (`tasks`, with LIMIT) -> collecting sink.
    Chain3,
    /// Two pipelines in ONE query, no dependency: A (1 task, LIMIT) and B (1 task, no LIMIT), each with its own sink.
    Independent,
}

#[derive(Clone, Copy, PartialEq)]
enum Ops {
    None,
    Limit,
    /// `[expand, LIMIT]`: the expanding operator emits one row per call (`HaveMoreOutput`), so an input is still
    /// pending upstream of the LIMIT when it finishes.
    ExpandLimit,
    /// `[pass, LIMIT, pass]`: the finishing stage is in the middle, so the sink slot is `finish_depth + 1`.
    PassLimitPass,
}

#[derive(Default, Clone, Copy)]
struct Expect {
    /// A sink pushed `Blocked` and later delivered (the rows survived the sibling's stop).
    gate_blocked: bool,
    /// A producer blocked on a full exchange was woken by the close and got `Finished`.
    blocked_then_finished: bool,
    /// A consumer task of a shared exchange ended while another consumer task of it was still live.
    cons_finished_while_other_live: bool,
}

#[derive(Clone, Copy)]
struct Cfg {
    shape: Shape,
    workers: usize,
    /// Tasks of the pipeline that holds the LIMIT.
    tasks: usize,
    prod_tasks: usize,
    morsels: usize,
    rows: usize,
    /// Morsels of pipeline B (Independent).
    morsels_b: usize,
    cap: usize,
    limit: Option<usize>,
    ops: Ops,
    /// The sink of the collecting pipeline returns `Blocked` until the first task of it combines (task end).
    gate: bool,
    /// The source stays `Blocked` (and never fires a waker) once its morsels are used up.
    quiet: bool,
    consumer_first: bool,
    cancel: bool,
    /// Index of the exchange whose producer side must have been told `Finished` (the stop crossed that exchange).
    stop_seen: Option<usize>,
    expect: Expect,
}

impl Cfg {
    fn base() -> Cfg {
        Cfg {
            shape: Shape::Single,
            workers: 2,
            tasks: 1,
            prod_tasks: 1,
            morsels: 3,
            rows: 1,
            morsels_b: 3,
            cap: 1,
            limit: None,
            ops: Ops::None,
            gate: false,
            quiet: false,
            consumer_first: false,
            cancel: false,
            stop_seen: None,
            expect: Expect::default(),
        }
    }
}

/// Counted over ALL executions (std atomics, outside the model), to show that the scenario a test is named after
/// really happened in some interleaving.
#[derive(Default)]
struct Cov {
    gate_blocked: StdAtomic,
    gate_delivered_after_block: StdAtomic,
    blocked_then_finished: StdAtomic,
    cons_finished_while_other_live: StdAtomic,
    executions: StdAtomic,
}

fn inc(a: &StdAtomic) {
    a.fetch_add(1, Relaxed);
}

fn check_coverage(name: &str, cov: &Cov, e: Expect) {
    eprintln!(
        "[cov]  {name}: executions {}, sink Blocked {}, delivered after Blocked {}, producer Blocked-then-Finished {}, a consumer ended while another was live {}",
        cov.executions.load(Relaxed),
        cov.gate_blocked.load(Relaxed),
        cov.gate_delivered_after_block.load(Relaxed),
        cov.blocked_then_finished.load(Relaxed),
        cov.cons_finished_while_other_live.load(Relaxed)
    );
    if cfg!(nata_loom) {
        assert!(!e.gate_blocked || cov.gate_blocked.load(Relaxed) > 0, "{name}: no execution had the sink Blocked");
        assert!(
            !e.gate_blocked || cov.gate_delivered_after_block.load(Relaxed) > 0,
            "{name}: no execution delivered rows after the sink was Blocked"
        );
        assert!(
            !e.blocked_then_finished || cov.blocked_then_finished.load(Relaxed) > 0,
            "{name}: no execution woke a blocked producer by closing the exchange"
        );
        assert!(
            !e.cons_finished_while_other_live || cov.cons_finished_while_other_live.load(Relaxed) > 0,
            "{name}: no execution had a consumer task end while another consumer task was live"
        );
    }
}

// ---------------------------------------------------------------- sources

struct MorselInner {
    total: usize,
    rows: usize,
    quiet: bool,
    next: AtomicUsize,
    claimed: StdAtomic,
    live: StdArc<Live>,
}

/// Morsel `m` holds the values `m * 100 + j`, `j < rows`; tasks claim morsels in order from one shared counter.
struct MorselFactory(StdArc<MorselInner>);

struct MorselSource(StdArc<MorselInner>);

impl SourceFactory for MorselFactory {
    fn make(&self) -> Box<dyn Source> {
        Box::new(MorselSource(StdArc::clone(&self.0)))
    }
}

impl Source for MorselSource {
    fn next(&mut self, _w: &Waker) -> Result<SourceResult> {
        let m = self.0.next.fetch_add(1, SeqCst);
        if m >= self.0.total {
            // A quiet source never ends and never fires the waker: only the stop can end the tasks parked on it.
            return Ok(if self.0.quiet { SourceResult::Blocked } else { SourceResult::Finished });
        }
        self.0.claimed.fetch_add(1, Relaxed);
        let vals = (0..self.0.rows).map(|j| (m * 100 + j) as i64).collect();
        Ok(SourceResult::Batch(Batch::new(vals, &self.0.live)))
    }
}

fn morsels(total: usize, rows: usize, quiet: bool, live: &StdArc<Live>) -> (MorselFactory, StdArc<MorselInner>) {
    let inner = StdArc::new(MorselInner {
        total,
        rows,
        quiet,
        next: AtomicUsize::new(0),
        claimed: StdAtomic::new(0),
        live: StdArc::clone(live),
    });
    (MorselFactory(StdArc::clone(&inner)), inner)
}

// ---------------------------------------------------------------- operators

/// Shared by the spy operators of one pipeline. The k-th `make()` of every operator factory belongs to task k:
/// `start_pipeline` makes all operators of task 0, then of task 1, and so on.
struct Spy {
    /// Some task's LIMIT has returned `Finished`.
    limit_done: StdBool,
    /// Task k's LIMIT returned `Finished` to task k.
    task_limit_done: Vec<StdBool>,
    limit_made: StdAtomic,
    expand_made: StdAtomic,
}

impl Spy {
    fn new(tasks: usize) -> StdArc<Spy> {
        StdArc::new(Spy {
            limit_done: StdBool::new(false),
            task_limit_done: (0..tasks).map(|_| StdBool::new(false)).collect(),
            limit_made: StdAtomic::new(0),
            expand_made: StdAtomic::new(0),
        })
    }
}

/// The REAL LIMIT, observed. Property (1): once it has returned `Finished` it produces no output; and the task that saw
/// it finish never runs it again.
struct SpyLimitFactory {
    inner: Limit,
    spy: StdArc<Spy>,
}

struct SpyLimitOp {
    inner: Box<dyn Operator>,
    spy: StdArc<Spy>,
    task: usize,
}

impl OperatorFactory for SpyLimitFactory {
    fn make(&self) -> Box<dyn Operator> {
        let task = self.spy.limit_made.fetch_add(1, Relaxed);
        Box::new(SpyLimitOp { inner: self.inner.make(), spy: StdArc::clone(&self.spy), task })
    }
}

impl Operator for SpyLimitOp {
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        assert!(
            !self.spy.task_limit_done[self.task].load(Relaxed),
            "the LIMIT stage ran again in the task that had already seen it finish"
        );
        let was_done = self.spy.limit_done.load(Relaxed);
        let r = self.inner.execute(input, out)?;
        if was_done {
            assert!(out.is_empty(), "the LIMIT produced output after it had reached its limit");
        }
        if matches!(r, OpResult::Finished) {
            self.spy.task_limit_done[self.task].store(true, Relaxed);
            self.spy.limit_done.store(true, Relaxed);
        }
        Ok(r)
    }
}

/// Emits ONE row per call and says `HaveMoreOutput` until the input is used up. Property (1): in the task whose LIMIT
/// has finished, this upstream stage never runs again (its pending input must have been dropped).
struct ExpandFactory {
    spy: StdArc<Spy>,
    live: StdArc<Live>,
}

struct ExpandOp {
    spy: StdArc<Spy>,
    live: StdArc<Live>,
    task: usize,
    pos: usize,
}

impl OperatorFactory for ExpandFactory {
    fn make(&self) -> Box<dyn Operator> {
        let task = self.spy.expand_made.fetch_add(1, Relaxed);
        Box::new(ExpandOp { spy: StdArc::clone(&self.spy), live: StdArc::clone(&self.live), task, pos: 0 })
    }
}

impl Operator for ExpandOp {
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        assert!(
            !self.spy.task_limit_done[self.task].load(Relaxed),
            "an upstream stage ran again in the task whose LIMIT had already finished (pending input not dropped)"
        );
        let vals = input.values();
        if vals.is_empty() {
            return Ok(OpResult::NeedMoreInput);
        }
        *out = Batch::new(vec![vals[self.pos]], &self.live);
        self.pos += 1;
        if self.pos == vals.len() {
            self.pos = 0;
            Ok(OpResult::NeedMoreInput)
        } else {
            Ok(OpResult::HaveMoreOutput)
        }
    }
}

struct PassFactory;

struct PassOp;

impl OperatorFactory for PassFactory {
    fn make(&self) -> Box<dyn Operator> {
        Box::new(PassOp)
    }
}

impl Operator for PassOp {
    fn execute(&mut self, input: &Batch, out: &mut Batch) -> Result<OpResult> {
        *out = input.clone();
        Ok(OpResult::NeedMoreInput)
    }
}

fn ops_for(cfg: &Cfg, spy: &StdArc<Spy>, live: &StdArc<Live>) -> Vec<StdArc<dyn OperatorFactory>> {
    let limit = || -> StdArc<dyn OperatorFactory> {
        StdArc::new(SpyLimitFactory { inner: Limit::new(cfg.limit.expect("a LIMIT value")), spy: StdArc::clone(spy) })
    };
    match cfg.ops {
        Ops::None => vec![],
        Ops::Limit => vec![limit()],
        Ops::ExpandLimit => {
            vec![StdArc::new(ExpandFactory { spy: StdArc::clone(spy), live: StdArc::clone(live) }), limit()]
        }
        Ops::PassLimitPass => vec![StdArc::new(PassFactory), limit(), StdArc::new(PassFactory)],
    }
}

// ---------------------------------------------------------------- sinks

type Out = Arc<Mutex<Vec<Vec<i64>>>>;

/// Backpressure downstream of the pipeline: pushes are `Blocked` (waker stored) until the first task of the pipeline
/// ends and combines. So a task whose LIMIT admitted rows is parked with them while its sibling stops the pipeline,
/// and it can only deliver them AFTER the stop is published.
struct Gate {
    st: Mutex<GateSt>,
}

struct GateSt {
    open: bool,
    wakers: Vec<Waker>,
}

impl Gate {
    fn new() -> Gate {
        Gate { st: Mutex::new(GateSt { open: false, wakers: Vec::new() }) }
    }

    fn open(&self) {
        let wakers = {
            let mut s = self.st.lock().unwrap();
            s.open = true;
            std::mem::take(&mut s.wakers)
        };
        for w in &wakers {
            w.wake();
        }
    }
}

struct CollectGlobal {
    out: Out,
    gate: Option<StdArc<Gate>>,
    cov: StdArc<Cov>,
}

struct CollectLocal {
    vals: Vec<i64>,
    out: Out,
    gate: Option<StdArc<Gate>>,
    cov: StdArc<Cov>,
    was_blocked: bool,
}

impl GlobalSink for CollectGlobal {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        Box::new(CollectLocal {
            vals: Vec::new(),
            out: Arc::clone(&self.out),
            gate: self.gate.clone(),
            cov: StdArc::clone(&self.cov),
            was_blocked: false,
        })
    }

    fn finalize(&self) -> Result<()> {
        Ok(())
    }
}

impl LocalSink for CollectLocal {
    fn push(&mut self, input: &mut Batch, waker: &Waker) -> Result<SinkResult> {
        if let Some(g) = &self.gate {
            let mut s = g.st.lock().unwrap();
            if !s.open {
                s.wakers.push(waker.clone());
                drop(s);
                inc(&self.cov.gate_blocked);
                self.was_blocked = true;
                return Ok(SinkResult::Blocked); // `input` untouched, as the protocol requires
            }
        }
        if self.was_blocked {
            inc(&self.cov.gate_delivered_after_block);
            self.was_blocked = false;
        }
        self.vals.extend(input.values());
        Ok(SinkResult::NeedMoreInput)
    }

    fn combine(self: Box<Self>) -> Result<()> {
        if let Some(g) = &self.gate {
            g.open();
        }
        self.out.lock().unwrap().push(self.vals);
        Ok(())
    }
}

// ---------------------------------------------------------------- exchange spies

/// What the spies saw at ONE exchange.
struct XCtl {
    /// Consumer tasks of the exchange that have not ended yet (starts at their number).
    live_consumers: StdAtomic,
    sink_blocked: StdAtomic,
    sink_finished: StdAtomic,
    blocked_then_finished: StdAtomic,
    cons_finished_while_other_live: StdAtomic,
}

impl XCtl {
    fn new(consumers: usize) -> StdArc<XCtl> {
        StdArc::new(XCtl {
            live_consumers: StdAtomic::new(consumers),
            sink_blocked: StdAtomic::new(0),
            sink_finished: StdAtomic::new(0),
            blocked_then_finished: StdAtomic::new(0),
            cons_finished_while_other_live: StdAtomic::new(0),
        })
    }
}

/// The REAL exchange sink, observed. Property (2): a producer is told `Finished` only when NO consumer task is live.
struct SpySink {
    inner: StdArc<dyn GlobalSink>,
    x: StdArc<XCtl>,
}

struct SpyLocal {
    inner: Box<dyn LocalSink>,
    x: StdArc<XCtl>,
    was_blocked: bool,
}

impl GlobalSink for SpySink {
    fn make(&self, task: usize) -> Box<dyn LocalSink> {
        Box::new(SpyLocal { inner: self.inner.make(task), x: StdArc::clone(&self.x), was_blocked: false })
    }

    fn finalize(&self) -> Result<()> {
        self.inner.finalize()
    }
}

impl LocalSink for SpyLocal {
    fn push(&mut self, input: &mut Batch, waker: &Waker) -> Result<SinkResult> {
        let r = self.inner.push(input, waker)?;
        match &r {
            SinkResult::Blocked => {
                inc(&self.x.sink_blocked);
                self.was_blocked = true;
            }
            SinkResult::Finished => {
                inc(&self.x.sink_finished);
                if self.was_blocked {
                    inc(&self.x.blocked_then_finished);
                }
                let live = self.x.live_consumers.load(Relaxed);
                assert_eq!(live, 0, "a producer was told Finished while {live} consumer task(s) of the exchange are still live");
            }
            SinkResult::NeedMoreInput => self.was_blocked = false,
        }
        Ok(r)
    }

    fn combine(self: Box<Self>) -> Result<()> {
        self.inner.combine()
    }
}

/// The REAL exchange source, observed: counts the consumer tasks that are still live (decremented BEFORE the real
/// `finish`, which is what may close the channel).
struct ProbeFactory {
    inner: StdArc<dyn SourceFactory>,
    x: StdArc<XCtl>,
}

struct ProbeSource {
    inner: Box<dyn Source>,
    x: StdArc<XCtl>,
}

impl SourceFactory for ProbeFactory {
    fn make(&self) -> Box<dyn Source> {
        Box::new(ProbeSource { inner: self.inner.make(), x: StdArc::clone(&self.x) })
    }
}

impl Source for ProbeSource {
    fn next(&mut self, w: &Waker) -> Result<SourceResult> {
        self.inner.next(w)
    }

    fn finish(&mut self) {
        let before = self.x.live_consumers.fetch_sub(1, Relaxed);
        assert!(before >= 1, "a consumer task ended twice");
        if before > 1 {
            inc(&self.x.cons_finished_while_other_live);
        }
        self.inner.finish();
    }
}

// ---------------------------------------------------------------- building a query

struct Handles {
    /// One per collecting sink, in pipeline order of the shape (A, then B for Independent).
    outs: Vec<Out>,
    xs: Vec<StdArc<XCtl>>,
    srcs: Vec<StdArc<MorselInner>>,
}

fn collect(gate: Option<StdArc<Gate>>, cov: &StdArc<Cov>) -> (StdArc<CollectGlobal>, Out) {
    let out: Out = Arc::new(Mutex::new(Vec::new()));
    (StdArc::new(CollectGlobal { out: Arc::clone(&out), gate, cov: StdArc::clone(cov) }), out)
}

fn build(cfg: &Cfg, live: &StdArc<Live>, cov: &StdArc<Cov>) -> (Query, Handles) {
    let gate = cfg.gate.then(|| StdArc::new(Gate::new()));
    let mut h = Handles { outs: vec![], xs: vec![], srcs: vec![] };
    let pipelines = match cfg.shape {
        Shape::Single => {
            let spy = Spy::new(cfg.tasks);
            let (src, inner) = morsels(cfg.morsels, cfg.rows, cfg.quiet, live);
            h.srcs.push(inner);
            let (sink, out) = collect(gate, cov);
            h.outs.push(out);
            vec![Pipeline {
                name: "single".into(),
                source: StdArc::new(src),
                ops: ops_for(cfg, &spy, live),
                sink,
                deps: vec![],
                tasks: cfg.tasks,
            }]
        }
        Shape::Chain2 | Shape::Chain3 => {
            let three = cfg.shape == Shape::Chain3;
            let mid_tasks = 1;
            let cons_tasks = cfg.tasks;
            // exchange 1: producer -> (middle | consumer)
            let (xs1, xsrc1, _c1) = exchange(cfg.cap, cfg.prod_tasks, if three { mid_tasks } else { cons_tasks });
            let x1 = XCtl::new(if three { mid_tasks } else { cons_tasks });
            h.xs.push(StdArc::clone(&x1));
            let (src, inner) = morsels(cfg.morsels, 1, false, live);
            h.srcs.push(inner);
            let producer = Pipeline {
                name: "producer".into(),
                source: StdArc::new(src),
                ops: vec![],
                sink: StdArc::new(SpySink { inner: xs1, x: x1.clone() }),
                deps: vec![],
                tasks: cfg.prod_tasks,
            };
            let mut ps = vec![producer];
            let mut feed: StdArc<dyn SourceFactory> = StdArc::new(ProbeFactory { inner: xsrc1, x: x1 });
            if three {
                let (xs2, xsrc2, _c2) = exchange(cfg.cap, mid_tasks, cons_tasks);
                let x2 = XCtl::new(cons_tasks);
                h.xs.push(StdArc::clone(&x2));
                ps.push(Pipeline {
                    name: "middle".into(),
                    source: feed,
                    ops: vec![],
                    sink: StdArc::new(SpySink { inner: xs2, x: x2.clone() }),
                    deps: vec![],
                    tasks: mid_tasks,
                });
                feed = StdArc::new(ProbeFactory { inner: xsrc2, x: x2 });
            }
            let spy = Spy::new(cons_tasks);
            let (sink, out) = collect(gate, cov);
            h.outs.push(out);
            ps.push(Pipeline {
                name: "consumer".into(),
                source: feed,
                ops: ops_for(cfg, &spy, live),
                sink,
                deps: vec![],
                tasks: cons_tasks,
            });
            if cfg.consumer_first {
                ps.reverse();
            }
            ps
        }
        Shape::Independent => {
            let spy = Spy::new(1);
            let (src_a, inner_a) = morsels(cfg.morsels, cfg.rows, false, live);
            let (src_b, inner_b) = morsels(cfg.morsels_b, cfg.rows, false, live);
            h.srcs.push(inner_a);
            h.srcs.push(inner_b);
            let (sink_a, out_a) = collect(None, cov);
            let (sink_b, out_b) = collect(None, cov);
            h.outs.push(out_a);
            h.outs.push(out_b);
            let a = Pipeline {
                name: "a-limited".into(),
                source: StdArc::new(src_a),
                ops: ops_for(cfg, &spy, live),
                sink: sink_a,
                deps: vec![],
                tasks: 1,
            };
            let b = Pipeline {
                name: "b-unlimited".into(),
                source: StdArc::new(src_b),
                ops: vec![],
                sink: sink_b,
                deps: vec![],
                tasks: 1,
            };
            if cfg.consumer_first { vec![b, a] } else { vec![a, b] }
        }
    };
    (Query::new(pipelines), h)
}

// ---------------------------------------------------------------- what a finished query must look like

fn flatten(out: &Out) -> Vec<Vec<i64>> {
    out.lock().unwrap().clone()
}

/// Every value comes from the source, none twice.
fn check_values(all: &[i64], morsels: usize, rows: usize, who: &str) {
    for &v in all {
        assert!(v >= 0 && (v as usize) / 100 < morsels && (v as usize) % 100 < rows, "{who}: value {v} is not from the source");
    }
    let mut s = all.to_vec();
    s.sort_unstable();
    assert!(s.windows(2).all(|w| w[0] != w[1]), "{who}: a row was delivered twice: {all:?}");
}

fn check_results(cfg: &Cfg, h: &Handles) {
    match cfg.shape {
        Shape::Independent => {
            let a: Vec<i64> = flatten(&h.outs[0]).into_iter().flatten().collect();
            let b: Vec<i64> = flatten(&h.outs[1]).into_iter().flatten().collect();
            let avail_a = cfg.morsels * cfg.rows;
            let want_a = cfg.limit.map_or(avail_a, |k| k.min(avail_a));
            assert_eq!(a.len(), want_a, "pipeline A (LIMIT): wrong row count: {a:?}");
            check_values(&a, cfg.morsels, cfg.rows, "A");
            assert_eq!(b.len(), cfg.morsels_b * cfg.rows, "pipeline B stopped by A's LIMIT, or lost rows: {b:?}");
            check_values(&b, cfg.morsels_b, cfg.rows, "B");
        }
        _ => {
            let per_task = flatten(&h.outs[0]);
            let all: Vec<i64> = per_task.iter().flatten().copied().collect();
            let rows = if cfg.shape == Shape::Single { cfg.rows } else { 1 };
            let avail = cfg.morsels * rows;
            let want = cfg.limit.map_or(avail, |k| k.min(avail));
            assert_eq!(all.len(), want, "wrong number of rows (want min(limit, available) = {want}): {all:?}");
            check_values(&all, cfg.morsels, rows, "result");
            let one_path = cfg.tasks == 1 && (cfg.shape == Shape::Single || cfg.prod_tasks == 1);
            if one_path {
                let expect: Vec<i64> = (0..want).map(|i| ((i / rows) * 100 + i % rows) as i64).collect();
                assert_eq!(all, expect, "one task end to end: rows lost, reordered or from the wrong morsel");
                if cfg.shape == Shape::Single && cfg.limit.is_some() {
                    // property (1): nothing is read after the LIMIT finished
                    let claimed = h.srcs[0].claimed.load(Relaxed);
                    assert_eq!(claimed, want.div_ceil(rows), "source read after the LIMIT had finished (or too little)");
                }
            }
        }
    }
    if let Some(i) = cfg.stop_seen {
        assert!(
            h.xs[i].sink_finished.load(Relaxed) >= 1,
            "the stop did not cross exchange {i}: its producer was never told Finished"
        );
    }
}

fn drive(name: &'static str, bound: Option<usize>, cfg: Cfg) {
    let cov = StdArc::new(Cov::default());
    let c = StdArc::clone(&cov);
    model(name, bound, move || {
        let live = StdArc::new(Live::default());
        let (query, h) = build(&cfg, &live, &c);
        if cfg.cancel {
            let running = query.start(cfg.workers).expect("start");
            running.cancel();
            let r = running.wait();
            assert_eq!(r, Err(QueryError("cancelled".into())), "cancel must end the query");
        } else {
            query.run(cfg.workers).expect("query failed");
            check_results(&cfg, &h);
        }
        inc(&c.executions);
        for x in &h.xs {
            c.blocked_then_finished.fetch_add(x.blocked_then_finished.load(Relaxed), Relaxed);
            c.cons_finished_while_other_live.fetch_add(x.cons_finished_while_other_live.load(Relaxed), Relaxed);
        }
        drop(h);
        // Property (3): nothing is left behind. A leaked channel (a stale waker closes a reference cycle through the
        // task, the scheduler and the channel) keeps its queued morsels alive; loom also fails an execution that ends
        // with a live `Arc`.
        assert_eq!(live.get(), 0, "morsels still alive after the query ended and every handle was dropped");
    });
    check_coverage(name, &cov, cfg.expect);
}

// ================================================================ (1) the stage that finished, and what is upstream of it

/// One task, `[expand, LIMIT 2]`, 2 morsels of 3 rows: the LIMIT finishes on the second row of morsel 0, while the
/// expanding operator still has one row of that morsel pending. That pending input must be dropped (the expanding
/// stage never runs again), the source must not be read again, and the result is exactly rows 0 and 1.
#[test]
fn s1_one_task_limit_drops_upstream_pending_and_stops_source() {
    let cfg = Cfg { workers: 1, morsels: 2, rows: 3, ops: Ops::ExpandLimit, limit: Some(2), ..Cfg::base() };
    drive("s1_one_task_limit_drops_upstream_pending_and_stops_source", Some(3), cfg);
}

/// The same with two tasks sharing the LIMIT (k = 3 of 4 rows): exactly 3 rows, no duplicate, in every interleaving.
#[test]
fn s1b_two_tasks_shared_limit_count_exact() {
    let cfg = Cfg { morsels: 2, rows: 2, tasks: 2, ops: Ops::ExpandLimit, limit: Some(3), ..Cfg::base() };
    drive("s1b_two_tasks_shared_limit_count_exact", Some(2), cfg);
}

// ================================================================ (2) the per-stage rule

/// LIMIT 1 behind a sink that is Blocked until some task ends. One task admits the row and parks on the sink holding
/// it; the other finds the LIMIT exhausted and stops the pipeline. The parked task must still deliver the row: the
/// stop drops what is pending BEFORE the finishing stage (`stop_depth`), not what is after it.
#[test]
fn s2_sibling_stop_keeps_rows_the_limit_admitted() {
    let cfg = Cfg { morsels: 2, tasks: 2, ops: Ops::Limit, limit: Some(1), gate: true, expect: Expect { gate_blocked: true, ..Default::default() }, ..Cfg::base() };
    drive("s2_sibling_stop_keeps_rows_the_limit_admitted", Some(3), cfg);
}

#[test]
fn s2_sibling_stop_keeps_rows_the_limit_admitted_one_worker() {
    let cfg = Cfg { workers: 1, morsels: 2, tasks: 2, ops: Ops::Limit, limit: Some(1), gate: true, expect: Expect { gate_blocked: true, ..Default::default() }, ..Cfg::base() };
    drive("s2_sibling_stop_keeps_rows_the_limit_admitted_one_worker", Some(3), cfg);
}

/// The finishing stage is in the middle (`[pass, LIMIT, pass]`): the stop depth is 2 and the sink slot is 3.
#[test]
fn s3_stop_depth_with_stages_on_both_sides_of_the_limit() {
    let cfg = Cfg { morsels: 2, tasks: 2, ops: Ops::PassLimitPass, limit: Some(1), gate: true, expect: Expect { gate_blocked: true, ..Default::default() }, ..Cfg::base() };
    drive("s3_stop_depth_with_stages_on_both_sides_of_the_limit", Some(3), cfg);
}

/// Two pipelines of ONE query; A has a LIMIT and stops, B has none and must deliver all 3 morsels: a stop belongs to
/// its pipeline.
#[test]
fn s4_stop_of_one_pipeline_does_not_stop_the_other() {
    let cfg = Cfg { shape: Shape::Independent, morsels: 2, morsels_b: 3, ops: Ops::Limit, limit: Some(1), ..Cfg::base() };
    drive("s4_stop_of_one_pipeline_does_not_stop_the_other", Some(3), cfg);
}

#[test]
fn s4_stop_of_one_pipeline_does_not_stop_the_other_b_queued_first() {
    let cfg = Cfg { shape: Shape::Independent, morsels: 2, morsels_b: 3, ops: Ops::Limit, limit: Some(1), consumer_first: true, ..Cfg::base() };
    drive("s4_stop_of_one_pipeline_does_not_stop_the_other_b_queued_first", Some(3), cfg);
}

/// An exchange with two consumer tasks (they share the LIMIT): the producer may be told `Finished` only when BOTH have
/// ended; closing it when the first one ends would cut off a consumer that still has a use for it.
#[test]
fn x2_shared_exchange_is_closed_only_by_the_last_consumer() {
    let cfg = Cfg {
        shape: Shape::Chain2,
        tasks: 2,
        ops: Ops::Limit,
        limit: Some(1),
        expect: Expect { cons_finished_while_other_live: true, ..Default::default() },
        ..Cfg::base()
    };
    drive("x2_shared_exchange_is_closed_only_by_the_last_consumer", Some(2), cfg);
}

// ================================================================ (3) no lost wake-up

/// The source has gone quiet (one morsel, then `Blocked` for ever, no waker). LIMIT 1 is satisfied by one task; the
/// sibling is parked on the source and only the stop can wake it (`wake_pipeline`), or the pipeline never ends.
#[test]
fn s5_task_parked_on_a_quiet_source_is_woken_by_the_stop() {
    let cfg = Cfg { morsels: 1, tasks: 2, ops: Ops::Limit, limit: Some(1), quiet: true, ..Cfg::base() };
    drive("s5_task_parked_on_a_quiet_source_is_woken_by_the_stop", Some(3), cfg);
}

#[test]
fn s5_task_parked_on_a_quiet_source_is_woken_by_the_stop_one_worker() {
    let cfg = Cfg { workers: 1, morsels: 1, tasks: 2, ops: Ops::Limit, limit: Some(1), quiet: true, ..Cfg::base() };
    drive("s5_task_parked_on_a_quiet_source_is_woken_by_the_stop_one_worker", Some(3), cfg);
}

// ================================================================ (1) the stop crosses exchanges

/// producer -> exchange (cap 1) -> consumer with LIMIT 1, 3 morsels: the producer cannot finish by itself (the
/// consumer takes one), so it must be told `Finished`; it is blocked on the full exchange when the consumer closes
/// it, and the close must wake it.
#[test]
fn x1_consumer_limit_stops_the_producer_producer_queued_first() {
    let cfg = Cfg {
        shape: Shape::Chain2,
        ops: Ops::Limit,
        limit: Some(1),
        stop_seen: Some(0),
        expect: Expect { blocked_then_finished: true, ..Default::default() },
        ..Cfg::base()
    };
    drive("x1_consumer_limit_stops_the_producer_producer_queued_first", Some(3), cfg);
}

#[test]
fn x1_consumer_limit_stops_the_producer_consumer_queued_first() {
    let cfg = Cfg {
        shape: Shape::Chain2,
        ops: Ops::Limit,
        limit: Some(1),
        consumer_first: true,
        stop_seen: Some(0),
        expect: Expect { blocked_then_finished: true, ..Default::default() },
        ..Cfg::base()
    };
    drive("x1_consumer_limit_stops_the_producer_consumer_queued_first", Some(3), cfg);
}

/// producer -> X1 -> middle -> X2 -> consumer with LIMIT 1: the consumer closes X2, the middle stage (blocked on it, or
/// pushing into it) is told `Finished`, ends, and its own `finish` closes X1 for the producer. Three pipelines, no hang.
#[test]
fn x3_stop_crosses_two_exchanges() {
    let cfg = Cfg { shape: Shape::Chain3, ops: Ops::Limit, limit: Some(1), stop_seen: Some(1), ..Cfg::base() };
    drive("x3_stop_crosses_two_exchanges", Some(2), cfg);
}

#[test]
fn x3_stop_crosses_two_exchanges_one_worker() {
    let cfg = Cfg { shape: Shape::Chain3, workers: 1, ops: Ops::Limit, limit: Some(1), stop_seen: Some(1), ..Cfg::base() };
    drive("x3_stop_crosses_two_exchanges_one_worker", Some(3), cfg);
}

// ================================================================ (3) cancel racing with the early finish, shutdown leaks

/// `cancel()` right after `start`, racing with the LIMIT's early close: the query ends with "cancelled", every task is
/// `Done`, the queue is empty (scheduler assertions), the channel keeps no waker (loom: no leaked `Arc`) and every
/// queued morsel is released (live-batch counter).
#[test]
fn x4_cancel_racing_with_the_early_close() {
    let cfg = Cfg { shape: Shape::Chain2, ops: Ops::Limit, limit: Some(1), cancel: true, consumer_first: true, ..Cfg::base() };
    drive("x4_cancel_racing_with_the_early_close", Some(3), cfg);
}

#[test]
fn x5_cancel_three_pipelines_one_worker() {
    let cfg = Cfg { shape: Shape::Chain3, workers: 1, ops: Ops::Limit, limit: Some(1), cancel: true, ..Cfg::base() };
    drive("x5_cancel_three_pipelines_one_worker", Some(3), cfg);
}
