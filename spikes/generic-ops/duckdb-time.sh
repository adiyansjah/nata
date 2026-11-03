#!/bin/bash
# Time DuckDB on the two experiment queries, in-process timer, 1/2/4/8 threads.
# Usage: duckdb-time.sh <lineitem.parquet> [runs]   -- the first run of each batch (cold) is dropped, in time order.
set -u
F=$(realpath "$1"); RUNS=${2:-16}
Q1="SELECT l_returnflag, l_linestatus, sum(l_quantity), sum(l_extendedprice), sum(l_extendedprice*(1-l_discount)), count(*) FROM '$F' WHERE l_shipdate <= DATE '1998-09-02' GROUP BY 1,2;"
Q6="SELECT sum(l_extendedprice*l_discount) FROM '$F' WHERE l_shipdate >= DATE '1994-01-01' AND l_shipdate < DATE '1995-01-01' AND l_discount BETWEEN 0.05 AND 0.07 AND l_quantity < 24;"
for q in Q1 Q6; do for n in 1 2 4 8; do
  args=(-c "SET threads=$n;" -c ".timer on"); for _ in $(seq 1 "$RUNS"); do args+=(-c "${!q}"); done
  env -u AI_AGENT duckdb -csv "${args[@]}" 2>&1 | grep -oE "real [0-9.]+" | awk '{print $2*1000}' \
    | tail -n +2 | sort -n | awk -v q=$q -v n=$n '{a[NR]=$1} END{printf "%s threads=%d  min %.0f  median %.0f  (n=%d)\n", q, n, a[1], a[int((NR+1)/2)], NR}'
done; done
