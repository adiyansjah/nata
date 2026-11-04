#!/usr/bin/env bash
# Time DuckDB on the same 7 queries as the engine (sql/<name>.sql), in-process timer, 1/2/4/8 threads.
# Usage: duckdb-time.sh [data-dir] [runs (default 16)] [threads list (default "1 2 4 8")]
#
# Two data modes, matching the engine's two benchmarks:
#   parquet  views over the Parquet files: Parquet decode is inside the timing (engine mode `bench`)
#   memory   tables created beforehand in an in-memory database: decode is outside the timing (`membench`)
# The result is not printed (printing 1.5M rows would dominate): each run computes
#   SELECT count(*), sum(hash(t)) FROM (<query>) t
# which needs every output column of the query, so nothing can be pruned; it adds the cost of hashing the
# result rows (small next to the aggregate; stated, not measured here).
# Per (mode, query, threads) one duckdb process runs the query RUNS times; the FIRST run in time order is
# dropped (cold file cache, lazy initialisation), never the fastest. min and median of the rest are printed.
set -u
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
DATA="$(cd "${1:-$ROOT/data/tpch-sf1}" && pwd)"
RUNS=${2:-16}
THREADS=${3:-"1 2 4 8"}
QUERIES=(agg_orderkey agg_partkey agg_modedate agg_comment q3 q12 j3)

dd() { env -u AI_AGENT -u CLAUDECODE duckdb -csv "$@"; }

VIEWS="" TABLES=""
for t in customer orders lineitem; do
  VIEWS+="CREATE OR REPLACE VIEW $t AS SELECT * FROM read_parquet('$DATA/$t.parquet');"
  TABLES+="CREATE OR REPLACE TABLE $t AS SELECT * FROM read_parquet('$DATA/$t.parquet');"
done

echo "duckdb $(dd -c 'select version();' | tail -1); runs=$RUNS (first dropped); load average at start: $(cut -d' ' -f1-3 /proc/loadavg)"
for mode in parquet memory; do
  setup=$VIEWS; [ "$mode" = memory ] && setup=$TABLES
  for q in "${QUERIES[@]}"; do
    body="$(sed 's/;[[:space:]]*$//' "$HERE/sql/$q.sql")"
    wrapped="SELECT count(*), sum(hash(t)) FROM ($body) t;"
    for n in $THREADS; do
      args=(-c "$setup" -c "SET threads=$n;" -c ".timer on")
      for _ in $(seq 1 "$RUNS"); do args+=(-c "$wrapped"); done
      dd "${args[@]}" 2>&1 | grep -oE "real [0-9.]+" | awk '{print $2*1000}' \
        | tail -n +2 | sort -n | awk -v m=$mode -v q=$q -v n=$n \
          '{a[NR]=$1} END{ if (NR==0) {printf "%-8s %-13s threads=%d  NO TIMINGS PARSED\n", m, q, n; exit 1}
             printf "%-8s %-13s threads=%d  min %.0f ms  median %.0f ms  (n=%d)\n", m, q, n, a[1], a[int((NR+1)/2)], NR}' \
        || { echo "duckdb timing failed for $mode $q $n" >&2; exit 1; }
    done
  done
done
echo "load average at end: $(cut -d' ' -f1-3 /proc/loadavg)"
