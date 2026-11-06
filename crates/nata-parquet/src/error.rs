//! Errors of the scan. Every error that comes from a file names the FILE and, where it is known, the ROW GROUP. No message carries a cell value:
//! messages of the `parquet` and `arrow` crates are passed through [`redact`], which keeps their words and drops anything that sits between quotes.

use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// `not supported yet: <what>`: a type, an encoding or a feature that is outside the card. Never a guess.
    NotSupported(String),
    /// The file cannot be opened or read (permissions, missing file, I/O error).
    Io(String),
    /// The file is not valid Parquet, or its pages cannot be decoded.
    Corrupt(String),
    /// A column of a later file differs from the first file (name or type).
    Schema(String),
    /// A column that is asked for does not exist, a repeated column name, an unusable option.
    Usage(String),
    /// A filter text that cannot be parsed or typed. Names the column.
    Filter(String),
    /// A bug of the reader or of a dependency (a panic was caught). Not the user's fault.
    Internal(String),
    /// The scan was cancelled from outside before it finished; what was delivered is a prefix of the result, never the whole of it.
    Cancelled(String),
    /// An error of the query's own expressions while running (division by zero, an overflow, a value that does not convert): `message` quotes the query,
    /// never data; `line` and `column` (1-based) are the construct in the query text. Raised by a [`crate::reader::BatchMap`].
    Eval {
        message: String,
        line: u32,
        column: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanError {
    pub file: Option<PathBuf>,
    pub row_group: Option<usize>,
    pub kind: ErrorKind,
}

impl ScanError {
    pub fn new(kind: ErrorKind) -> ScanError {
        ScanError {
            file: None,
            row_group: None,
            kind,
        }
    }

    pub fn in_file(mut self, file: &Path) -> ScanError {
        self.file = Some(file.to_path_buf());
        self
    }

    pub fn in_row_group(mut self, row_group: usize) -> ScanError {
        self.row_group = Some(row_group);
        self
    }

    pub fn not_supported(what: impl Into<String>) -> ScanError {
        ScanError::new(ErrorKind::NotSupported(what.into()))
    }

    pub fn usage(what: impl Into<String>) -> ScanError {
        ScanError::new(ErrorKind::Usage(what.into()))
    }

    pub fn is_not_supported(&self) -> bool {
        matches!(self.kind, ErrorKind::NotSupported(_))
    }
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(file) = &self.file {
            write!(f, "{}: ", file.display())?;
        }
        if let Some(rg) = self.row_group {
            write!(f, "row group {rg}: ")?;
        }
        match &self.kind {
            ErrorKind::NotSupported(what) => write!(f, "not supported yet: {what}"),
            ErrorKind::Io(m) => write!(f, "cannot read the file: {m}"),
            ErrorKind::Corrupt(m) => write!(f, "corrupt or unreadable Parquet data: {m}"),
            ErrorKind::Schema(m) => write!(f, "schema differs from the first file: {m}"),
            ErrorKind::Usage(m) => write!(f, "{m}"),
            ErrorKind::Filter(m) => write!(f, "{m}"),
            ErrorKind::Internal(m) => write!(
                f,
                "internal error (a bug, or a file that triggers one): {m}"
            ),
            ErrorKind::Cancelled(m) => write!(f, "cancelled: {m}"),
            ErrorKind::Eval {
                message,
                line,
                column,
            } => write!(
                f,
                "execution error: {message} (line {line}, column {column})"
            ),
        }
    }
}

impl std::error::Error for ScanError {}

/// The words of a library error message without anything that could be a cell value: text between single or double quotes is replaced by `'..'`,
/// control characters are dropped, and the result is cut at 300 characters.
pub fn redact(message: &str) -> String {
    let mut out = String::new();
    let mut quote: Option<char> = None;
    for c in message.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                    out.push_str("'..'");
                }
            }
            None => {
                if c == '\'' || c == '"' || c == '`' {
                    quote = Some(c);
                } else if !c.is_control() {
                    out.push(c);
                }
            }
        }
    }
    if quote.is_some() {
        out.push_str("'..'");
    }
    if out.chars().count() > 300 {
        out = out.chars().take(300).collect::<String>() + "...";
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_drops_quoted_text_and_control_characters() {
        assert_eq!(
            redact("bad value 'SECRET-1' in `col` \"x\" end"),
            "bad value '..' in '..' '..' end"
        );
        assert_eq!(redact("unterminated 'SECRET"), "unterminated '..'");
        assert_eq!(redact("a\nb\u{7}c"), "abc");
        assert!(redact(&"x".repeat(1000)).len() < 400);
    }

    #[test]
    fn display_names_file_and_row_group() {
        let e = ScanError::new(ErrorKind::Corrupt("bad page".into()))
            .in_file(Path::new("/d/a.parquet"))
            .in_row_group(3);
        assert_eq!(
            e.to_string(),
            "/d/a.parquet: row group 3: corrupt or unreadable Parquet data: bad page"
        );
        let e =
            ScanError::not_supported("column x of type FLOAT16").in_file(Path::new("f.parquet"));
        assert_eq!(
            e.to_string(),
            "f.parquet: not supported yet: column x of type FLOAT16"
        );
    }
}
