#!/bin/bash
# Q1 and Q6 on the protocol scheduler against DuckDB, digit for digit (exact decimals), at 1, 4 and 8 threads
# and with tasks = threads and 2 x threads. Usage: scripts/duckdb_diff.sh [lineitem.parquet]
set -u
cd "$(dirname "$0")/.."
F=$(realpath "${1:-../../data/tpch-sf1/lineitem.parquet}")
BIN=${BIN:-target/release/spike-arrow-pipeline}
[ -x "$BIN" ] || CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2} cargo build --release -q || exit 2
Q1="SELECT l_returnflag, l_linestatus, sum(l_quantity), sum(l_extendedprice), sum(l_extendedprice*(1-l_discount)), count(*) FROM '$F' WHERE l_shipdate <= DATE '1998-09-02' GROUP BY 1,2 ORDER BY 1,2;"
Q6="SELECT sum(l_extendedprice*l_discount) FROM '$F' WHERE l_shipdate >= DATE '1994-01-01' AND l_shipdate < DATE '1995-01-01' AND l_discount BETWEEN 0.05 AND 0.07 AND l_quantity < 24;"
fail=0; n=0
for q in q1 q6; do
  want_q=$([ $q = q1 ] && echo "$Q1" || echo "$Q6")
  want=$(env -u AI_AGENT duckdb -noheader -csv -c "$want_q") || { echo "duckdb failed"; exit 2; }
  for t in 1 4 8; do for mult in 1 2; do
    tasks=$((t*mult))
    got=$("$BIN" csv "$F" "$q" "$t" "$tasks" 2>/dev/null)
    n=$((n+1))
    if [ "$got" == "$want" ]; then echo "ok    $q threads=$t tasks=$tasks identical to DuckDB ($(echo "$want" | head -1 | cut -c1-70))"
    else echo "DIFF  $q threads=$t tasks=$tasks"; diff <(echo "$want") <(echo "$got"); fail=1; fi
  done; done
done
echo "$n configurations compared"
exit $fail
