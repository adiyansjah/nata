//! The runtime abstraction: everything a node of the engine may touch that is not pure computation.
//!
//! Protocol code (`protocol.rs`) is generic over `Env` and must reach the outside world ONLY through it:
//! time, randomness, the network, the object store, task spawning, fault points, and observation (events,
//! resource tracking, invariant violations). `sim::SimEnv` and `real::RealEnv` are the two implementations.
//! The synchronous compute fragments (operators, the scheduler) take no `Env`: they are run to completion.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

pub use crate::exec::{Nanos, NodeId, TaskCtl};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreError {
    NotFound,
    /// A transient failure: the caller may retry.
    Unavailable,
}

/// A counted resource (a buffer, a connection). It is counted while it is alive; leaks show up as a
/// non-zero count at the end of a run. A crash drops the node's futures, which drops their guards.
pub struct Tracked(Arc<AtomicI64>);

impl Tracked {
    pub fn new(counter: &Arc<AtomicI64>) -> Tracked {
        counter.fetch_add(1, Ordering::SeqCst);
        Tracked(Arc::clone(counter))
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub trait Env: Clone + 'static {
    fn me(&self) -> NodeId;
    /// Time since the start of the run. Virtual in simulation. Never use `Instant`/`SystemTime` in protocol code.
    fn now(&self) -> Nanos;
    /// A random number from this node's own seeded stream. Never use `rand` or `RandomState`.
    fn rand(&self) -> u64;
    fn sleep(&self, d: Nanos) -> impl Future<Output = ()>;
    /// Fire and forget: the network may drop, duplicate, delay and reorder it.
    fn send(&self, to: NodeId, bytes: Vec<u8>);
    /// The next message from anyone, or `None` after `d`.
    fn recv_timeout(&self, d: Nanos) -> impl Future<Output = Option<(NodeId, Vec<u8>)>>;
    /// Spawn a task that belongs to this node: it dies with the node.
    fn spawn(&self, f: impl Future<Output = ()> + 'static) -> TaskCtl;
    fn store_get(&self, key: &str) -> impl Future<Output = Result<Vec<u8>, StoreError>>;
    fn store_put(&self, key: &str, val: Vec<u8>) -> impl Future<Output = Result<(), StoreError>>;
    /// A named place where the simulator may crash a node (nothing happens in the real runtime).
    fn fault_point(&self, name: &'static str);
    fn track(&self, what: &'static str) -> Tracked;
    /// A protocol event: part of the trace in simulation, ignored by the real runtime.
    fn event(&self, kind: &'static str, a: u64, b: u64);
    /// A numeric fact the invariant checks read (`cancel_seen`, `outcome`, ...). Reset when the node crashes.
    fn gauge_set(&self, name: &'static str, v: i64);
    /// A broken invariant, found by the protocol code itself.
    fn violation(&self, msg: String);
}
