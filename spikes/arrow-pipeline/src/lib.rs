//! Spike: the ADR 0001 protocol and scheduler carrying Arrow batches with selection vectors and strict kernels,
//! plus LIMIT and early finish. See `bench/results/20261104-arrow-pipeline.md`.
pub mod batch;
pub mod exchange;
pub mod genops;
pub mod ops;
pub mod proto;
pub mod queries;
pub mod sinks;
pub mod sources;
pub mod synth;
