#!/usr/bin/env bash
# mk-layouts.sh <lineitem.parquet> <out-dir> <sf1|sf10|trial> : the file layouts of the reference run, written under <out-dir>/<sfN>/<layout>/part-NNNN.parquet.
#   sf1:   big1 (1.5M-row groups), mid1 (122,880), small1 (20,000), files100 (100 files)   (the layouts of the earlier object-store spikes)
#   sf10:  mid10 (one file, 122,880-row groups), files10 (10 files, 1M-row groups)
#   trial: the same shapes at 1/20 of the row-group sizes, for a local trial on TPC-H SF0.1 (names as sf1, under <out-dir>/sf1)
set -euo pipefail
LI=${1:?usage: mk-layouts.sh <lineitem.parquet> <out-dir> <sf1|sf10|trial>}
OUT=${2:?}; KIND=${3:?}
command -v duckdb >/dev/null || { echo "duckdb is not on PATH" >&2; exit 4; }
unset AI_AGENT
NROWS=$(duckdb -noheader -list -c "SELECT count(*) FROM read_parquet('$LI')")
copy() { echo "COPY (SELECT * FROM src) TO '$1' (FORMAT parquet, ROW_GROUP_SIZE $2);"; }
filespart() { # dir nfiles rg
  local n=$2 per=$(( (NROWS + $2 - 1) / $2 ))
  for i in $(seq 0 $((n - 1))); do
    printf "COPY (SELECT * FROM src WHERE rowid >= %d AND rowid < %d) TO '%s/part-%04d.parquet' (FORMAT parquet, ROW_GROUP_SIZE %d);\n" $((i * per)) $(((i + 1) * per)) "$1" "$i" "$3"
  done
}
SQL=$(mktemp "${TMPDIR:-/tmp}/mk-layouts.XXXXXX.sql")
trap 'rm -f "$SQL"' EXIT
{
  echo "CREATE TABLE src AS SELECT * FROM read_parquet('$LI');"
  case "$KIND" in
    sf1)   D="$OUT/sf1"; mkdir -p "$D"/{big1,mid1,small1,files100}
           copy "$D/big1/part-0000.parquet" 1500000; copy "$D/mid1/part-0000.parquet" 122880; copy "$D/small1/part-0000.parquet" 20000
           filespart "$D/files100" 100 1000000 ;;
    sf10)  D="$OUT/sf10"; mkdir -p "$D"/{mid10,files10}
           copy "$D/mid10/part-0000.parquet" 122880; filespart "$D/files10" 10 1000000 ;;
    trial) D="$OUT/sf1"; mkdir -p "$D"/{big1,mid1,small1,files100}
           copy "$D/big1/part-0000.parquet" 75000; copy "$D/mid1/part-0000.parquet" 6144; copy "$D/small1/part-0000.parquet" 1000
           filespart "$D/files100" 100 50000 ;;
    *) echo "unknown kind $KIND" >&2; exit 2 ;;
  esac
} > "$SQL"
duckdb -bail < "$SQL"
echo "layouts written under $OUT ($NROWS rows, kind $KIND)"
