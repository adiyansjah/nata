//! Postgres wire 3.0: checked framing and bounded simple and extended sessions (ADR 0008).
#![forbid(unsafe_code)]
mod binary;
pub mod codec;
mod extended;
pub mod server;
mod session;
