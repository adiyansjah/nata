#!/usr/bin/env bash
# Also callable before upload; never contacts GCP.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
for input in baseline.tar.gz baseline.sha256 baseline.commit; do
    if [ ! -s ".slice-close/$input" ]; then
        echo "missing packaged .slice-close/$input; run: bash scripts/wire-browse/package-close.sh [--steps STEPS_FILE] before orchestrate.sh run" >&2
        exit 1
    fi
done
(cd .slice-close && sha256sum -c baseline.sha256)
grep -Eq '^[0-9a-f]{40}$' .slice-close/baseline.commit || { echo 'invalid packaged baseline.commit' >&2; exit 1; }
