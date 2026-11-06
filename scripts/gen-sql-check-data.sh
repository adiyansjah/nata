#!/usr/bin/env bash
# Small Parquet files for the differential queries of check/queries/sql (slice 4): every supported column type with NULLs and extreme values over
# several row groups, and the edge cases of docs/way-of-working.md item 2 (an empty file, a one-row file, an all-NULL file, duplicate rows, a glob of
# parts, a larger file of many row groups). Written by DuckDB into <dir>/sqlcheck/; nothing here is committed.
# Usage: scripts/gen-sql-check-data.sh <output-dir>      (the same directory as the TPC-H files: nata-check substitutes it for {data})
set -euo pipefail
out="${1:?usage: gen-sql-check-data.sh <output-dir>}"
command -v duckdb >/dev/null || { echo "error: duckdb not found on PATH" >&2; exit 1; }
unset AI_AGENT
mkdir -p "$out/sqlcheck/parts"
out="$(cd "$out/sqlcheck" && pwd)"
rm -f "$out"/*.parquet "$out"/parts/*.parquet

duckdb -csv -c "
CREATE TABLE t (id INTEGER, ti TINYINT, si SMALLINT, i INTEGER, b BIGINT, r REAL, d DOUBLE, dec52 DECIMAL(5,2), dec183 DECIMAL(18,3),
                dec3810 DECIMAL(38,10), dec33 DECIMAL(3,3), s VARCHAR, dt DATE, f BOOLEAN);
INSERT INTO t VALUES
 (1, 0, 0, 0, 0, 0.0, 0.0, 0.00, 0.000, 0.0000000000, 0.000, '', DATE '1970-01-01', false),
 (2, 1, 1, 1, 1, 1.5, 1.5, 1.50, 1.500, 1.5000000000, 0.500, 'a', DATE '1994-01-01', true),
 (3, -1, -1, -1, -1, -1.5, -1.5, -1.50, -1.500, -1.5000000000, -0.500, 'A', DATE '1994-12-31', false),
 (4, 127, 32767, 2147483647, 9223372036854775807, 3.4028235e38, 1.7976931348623157e308, 999.99, 999999999999999.999, 9999999999999999999999999999.9999999999, 0.999, 'ab', DATE '2000-02-29', true),
 (5, -128, -32768, -2147483648, -9223372036854775808, -3.4028235e38, -1.7976931348623157e308, -999.99, -999999999999999.999, -9999999999999999999999999999.9999999999, -0.999, 'Ab', DATE '1999-12-31', false),
 (6, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL),
 (7, 2, 2, 2, 2, 'NaN'::REAL, 'NaN'::DOUBLE, 2.00, 2.000, 2.0000000000, 0.250, 'é', DATE '2024-02-29', true),
 (8, 3, 3, 3, 3, 'Infinity'::REAL, 'Infinity'::DOUBLE, 3.00, 3.000, 3.0000000000, 0.125, '日本語', DATE '1900-01-01', false),
 (9, 4, 4, 4, 4, '-Infinity'::REAL, '-Infinity'::DOUBLE, 0.01, 0.001, 0.0000000001, 0.001, ' a ', DATE '0001-01-01', true),
 (10, 5, 5, 5, 5, -0.0::REAL, -0.0::DOUBLE, -0.01, -0.001, -0.0000000001, -0.001, 'it''s', DATE '9999-12-31', NULL),
 (11, 6, 6, 6, 6, 0.1::REAL, 0.1, 0.10, 0.100, 0.1000000000, 0.100, 'a,b', DATE '2021-03-14', false),
 (12, 7, 7, 7, 7, 2.5, 2.5, 2.50, 2.500, 2.5000000000, 0.005, E'x\ny', DATE '2021-11-07', true),
 (13, 8, 8, 8, 8, 0.3::REAL, 0.30000000000000004, 0.30, 0.300, 0.3000000000, 0.300, 'z', NULL, NULL),
 (14, 9, 9, 9, 9, 7.0, 7.0, 7.00, 7.000, 7.0000000000, 0.700, 'Z', DATE '1992-01-01', true),
 (15, 10, 10, 10, 10, 8.0, 8.0, 8.00, 8.000, 8.0000000000, 0.800, 'ß', DATE '1993-06-15', false);
COPY t TO '$out/types.parquet' (FORMAT parquet, ROW_GROUP_SIZE 4);
COPY (SELECT * FROM t WHERE false) TO '$out/empty.parquet' (FORMAT parquet);
COPY (SELECT * FROM t WHERE id = 2) TO '$out/one_row.parquet' (FORMAT parquet);
COPY (SELECT id, NULL::TINYINT ti, NULL::SMALLINT si, NULL::INTEGER i, NULL::BIGINT b, NULL::REAL r, NULL::DOUBLE d, NULL::DECIMAL(5,2) dec52,
             NULL::DECIMAL(18,3) dec183, NULL::DECIMAL(38,10) dec3810, NULL::DECIMAL(3,3) dec33, NULL::VARCHAR s, NULL::DATE dt, NULL::BOOLEAN f
      FROM range(1, 6) r(id)) TO '$out/all_null.parquet' (FORMAT parquet);
COPY (SELECT * FROM t WHERE id IN (2, 3, 6) UNION ALL SELECT * FROM t WHERE id IN (2, 3, 6) UNION ALL SELECT * FROM t WHERE id = 2)
     TO '$out/dups.parquet' (FORMAT parquet, ROW_GROUP_SIZE 3);
COPY (SELECT * FROM t WHERE id <= 5) TO '$out/parts/p0.parquet' (FORMAT parquet, ROW_GROUP_SIZE 2);
COPY (SELECT * FROM t WHERE id > 5 AND id <= 10) TO '$out/parts/p1.parquet' (FORMAT parquet, ROW_GROUP_SIZE 2);
COPY (SELECT * FROM t WHERE id > 10) TO '$out/parts/p2.parquet' (FORMAT parquet, ROW_GROUP_SIZE 2);
COPY (SELECT i::INTEGER AS i, (i % 97)::INTEGER AS m, (i * 1000)::BIGINT AS big, (i / 7.0)::DECIMAL(12,4) AS q, 'k' || (i % 13)::VARCHAR AS s,
             (DATE '2000-01-01' + (i % 3000)::INTEGER) AS dt, CASE WHEN i % 11 = 0 THEN NULL ELSE i END AS n
      FROM range(1, 100001) r(i)) TO '$out/big.parquet' (FORMAT parquet, ROW_GROUP_SIZE 5000);
" > /dev/null
echo "wrote $out"
ls -1 "$out" "$out/parts"
