//! The source of a query (`FROM '<path or glob>'`): which files, and the schema they share, read from the Parquet footers by `nata-parquet`.

use std::path::{Path, PathBuf};

use nata_parquet::ScanError;
use nata_parquet::schema::{Kind, Leaf};

use crate::error::{Pos, Result, SqlError};
use crate::glob;
use crate::types::SqlType;

/// Where files and schemas come from. The default reads the file system; tests substitute an in-memory one.
pub trait SourceProvider: Sync {
    /// Resolve a registered name from a pinned catalog snapshot. File-only providers refuse it.
    fn named(&self, _name: &[String]) -> std::result::Result<SourceInfo, String> {
        Err("registered table names require a catalog snapshot".into())
    }
    /// Whether this provider reads `s3://` and `gs://` locations (card 0006). The default, the file system, refuses them with `not supported yet`.
    fn supports_remote(&self) -> bool {
        false
    }
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

/// The file system for local paths and a remote reader for `s3://` and `gs://` locations and globs (card 0006): the front end binds names and types from
/// the footer of the first object, which comes from the reader's identity-validated footer cache, so the scan that follows finds it warm.
pub struct RoutedSource<'a> {
    pub local: &'a dyn SourceProvider,
    pub remote: &'a dyn nata_parquet::RemoteIo,
    /// Polled while the binder waits for the store (Ctrl-C).
    pub cancel: &'a (dyn Fn() -> bool + Sync),
}

impl SourceProvider for RoutedSource<'_> {
    fn named(&self, name: &[String]) -> std::result::Result<SourceInfo, String> {
        self.local.named(name)
    }

    fn supports_remote(&self) -> bool {
        true
    }

    fn expand(&self, pattern: &str) -> std::result::Result<Vec<PathBuf>, String> {
        if nata_parquet::is_remote_location(pattern) {
            return self
                .remote
                .expand(pattern, self.cancel)
                .map(|v| v.into_iter().map(PathBuf::from).collect())
                .map_err(|e| e.message);
        }
        self.local.expand(pattern)
    }

    fn leaves(&self, file: &Path) -> std::result::Result<Vec<Leaf>, ScanError> {
        let Some(location) = file
            .to_str()
            .filter(|l| nata_parquet::is_remote_location(l))
        else {
            return self.local.leaves(file);
        };
        let meta = self
            .remote
            .footer(location, self.cancel)
            .map_err(|e| nata_parquet::remote_scan_error(&e, file))?;
        nata_parquet::schema::leaves(meta.file_metadata().schema_descr())
            .map_err(|m| ScanError::not_supported(m).in_file(file))
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

/// What the binder does with the files of a glob.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BindOptions {
    /// Read the footer of EVERY file before the query starts and require one schema (what the binder did before register item P10: 10,000 files meant
    /// 10,000 sequential reads at bind time). The default reads only the first file's footer for the column names and types; the scan checks every other
    /// file's schema when it reaches the file (`ScanOptions::strict_schema`): a mismatch is an error naming the file, found at scan time.
    pub all_footers: bool,
}

/// Resolve `pattern`: expand it, read the footer of the first file (the schema of the query). `pos` is the place of the path in the query. The other files
/// are not read here: see [`BindOptions`]; [`resolve_with`] reads them all.
pub fn resolve(provider: &dyn SourceProvider, pattern: &str, pos: Pos) -> Result<SourceInfo> {
    resolve_with(provider, pattern, pos, BindOptions::default())
}

/// As [`resolve`], and with `all_footers` read the footer of every file and require one schema.
pub fn resolve_with(
    provider: &dyn SourceProvider,
    pattern: &str,
    pos: Pos,
    options: BindOptions,
) -> Result<SourceInfo> {
    if pattern.is_empty() {
        return Err(SqlError::bind(pos, "the path is empty"));
    }
    if pattern.contains("://")
        && !(provider.supports_remote() && nata_parquet::is_remote_location(pattern))
    {
        return Err(SqlError::not_supported(
            pos,
            "remote paths other than s3:// and gs:// (local files and globs, and objects in S3 or GCS)",
        ));
    }
    let files = provider
        .expand(pattern)
        .map_err(|e| SqlError::bind(pos, format!("cannot list the files of the pattern: {e}")))?;
    let Some(first) = files.first() else {
        return Err(SqlError::bind(pos, "no file matches the pattern"));
    };
    let leaves = provider.leaves(first).map_err(|e| scan_error(&e, pos))?;
    let others: &[PathBuf] = if options.all_footers {
        &files[1..]
    } else {
        &[]
    };
    for f in others {
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
