//! Read-only catalog seam for slice 10; slice 12 can supply the same immutable snapshot.
//! Registry format: catalog<TAB>schema<TAB>table<TAB>local Parquet file, one row per table.
use nata_sql::{
    SourceProvider,
    source::{BindOptions, SourceInfo},
};
use std::{
    collections::BTreeSet,
    io::{self, Read},
    path::{Path, PathBuf},
};

pub const MAX_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
#[derive(Clone, Debug)]
pub struct Table {
    pub catalog: String,
    pub schema: String,
    pub name: String,
    pub oid: i32,
    pub schema_oid: i32,
    pub source: SourceInfo,
}
/// Immutable for the connection lifetime. No directory walking, refresh or inferred names.
#[derive(Clone, Debug)]
pub struct Catalog {
    pub name: String,
    pub oid: i32,
}
#[derive(Clone, Debug)]
pub struct Schema {
    pub catalog: String,
    pub name: String,
    pub oid: i32,
}
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub catalogs: Vec<Catalog>,
    pub schemas: Vec<Schema>,
    pub tables: Vec<Table>,
}
impl Snapshot {
    pub fn load(path: &Path) -> io::Result<Self> {
        Self::load_with(path, None)
    }

    /// As `load`; with a remote reader a table's location may also be one object in a store (`s3://bucket/key`, `gs://bucket/key`; no glob), whose
    /// schema is read from its footer through the reader (card 0006, ADR 0014's snapshot interface).
    pub fn load_with(path: &Path, remote: Option<&dyn nata_parquet::RemoteIo>) -> io::Result<Self> {
        let invalid = || io::Error::other("invalid or oversized local catalog registry");
        let mut text = String::new();
        std::fs::File::open(path)?
            .take((MAX_SNAPSHOT_BYTES + 1) as u64)
            .read_to_string(&mut text)?;
        if text.len() > MAX_SNAPSHOT_BYTES {
            return Err(invalid());
        }
        let mut snapshot = Self::default();
        let mut identities = BTreeSet::new();
        let mut schemas = std::collections::BTreeMap::new();
        let mut next_oid = 16_384i32;
        let mut bytes = text.len().saturating_mul(8);
        for line in text
            .lines()
            .filter(|s| !s.is_empty() && !s.starts_with('#'))
        {
            let parts: Vec<_> = line.split('\t').collect();
            if !matches!(parts.len(), 1 | 2 | 4)
                || parts
                    .iter()
                    .take(3)
                    .any(|s| s.is_empty() || s.len() > 63 || s.contains('\0'))
            {
                return Err(invalid());
            }
            let catalog = parts[0];
            if !snapshot.catalogs.iter().any(|c| c.name == catalog) {
                let oid = next_oid;
                next_oid = next_oid.checked_add(1).ok_or_else(invalid)?;
                snapshot.catalogs.push(Catalog {
                    name: catalog.into(),
                    oid,
                });
            }
            if parts.len() == 1 {
                continue;
            }
            let schema = parts[1];
            if matches!(schema, "pg_catalog" | "information_schema") {
                return Err(invalid());
            }
            let schema_oid =
                if let Some(oid) = schemas.get(&(catalog.to_string(), schema.to_string())) {
                    *oid
                } else {
                    let oid = next_oid;
                    next_oid = next_oid.checked_add(1).ok_or_else(invalid)?;
                    schemas.insert((catalog.to_string(), schema.to_string()), oid);
                    snapshot.schemas.push(Schema {
                        catalog: catalog.into(),
                        name: schema.into(),
                        oid,
                    });
                    oid
                };
            if parts.len() == 2 {
                continue;
            }
            let [catalog, schema, name, file] = parts.as_slice() else {
                return Err(invalid());
            };
            if [catalog, schema, name]
                .iter()
                .any(|s| s.is_empty() || s.len() > 63 || s.contains('\0'))
                || (file.contains("://")
                    && !(remote.is_some() && nata_parquet::is_remote_location(file)))
                || file.contains(['*', '?', '[', '\0'])
                || matches!(*schema, "pg_catalog" | "information_schema")
                || !identities.insert((catalog.to_string(), schema.to_string(), name.to_string()))
            {
                return Err(invalid());
            }
            let p = PathBuf::from(file);
            let p = if p.is_absolute() || nata_parquet::is_remote_location(file) {
                p
            } else {
                path.parent().unwrap_or(Path::new(".")).join(p)
            };
            let no_cancel = || false;
            let routed;
            let provider: &dyn SourceProvider = match remote {
                Some(remote) => {
                    routed = nata_sql::RoutedSource {
                        local: &nata_sql::FsSource,
                        remote,
                        cancel: &no_cancel,
                    };
                    &routed
                }
                None => &nata_sql::FsSource,
            };
            let source = nata_sql::source::resolve_with(
                provider,
                p.to_str().ok_or_else(invalid)?,
                nata_sql::Pos::START,
                BindOptions { all_footers: true },
            )
            .map_err(|_| io::Error::other("cannot read registered table schema"))?;
            if source.columns.iter().any(|c| c.ty.is_err()) {
                return Err(io::Error::other("unsupported registered column type"));
            }
            bytes = bytes.saturating_add(
                std::mem::size_of::<Table>()
                    + source
                        .columns
                        .iter()
                        .map(|c| {
                            c.name.len() + std::mem::size_of::<nata_sql::source::SourceColumn>()
                        })
                        .sum::<usize>()
                    + p.as_os_str().len(),
            );
            if bytes > MAX_SNAPSHOT_BYTES {
                return Err(invalid());
            }
            let oid = next_oid;
            next_oid = next_oid.checked_add(1).ok_or_else(invalid)?;
            snapshot.tables.push(Table {
                catalog: catalog.to_string(),
                schema: schema.to_string(),
                name: name.to_string(),
                oid,
                schema_oid,
                source,
            });
        }
        Ok(snapshot)
    }
    pub(crate) fn resolve(
        &self,
        name: &[String],
        database: &str,
        search_path: &[String],
    ) -> Result<SourceInfo, String> {
        for schema in search_path
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(""))
        {
            let found = self.tables.iter().find(|t| match name {
                [table] => t.catalog == database && t.schema == schema && t.name == *table,
                [s, table] => t.catalog == database && t.schema == *s && t.name == *table,
                [c, s, table] => t.catalog == *c && t.schema == *s && t.name == *table,
                _ => false,
            });
            if let Some(t) = found {
                return Ok(t.source.clone());
            }
        }
        Err("unknown registered table".into())
    }
}
impl SourceProvider for Snapshot {
    fn named(&self, name: &[String]) -> Result<SourceInfo, String> {
        self.resolve(name, "nata", &["public".into()])
    }
    fn expand(&self, pattern: &str) -> Result<Vec<PathBuf>, String> {
        nata_sql::FsSource.expand(pattern)
    }
    fn leaves(
        &self,
        file: &Path,
    ) -> Result<Vec<nata_parquet::schema::Leaf>, nata_parquet::ScanError> {
        nata_sql::FsSource.leaves(file)
    }
}
