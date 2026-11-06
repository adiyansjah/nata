#!/usr/bin/env bash
# Run every harness of this crate with a time cap; one line per harness; exit 1 unless all are SUCCESSFUL.
#   run.sh [cap seconds, default 600]
cd "$(dirname "$0")" || exit 2
cap=${1:-600}; rc=0
for h in $(awk '/#\[kani::proof\]/{p=1;next} p && /^ *fn /{sub(/^ *fn /,"");sub(/\(.*/,"");print;p=0}' src/lib.rs | grep -v '^negative_'); do
  out=$(timeout "$cap" cargo kani --harness "$h" 2>&1); r=$?
  res=$(echo "$out" | grep -m1 '^VERIFICATION:' | sed 's/VERIFICATION:- //'); [ $r = 124 ] && res="TIMEOUT after ${cap}s"
  echo "$h: ${res:-NO RESULT} | $(echo "$out" | grep -m1 '^ \*\* ')"
  [ "$res" = SUCCESSFUL ] || rc=1
done
exit $rc
