//! Spike (verification-plan item 31): blocking operators on the UNCHANGED operator protocol (`proto/`).
//! Sort and top-N with spill, hash joins (inner, left, right, full, semi, anti, with a residual `<>`) with a partitioned spilling
//! variant, hash aggregation, window functions, and TPC-H Q18 and Q21 as pipelines. Everything is differential-tested against
//! naive references (tests) and DuckDB (`check.sh`). See `README.md` for what the protocol could and could not express.
pub mod agg;
pub mod batch;
pub mod io;
pub mod join;
pub mod ops;
pub mod proto;
pub mod queries;
pub mod sort;
pub mod spill;
pub mod window;
