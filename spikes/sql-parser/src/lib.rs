//! Spike for rule R1 of slice 4: `sqlparser` (arm A, with a conversion into the subset AST) against a hand-written parser (arm B).
pub mod ast;
pub mod hand;
#[cfg(feature = "arm-a")]
pub mod sqlp;

pub use ast::*;
