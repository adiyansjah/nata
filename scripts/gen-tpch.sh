#!/usr/bin/env bash
# Generate TPC-H tables as Parquet using DuckDB's tpch extension.
# Usage: scripts/gen-tpch.sh <scale-factor> <output-dir>
set -euo pipefail

sf="${1:?usage: gen-tpch.sh <scale-factor> <output-dir>}"
out="${2:?usage: gen-tpch.sh <scale-factor> <output-dir>}"

command -v duckdb >/dev/null || { echo "error: duckdb not found on PATH" >&2; exit 1; }

mkdir -p "$out"
out="$(cd "$out" && pwd)"

copies() { for t in customer lineitem nation orders part partsupp region supplier; do echo "COPY ${t} TO '${out}/${t}.parquet' (FORMAT parquet);"; done; }
# Large scale factors (or GEN_CHILDREN set): generate in steps into a database FILE next to the output, so that DuckDB can spill and the
# memory needed is about 1/children of the data (in memory, SF100 needs more than 100 GB). GEN_CHILDREN defaults to ceil(sf / 10).
children=${GEN_CHILDREN:-}
if [ -z "$children" ] && awk -v s="$sf" 'BEGIN { exit !(s >= 30) }'; then children=$(awk -v s="$sf" 'BEGIN { c = int(s / 10); if (c * 10 < s) c++; print c }'); fi
if [ -n "$children" ]; then
  db="$out/.gen.duckdb"; rm -f "$db" "$db.wal"; mkdir -p "$out/.gen-tmp"
  for i in $(seq 0 $((children - 1))); do
    duckdb "$db" -c "SET temp_directory='$out/.gen-tmp'; INSTALL tpch; LOAD tpch; CALL dbgen(sf=${sf}, children=${children}, step=${i});" > /dev/null
  done
  duckdb "$db" -csv -c "SET temp_directory='$out/.gen-tmp'; $(copies)" > /dev/null
  rm -rf "$db" "$db.wal" "$out/.gen-tmp"
else
  duckdb -csv -c "
INSTALL tpch; LOAD tpch;
CALL dbgen(sf=${sf});
$(copies)
" >/dev/null
fi

echo "wrote TPC-H sf=${sf} to ${out}"
ls -1 "$out"
