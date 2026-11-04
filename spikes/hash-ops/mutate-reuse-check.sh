#!/usr/bin/env bash
# Regression check for a defect of the parallel mutation driver (found on GCP, 2026-11-04): two mutate.py runs that
# share one private root (a worker's scratch directory) must both pass their baseline. With restored files that kept
# their old mtime, cargo reused the first run's mutant binary and the second baseline failed.
# Usage: mutate-reuse-check.sh [data-dir]   (needs the data for mutate.py's argument parsing only; --layer tests runs no queries)
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"
R=$(mktemp -d "${TMPDIR:-/tmp}/ho-reuse.XXXXXX"); trap 'rm -rf "$R"' EXIT
export HASH_OPS_MUT_ROOT=$R HASH_OPS_DATA="${1:-../../data/tpch-sf1}"
python3 -I mutate.py --layer tests "fan-out: every match emitted twice" | tail -3
python3 -I mutate.py --layer tests "aggregate ignores the selection vector" | tail -3
# the copy must keep check.sh executable (the duckdb layer runs ./check.sh; a copy that lost the mode failed on GCP)
python3 -I mutate.py --dry "aggregate ignores the selection vector" > /dev/null
[ -x "$R/work/check.sh" ] || { echo "FAIL: check.sh lost its executable bit in the private copy"; exit 1; }
echo "reuse check passed: the second baseline did not see the first mutant"
