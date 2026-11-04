//! Loom model of the early-finish protocol of `spikes/arrow-pipeline` (per-stage `stop_depth`, LIMIT, exchange close).
//!
//! The scheduler (`proto/`), the exchange and LIMIT are the spike's own files, included by path. The only module
//! of this crate that is not the spike's is `batch`: a tiny stand-in for the Arrow batch with the methods the
//! included files call (`len`, `is_empty`, `sel`, `with_sel`, `compact`, `Default`), plus a counter of live batches.
//!
//! Build with `RUSTFLAGS="--cfg nata_loom"` for the model checker (std otherwise, then the tests are a plain stress loop).

pub mod batch;

#[path = "../../src/proto/mod.rs"]
pub mod proto;

#[path = "../../src/exchange.rs"]
pub mod exchange;

#[path = "../../src/limit.rs"]
pub mod limit;
