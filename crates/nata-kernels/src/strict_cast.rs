//! The one place that may call the lenient arrow cast functions.
//!
//! `arrow_cast::cast::cast` and `cast_with_options` (default `safe: true`) turn a value that does not
//! fit into NULL. SQL needs an error instead (docs/decimal-rules.md), so every cast goes through here
//! with `safe: false`. The workspace `clippy.toml` bans the two functions everywhere else.
use arrow_array::ArrayRef;
use arrow_cast::cast::{CastOptions, cast_with_options};
use arrow_schema::{ArrowError, DataType};

/// Cast `array` to `to`, failing on any value that does not fit instead of producing NULL.
#[allow(
    clippy::disallowed_methods,
    reason = "the single sanctioned caller; safe: false is set explicitly"
)]
pub fn strict_cast(array: &dyn arrow_array::Array, to: &DataType) -> Result<ArrayRef, ArrowError> {
    let options = CastOptions {
        safe: false,
        ..CastOptions::default()
    };
    cast_with_options(array, to, &options)
}
