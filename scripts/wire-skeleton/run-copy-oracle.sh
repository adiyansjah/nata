#!/usr/bin/env bash
set -euo pipefail
: "${WORK:?}" "${RESULTS:?}"
root=$(cd "$(dirname "$0")/../.." && pwd)
export TMPDIR="$WORK/wire-skeleton-tmp"
mkdir -p "$TMPDIR"
# The default-client runner creates this environment first. Use a distinct
# work directory per invocation; initdb refuses an existing cluster.
pg_bin=$(pg_config --bindir)
"$WORK/wire-skeleton-venv/bin/python" "$root/scripts/wire-skeleton/compare-copy.py" \
  --nata "${CARGO_TARGET_DIR:-$root/target}/debug/nata" \
  --work "$RESULTS/copy-oracle" --pg-bin "$pg_bin"
