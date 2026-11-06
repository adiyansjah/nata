//! Errors of the evaluator: a message that quotes the query and never a data value, and the position of the construct that failed.

use nata_parquet::ErrorKind;
use nata_sql::Pos;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalError {
    pub pos: Pos,
    pub message: String,
    /// A bug of the engine (an unexpected type or shape), not an error of the data.
    pub is_internal: bool,
}

impl EvalError {
    /// An error of the data at `pos` (division by zero, an overflow, a value that does not convert).
    pub fn at(pos: Pos, message: impl Into<String>) -> EvalError {
        EvalError {
            pos,
            message: message.into(),
            is_internal: false,
        }
    }

    pub fn internal(message: impl Into<String>) -> EvalError {
        EvalError {
            pos: Pos::NONE,
            message: message.into(),
            is_internal: true,
        }
    }

    /// The error as the scan's tasks carry it (they add the file and the row group).
    pub fn into_kind(self) -> ErrorKind {
        if self.is_internal {
            ErrorKind::Internal(self.message)
        } else {
            ErrorKind::Eval {
                message: self.message,
                line: self.pos.line,
                column: self.pos.col,
            }
        }
    }
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.pos)
    }
}

impl std::error::Error for EvalError {}

pub type R<T> = Result<T, EvalError>;
