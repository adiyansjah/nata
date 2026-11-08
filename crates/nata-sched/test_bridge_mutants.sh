#!/usr/bin/env bash
# Exercise the verdict harness without compiling: tool availability and INVALID
# outcomes must never be mistaken for a caught concurrency mutant.
set -euo pipefail
cd "$(dirname "$0")/../.."
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/crates/nata-sched" "$work/bin"
cp crates/nata-sched/check_bridge_mutants.sh "$work/crates/nata-sched/"
# Reproduce a runner without ripgrep, even on a developer machine with it.
printf '#!/bin/sh\nexit 127\n' > "$work/bin/rg"
chmod +x "$work/bin/rg"
cat > "$work/crates/nata-sched/run_bridge_loom.sh" <<'DRIVER'
#!/usr/bin/env bash
if [ -z "${BRIDGE_MUTANT:-}" ]; then exit "${CONTROL_RC:-0}"; fi
printf '%s\n' "$BRIDGE_MUTANT" >> "$CALLS"
printf '%s\n' "${DIAGNOSTIC:-deadlock; threads = blocked}"
exit "${MUTANT_RC:-101}"
DRIVER
export PATH="$work/bin:$PATH" CALLS="$work/calls"
check() {
    local want=$1 verdict=$2 rc=0
    shift 2
    : > "$CALLS"
    env "$@" bash "$work/crates/nata-sched/check_bridge_mutants.sh" > "$work/log" 2>&1 || rc=$?
    if [ "$rc" -ne "$want" ] || { [ -n "$verdict" ] && ! grep -q "$verdict" "$work/log"; }; then
        cat "$work/log"
        echo "expected exit $want, verdict $verdict; got $rc" >&2
        exit 1
    fi
}
check 0 'bridge-drop-cancel CAUGHT' MUTANT_RC=101
[ "$(wc -l < "$CALLS")" -eq 2 ]
check 0 'bridge-drop-cancel CAUGHT' MUTANT_RC=134 DIAGNOSTIC='assertion left == right failed'
check 1 'bridge-drop-wake SURVIVED' MUTANT_RC=0
check 1 'bridge-drop-wake INVALID' MUTANT_RC=124
check 1 'bridge-drop-wake INVALID' MUTANT_RC=101 DIAGNOSTIC='could not compile nata-sched'
check 1 'bridge-drop-wake INVALID' MUTANT_RC=127
check 124 '' CONTROL_RC=124
[ ! -s "$CALLS" ]
echo 'bridge mutant harness: PASS (caught, survived, invalid, clean-control failure; no ripgrep)'
