#!/bin/bash
# Everything for the arrow-pipeline spike, for a run on a QUIET machine (a GCP VM, x86-64 or ARM64):
#   1. build (release, overflow-checks ON) and verify that overflow-checks is on
#   2. all tests (Q1/Q6 vs generic-ops and vs the scalar NULL reference, LIMIT / early-finish properties,
#      exchange, stress under CPU contention)
#   3. Q1 and Q6 against DuckDB, digit for digit (scripts/duckdb_diff.sh)
#   4. check and nullcheck from the binary
#   5. a longer stress (more repetitions, extra busy threads)
#   6. the mutation check (private copy; every mutant must be caught)
#   7. the benchmark: protocol scheduler vs the generic-ops engine, Q1 and Q6, 1/2/4/8 threads, interleaved with a
#      control arm (protocol against itself): end to end and engine only
#
# Usage: ./run-all.sh [lineitem.parquet] [bench-rounds]       Needs: cargo, duckdb on PATH, the TPC-H SF1 lineitem file.
# Output: out/<timestamp>/*.log (one file per step) and a summary on stdout. Exit status != 0 if any step fails.
# Speed numbers are only evidence on a quiet machine: step 7 prints the load average before and after.
set -u
cd "$(dirname "$0")"
LI=$(realpath "${1:-../../data/tpch-sf1/lineitem.parquet}")
ROUNDS=${2:-25}
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}
export LINEITEM=$LI
OUT=out/$(date +%Y%m%d-%H%M%S); mkdir -p "$OUT"
status=0
step() { # name command...
  local name=$1; shift
  echo "=== $name"; local t=$(date +%s)
  if "$@" > "$OUT/$name.log" 2>&1; then echo "    PASS ($(( $(date +%s) - t ))s)  -> $OUT/$name.log"
  else echo "    FAIL ($(( $(date +%s) - t ))s)  -> $OUT/$name.log"; tail -15 "$OUT/$name.log" | sed 's/^/      /'; status=1; fi
}
{ echo "host: $(uname -srm)  cpus: $(nproc)  load: $(cut -d' ' -f1-3 /proc/loadavg)"; rustc --version; env -u AI_AGENT duckdb --version; grep -n 'overflow-checks' Cargo.toml; } | tee "$OUT/env.log"
LOAD1=$(cut -d' ' -f1 /proc/loadavg); awk -v l="$LOAD1" 'BEGIN{ if (l+0 > 1.0) print "WARNING: load average " l " > 1: do not use the benchmark numbers (steps 1-6 are still valid)" }'

step 1-build cargo build --release
step 2-tests cargo test --release --no-fail-fast -- --nocapture --test-threads=1
step 3-duckdb-diff scripts/duckdb_diff.sh "$LI"
step 4-check target/release/spike-arrow-pipeline check "$LI"
step 4-nullcheck target/release/spike-arrow-pipeline nullcheck "$LI"
STRESS_REPS=3000 STRESS_EXTRA_BUSY=4 WATCHDOG_SECS=60 step 5-stress cargo test --release --test stress -- --nocapture --test-threads=1
LIMIT_CASES=10000 step 5-limit-many cargo test --release --test limit -- --nocapture --test-threads=1
# the private copy and target dir live under the output dir (not /tmp, which can be a small tmpfs) and are deleted at the end
step 6-mutation env MUT_ROOT="${MUT_ROOT:-$PWD/$OUT/mut}" python3 scripts/mutate.py
echo "=== 7-bench (sanity of the numbers requires a quiet machine; load now: $(cut -d' ' -f1-3 /proc/loadavg))"
target/release/spike-arrow-pipeline bench    "$LI" "$ROUNDS" | tee "$OUT/7-bench-end-to-end.log"
target/release/spike-arrow-pipeline membench "$LI" "$ROUNDS" | tee "$OUT/7-bench-engine-only.log"
echo "=== done; logs in $OUT; status $status"
exit $status
