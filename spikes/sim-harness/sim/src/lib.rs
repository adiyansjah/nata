//! Spike for verification-plan item 23: a deterministic simulation harness, built before any distributed code.
//! See bench/results/20261104-sim-harness.md. The harness itself has no dependencies.
#![allow(clippy::new_without_default)]

pub mod engine;
pub mod env;
pub mod exec;
pub mod frame;
pub mod hooks;
pub mod protocol;
pub mod real;
pub mod rng;
pub mod scenario;
pub mod sim;
pub mod trace;
pub mod trialsupport;

// The real scheduler, operator protocol and exchange, included by path (never copied). `sched.rs` and `ops.rs`
// come from spikes/operator-trait, `exchange.rs` from the exchange-loom snapshot (which has the cancel-leak fix).
#[path = "../../../operator-trait/src/api.rs"]
pub mod api;
#[path = "../../../operator-trait/src/batch.rs"]
pub mod batch;
#[path = "../../../exchange-loom/src/exchange.rs"]
#[allow(dead_code)]
pub mod exchange;
#[path = "../../../operator-trait/src/ops.rs"]
#[allow(dead_code)]
pub mod ops;
#[path = "../../../operator-trait/src/sched.rs"]
pub mod sched;
#[path = "../../../operator-trait/src/sync.rs"]
pub mod sync;
