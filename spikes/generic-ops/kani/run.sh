#!/usr/bin/env bash
# Run every Kani harness one at a time (never in parallel), a time cap each; one line per harness.
# Usage: kani/run.sh [seconds-cap, default 600] [name-filter-regex]
cd "$(dirname "$0")" || exit 2
cap="${1:-600}"; filt="${2:-.}"
harnesses=$(grep -A8 '#\[kani::proof\]' src/lib.rs | grep -o 'fn [a-z0-9_]*()' | sed 's/fn //;s/()//' | grep -E "$filt")
rc=0
for h in $harnesses; do
  line=$(./run1.sh "$h" "$cap"); echo "$line"
  case "$line" in *": SUCCESSFUL"*) ;; *) rc=1;; esac
done
exit $rc
