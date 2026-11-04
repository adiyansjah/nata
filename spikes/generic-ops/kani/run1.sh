#!/usr/bin/env bash
# run one harness with a time cap: kani/run1.sh <harness> [seconds]
cd "$(dirname "$0")" || exit 2
start=$(date +%s)
out=$(CARGO_BUILD_JOBS=2 timeout "${2:-300}" cargo kani -Z stubbing --harness "$1" 2>&1); rc=$?
res=$(echo "$out" | grep -m1 '^VERIFICATION:' | sed 's/VERIFICATION:- //')
t=$(echo "$out" | grep -m1 '^Verification Time' | sed 's/Verification Time: //')
sm=$(echo "$out" | grep '^ \*\* ' | tr '\n' ';')   # every summary line: failed checks AND cover properties
[ "$rc" = 124 ] && res="TIMEOUT after ${2:-300}s"
echo "$1: ${res:-NO RESULT (compile error?)} | verification ${t:-?} | wall $(( $(date +%s)-start ))s |$sm| covers UNSATISFIABLE (never reached = vacuous): $(echo "$out" | grep -c 'Status: UNSATISFIABLE')"
