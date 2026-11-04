//! Spike: the ADR 0001 protocol and scheduler carrying Arrow batches with selection vectors and strict kernels,
//! plus LIMIT and early finish. See `bench/results/20261104-arrow-pipeline.md`.
// The sinks of this crate lock std mutexes through the scheduler's `lock()`, so the crate cannot be built with the
// instrumented primitives. The early-finish protocol is model-checked by `loom-model/`, which includes the scheduler,
// the exchange and LIMIT from this crate's own source files with a stand-in batch type.
#[cfg(nata_loom)]
compile_error!("spikes/arrow-pipeline is not loom-instrumented: run the model checker through loom-model/ (gcp-check-loom.sh)");

pub mod batch;
pub mod exchange;
pub mod genops;
pub mod limit;
pub mod ops;
pub mod proto;
pub mod queries;
pub mod sinks;
pub mod sources;
pub mod synth;
