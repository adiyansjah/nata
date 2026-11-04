//! Model checks of the REAL exchange (`src/exchange.rs`, verbatim from the prototype's `ops.rs`)
//! running under the REAL scheduler (`src/sched.rs`), with `loom`.
//!
//! Run: `RUSTFLAGS="--cfg nata_loom" CARGO_TARGET_DIR=/tmp/nata-exl-target cargo test --release --test loom_exchange -- --nocapture --test-threads=1`
//! (see `bench/results/20261104-exchange-loom.md` for bounds, execution counts, the mutation table).
//! Without `--cfg nata_loom` the same tests run as a plain std-thread stress loop (`EXL_STRESS=<n>`
//! repetitions each, default 200): same fixtures, same assertions, no exploration.
//!
//! Shape of every test: one producer pipeline (`P` tasks: source -> exchange sink) and one consumer
//! pipeline (`C` tasks: exchange source -> [LIMIT] -> collecting sink), capacity 1, 1 or 2 worker
//! threads, 0-3 items per producer. Which pipeline is index 0 decides which task the scheduler
//! queues first (`consumer_first`). The model checker explores every thread interleaving up to a
//! preemption bound (`LOOM_MAX_PREEMPTIONS` overrides the default of each test). A deadlock (all
//! threads blocked) is a failure: it is how a lost wake-up shows up.
//!
//! Properties checked, by test prefix: a = no lost wake-up in the full / empty handshakes,
//! b = close races (producer finishes while the consumer is blocked; the consumer closes early
//! while a producer is blocked on a full exchange), c = cancel racing with those, d = the bound,
//! e = no item lost, duplicated or reordered within one producer, f = nothing leaks after a cancel.
//! (tests/loom_channel.rs: scheduler-free models aimed at single exchange operations.)

use std::sync::Arc as StdArc;
use std::sync::atomic::{AtomicUsize as StdAtomic, Ordering::Relaxed};

use spike_exchange_loom::sync::Ordering::SeqCst;
use spike_exchange_loom::sync::{Arc, AtomicUsize, Mutex};
use spike_exchange_loom::*;

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
    eprintln!(
        "[loom] {name}: {} executions, preemption bound {:?}",
        runs.load(Relaxed),
        b.preemption_bound
    );
}

