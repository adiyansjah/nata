//! Postgres wire 3.0: checked framing and bounded simple and extended sessions (ADR 0008).
#![forbid(unsafe_code)]
mod binary;
pub mod catalog;
pub mod codec;
mod copy;
mod date;
mod extended;
mod float;
mod metadata;
pub mod server;
mod session;

mod browse;

mod row;
