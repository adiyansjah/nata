//! The data of the differential tests: SQL that DuckDB runs to write the base files, and the matrix of writers that re-encode them.
#![allow(dead_code, reason = "each test file uses a different part")]

use std::path::{Path, PathBuf};
use std::process::Command;

use super::{duckdb_csv, pyarrow_run, writers};

/// TPC-H `lineitem` at SF0.01 (60,175 rows). Looked for in `NATA_TPCH_DIR`, then `data/tpch-sf0.01` of the repository; generated with
/// `scripts/gen-tpch.sh` if missing.
pub fn lineitem_source() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = std::env::var("NATA_TPCH_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root.join("data/tpch-sf0.01"));
    let f = dir.join("lineitem.parquet");
    if !f.exists() {
        let ok = Command::new("bash")
            .arg(root.join("scripts/gen-tpch.sh"))
            .arg("0.01")
            .arg(&dir)
            .output()
            .is_ok_and(|o| o.status.success());
        if !ok || !f.exists() {
            assert!(
                std::env::var("NATA_REQUIRE_DUCKDB").is_err(),
                "NATA_REQUIRE_DUCKDB is set and lineitem SF0.01 cannot be generated"
            );
            eprintln!(
                "SKIPPED: no TPC-H lineitem at {} and scripts/gen-tpch.sh failed",
                f.display()
            );
            return None;
        }
    }
    Some(f)
}

/// `lineitem` sorted by `l_shipdate`, written by DuckDB with row groups of 2,048 rows (30 row groups).
pub fn lineitem_sorted(duck: &str, dir: &Path) -> Option<PathBuf> {
    let src = lineitem_source()?;
    let out = dir.join("lineitem-duckdb.parquet");
    duckdb_csv(
        duck,
        &format!(
            "COPY (SELECT * FROM '{}' ORDER BY l_shipdate, l_orderkey, l_linenumber) TO '{}' (FORMAT parquet, ROW_GROUP_SIZE 2048)",
            src.display(),
            out.display()
        ),
    );
    Some(out)
}

/// One base file for the edge cases (four row groups of 2,048 rows each, written by DuckDB).
pub fn edge_sql(name: &str) -> String {
    // `g` = the row group of the row
    let from = "FROM (SELECT i, i // 2048 AS g FROM range(0, 8192) t(i))";
    match name {
        // normal numbers / NaN mixed in / all NULL / zeros and infinities
        "floats" => format!(
            "SELECT i AS id,
               CASE g WHEN 0 THEN i::DOUBLE WHEN 1 THEN CASE WHEN i % 10 = 0 THEN 'NaN'::DOUBLE ELSE (i - 2048)::DOUBLE END WHEN 2 THEN NULL
                 ELSE CASE i % 4 WHEN 0 THEN -0.0 WHEN 1 THEN 0.0 WHEN 2 THEN 'inf'::DOUBLE ELSE '-inf'::DOUBLE END END AS d,
               CASE g WHEN 0 THEN (i / 4.0)::FLOAT WHEN 1 THEN CASE WHEN i % 10 = 0 THEN 'NaN'::FLOAT ELSE ((i - 2048) / 4.0)::FLOAT END WHEN 2 THEN NULL
                 ELSE CASE i % 4 WHEN 0 THEN -0.0::FLOAT WHEN 1 THEN 0.0::FLOAT WHEN 2 THEN 'inf'::FLOAT ELSE '-inf'::FLOAT END END AS f
             {from}"
        ),
        // the same without the all-NULL group: a NaN row group sits between ordinary ones
        "floats_nonull" => format!(
            "SELECT i AS id,
               CASE g WHEN 0 THEN i::DOUBLE WHEN 1 THEN CASE WHEN i % 10 = 0 THEN 'NaN'::DOUBLE ELSE i::DOUBLE END WHEN 2 THEN (i + 100000)::DOUBLE
                 ELSE CASE i % 4 WHEN 0 THEN -0.0 WHEN 1 THEN 0.0 WHEN 2 THEN 'inf'::DOUBLE ELSE '-inf'::DOUBLE END END AS d
             {from}"
        ),
        // decimals of every storage width, sorted: group g holds 1000*g + small numbers
        "decimals" => format!(
            "SELECT i AS id, (i / 100.0)::DECIMAL(4,2) AS d4, (i / 1000.0)::DECIMAL(9,3) AS d9, (i * 100 + 0.25)::DECIMAL(15,2) AS d15,
                    (i * 1.0001)::DECIMAL(18,4) AS d18, (i * 100000000000000000000 + 0.5)::DECIMAL(28,2) AS d28,
                    (i * 100000000000000000000000000000)::DECIMAL(38,0) AS d38
             {from}"
        ),
        // text: ASCII, 'é', CJK, astral characters per row group; NULLs in a few rows
        "strings" => format!(
            "SELECT i AS id,
               CASE WHEN i % 97 = 5 THEN NULL
                    ELSE CASE g WHEN 0 THEN 'apple-' WHEN 1 THEN 'été-' WHEN 2 THEN '日本-' ELSE '𝄞-' END || printf('%05d', i) || 'abcdefghij' END AS s,
               CASE g WHEN 0 THEN 'aaaa' WHEN 1 THEN 'aaab' WHEN 2 THEN 'aaaa' ELSE 'zzzz' END AS t
             {from}"
        ),
        // NULLs: all NULL / none / some / a constant
        "nullsome" => format!(
            "SELECT i AS id,
               CASE g WHEN 0 THEN NULL WHEN 1 THEN i WHEN 2 THEN CASE WHEN i % 3 = 0 THEN NULL ELSE i END ELSE 7 END AS x
             {from}"
        ),
        // integers, dates, timestamps, booleans: sorted by row group
        "ints" => format!(
            "SELECT i AS id, (g * 10 + i % 7)::TINYINT AS i8, (g * 1000 + i % 100)::SMALLINT AS i16, (i * 3)::INTEGER AS i32, (i * 3000000000)::BIGINT AS i64,
                    (g * 50 + i % 9)::UTINYINT AS u8, (i * 3)::UINTEGER AS u32, (i * 3000000000)::UBIGINT AS u64,
                    DATE '1992-01-01' + i::INTEGER AS dt, TIMESTAMP '2020-01-01 00:00:00' + INTERVAL (i * 1500000) MICROSECOND AS ts,
                    g >= 2 AS b
             {from}"
        ),
        // bytes with the high bit set sort above the others (unsigned order)
        "blobs" => format!(
            "SELECT i AS id, from_hex(CASE g WHEN 0 THEN '00' WHEN 1 THEN '7f' WHEN 2 THEN '80' ELSE 'ff' END || printf('%04x', i)) AS b {from}"
        ),
        other => panic!("unknown edge file {other}"),
    }
}