#[cfg(not(nata_loom))]
fn model(name: &str, _bound: Option<usize>, f: impl Fn() + Sync + Send + 'static) {
    let n: usize = std::env::var("EXL_STRESS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);
    for _ in 0..n {
        f();
    }
    eprintln!("[stress] {name}: {n} runs with std threads");
}

// ---------------------------------------------------------------- configuration and coverage

#[derive(Clone, Copy)]
struct Cfg {
    workers: usize,
    cap: usize,
    /// Items emitted by EACH producer task.
    items: usize,
    prod_tasks: usize,
    cons_tasks: usize,
    limit: Option<usize>,
    /// Pipeline 0 is the consumer: its tasks are queued first.
    consumer_first: bool,
    /// The consumer never reads the exchange (and never registers a waker).
    stuck_consumer: bool,
    /// Check queue length <= capacity after every push and the end-to-end flow bound.
    check_bound: bool,
}

impl Cfg {
    fn base() -> Cfg {
        Cfg {
            workers: 2,
            cap: 1,
            items: 2,
            prod_tasks: 1,
            cons_tasks: 1,
            limit: None,
            consumer_first: false,
            stuck_consumer: false,
            check_bound: false,
        }
    }
}

/// Counted over ALL executions (std atomics, outside the model), to show that the scenario a test
/// is named after really happened in some interleaving.
#[derive(Default)]
struct Cov {
    prod_blocked: StdAtomic,
    prod_finished: StdAtomic,
    /// A producer that was `Blocked` on a full exchange later got `Finished` (woken by the close).
    blocked_then_finished: StdAtomic,
    cons_blocked: StdAtomic,
    /// Wakers still stored in the channel after a cancelled query ended (summed over executions).
    stale_wakers: StdAtomic,
}

fn inc(a: &StdAtomic) {
    a.fetch_add(1, Relaxed);
}

/// What a test must have seen in at least one execution (enforced under loom only).
#[derive(Default, Clone, Copy)]
struct Expect {
    prod_blocked: bool,
    cons_blocked: bool,
    blocked_then_finished: bool,
}

fn check_coverage(name: &str, cov: &Cov, e: Expect) {
    eprintln!(
        "[cov]  {name}: producer Blocked {}, producer Finished {}, producer Blocked-then-Finished {}, consumer Blocked {}, stale wakers after cancel {}",
        cov.prod_blocked.load(Relaxed),
        cov.prod_finished.load(Relaxed),
        cov.blocked_then_finished.load(Relaxed),
        cov.cons_blocked.load(Relaxed),
        cov.stale_wakers.load(Relaxed)
    );
    if cfg!(nata_loom) {
        assert!(
            !e.prod_blocked || cov.prod_blocked.load(Relaxed) > 0,
            "{name}: no execution had a producer blocked on a full exchange"
        );
        assert!(
            !e.cons_blocked || cov.cons_blocked.load(Relaxed) > 0,
            "{name}: no execution had a consumer blocked on an empty exchange"
        );
        assert!(
            !e.blocked_then_finished || cov.blocked_then_finished.load(Relaxed) > 0,
            "{name}: no execution woke a blocked producer by closing the exchange"
        );
    }
}

// ---------------------------------------------------------------- fixtures

/// Values are `producer_id * 100 + sequence number`, so order within one producer is checkable.
fn value(id: usize, seq: usize) -> i64 {
    (id * 100 + seq) as i64
}

struct Flow {
    handed: Arc<AtomicUsize>,
    collected: Arc<AtomicUsize>,
    slack: usize,
}

struct GenFactory {
    items: usize,
    next_id: AtomicUsize,
    flow: Option<StdArc<Flow>>,
}

impl SourceFactory for GenFactory {
    fn make(&self) -> Box<dyn Source> {
        Box::new(GenSource {
            id: self.next_id.fetch_add(1, SeqCst),
            seq: 0,
            items: self.items,
            flow: self.flow.clone(),
        })
    }
}

struct GenSource {
    id: usize,
    seq: usize,
    items: usize,
    flow: Option<StdArc<Flow>>,
}

impl Source for GenSource {
    fn next(&mut self, _w: &Waker) -> Result<SourceResult> {
        if self.seq >= self.items {
            return Ok(SourceResult::Finished);
        }
        let v = value(self.id, self.seq);
        self.seq += 1;
        if let Some(f) = &self.flow {
            let handed = f.handed.fetch_add(1, SeqCst) + 1;
            let collected = f.collected.load(SeqCst);
            // queue <= cap, plus one batch held by each blocked producer and each consumer task.
            assert!(
                handed <= collected + f.slack,
                "flow bound violated: {} items in flight, allowed {}",
                handed - collected,
                f.slack
            );
        }
        Ok(SourceResult::Batch(Batch::from_cols(vec![vec![v]])))
    }
}

/// The real exchange sink, observed.
struct SpySink {
    inner: StdArc<dyn GlobalSink>,
    chan: StdArc<Channel>,
    cov: StdArc<Cov>,
    check_bound: bool,
}

struct SpyLocal {
    inner: Box<dyn LocalSink>,
    chan: StdArc<Channel>,
    cov: StdArc<Cov>,
    check_bound: bool,
    was_blocked: bool,
}

impl GlobalSink for SpySink {
    fn make(&self, task: usize) -> Box<dyn LocalSink> {
        Box::new(SpyLocal {
            inner: self.inner.make(task),
            chan: StdArc::clone(&self.chan),
            cov: StdArc::clone(&self.cov),
            check_bound: self.check_bound,
            was_blocked: false,
        })
    }

    fn finalize(&self) -> Result<()> {
        self.inner.finalize()
    }
}

impl LocalSink for SpyLocal {
    fn push(&mut self, input: &mut Batch, waker: &Waker) -> Result<SinkResult> {
        let r = self.inner.push(input, waker)?;
        match r {
            SinkResult::Blocked => {
                inc(&self.cov.prod_blocked);
                self.was_blocked = true;
            }
            SinkResult::Finished => {
                inc(&self.cov.prod_finished);
                if self.was_blocked {
                    inc(&self.cov.blocked_then_finished);
                }
            }
            SinkResult::NeedMoreInput => self.was_blocked = false,
        }
        if self.check_bound {
            let len = self.chan.queue_len();
            assert!(
                len <= self.chan.capacity(),
                "bound violated: {len} batches queued, capacity {}",
                self.chan.capacity()
            );
        }
        Ok(r)
    }

    fn combine(self: Box<Self>) -> Result<()> {
        self.inner.combine()
    }
}

/// The real exchange source, observed; optionally replaced by a consumer that never reads.
struct ProbeFactory {
    inner: StdArc<dyn SourceFactory>,
    cov: StdArc<Cov>,
    stuck: bool,
}

struct ProbeSource {
    inner: Box<dyn Source>,
    cov: StdArc<Cov>,
    stuck: bool,
}

impl SourceFactory for ProbeFactory {
    fn make(&self) -> Box<dyn Source> {
        Box::new(ProbeSource {
            inner: self.inner.make(),
            cov: StdArc::clone(&self.cov),
            stuck: self.stuck,
        })
    }
}

impl Source for ProbeSource {
    fn next(&mut self, w: &Waker) -> Result<SourceResult> {
        if self.stuck {
            inc(&self.cov.cons_blocked);
            return Ok(SourceResult::Blocked); // no waker stored: only cancel can end this task
        }
        let r = self.inner.next(w)?;
        if matches!(r, SourceResult::Blocked) {
            inc(&self.cov.cons_blocked);
        }
        Ok(r)
    }

    fn finish(&mut self) {
        self.inner.finish();
    }
}

/// Consumer sink: every task records its values in arrival order and publishes them in `combine`.
struct CollectGlobal {
    out: Arc<Mutex<Vec<Vec<i64>>>>,
    collected: Arc<AtomicUsize>,
}

struct CollectLocal {
    vals: Vec<i64>,
    out: Arc<Mutex<Vec<Vec<i64>>>>,
    collected: Arc<AtomicUsize>,
}

impl GlobalSink for CollectGlobal {
    fn make(&self, _task: usize) -> Box<dyn LocalSink> {
        Box::new(CollectLocal {
            vals: Vec::new(),
            out: Arc::clone(&self.out),
            collected: Arc::clone(&self.collected),
        })
    }

    fn finalize(&self) -> Result<()> {
        Ok(())
    }
}

impl LocalSink for CollectLocal {
    fn push(&mut self, input: &mut Batch, _w: &Waker) -> Result<SinkResult> {
        for r in input.rows() {
            self.vals.push(r[0]);
        }
        self.collected.fetch_add(input.len(), SeqCst);
        Ok(SinkResult::NeedMoreInput)
    }

    fn combine(self: Box<Self>) -> Result<()> {
        self.out.lock().unwrap().push(self.vals);
        Ok(())
    }
}

struct Built {
    query: Query,
    out: Arc<Mutex<Vec<Vec<i64>>>>,
    chan: StdArc<Channel>,
}

fn build(cfg: Cfg, cov: &StdArc<Cov>) -> Built {
    let (xsink, xsource, chan) = exchange(cfg.cap, cfg.prod_tasks, cfg.cons_tasks);
    let collected = Arc::new(AtomicUsize::new(0));
    let out = Arc::new(Mutex::new(Vec::new()));
    let flow = (cfg.check_bound && cfg.limit.is_none()).then(|| {
        StdArc::new(Flow {
            handed: Arc::new(AtomicUsize::new(0)),
            collected: Arc::clone(&collected),
            slack: cfg.cap + cfg.prod_tasks + cfg.cons_tasks,
        })
    });
    let producer = Pipeline {
        name: "producer".into(),
        source: StdArc::new(GenFactory {
            items: cfg.items,
            next_id: AtomicUsize::new(0),
            flow,
        }),
        ops: vec![],
        sink: StdArc::new(SpySink {
            inner: xsink,
            chan: StdArc::clone(&chan),
            cov: StdArc::clone(cov),
            check_bound: cfg.check_bound,
        }),
        deps: vec![],
        tasks: cfg.prod_tasks,
    };
    let ops: Vec<StdArc<dyn OperatorFactory>> = cfg
        .limit
        .map(|n| StdArc::new(Limit::new(n)) as StdArc<dyn OperatorFactory>)
        .into_iter()
        .collect();
    let consumer = Pipeline {
        name: "consumer".into(),
        source: StdArc::new(ProbeFactory {
            inner: xsource,
            cov: StdArc::clone(cov),
            stuck: cfg.stuck_consumer,
        }),
        ops,
        sink: StdArc::new(CollectGlobal {
            out: Arc::clone(&out),
            collected,
        }),
        deps: vec![],
        tasks: cfg.cons_tasks,
    };
    let pipelines = if cfg.consumer_first {
        vec![consumer, producer]
    } else {
        vec![producer, consumer]
    };
    Built {
        query: Query::new(pipelines),
        out,
        chan,
    }
}

// ---------------------------------------------------------------- what a finished query must look like

/// The consumer received exactly what the producers sent: nothing lost, duplicated or (within one
/// producer) reordered. With a `LIMIT n`: exactly `n` rows (or all, if fewer exist), and for each
/// producer a gap-free prefix of its sequence.
fn check_stream(out: &[Vec<i64>], cfg: &Cfg) {
    assert_eq!(out.len(), cfg.cons_tasks, "every consumer task combines once");
    let all: Vec<i64> = out.iter().flatten().copied().collect();
    let total = cfg.items * cfg.prod_tasks;
    let want = cfg.limit.map_or(total, |n| n.min(total));
    assert_eq!(all.len(), want, "wrong number of items received: {all:?}");
    for id in 0..cfg.prod_tasks {
        // Order of arrival within ONE consumer task must follow the producer's order. Across
        // consumer tasks only the set is defined, so check gap-free prefix on the sorted set.
        let mut seqs: Vec<usize> = all
            .iter()
            .filter(|v| (**v as usize) / 100 == id)
            .map(|v| (*v as usize) % 100)
            .collect();
        if cfg.cons_tasks == 1 {
            let expect: Vec<usize> = (0..seqs.len()).collect();
            assert_eq!(seqs, expect, "producer {id}: items lost, duplicated or reordered: {all:?}");
        } else {
            seqs.sort_unstable();
            let expect: Vec<usize> = (0..seqs.len()).collect();
            assert_eq!(seqs, expect, "producer {id}: items lost or duplicated: {all:?}");
        }
        if cfg.limit.is_none() {
            assert_eq!(seqs.len(), cfg.items, "producer {id}: not every item arrived: {all:?}");
        }
    }
}

enum Mode {
    /// Run to completion; the result must be Ok and the stream exact.
    Plain,
    /// `cancel()` right after `start`, from the main thread; the query must still terminate with
    /// "cancelled", every task `Done` and the queue empty (asserted inside `wait`, loom builds),
    /// and the channel stores no waker any more (a stored waker is a reference cycle: finding F1;
    /// loom also fails an execution that ends with a live `Arc`).
    Cancel,
}

fn drive(name: &'static str, bound: Option<usize>, cfg: Cfg, mode: Mode, expect: Expect) {
    let cov = StdArc::new(Cov::default());
    let c = StdArc::clone(&cov);
    let cancel = !matches!(mode, Mode::Plain);
    model(name, bound, move || {
        let b = build(cfg, &c);
        if cancel {
            let running = b.query.start(cfg.workers).expect("start");
            running.cancel();
            let r = running.wait();
            assert_eq!(r, Err(QueryError("cancelled".into())), "cancel must end the query");
            let (cw, pw) = b.chan.stored_wakers();
            c.stale_wakers.fetch_add(cw + pw, Relaxed);
            assert_eq!((cw, pw), (0, 0), "waker(s) still stored in the channel after the query ended");
        } else {
            b.query.run(cfg.workers).expect("query failed");
            let out = b.out.lock().unwrap().clone();
            check_stream(&out, &cfg);
        }
    });
    check_coverage(name, &cov, expect);
}

// ---------------------------------------------------------------- (a) no lost wake-up

/// Producer blocks on a full exchange (cap 1, 2 items) and must be woken by the consumer's pop.
#[test]
fn a1_producer_blocked_on_full_producer_queued_first() {
    let cfg = Cfg { items: 2, ..Cfg::base() };
    let e = Expect { prod_blocked: true, ..Default::default() };
    drive("a1_producer_blocked_on_full_producer_queued_first", Some(3), cfg, Mode::Plain, e);
}

#[test]
fn a1_producer_blocked_on_full_consumer_queued_first() {
    let cfg = Cfg { items: 2, consumer_first: true, ..Cfg::base() };
    let e = Expect { prod_blocked: true, ..Default::default() };
    drive("a1_producer_blocked_on_full_consumer_queued_first", Some(3), cfg, Mode::Plain, e);
}

/// Consumer blocks on an empty exchange and must be woken by the producer's push.
#[test]
fn a2_consumer_blocked_on_empty_consumer_queued_first() {
    let cfg = Cfg { items: 1, consumer_first: true, ..Cfg::base() };
    let e = Expect { cons_blocked: true, ..Default::default() };
    drive("a2_consumer_blocked_on_empty_consumer_queued_first", Some(3), cfg, Mode::Plain, e);
}

#[test]
fn a2_consumer_blocked_on_empty_producer_queued_first() {
    let cfg = Cfg { items: 1, ..Cfg::base() };
    let e = Expect { cons_blocked: true, ..Default::default() };
    drive("a2_consumer_blocked_on_empty_producer_queued_first", Some(3), cfg, Mode::Plain, e);
}

/// Both handshakes on ONE worker thread: a blocked task must hand the thread to the other side.
#[test]
fn a3_one_worker_consumer_queued_first() {
    let cfg = Cfg { workers: 1, items: 3, consumer_first: true, ..Cfg::base() };
    let e = Expect { prod_blocked: true, cons_blocked: true, ..Default::default() };
    drive("a3_one_worker_consumer_queued_first", Some(3), cfg, Mode::Plain, e);
}

#[test]
fn a3_one_worker_producer_queued_first() {
    let cfg = Cfg { workers: 1, items: 3, ..Cfg::base() };
    let e = Expect { prod_blocked: true, ..Default::default() };
    drive("a3_one_worker_producer_queued_first", Some(3), cfg, Mode::Plain, e);
}

// ---------------------------------------------------------------- (b) close races

/// The producer finishes with nothing to send while the consumer is blocked on the empty exchange:
/// `producer_done` must wake it.
#[test]
fn b1_producer_finishes_while_consumer_blocked_consumer_queued_first() {
    let cfg = Cfg { items: 0, consumer_first: true, ..Cfg::base() };
    let e = Expect { cons_blocked: true, ..Default::default() };
    drive("b1_producer_finishes_while_consumer_blocked_consumer_queued_first", Some(3), cfg, Mode::Plain, e);
}

#[test]
fn b1_producer_finishes_while_consumer_blocked_producer_queued_first() {
    let cfg = Cfg { items: 0, ..Cfg::base() };
    let e = Expect { cons_blocked: true, ..Default::default() };
    drive("b1_producer_finishes_while_consumer_blocked_producer_queued_first", Some(3), cfg, Mode::Plain, e);
}

/// LIMIT 1 on the consumer side, 3 items, cap 1: the consumer finishes early (`source.finish()` ->
/// `close_consumers`) while the producer is blocked on the full exchange; it must be woken and end.
#[test]
fn b2_consumer_limit_closes_blocked_producer_consumer_queued_first() {
    let cfg = Cfg { items: 3, limit: Some(1), consumer_first: true, ..Cfg::base() };
    let e = Expect { prod_blocked: true, blocked_then_finished: true, ..Default::default() };
    drive("b2_consumer_limit_closes_blocked_producer_consumer_queued_first", Some(3), cfg, Mode::Plain, e);
}

#[test]
fn b2_consumer_limit_closes_blocked_producer_producer_queued_first() {
    let cfg = Cfg { items: 3, limit: Some(1), ..Cfg::base() };
    let e = Expect { prod_blocked: true, blocked_then_finished: true, ..Default::default() };
    drive("b2_consumer_limit_closes_blocked_producer_producer_queued_first", Some(3), cfg, Mode::Plain, e);
}

#[test]
fn b2_consumer_limit_closes_blocked_producer_one_worker() {
    let cfg = Cfg { workers: 1, items: 3, limit: Some(1), consumer_first: true, ..Cfg::base() };
    let e = Expect { prod_blocked: true, blocked_then_finished: true, ..Default::default() };
    drive("b2_consumer_limit_closes_blocked_producer_one_worker", Some(3), cfg, Mode::Plain, e);
}

/// Two producer tasks, both possibly blocked on the full exchange when the consumer closes it.
#[test]
fn b3_two_producers_blocked_when_consumer_closes() {
    let cfg = Cfg { prod_tasks: 2, items: 2, limit: Some(1), consumer_first: true, ..Cfg::base() };
    let e = Expect { prod_blocked: true, blocked_then_finished: true, ..Default::default() };
    drive("b3_two_producers_blocked_when_consumer_closes", Some(2), cfg, Mode::Plain, e);
}

// ---------------------------------------------------------------- (c) cancel racing with all of the above

#[test]
fn c1_cancel_during_full_empty_handshakes() {
    let cfg = Cfg { items: 2, consumer_first: true, ..Cfg::base() };
    drive("c1_cancel_during_full_empty_handshakes", Some(3), cfg, Mode::Cancel, Expect::default());
}

#[test]
fn c2_cancel_during_early_close() {
    let cfg = Cfg { items: 3, limit: Some(1), consumer_first: true, ..Cfg::base() };
    drive("c2_cancel_during_early_close", Some(3), cfg, Mode::Cancel, Expect::default());
}

#[test]
fn c3_cancel_while_producer_finishes_and_consumer_blocked() {
    let cfg = Cfg { items: 0, consumer_first: true, ..Cfg::base() };
    drive("c3_cancel_while_producer_finishes_and_consumer_blocked", Some(3), cfg, Mode::Cancel, Expect::default());
}

/// The consumer never reads: the producer fills the exchange and stays blocked until cancelled.
#[test]
fn c4_cancel_producer_blocked_forever_on_full_exchange() {
    let cfg = Cfg { items: 2, stuck_consumer: true, ..Cfg::base() };
    drive("c4_cancel_producer_blocked_forever_on_full_exchange", Some(3), cfg, Mode::Cancel, Expect::default());
}

#[test]
fn c5_cancel_one_worker() {
    let cfg = Cfg { workers: 1, items: 3, consumer_first: true, ..Cfg::base() };
    drive("c5_cancel_one_worker", Some(3), cfg, Mode::Cancel, Expect::default());
}

/// Regression test for finding F1 (see tests/leak.rs): a consumer blocked on the empty exchange is
/// cancelled; afterwards the channel must store no waker (before the fix: `Arc leaked`).
/// One worker, so it is small.
#[test]
fn f1_cancel_leaves_no_waker_in_channel() {
    let cfg = Cfg { workers: 1, items: 0, consumer_first: true, ..Cfg::base() };
    drive("f1_cancel_leaves_no_waker_in_channel", Some(3), cfg, Mode::Cancel, Expect { cons_blocked: true, ..Default::default() });
}

// ---------------------------------------------------------------- (d) the bound

/// Queue length <= capacity after every push, and (produced - consumed) <= capacity + one batch
/// per producer task + one per consumer task (the documented slack: a blocked producer holds its
/// batch outside the queue, a consumer holds a batch between `pop` and its sink).
#[test]
fn d1_bound_one_producer() {
    let cfg = Cfg { items: 3, check_bound: true, ..Cfg::base() };
    let e = Expect { prod_blocked: true, ..Default::default() };
    drive("d1_bound_one_producer", Some(3), cfg, Mode::Plain, e);
}

#[test]
fn d2_bound_two_producers() {
    let cfg = Cfg { prod_tasks: 2, items: 1, check_bound: true, consumer_first: true, ..Cfg::base() };
    let e = Expect { prod_blocked: true, ..Default::default() };
    drive("d2_bound_two_producers", Some(3), cfg, Mode::Plain, e);
}

// ---------------------------------------------------------------- (e) no loss, duplication or reordering

/// One producer, 3 items through a capacity-1 exchange: the consumer sees exactly [0, 1, 2].
#[test]
fn e1_order_one_producer() {
    let cfg = Cfg { items: 3, ..Cfg::base() };
    drive("e1_order_one_producer", Some(3), cfg, Mode::Plain, Expect::default());
}

/// Two producers, 2 items each: per producer the sequence is complete and in order.
#[test]
fn e2_order_per_producer_with_two_producers() {
    let cfg = Cfg { prod_tasks: 2, items: 2, consumer_first: true, ..Cfg::base() };
    drive("e2_order_per_producer_with_two_producers", Some(2), cfg, Mode::Plain, Expect::default());
}

/// One producer, two consumer tasks: every item arrives exactly once.
#[test]
fn e3_two_consumers_each_item_once() {
    let cfg = Cfg { cons_tasks: 2, items: 2, ..Cfg::base() };
    drive("e3_two_consumers_each_item_once", Some(2), cfg, Mode::Plain, Expect::default());
}

/// Items still queued when the last producer is done are delivered, not dropped.
#[test]
fn e4_items_queued_at_producer_done_are_delivered() {
    let cfg = Cfg { items: 2, cap: 2, consumer_first: false, ..Cfg::base() };
    drive("e4_items_queued_at_producer_done_are_delivered", Some(3), cfg, Mode::Plain, Expect::default());
}
