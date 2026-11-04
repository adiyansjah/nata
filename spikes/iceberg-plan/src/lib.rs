//! Spike (verification-plan items 25 and 26): Iceberg v2 tables written and read by us, interoperating with DuckDB and pyiceberg, and an
//! inclusive-projection scan planner checked for soundness against row-level truth. See README.md.
pub mod transform;
pub mod types;

pub type Res<T> = Result<T, String>;
pub fn err<T>(m: impl Into<String>) -> Res<T> { Err(m.into()) }
