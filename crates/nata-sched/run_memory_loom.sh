#!/usr/bin/env bash
# Real memory service, one model at a time. Bound 2 is a short local trial;
# bound 3 belongs to the parent's heavy job. Nonzero includes timeout/build errors.
set -euo pipefail
cd "$(dirname "$0")/../.."
export RUSTFLAGS="--cfg nata_loom"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target/loom-memory}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export LOOM_MAX_PREEMPTIONS="${LOOM_MAX_PREEMPTIONS:-2}"
binary=$(cargo test --locked -p nata-sched --test memory --no-run --message-format=json | python3 -c '
import json, sys
artifacts = [json.loads(line) for line in sys.stdin]
executables = [a["executable"] for a in artifacts if a.get("reason") == "compiler-artifact" and a.get("executable")]
assert len(executables) == 1, "expected exactly one current memory test executable"
print(executables[0])
')
[ -n "$binary" ]
while read -r name; do
    timeout "${MEMORY_MODEL_TIMEOUT:-120}" "$binary" "$name" --exact --nocapture
done < <("$binary" --list | sed -n 's/: test$//p')
