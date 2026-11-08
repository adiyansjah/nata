#!/usr/bin/env bash
# Production parking core, completions, fill subscribers and grants. Full
# five-thread scheduler exploration is opt-in for GCP (BRIDGE_COMBINED_IO=1).
set -euo pipefail
cd "$(dirname "$0")/../.."
export RUSTFLAGS="--cfg nata_loom"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target/loom-bridge}"
export LOOM_MAX_PREEMPTIONS="${LOOM_MAX_PREEMPTIONS:-2}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
profile=()
[ -z "${BRIDGE_RELEASE:-}" ] || profile=(--release)
features=()
[ -z "${BRIDGE_MUTANT:-}" ] || features=(--features "$BRIDGE_MUTANT")
binaries=$(cargo test --locked "${profile[@]}" -p nata-sched --lib --test bridge "${features[@]}" --no-run --message-format=json | python3 -c '
import json,sys
xs=[json.loads(l) for l in sys.stdin]
for x in xs:
    if x.get("reason")=="compiler-message": print(x["message"].get("rendered", ""), file=sys.stderr)
xs=[x["executable"] for x in xs if x.get("reason")=="compiler-artifact" and x.get("executable")]
assert len(xs)==2, "expected library and bridge test binaries"
print("\n".join(xs))')
ran=0
while read -r binary; do
    while read -r name; do
        if [ -n "${BRIDGE_ONLY:-}" ] && ! [[ "$name" =~ $BRIDGE_ONLY ]]; then continue; fi
        if [ "$name" = two_workers_two_waiters_keep_the_window_bound ] && [ -z "${BRIDGE_COMBINED_IO:-}" ]; then continue; fi
        ((ran+=1))
        timeout "${BRIDGE_MODEL_TIMEOUT:-180}" "$binary" "$name" --exact --include-ignored --nocapture
    done < <("$binary" --list | sed -n 's/: test$//p')
done <<< "$binaries"
((ran > 0))
