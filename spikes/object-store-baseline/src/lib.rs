//! Baseline spike for card 0006 part 4: the request policy of ADR 0011 as a thin wrapper around `object_store`, the bounded
//! identity-validated metadata cache of ADR 0012, a hostile fake S3 endpoint to test them against, and a small scan engine to time
//! them (split runtime and separate-tokio-CPU-runtime arms of ADR 0002).

pub mod bench;
pub mod cache;
pub mod cachecases;
pub mod engine;
pub mod fake;
pub mod faultcases;
pub mod plan;
pub mod policy;
pub mod remote;
pub mod testdata;
pub mod util;
