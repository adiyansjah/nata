//! Throwaway spike for verification-plan item 24: the Iceberg commit under concurrent writers, retries and crashes.
//!
//! * `avro`    - a minimal Avro object-container writer/reader for the few Iceberg schemas we need (no dependency)
//! * `ice`     - Iceberg v2 manifests, manifest lists, tiny Avro data files and position-delete files; snapshot views
//! * `catalog` - the catalog interface; REST (plain HTTP), conditional-create object store, and a fault injector
//! * `writer`  - append / delete-where / compaction writers: the correct protocol and three known-bad variants
//! * `checker` - replays every snapshot and the metadata history and checks the invariants I1..I7
//! * `mock`    - a test double of the REST catalog (NOT evidence about Iceberg; the evidence is the reference fixture)
//! * `suite`   - the scenarios that `gcp-check.sh` runs against the real catalog
pub mod avro;
pub mod catalog;
pub mod checker;
pub mod ice;
pub mod mock;
pub mod suite;
pub mod writer;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub type Res<T> = Result<T, String>;

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Small deterministic PRNG (xorshift64*): no dependency, and a seed replays the same plan.
#[derive(Clone, Debug)]
pub struct Prng(u64);
impl Prng {
    pub fn new(seed: u64) -> Prng {
        Prng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }
    /// True with probability p (0.0..=1.0).
    pub fn chance(&mut self, p: f64) -> bool {
        ((self.next() >> 11) as f64 / (1u64 << 53) as f64) < p
    }
    /// A positive i64, like the Java snapshot-id generator.
    pub fn snapshot_id(&mut self) -> i64 {
        (self.next() & 0x7FFF_FFFF_FFFF_FFFF) as i64 | 1
    }
}

/// `file:///x`, `file:/x` and `/x` all name the same local path.
pub fn to_local(loc: &str) -> PathBuf {
    let s = loc.strip_prefix("file://").or_else(|| loc.strip_prefix("file:")).unwrap_or(loc);
    PathBuf::from(s)
}

/// Atomic publish: the file appears completely or not at all (an object-store PUT).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Res<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension(format!("tmp-{}-{}", std::process::id(), now_ms()));
    std::fs::write(&tmp, bytes).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename to {}: {e}", path.display()))
}
