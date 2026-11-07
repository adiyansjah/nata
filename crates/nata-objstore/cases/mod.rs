#![allow(dead_code)] // each test binary includes this module and uses part of it
//! The reliability cases of the wrapper and the cache against the hostile fake endpoint. Used by `tests/suites.rs` (cargo test) and by the mutant sweep
//! (`src/bin/sweep.rs`, feature `mutants`); the cases name the crate by its public path, so both compile this same source.

pub mod cache;
pub mod fault;
pub mod scan_support;
pub mod testdata;
