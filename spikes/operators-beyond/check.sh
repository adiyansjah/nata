#!/bin/bash
# Differential check of every query against DuckDB. Usage: check.sh <tpch dir> <results dir> [--quick]
# For each query and mode: run opsbeyond, write its result as Parquet, then (1) the two result sets must be equal as multisets (EXCEPT ALL both
# ways, equal row counts) and (2) for ordered queries the order-by columns must match row by row. Spill modes must leave no spill files.
# The result sets of the join-count queries have a constant extra column `g` that DuckDB does not have; it is dropped before the comparison.
# Exit 0 only if every comparison passed. Needs the duckdb CLI on PATH.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); DATA=${1:?tpch dir}; OUT=${2:?results dir}; QUICK=${3:-}
mkdir -p "$OUT"; DATA=$(cd "$DATA" && pwd); OUT=$(cd "$OUT" && pwd)
BIN=$HERE/target/release/opsbeyond; [ -x "$BIN" ] || { echo "build first: cargo build --release"; exit 2; }
FAILED=0; SUM=$OUT/summary.txt; : > "$SUM"
log(){ printf '%s\n' "$*" | tee -a "$SUM"; }
views="create view customer as select * from '$DATA/customer.parquet'; create view orders as select * from '$DATA/orders.parquet'; create view lineitem as select * from '$DATA/lineitem.parquet'; create view supplier as select * from '$DATA/supplier.parquet'; create view nation as select * from '$DATA/nation.parquet';"
# name | order-by columns to compare row by row ('-' = unordered)
check() { # name tag cols args...
  local q=$1 tag=$2 cols=$3; shift 3
  local ours=$OUT/$tag.parquet exp=$OUT/$tag.expected.parquet
  local line; line=$("$BIN" "$q" --data "$DATA" --out "$ours" --spill-dir "$OUT/spill-$tag" "$@" 2>&1 | tail -1)
  echo "$line" > "$OUT/$tag.stats.txt"
  if ! grep -q "status=ok" <<<"$line"; then log "FAIL $tag: $line"; FAILED=1; return; fi
  if ! grep -q "leftover_spill_files=0" <<<"$line"; then log "FAIL $tag: spill files left behind: $line"; FAILED=1; return; fi
  duckdb -c "$views copy ($(cat "$HERE/sql/$q.sql")) to '$exp' (format parquet);" >/dev/null 2>&1 || { log "FAIL $tag: duckdb could not run the reference"; FAILED=1; return; }
  local drop=""; [[ $q == join-* ]] && drop="* exclude (g)"; [ -z "$drop" ] && drop="*"
  local res
  res=$(duckdb -noheader -list -c "
    create view o as select $drop from '$ours'; create view e as select * from '$exp';
    select (select count(*) from o) || ' ' || (select count(*) from e) || ' ' || (select count(*) from (select * from o except all select * from e)) || ' ' || (select count(*) from (select * from e except all select * from o));" 2>&1 | tail -1)
  read -r no ne d1 d2 <<<"$res"
  if [ "${no:-x}" != "${ne:-y}" ] || [ "${d1:-1}" != 0 ] || [ "${d2:-1}" != 0 ]; then log "FAIL $tag: rows ours=$no expected=$ne only-ours=$d1 only-expected=$d2 ($res)"; FAILED=1; return; fi
  if [ "$cols" != "-" ]; then
    local cond; cond=$(tr ',' '\n' <<<"$cols" | awk '{printf "%sa.%s is distinct from b.%s", (NR>1?" or ":""), $1, $1}')
    local bad; bad=$(duckdb -noheader -list -c "select count(*) from read_parquet('$ours', file_row_number=true) a join read_parquet('$exp', file_row_number=true) b using (file_row_number) where $cond;" 2>&1 | tail -1)
    if [ "$bad" != 0 ]; then log "FAIL $tag: $bad rows differ in the order-by columns ($cols)"; FAILED=1; return; fi
  fi
  log "ok   $tag: $no rows, $(sed -e 's/.*secs=\([0-9.]*\).*spilled_mb=\([0-9]*\).*spill_files=\([0-9]*\) max_open_files=\([0-9]*\).*/\1 s, spilled \2 MB in \3 files, most open \4/' <<<"$line")"
}
T=${OB_THREADS:-4}
# OB_SPILL_ONLY=1 (the scale runs, where the in-memory modes do not fit): only the spill modes below
[ "${OB_SPILL_ONLY:-0}" = 1 ] && log "OB_SPILL_ONLY=1: the in-memory modes are skipped"
for mode in $([ "${OB_SPILL_ONLY:-0}" = 1 ] || echo default); do
  check q18 q18 "o_totalprice,o_orderdate" --threads $T
  check q21 q21 "numwait,s_name" --threads $T
  check q13 q13-mem "custdist,c_count" --threads $T
  for k in inner left right full semi anti; do check join-$k join-$k-mem - --threads $T; done
  check topn topn "l_extendedprice,l_orderkey,l_linenumber" --threads $T
  check sort sort "l_shipdate,l_orderkey,l_linenumber" --threads $T
  check window-peers window-peers - --threads $T
  check window-unique window-unique - --threads $T
done
# spill modes: small memory, partitioned joins, merge passes with a small fan-in, single task and many tasks
MEM=${OB_MEM_MB:-64}; GR=${OB_GRACE:-32}
check q13 q13-grace "custdist,c_count" --threads $T --mem-mb $MEM --grace $GR
for k in inner left right full semi anti; do check join-$k join-$k-grace - --threads $T --mem-mb $MEM --grace $GR; done
check sort sort-spill "l_shipdate,l_orderkey,l_linenumber" --threads $T --mem-mb $MEM --fanin 4
check sort sort-spill-1task "l_shipdate,l_orderkey,l_linenumber" --threads 1 --tasks 1 --mem-mb $MEM --fanin 2
check window-peers window-peers-spill - --threads $T --mem-mb $MEM --fanin 3
check window-unique window-unique-spill - --threads $T --mem-mb $MEM --fanin 3
if [ "$QUICK" != --quick ] && [ "${OB_SPILL_ONLY:-0}" != 1 ]; then
  for t in 1 2 8; do check q21 q21-t$t "numwait,s_name" --threads $t --tasks $t; check q18 q18-t$t "o_totalprice,o_orderdate" --threads $t --tasks $t; done
fi
[ $FAILED = 0 ] && log "VERDICT: all comparisons passed" || log "VERDICT: FAILED"
exit $FAILED
