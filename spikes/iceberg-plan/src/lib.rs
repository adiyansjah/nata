//! Spike (verification-plan items 25 and 26): Iceberg v2 tables written and read by us, interoperating with DuckDB and pyiceberg, and an
//! inclusive-projection scan planner checked for soundness against row-level truth. See README.md.
pub mod expr;
pub mod manifest;
pub mod plan;
pub mod read;
pub mod table;
pub mod meta;
pub mod transform;
pub mod types;

pub type Res<T> = Result<T, String>;
pub fn err<T>(m: impl Into<String>) -> Res<T> { Err(m.into()) }

/// Local files only; accepts plain paths and `file:` / `file://` URIs.
pub fn local_path(p: &str) -> &str {
    let p = p.strip_prefix("file://").or_else(|| p.strip_prefix("file:")).unwrap_or(p);
    if p.starts_with("//") { &p[1..] } else { p }
}

pub fn read_file(p: &str) -> Res<Vec<u8>> { std::fs::read(local_path(p)).map_err(|e| format!("{p}: {e}")) }
