//! Errors of the front end: four classes (`docs/dialect.md` section 8), each with the line and column of the construct it is about.

use std::fmt;

/// A position in the query text: 1-based line and 1-based column, the column counting characters (not bytes). Only `\n` ends a line (as in sqlparser's
/// tokenizer), so a `\r` before it is a character of the line it ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

impl Pos {
    pub const START: Pos = Pos { line: 1, col: 1 };
    /// "No position known": only inside the syntax tree, never in an error.
    pub const NONE: Pos = Pos { line: 0, col: 0 };

    pub fn is_none(self) -> bool {
        self.line == 0
    }

    /// The position of the end of `text` (the place of an error "at the end of the input").
    pub fn end_of(text: &str) -> Pos {
        let mut p = Pos::START;
        for c in text.chars() {
            if c == '\n' {
                p.line = p.line.saturating_add(1);
                p.col = 1;
            } else {
                p.col = p.col.saturating_add(1);
            }
        }
        p
    }
}

impl fmt::Display for Pos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}, column {}", self.line, self.col)
    }
}

/// The class of an error (dialect section 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorClass {
    /// The text is not valid SQL of the subset's grammar; also a query beyond the limits of section 8.
    Syntax,
    /// A name, type or rule of the dialect is violated (an unknown column, `1 = '1'`, a decimal beyond 38 digits, an impossible date).
    Bind,
    /// `not supported yet: <what>`.
    NotSupported,
    /// Raised while running (division by zero, overflow, a bad cast). Never produced by this crate: the evaluator of step 3 uses it.
    Execution,
}

impl ErrorClass {
    pub fn name(self) -> &'static str {
        match self {
            ErrorClass::Syntax => "syntax error",
            ErrorClass::Bind => "bind error",
            ErrorClass::NotSupported => "not supported yet",
            ErrorClass::Execution => "execution error",
        }
    }
}

/// An error with its class, a message that quotes the query and never data, a position, and, for an unknown column, the closest column name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlError {
    pub class: ErrorClass,
    pub message: String,
    pub pos: Pos,
    pub suggestion: Option<String>,
}

impl SqlError {
    pub fn new(class: ErrorClass, pos: Pos, message: impl Into<String>) -> SqlError {
        SqlError {
            class,
            message: message.into(),
            pos,
            suggestion: None,
        }
    }

    pub fn syntax(pos: Pos, message: impl Into<String>) -> SqlError {
        SqlError::new(ErrorClass::Syntax, pos, message)
    }

    pub fn bind(pos: Pos, message: impl Into<String>) -> SqlError {
        SqlError::new(ErrorClass::Bind, pos, message)
    }

    /// `not supported yet: <what>`.
    pub fn not_supported(pos: Pos, what: impl Into<String>) -> SqlError {
        SqlError::new(ErrorClass::NotSupported, pos, what)
    }

    pub fn with_suggestion(mut self, name: impl Into<String>) -> SqlError {
        self.suggestion = Some(name.into());
        self
    }

    /// True for the errors the front end raises about the query's size or depth (dialect section 8, "Limits").
    pub fn is_limit(&self) -> bool {
        self.class == ErrorClass::Syntax && self.message.starts_with(LIMIT_PREFIX)
    }
}

/// The start of the message of every error about a limit of section 8.
pub(crate) const LIMIT_PREFIX: &str = "query too large or too deeply nested:";

impl fmt::Display for SqlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.class {
            ErrorClass::NotSupported => write!(f, "not supported yet: {}", self.message)?,
            c => write!(f, "{}: {}", c.name(), self.message)?,
        }
        if let Some(s) = &self.suggestion {
            write!(f, " (did you mean `{s}`?)")?;
        }
        write!(f, " ({})", self.pos)
    }
}

impl std::error::Error for SqlError {}

pub type Result<T> = std::result::Result<T, SqlError>;
