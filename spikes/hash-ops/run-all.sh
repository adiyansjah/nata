#!/usr/bin/env bash
# Everything the hash-ops spike claims, in order, stopping at the first failure:
#   1 cargo test --release          property + kernel tests (engine vs scalar reference)
#   2 check.sh                      7 queries, engine at 1/4/8 threads vs DuckDB, digit for digit
#   3 nullcheck                     10/50/100% NULLs on every column read, engine vs scalar reference
#   4 mutate.py                     every deliberate breakage must be caught by one of the checks above
#   5 rss                           peak memory of the 1.5M-group aggregate and the fan-out join
#   6 bench                         engine vs hand-fused, end to end from Parquet
#   7 membench (25 rounds)          the same with the batches already decoded in memory
#   8 duckdb-time.sh                DuckDB on the same queries, same threads, first run dropped
# Steps 1-4 are correctness and must pass before any number from steps 5-8 means anything.
#
# Needs only `cargo` and `duckdb` on PATH. Data: data/tpch-sf1 (generated with scripts/gen-tpch.sh 1 if absent).
# Run it on a QUIET machine for steps 6-8: it prints the load average before each and warns above 0.5 per core.
# Usage: run-all.sh [rounds (default 25)]      Logs and a summary go to target/run-all/<timestamp>/.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
ROUNDS=${1:-25}
DATA="$ROOT/data/tpch-sf1"
OUT="$HERE/target/run-all/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$OUT"
cd "$HERE"

for tool in cargo duckdb; do
  command -v "$tool" >/dev/null || { echo "error: $tool not found on PATH" >&2; exit 2; }
done
if [ ! -f "$DATA/lineitem.parquet" ]; then
  echo "generating $DATA with scripts/gen-tpch.sh (needs the duckdb tpch extension)"
  "$ROOT/scripts/gen-tpch.sh" 1 "$DATA"
fi

{
  echo "date:     $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "host:     $(uname -srm)"
  echo "cpu:      $(grep -m1 -E 'model name|Model name' /proc/cpuinfo 2>/dev/null || lscpu 2>/dev/null | grep -m1 'Model name' || echo unknown) ($(nproc) cores)"
  echo "memory:   $(awk '/MemTotal/ {printf "%.1f GiB", $2/1048576}' /proc/meminfo)"
  echo "rustc:    $(rustc --version)"
  echo "duckdb:   $(env -u AI_AGENT -u CLAUDECODE duckdb -csv -noheader -c 'select version();' | tail -1)"
  echo "commit:   $(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)$(git -C "$ROOT" diff --quiet -- spikes/hash-ops 2>/dev/null || echo ' (+uncommitted changes in spikes/hash-ops)')"
  echo "load:     $(cut -d' ' -f1-3 /proc/loadavg)"
} | tee "$OUT/machine.txt"

step() {  # step <name> <command...>   (output to the screen and to $OUT/<name>.log)
  local name=$1; shift
  echo; echo "=== $name  ($(date +%H:%M:%S), load $(cut -d' ' -f1 /proc/loadavg)) ==="
  local t0=$SECONDS
  if "$@" 2>&1 | tee "$OUT/$name.log"; then
    echo "--- $name OK in $((SECONDS - t0)) s" | tee -a "$OUT/summary.txt"
  else
    echo "--- $name FAILED (log: $OUT/$name.log); stopping" | tee -a "$OUT/summary.txt"
    exit 1
  fi
}

quiet() {  # warn when the machine is not quiet; numbers taken now are not evidence
  local l c; l=$(cut -d' ' -f1 /proc/loadavg); c=$(nproc)
  awk -v l="$l" -v c="$c" 'BEGIN { if (l > 0.5 * c) printf "WARNING: load average %s on %d cores: the numbers below are NOT evidence\n", l, c }'
}

cargo build --release --quiet
BIN="$HERE/target/release/hash-ops"

step 1-tests      cargo test --release
step 2-check      "$HERE/check.sh" "$DATA"
step 3-nullcheck  "$BIN" nullcheck "$DATA"
step 4-mutate     python3 "$HERE/mutate.py"

rss() {
  for spec in "agg_orderkey engine" "agg_orderkey fused" "j3 engine" "j3 fused"; do
    set -- $spec
    if [ -x /usr/bin/time ]; then /usr/bin/time -v "$BIN" rss "$DATA" "$1" "$2" 4 2>&1 | grep -E "groups|Maximum resident|Elapsed"
    else "$BIN" rss "$DATA" "$1" "$2" 4; fi
  done
}
step 5-rss        rss

quiet
step 6-bench      "$BIN" bench "$DATA" "$ROUNDS"
quiet
step 7-membench   "$BIN" membench "$DATA" "$ROUNDS"
quiet
step 8-duckdb     "$HERE/duckdb-time.sh" "$DATA" 16

echo; echo "ALL STEPS PASSED. Logs in $OUT"; cat "$OUT/summary.txt"
