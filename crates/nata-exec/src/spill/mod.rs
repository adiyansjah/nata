//! Query-owned spill files and a bounded Arrow IPC trust boundary.
//!
//! Disk methods are synchronous and must be invoked by the bounded I/O service,
//! never by an operator on a CPU worker. `Workspace` owns a reservation for all
//! simultaneous codec buffers; callers cannot decode without that reservation.
mod admission;
mod cleanup;
pub use cleanup::cleanup_stale;
#[path = "io.rs"]
mod disk_io;
pub use admission::available_bytes;
pub(crate) use admission::descriptor_allowance;
mod ipc;
mod registry;

pub use disk_io::{DiskJob, IoService, Submit, Ticket};
pub use ipc::{MAX_PAYLOAD, MAX_ROWS, SpillBatch, Workspace, schema};
pub use registry::{Registry, SpillFile, SpillHandle, SpillStats};

use std::io;
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "bounded spill rejection")
}
