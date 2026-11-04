#!/bin/bash
# Verification-plan item 33 on a clean VM (GCP): the fuzzer's own tests, `prove` (control clean, all 28 known-bad engines caught) on three seeds,
# and long clean control campaigns (DuckDB against DuckDB: any difference means a nondeterministic oracle or a fuzzer bug).
#   usage: spikes/sqlfuzz/gcp-check.sh <results-dir> [control-queries-per-seed]     Needs python3 and the duckdb CLI.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); OUT=${1:?usage: gcp-check.sh <results-dir> [control-queries]}; mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)
N=${2:-60000}; J=$(nproc); cd "$HERE" || exit 1
SUMMARY=$OUT/summary.txt; : > "$SUMMARY"; FAILED=0
log() { printf '%s\n' "$*" | tee -a "$SUMMARY"; }
phase() { local n=$1; shift; local t0=$SECONDS; if "$@" > "$OUT/$n.txt" 2>&1; then log "PASS $n ($((SECONDS - t0)) s)"; else log "FAIL $n ($((SECONDS - t0)) s), see $n.txt"; FAILED=1; fi; }
log "host: $(uname -m), $J vCPU; $(python3 --version); duckdb $(env -u AI_AGENT duckdb --version)"
phase tests python3 -I tests/test_sqlfuzz.py
for s in 1 2 3; do phase prove-seed$s python3 -I fuzz.py prove --seed $s --cases 6000 --control 4000 --jobs "$J" --out "$OUT/repro-seed$s"; done
for s in 101 102; do phase control-seed$s python3 -I fuzz.py run --seed $s --cases "$N" --jobs "$J" --shrink 5 --out "$OUT/control-diffs-$s"; done
grep -h "^queries" "$OUT"/control-seed*.txt | tee -a "$SUMMARY" > /dev/null
cp "$OUT"/prove-seed*.txt "$OUT"/ 2>/dev/null
[ $FAILED = 0 ] && log "VERDICT: all gating phases passed" || log "VERDICT: FAILED"
exit $FAILED
