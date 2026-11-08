#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
# Build failure/timeout is INVALID, never evidence that a mutant was caught.
bash crates/nata-sched/run_bridge_loom.sh
for mutant in bridge-drop-wake bridge-drop-cancel; do
    log=$(mktemp)
    rc=0
    if BRIDGE_MUTANT="$mutant" bash crates/nata-sched/run_bridge_loom.sh >"$log" 2>&1; then
        cat "$log"; rm "$log"; echo "$mutant SURVIVED"; exit 1
    else
        rc=$?
    fi
    if { [ "$rc" -ne 101 ] && [ "$rc" -ne 134 ]; } || ! grep -Eq 'deadlock|assertion.*failed' "$log"; then
        cat "$log"; rm "$log"; echo "$mutant INVALID"; exit 1
    fi
    rm "$log"; echo "$mutant CAUGHT (exit $rc)"
done
