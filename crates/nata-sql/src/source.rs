//! The source of a query (`FROM '<path or glob>'`): which files, and the schema they share, read from the Parquet footers by `nata-parquet`.

use std::path::{Path, PathBuf};

use nata_parquet::ScanError;
use nata_parquet::schema::{Kind, Leaf};

use crate::error::{Pos, Result, SqlError};
use crate::glob;
use crate::types::SqlType;

/// Where files and schemas come from. The default reads the file system; tests substitute an in-memory one.
pub trait SourceProvider: Sync {
    /// The files that a path or glob names, in the order of the scan.
    fn expand(&self, pattern: &str) -> std::result::Result<Vec<PathBuf>, String>;
    /// The top-level columns of a file, from its footer alone.
    fn leaves(&self, file: &Path) -> std::result::Result<Vec<Leaf>, ScanError>;
}

/// The file system.
#[derive(Clone, Copy, Debug, Default)]
pub struct FsSource;

impl SourceProvider for FsSource {
    fn expand(&self, pattern: &str) -> std::result::Result<Vec<PathBuf>, String> {
        glob::expand(pattern).map_err(|e| e.to_string())
    }

    fn leaves(&self, file: &Path) -> std::result::Result<Vec<Leaf>, ScanError> {
        nata_parquet::read_leaves(file)
    }
}

/// A column of the source, in file order.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceColumn {
    pub name: String,
    /// The Parquet kind, or why the scan does not support the column.
    pub kind: std::result::Result<Kind, String>,
    /// The SQL type, or the `<what>` of `not supported yet: <what>` for a column the dialect has no type for.
    pub ty: std::result::Result<SqlType, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SourceInfo {
    /// The path or glob as written.
    pub pattern: String,
    pub files: Vec<PathBuf>,
    pub columns: Vec<SourceColumn>,
}

fn scan_error(e: &ScanError, pos: Pos) -> SqlError {
    use nata_parquet::ErrorKind;
    match &e.kind {
        _ if e.is_not_supported() => SqlError::not_supported(pos, e.to_string()),
        // a file that is missing, unreadable or damaged is a failure of the data, not of the query: class `execution` (exit code 1 of `nata sql`)
        ErrorKind::Io(_) | ErrorKind::Corrupt(_) | ErrorKind::Internal(_) => SqlError::new(
            crate::error::ErrorClass::Execution,
            pos,
            format!("cannot read the source: {e}"),
        ),
        _ => SqlError::bind(pos, format!("cannot read the source: {e}")),
    }
}

fn describe(l: &Leaf) -> String {
    match &l.kind {
        Ok(k) => format!("{} {}", l.name, k.sql_name()),
        Err(_) => format!("{} (unsupported type)", l.name),
    }
}

/// Resolve `pattern`: expand it, read the footer of every file, require one schema. `pos` is the place of the path in the query.
pub fn resolve(provider: &dyn SourceProvider, pattern: &str, pos: Pos) -> Result<SourceInfo> {
    if pattern.is_empty() {
        return Err(SqlError::bind(pos, "the path is empty"));
    }
    if pattern.contains("://") {
        return Err(SqlError::not_supported(
            pos,
            "remote paths (only local files and globs)",
        ));
    }
    let files = provider
        .expand(pattern)
        .map_err(|e| SqlError::bind(pos, format!("cannot list the files of the pattern: {e}")))?;
    let Some(first) = files.first() else {
        return Err(SqlError::bind(pos, "no file matches the pattern"));
    };
    let leaves = provider.leaves(first).map_err(|e| scan_error(&e, pos))?;
    for f in &files[1..] {
        let other = provider.leaves(f).map_err(|e| scan_error(&e, pos))?;
        let same = other.len() == leaves.len()
            && other
                .iter()
                .zip(&leaves)
                .all(|(a, b)| a.name == b.name && a.kind == b.kind);
        if !same {
            // name the first column that differs
            let why = other
                .iter()
                .zip(&leaves)
                .find(|(a, b)| a.name != b.name || a.kind != b.kind)
                .map(|(a, b)| {
                    format!(
                        "column {} here, {} in the first file",
                        describe(a),
                        describe(b)
                    )
                })
                .unwrap_or_else(|| {
                    format!(
                        "{} columns here, {} in the first file",
                        other.len(),
                        leaves.len()
                    )
                });
            return Err(SqlError::bind(
                pos,
                format!(
                    "the files of a pattern must have the same schema: {} differs from {} ({why})",
                    f.display(),
                    first.display()
                ),
            ));
        }
    }
    let columns = leaves
        .into_iter()
        .map(|l| {
            let ty = match &l.kind {
                Ok(k) => SqlType::from_kind(*k),
                Err(why) => Err(why.clone()),
            };
            SourceColumn {
                name: l.name,
                kind: l.kind,
                ty,
            }
        })
        .collect();
    Ok(SourceInfo {
        pattern: pattern.to_string(),
        files,
        columns,
    })
}

/// An in-memory source for tests (here and in step 3): files and their schemas without touching the disk.
#[derive(Clone, Debug, Default)]
pub struct MemSource {
    patterns: std::collections::HashMap<String, Vec<PathBuf>>,
    schemas: std::collections::HashMap<PathBuf, Vec<Leaf>>,
}

impl MemSource {
    pub fn new() -> MemSource {
        MemSource::default()
    }

    /// A file with these columns, in order, each with a kind or the reason it is unsupported. The file is also the pattern `path`.
    pub fn file(
        mut self,
        path: &str,
        columns: &[(&str, std::result::Result<Kind, &str>)],
    ) -> MemSource {
        let leaves = columns
            .iter()
            .enumerate()
            .map(|(index, (name, kind))| Leaf {
                name: (*name).to_string(),
                index,
                physical: parquet::basic::Type::INT32,
                kind: (*kind).map_err(str::to_string),
            })
            .collect();
        self.schemas.insert(PathBuf::from(path), leaves);
        self.patterns
            .insert(path.to_string(), vec![PathBuf::from(path)]);
        self
    }

    /// A glob that names these files (each added with `file`).
    pub fn glob(mut self, pattern: &str, files: &[&str]) -> MemSource {
        self.patterns.insert(
            pattern.to_string(),
            files.iter().map(PathBuf::from).collect(),
        );
        self
    }
}

impl SourceProvider for MemSource {
    fn expand(&self, pattern: &str) -> std::result::Result<Vec<PathBuf>, String> {
        Ok(self.patterns.get(pattern).cloned().unwrap_or_else(|| {
            if glob::has_glob(pattern) {
                Vec::new()
            } else {
                vec![PathBuf::from(pattern)]
            }
        }))
    }

    fn leaves(&self, file: &Path) -> std::result::Result<Vec<Leaf>, ScanError> {
        self.schemas.get(file).cloned().ok_or_else(|| {
            ScanError::new(nata_parquet::ErrorKind::Io(
                "No such file or directory (os error 2)".into(),
            ))
            .in_file(file)
        })
    }
}
