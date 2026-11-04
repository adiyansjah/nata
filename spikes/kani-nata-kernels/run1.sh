#!/usr/bin/env bash
# Run one harness with a time cap: run1.sh <harness> [seconds]. Prints one line:
# name: RESULT | verification Ns | wall Ns | checks summary | cover UNSATISFIED: n
cd "$(dirname "$0")" || exit 2
start=$(date +%s)
memf=$(mktemp)
# peak memory is measured with GNU time when it is installed (Debian: apt install time); without it the column shows 0
if [ -x /usr/bin/time ]; then timer=(/usr/bin/time -f "%M" -o "$memf"); else timer=(); fi
out=$(CARGO_BUILD_JOBS=2 "${timer[@]}" timeout "${2:-300}" cargo kani -Z stubbing --harness "$1" 2>&1); rc=$?
mem=$(tail -1 "$memf" 2>/dev/null); rm -f "$memf"
res=$(echo "$out" | grep -m1 '^VERIFICATION:' | sed 's/VERIFICATION:- //')
t=$(echo "$out" | grep -m1 '^Verification Time' | sed 's/Verification Time: //')
sm=$(echo "$out" | grep -m1 '^ \*\* ')
[ "$rc" = 124 ] && res="TIMEOUT after ${2:-300}s"
# "VERIFICATION:- FAILED" is also printed when CBMC itself is killed or crashes (out of memory, SIGTERM): that is NOT a
# failed proof. A real failure has at least one 'Status: FAILURE' check.
if [ "$res" = FAILED ] && ! echo "$out" | grep -q 'Status: FAILURE'; then
  res="NO RESULT ($(echo "$out" | grep -m1 'CBMC failed' || echo 'FAILED without a failing check'))"
fi
# cover checks (kani::cover!) are printed as Status: SATISFIED / UNSATISFIABLE; the summary line says how many held. A
# proof whose covers are not all satisfied may be vacuous, so it is not reported as a plain SUCCESSFUL.
cov=$(echo "$out" | grep -m1 'cover properties satisfied' | sed -E 's/^ *\*\* *//; s/ cover properties satisfied//')
nunsat=$(echo "$out" | grep -c 'Status: UNSATISFIABLE')
[ "$res" = SUCCESSFUL ] && [ "$nunsat" != 0 ] && res="SUCCESSFUL but COVER GAP ($nunsat unsatisfiable)"
echo "$1: ${res:-NO RESULT (compile error?)} | verification ${t:-?} | wall $(( $(date +%s)-start ))s |$sm| covers satisfied: ${cov:-none} | failing checks: $(echo "$out" | grep -c 'Status: FAILURE') | peak RSS of the largest process $(( ${mem:-0} / 1024 )) MB"
