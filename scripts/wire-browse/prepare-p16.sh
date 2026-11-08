#!/usr/bin/env bash
set -euo pipefail
: "${WORK:?}"
root=$(cd "$(dirname "$0")/../.." && pwd)
bash "$root/scripts/wire-browse/check-close-package.sh"
# Paired baseline is this task's stated starting main; record its immutable SHA.
cd "$root/.slice-close"
sha256sum -c baseline.sha256
cp baseline.commit "$RESULTS/p16-baseline.commit"
mkdir "$WORK/p16-baseline-source"
tar -xzf baseline.tar.gz -C "$WORK/p16-baseline-source"
cd "$root"
CARGO_TARGET_DIR="$WORK/target-p16-baseline" cargo build --manifest-path "$WORK/p16-baseline-source/Cargo.toml" --locked --release -p nata -j 4
CARGO_TARGET_DIR="$WORK/target-p16" cargo build --locked --release -p nata -j 4
