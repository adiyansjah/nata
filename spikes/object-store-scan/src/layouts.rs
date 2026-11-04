//! The file layouts the throughput measurement is run against. The same names are used in the simulated
//! store, in the real store (`<prefix>/<layout>/part-NNNN.parquet`) and in `gcp-check.sh`, which generates them with
//! DuckDB from `scripts/gen-tpch.sh`'s `lineitem.parquet` (so every layout holds exactly the same rows).

#[derive(Clone, Copy, Debug)]
pub struct LayoutDef {
    pub name: &'static str,
    pub files: usize,
    /// Rows per row group requested from DuckDB (`ROW_GROUP_SIZE`); 0 = one row group per file.
    pub rows_per_rg: usize,
    pub note: &'static str,
}

pub const LAYOUTS: [LayoutDef; 4] = [
    LayoutDef {
        name: "big1",
        files: 1,
        rows_per_rg: 1_500_000,
        note: "1 file, a few large row groups (~1.5M rows each)",
    },
    LayoutDef {
        name: "mid1",
        files: 1,
        rows_per_rg: 122_880,
        note: "1 file, DuckDB's default row group size (122,880 rows)",
    },
    LayoutDef {
        name: "small1",
        files: 1,
        rows_per_rg: 20_000,
        note: "1 file, many small row groups (20,000 rows each)",
    },
    LayoutDef {
        name: "files100",
        files: 100,
        rows_per_rg: 0,
        note: "100 small files, one row group each (~60,000 rows)",
    },
];

pub fn layout(name: &str) -> Option<&'static LayoutDef> {
    LAYOUTS.iter().find(|l| l.name == name)
}

pub fn file_name(i: usize) -> String {
    format!("part-{i:04}.parquet")
}

/// Key of file `i` of `layout` relative to the dataset root (the prefix is added by the store spec).
pub fn rel_key(layout: &str, i: usize) -> String {
    format!("{layout}/{}", file_name(i))
}
