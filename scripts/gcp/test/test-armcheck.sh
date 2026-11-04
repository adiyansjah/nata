#!/usr/bin/env bash
# Tests of armcheck.sh with stub tasks: concurrency, per-task exit codes, skipping of absent tasks, overall status.
set -u
K=$(cd "$(dirname "$0")/.." && pwd)/armcheck.sh; T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fail=0; ok() { echo "ok   $1"; }; bad() { echo "FAIL $1"; fail=1; }
export ARMCHECK_FAKE=1
start=$(date +%s); "$K" "$T/a" > "$T/a.txt" 2>&1; rc=$?; el=$(( $(date +%s) - start ))
[ "$rc" = 0 ] && ok "all stub tasks pass -> exit 0" || bad "exit $rc"
[ "$el" -le 5 ] && ok "the seven tasks after the first phase ran concurrently, not one after the other (${el}s)" || bad "tasks did not overlap: ${el}s"
# arrow-regress runs first and alone: no other task may have started before it finished
a=$(grep -n "arrow-regress: finished" "$T/a.txt" | head -1 | cut -d: -f1); b=$(grep -n "exloom-suite: started" "$T/a.txt" | head -1 | cut -d: -f1)
[ -n "$a" ] && [ -n "$b" ] && [ "$a" -lt "$b" ] && ok "arrow-regress finished before any other task started" || bad "phase order: finished at line ${a:-?}, next task started at line ${b:-?}"
grep -q "ALL TASKS THAT RAN PASSED" "$T/a.txt" && ok "summary says all passed" || bad "summary"
FAKE_nullmut_RC=3 "$K" "$T/b" > "$T/b.txt" 2>&1; rc=$?
[ "$rc" = 1 ] && grep -q "nullmut *FAIL (rc=3)" "$T/b.txt" && grep -q "exloom-suite *PASS" "$T/b.txt" && ok "one failing task fails the bundle, the others still ran" || bad "failure handling: rc=$rc $(cat "$T/b.txt")"
FAKE_stop_depth_MISSING=1 "$K" "$T/c" > "$T/c.txt" 2>&1; rc=$?
[ "$rc" = 0 ] && grep -q "stop-depth *SKIPPED" "$T/c.txt" && ok "a task that is not present is SKIPPED, not failed" || bad "skip: rc=$rc"
ARMCHECK_TASKS="nullmut" "$K" "$T/d" > "$T/d.txt" 2>&1
grep -q "^nullmut *PASS" "$T/d.txt" && ! grep -q "exloom-suite" "$T/d.txt" && ok "ARMCHECK_TASKS selects tasks" || bad "selection: $(cat "$T/d.txt")"
# the REAL commands (not the stubs): valid bash, and every script or directory they use exists in the repository
unset ARMCHECK_FAKE
REPO=$(cd "$(dirname "$K")/../.." && pwd)
SRC=$REPO DATA=/nonexistent ARMCHECK_PRINT=1 "$K" "$T/p" > "$T/print.txt" 2>&1
n=$(grep -c '^TASK ' "$T/print.txt"); [ "$n" = 8 ] && ok "eight task commands are printed" || bad "printed $n task lines"
while IFS= read -r line; do
  name=${line#TASK }; name=${name%%:*}; cmd=${line#*: }
  if [ -z "$cmd" ]; then bad "task $name has no command although its script should exist"; continue; fi
  bash -n -c "$cmd" 2>/dev/null && ok "task $name: command is valid bash" || bad "task $name: syntax error in its command"
  for path in $(echo "$cmd" | grep -o "\(\./\|scripts/\|spikes/\)[A-Za-z0-9_./-]*\.\(sh\|py\)" | sort -u); do
    base=$(echo "$cmd" | sed -n 's/^cd \([^ ]*\) .*/\1/p')   # a relative path is relative to the directory the command cd's into
    full="$base/${path#./}"; [ -f "$full" ] || full="$REPO/$path"
    [ -f "$full" ] && ok "task $name: $path exists" || bad "task $name: $path does not exist (looked in $base and $REPO)"
  done
done < <(grep '^TASK ' "$T/print.txt")
[ "$fail" = 0 ] && echo "all armcheck tests passed" || { echo "SOME TESTS FAILED"; exit 1; }
