#!/usr/bin/env bash
# LOCAL preparation only; no cloud calls, no builds, no secrets in the archive.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
steps=scripts/wire-browse/slice-close.steps.example
if [ "$#" -ne 0 ]; then
    [ "$#" -eq 2 ] && [ "$1" = --steps ] || { echo 'usage: package-close.sh [--steps STEPS_FILE]' >&2; exit 2; }
    steps=$2
fi
[ -s "$steps" ] || { echo "missing steps file: $steps" >&2; exit 1; }
mkdir -p .slice-close
git archive --format=tar.gz a3e8e3b > .slice-close/baseline.tar.gz
(cd .slice-close && sha256sum baseline.tar.gz > baseline.sha256)
git rev-parse a3e8e3b > .slice-close/baseline.commit
bash scripts/wire-browse/check-close-package.sh
cp "$steps" heavy.steps
