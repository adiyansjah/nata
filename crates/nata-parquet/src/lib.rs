//! The Parquet scan of slice 3 (`docs/slices/0001-parquet-scan.md`), single-thread step: footer statistics to pruning decisions, a filter parser, an
//! exact row filter, a row-group reader and DuckDB-compatible CSV output. See the README for the contract and the checks.
//!
//! Layout: [`schema`] (which Parquet columns the scan understands), [`adapter`] (footer statistics to `nata-scan` types), [`filter`] (`col op literal`,
//! DuckDB row semantics), [`reader`] (plan, row-group tasks, serial driver, report), [`csv`] (output text), [`civil`] (calendar arithmetic),
//! [`error`] (errors that name the file and row group and never carry cell values).

pub mod adapter;
pub mod civil;
pub mod csv;
pub mod error;
pub mod filter;
mod footer;
pub mod opener;
pub mod parallel;
pub mod reader;
pub mod remote;
pub mod schema;
pub mod stats;

pub use error::{ErrorKind, ScanError};
pub use opener::{FsOpener, OpenFile, OpenFor, Opener};
pub use parallel::{ParallelConfig, ScanCancel, run_parallel, run_parallel_with};
pub use reader::remote_error as remote_scan_error;
pub use reader::{
    BatchMap, CancelHook, RowGroupCursor, RowGroupTask, ScanOptions, ScanPlan, ScanReport,
    TaskOutput, TaskResult, TaskSink, TaskSinkFactory, read_leaves, run_serial,
};
pub use remote::{
    RemoteBytes, RemoteError, RemoteErrorKind, RemoteFile, RemoteIo, RemoteQuery,
    is_remote_location,
};
pub use stats::ScanStats;