pub const EDGE_NAMES: [&str; 7] = [
    "floats",
    "floats_nonull",
    "decimals",
    "strings",
    "nullsome",
    "ints",
    "blobs",
];

pub fn write_edge(duck: &str, dir: &Path, name: &str) -> PathBuf {
    let out = dir.join(format!("{name}-duckdb.parquet"));
    duckdb_csv(
        duck,
        &format!(
            "COPY ({}) TO '{}' (FORMAT parquet, ROW_GROUP_SIZE 2048)",
            edge_sql(name),
            out.display()
        ),
    );
    out
}

/// A file re-encoded by parquet-rs in a style (`None` is never returned: parquet-rs is a dependency).
pub fn rewrite_parquet_rs(src: &Path, dir: &Path, stem: &str, style: &writers::Style) -> PathBuf {
    let out = dir.join(format!("{stem}-prs-{}.parquet", style.name));
    let batch = writers::read_all(src);
    writers::write(&batch, &out, style);
    out
}

const PYARROW_REWRITE: &str = r#"
import sys
import pyarrow.parquet as pq
src, out, variant = sys.argv[1], sys.argv[2], sys.argv[3]
t = pq.read_table(src)
kw = dict(row_group_size=2048, write_statistics=True)
if variant == "default":
    kw.update(compression="snappy", version="2.6", data_page_version="1.0")
elif variant == "v2-zstd":
    kw.update(compression="zstd", version="2.6", data_page_version="2.0")
elif variant == "delta":
    kw.update(compression="gzip", version="2.6", data_page_version="2.0", use_dictionary=False,
              column_encoding={n: "DELTA_BINARY_PACKED" for n, ty in zip(t.schema.names, t.schema.types)
                               if str(ty) in ("int8", "int16", "int32", "int64", "uint8", "uint16", "uint32", "uint64")})
elif variant == "int-decimals":
    kw.update(compression="lz4", version="2.6", store_decimal_as_integer=True)
elif variant == "ms":
    import pyarrow as pa
    cols = []
    for f, c in zip(t.schema, t.columns):
        cols.append(c.cast(pa.timestamp("ms")) if str(f.type).startswith("timestamp") else c)
    t = pa.table(cols, names=t.schema.names)
    kw.update(compression="none", version="2.6")
else:
    raise SystemExit("unknown variant " + variant)
pq.write_table(t, out, **kw)
"#;

pub const PYARROW_VARIANTS: [&str; 5] = ["default", "v2-zstd", "delta", "int-decimals", "ms"];

/// A file re-encoded by pyarrow; `None` if pyarrow is not available.
pub fn rewrite_pyarrow(src: &Path, dir: &Path, stem: &str, variant: &str) -> Option<PathBuf> {
    let out = dir.join(format!("{stem}-pa-{variant}.parquet"));
    let scripts = dir.join(format!("py-{stem}-{variant}"));
    std::fs::create_dir_all(&scripts).expect("script dir");
    match pyarrow_run(
        &scripts,
        PYARROW_REWRITE,
        &[src.to_str()?, out.to_str()?, variant],
    )? {
        Ok(()) => Some(out),
        Err(e) => panic!("pyarrow could not write {variant} for {stem}: {e}"),
    }
}
