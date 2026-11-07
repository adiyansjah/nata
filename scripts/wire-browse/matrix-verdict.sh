#!/usr/bin/env bash
set -euo pipefail
: "${WORK:?}" "${RESULTS:?}"
set +e
CARGO_BUILD_JOBS=4 bash scripts/wire-skeleton/run.sh
matrix_exit=$?
set -e
printf '%s\n' "$matrix_exit" > "$RESULTS/wire-skeleton/matrix.exit"
[ "$matrix_exit" -le 1 ] || exit "$matrix_exit"
python3 scripts/wire-skeleton/check-deviations.py "$RESULTS/wire-skeleton/matrix.json"
