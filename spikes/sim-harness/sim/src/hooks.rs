//! The abstraction layer whose cost the benchmark measures (`bin/simbench.rs`).
//!
//! Where does the hot path of the engine cross the runtime abstraction? Not in the scheduler (which never
//! asks for time or randomness per batch), but wherever a batch crosses an exchange: in a distributed engine
//! that boundary must be able to hand a batch to a simulated or a real network, to count it, to time it and
//! to trace it. `HookedSink` / `HookedSource` wrap the REAL in-process exchange of spikes/exchange-loom with
//! exactly that: one clock read and one event call per batch on each side. `H` is a concrete type (static
//! dispatch) or `dyn Hooks` (dynamic dispatch): both are measured.

use std::sync::Arc;
use std::time::Instant;

use crate::api::*;
use crate::batch::Batch;

pub trait Hooks: Send + Sync + 'static {
    fn now(&self) -> u64;
    fn event(&self, kind: u8, a: u64);
}

/// The real runtime's hooks: a monotonic clock read, events ignored.
pub struct RealHooks {
    start: Instant,
}

impl RealHooks {
    pub fn new() -> Self {
        RealHooks { start: Instant::now() }
    }
}

impl Hooks for RealHooks {
    fn now(&self) -> u64 {
        self.start.elapsed().as_nanos() as u64
    }

    fn event(&self, kind: u8, a: u64) {
        std::hint::black_box((kind, a));
    }
}

/// Pass-through: isolates the cost of the extra indirection from the cost of reading the clock.
pub struct NoHooks;

impl Hooks for NoHooks {
    fn now(&self) -> u64 {
        0
    }

    fn event(&self, kind: u8, a: u64) {
        std::hint::black_box((kind, a));
    }
}

/// Harness self-test: a hook with a KNOWN cost, `spin_ns` per call to `event`.
pub struct SlowHooks {
    pub spin_ns: u64,
}

impl Hooks for SlowHooks {
    fn now(&self) -> u64 {
        0
    }

    fn event(&self, kind: u8, a: u64) {
        std::hint::black_box((kind, a));
        let t = Instant::now();
        while (t.elapsed().as_nanos() as u64) < self.spin_ns {
            std::hint::spin_loop();
        }
    }
}

pub struct HookedSink<H: Hooks + ?Sized>(pub Arc<dyn GlobalSink>, pub Arc<H>);

struct HookedLocalSink<H: Hooks + ?Sized> {
    inner: Box<dyn LocalSink>,
    hooks: Arc<H>,
}

impl<H: Hooks + ?Sized> GlobalSink for HookedSink<H> {
    fn make(&self, task: usize) -> Box<dyn LocalSink> {
        Box::new(HookedLocalSink { inner: self.0.make(task), hooks: Arc::clone(&self.1) })
    }

    fn finalize(&self) -> Result<()> {
        self.0.finalize()
    }
}

impl<H: Hooks + ?Sized> LocalSink for HookedLocalSink<H> {
    fn push(&mut self, input: &mut Batch, waker: &Waker) -> Result<SinkResult> {
        let t = self.hooks.now();
        let r = self.inner.push(input, waker);
        self.hooks.event(0, t);
        r
    }

    fn combine(self: Box<Self>) -> Result<()> {
        self.inner.combine()
    }
}

pub struct HookedSource<H: Hooks + ?Sized>(pub Arc<dyn SourceFactory>, pub Arc<H>);

struct HookedLocalSource<H: Hooks + ?Sized> {
    inner: Box<dyn Source>,
    hooks: Arc<H>,
}

impl<H: Hooks + ?Sized> SourceFactory for HookedSource<H> {
    fn make(&self) -> Box<dyn Source> {
        Box::new(HookedLocalSource { inner: self.0.make(), hooks: Arc::clone(&self.1) })
    }
}

impl<H: Hooks + ?Sized> Source for HookedLocalSource<H> {
    fn next(&mut self, waker: &Waker) -> Result<SourceResult> {
        let t = self.hooks.now();
        let r = self.inner.next(waker);
        self.hooks.event(1, t);
        r
    }

    fn finish(&mut self) {
        self.inner.finish();
    }
}
