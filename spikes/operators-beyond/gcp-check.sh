#!/bin/bash
# Verification-plan item 31 on a clean VM (GCP): build, unit tests against naive references, the DuckDB differential check of every query
# (in memory and with spill, 1/2/4/8 tasks), the mutation check. Exit 0 only if every gating phase passed.
#   usage: spikes/operators-beyond/gcp-check.sh <results-dir> [tpch-dir]      (default tpch-dir: <repo>/data/tpch-sf1)
# Needs cargo and the duckdb CLI on PATH, and TPC-H SF1 Parquet (scripts/gen-tpch.sh 1 data/tpch-sf1).
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); OUT=${1:?usage: gcp-check.sh <results-dir> [tpch-dir]}; mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)
DATA=${2:-$HERE/../../data/tpch-sf1}
cd "$HERE" || exit 1
SUMMARY=$OUT/summary.txt; : > "$SUMMARY"; FAILED=0
log() { printf '%s\n' "$*" | tee -a "$SUMMARY"; }
phase() { # name cmd...
  local n=$1; shift; local t0=$SECONDS
  if "$@" > "$OUT/$n.txt" 2>&1; then log "PASS $n ($((SECONDS - t0)) s)"; else log "FAIL $n ($((SECONDS - t0)) s), see $n.txt"; FAILED=1; fi
}
log "host: $(uname -m), $(nproc) vCPU, $(awk '/MemTotal/{printf "%d MiB",$2/1024}' /proc/meminfo); $(rustc --version); $(duckdb --version)"
phase build cargo build --release --locked
phase unit-tests cargo test --release --locked
export OB_THREADS=$(nproc)
phase differential ./check.sh "$DATA" "$OUT/check"
cp "$OUT/check/summary.txt" "$OUT/differential-summary.txt" 2>/dev/null
for f in "$OUT"/check/*.stats.txt; do cat "$f"; done > "$OUT/stats.txt" 2>/dev/null
phase mutants python3 -I mutate.py --work "$OUT/mutwork"
rm -rf "$OUT/mutwork" "$OUT"/check/*.parquet "$OUT"/check/spill-*   # results stay small: the orchestrator pulls this directory
# informational, NOT a benchmark: DuckDB's own time for the same queries on this machine, one run each, warm cache
for q in q18 q21 q13 sort; do
  /usr/bin/time -f "$q duckdb %e s" duckdb -c "create view customer as select * from '$DATA/customer.parquet'; create view orders as select * from '$DATA/orders.parquet'; create view lineitem as select * from '$DATA/lineitem.parquet'; create view supplier as select * from '$DATA/supplier.parquet'; create view nation as select * from '$DATA/nation.parquet'; create table r as $(cat sql/$q.sql);" >/dev/null 2>> "$OUT/duckdb-times.txt"
done
[ $FAILED = 0 ] && log "VERDICT: all gating phases passed" || log "VERDICT: FAILED"
exit $FAILED
