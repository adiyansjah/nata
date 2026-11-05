#!/usr/bin/env bash
# make-layouts.sh <lineitem.parquet> <out-dir> [sql-file]
# The four file layouts of the object-store scan (spikes/object-store-scan/src/layouts.rs): the same rows with different row-group sizes and file counts.
#   big1 (one file, 1.5M-row groups), mid1 (122,880), small1 (20,000), files100 (100 files). Written under <out-dir>/<layout>/part-NNNN.parquet and marked
#   with <out-dir>/.done. Shared by gcp-check.sh (on the VM) and by the person who uploads the layouts to a real bucket, so both make them with the same code.
set -euo pipefail
LI=${1:?usage: make-layouts.sh <lineitem.parquet> <out-dir> [sql-file]}
OUT=${2:?usage: make-layouts.sh <lineitem.parquet> <out-dir> [sql-file]}
SQL=${3:-$(mktemp)}
command -v duckdb >/dev/null || { echo "duckdb is not on PATH" >&2; exit 4; }
NROWS=$(duckdb -noheader -list -c "SELECT count(*) FROM read_parquet('$LI')")
PER=$(( (NROWS + 99) / 100 ))
{
  echo "CREATE TABLE li AS SELECT * FROM read_parquet('$LI');"
  echo "COPY li TO '$OUT/big1/part-0000.parquet' (FORMAT parquet, ROW_GROUP_SIZE 1500000);"
  echo "COPY li TO '$OUT/mid1/part-0000.parquet' (FORMAT parquet, ROW_GROUP_SIZE 122880);"
  echo "COPY li TO '$OUT/small1/part-0000.parquet' (FORMAT parquet, ROW_GROUP_SIZE 20000);"
  for i in $(seq 0 99); do
    printf "COPY (SELECT * FROM li WHERE rowid >= %d AND rowid < %d) TO '%s/files100/part-%04d.parquet' (FORMAT parquet, ROW_GROUP_SIZE 1000000);\n" $((i * PER)) $(((i + 1) * PER)) "$OUT" "$i"
  done
} > "$SQL"
mkdir -p "$OUT/big1" "$OUT/mid1" "$OUT/small1" "$OUT/files100"
duckdb -bail < "$SQL"
touch "$OUT/.done"
echo "layouts written to $OUT ($NROWS rows)"
