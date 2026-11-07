//! Bounded reads of Parquet objects from S3, GCS and MinIO (card 0006, ADRs 0011 and 0012).
//!
//! * [`remote`]: the wrapper around `object_store` that owns every request: admission, the retry policy, identity pinning, bounded bodies, grants, cancel.
//! * [`cache`]: the identity-validated, charged, bounded Parquet footer cache.
//! * [`plan`]: range planning inside one row group.
//! * [`store`]: locations, credentials from the environment and instance metadata, the stores.
//! * [`node`]: the I/O runtime and the handle the Parquet scan reads through (`nata_parquet::RemoteIo`).
//!
//! Outside code is untrusted input (way of working section 6): every byte, header and length that comes from the network reaches `object_store` only
//! through [`remote::BoundedConnector`] and reaches the Parquet decoder only after the checks of [`remote::Query`].

pub mod cache;
pub mod node;
pub mod plan;
pub mod policy;
pub mod remote;
pub mod store;
pub mod util;

pub use node::{LazyObjectIo, NodeOptions, ObjectHandle, ObjectNode};
pub use policy::{CacheMutants, Config, Mutants};
