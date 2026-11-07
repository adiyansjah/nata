#!/usr/bin/env bash
# LOCAL preparation only; no cloud calls, no builds, no secrets in the archive.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
mkdir -p .slice-close
git archive --format=tar.gz a3e8e3b > .slice-close/baseline.tar.gz
(cd .slice-close && sha256sum baseline.tar.gz > baseline.sha256)
git rev-parse a3e8e3b > .slice-close/baseline.commit
cp scripts/wire-browse/slice-close.steps.example heavy.steps
