//! TEST-ONLY. A hand-written recursive-descent parser for exactly the subset (moved from the bake-off spike `spikes/sql-parser`, extended with `::`, `//`,
//! `TRUE`/`FALSE`), kept until the slice closes as an independent oracle for [`crate::parse`]: on the accepted queries of `corpus/testset.jsonl` and
//! on generated ones both must give the same tree; on the refused ones both must refuse.
#![allow(
    dead_code,
    reason = "the oracle keeps the spike's mutants and helpers; only part of it is used by the tests"
)]
#![allow(
    clippy::all,
    reason = "moved from the spike, whose own lints were the workspace's"
)]

pub mod hand;
#[cfg(test)]
mod tests;

/// The oracle's own error type (the spike's): optional positions, three kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Syntax,
    NotSupported,
    Limit,
}

#[derive(Debug, Clone)]
pub struct ParseError {
    pub kind: ErrorKind,
    pub message: String,
    pub line: Option<usize>,
    pub col: Option<usize>,
    pub offset: Option<usize>,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)?;
        if let (Some(l), Some(c)) = (self.line, self.col) {
            write!(f, " (line {l}, column {c})")?;
        }
        Ok(())
    }
}

/// Line and column (1-based; the column counts characters) of a byte offset.
pub fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for (i, ch) in text.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}
