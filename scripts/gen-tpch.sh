#!/usr/bin/env bash
# Generate TPC-H tables as Parquet using DuckDB's tpch extension.
# Usage: scripts/gen-tpch.sh <scale-factor> <output-dir>
set -euo pipefail

sf="${1:?usage: gen-tpch.sh <scale-factor> <output-dir>}"
out="${2:?usage: gen-tpch.sh <scale-factor> <output-dir>}"

command -v duckdb >/dev/null || { echo "error: duckdb not found on PATH" >&2; exit 1; }

mkdir -p "$out"
out="$(cd "$out" && pwd)"

duckdb -csv -c "
INSTALL tpch; LOAD tpch;
CALL dbgen(sf=${sf});
$(for t in customer lineitem nation orders part partsupp region supplier; do
    echo "COPY ${t} TO '${out}/${t}.parquet' (FORMAT parquet);"
  done)
" >/dev/null

echo "wrote TPC-H sf=${sf} to ${out}"
ls -1 "$out"
