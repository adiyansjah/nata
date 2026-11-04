#!/usr/bin/env bash
# Regression check for the mutation script (found 2026-11-04): two mutate.py invocations in a row must both pass their
# unmutated baseline. With the source copied with its OLD mtimes, cargo reused the previous invocation's last mutant from
# the mutation target directory, and the second baseline failed. Run it through scripts/local-heavy.sh on the shared box.
# Usage: mutate-reuse-check.sh <lineitem.parquet>     (about 4 minutes; unit-test layer only)
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
LI=$(realpath "${1:?usage: mutate-reuse-check.sh <lineitem.parquet>}")
for i in 1 2; do
  out=$(HEAVY="${HEAVY:-}" MUTATE_SKIP_E2E=1 MUTATE_TEST=non_null_counts python3 -I mutate.py "$LI" "non-NULL count" "NULL counter" 2>&1 | grep -E "baseline|mutants:")
  echo "invocation $i: $out" | tr '\n' ' '; echo
  echo "$out" | grep -q "baseline (unmutated copy) fails at: None" || { echo "FAIL: the baseline of invocation $i did not pass"; exit 1; }
  echo "$out" | grep -q "4 caught by unit tests, 0 caught only end to end, 0 survived" || { echo "FAIL: not all 4 mutants were caught"; exit 1; }
done
echo "reuse check passed: both baselines passed and all mutants were caught"
