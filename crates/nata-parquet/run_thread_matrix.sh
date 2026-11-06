#!/bin/bash
# The differential tests against DuckDB (and the random-file soundness property) at every thread count, ordered and unordered: the same test files
# as the single-thread step, with the scan run by `run_parallel` (NATA_SCAN_THREADS=N; NATA_SCAN_UNORDERED=1 compares as a multiset of rows).
# usage (from the repository root, one heavy command at a time):  scripts/local-heavy.sh crates/nata-parquet/run_thread_matrix.sh
# Env: THREADS="1 2 3 4 8", ORDERS="0 1" (0 = ordered, 1 = unordered). About 2 to 3 minutes per row on an 8-core machine.
set -u
cd "$(dirname "$0")/../.."
export NATA_REQUIRE_DUCKDB=1 CARGO_TERM_COLOR=never
cargo test --offline --release -p nata-parquet --no-run 2>&1 | tail -1
fail=0
for t in ${THREADS:-1 2 3 4 8}; do
  for u in ${ORDERS:-0 1}; do
    s=$(date +%s)
    out=$(NATA_SCAN_THREADS=$t NATA_SCAN_UNORDERED=$u cargo test --offline --release -p nata-parquet \
      --test differential_lineitem --test differential_edge --test special_files --test soundness_files 2>&1); rc=$?
    passed=$(echo "$out" | grep -E '^test result' | sed -E 's/.* ([0-9]+) passed; ([0-9]+) failed.*/\1 \2/' | awk '{p+=$1; f+=$2} END {print p" passed, "f" failed"}')
    printf 'threads=%s unordered=%s rc=%s  %s  (%ss)\n' "$t" "$u" "$rc" "$passed" "$(( $(date +%s) - s ))"
    [ "$rc" -ne 0 ] && { fail=1; echo "$out" | grep -E 'FAILED|panicked' | head -5; }
  done
done
exit $fail
