//! Spike for verification-plan item 27: the object-store read path with real latency.
//!
//! Reuse, by `#[path]`, nothing copied:
//!  * `api`, `batch`: the operator protocol of `spikes/operator-trait` (byte-identical to the snapshot in
//!    `spikes/async-bridge`; `gcp-check.sh` checks that with `cmp`);
//!  * `chan`, `sched`, `sync`: the tokio-to-CPU-pool bridge channel, the scheduler snapshot it was tested with
//!    and the loom/std shim, from `spikes/async-bridge` (ADR 0002).

#[path = "../../operator-trait/src/api.rs"]
pub mod api;
#[path = "../../operator-trait/src/batch.rs"]
pub mod batch;
#[path = "../../async-bridge/src/chan.rs"]
pub mod chan;
#[path = "../../async-bridge/src/sched.rs"]
pub mod sched;
#[path = "../../async-bridge/src/sync.rs"]
pub mod sync;

pub mod bench;
pub mod duck;
pub mod exec;
pub mod fetch;
pub mod testdata;
pub mod layouts;
pub mod scan;
pub mod scenarios;
pub mod sim;
pub mod store;
pub mod util;
