//! SNAPSHOT of `spikes/generic-ops/src` as committed (git HEAD ce5f715), the baseline engine: a simple morsel loop,
//! `Op::exec(&mut Batch)` stages, checked NULL-aware selection-aware kernels, the Parquet scan and the scalar NULL
//! reference. Only the `use crate::...` paths changed (to `crate::genops::...`). The new work is outside this module.
#![allow(dead_code)]
pub mod engine;
pub mod fused;
pub mod kernels;
pub mod nulls;
pub mod source;
