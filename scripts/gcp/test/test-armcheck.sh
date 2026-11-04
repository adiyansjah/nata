#!/usr/bin/env bash
# Tests of armcheck.sh with stub tasks: concurrency, per-task exit codes, skipping of absent tasks, overall status.
set -u
K=$(cd "$(dirname "$0")/.." && pwd)/armcheck.sh; T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fail=0; ok() { echo "ok   $1"; }; bad() { echo "FAIL $1"; fail=1; }
export ARMCHECK_FAKE=1
start=$(date +%s); "$K" "$T/a" > "$T/a.txt" 2>&1; rc=$?; el=$(( $(date +%s) - start ))
[ "$rc" = 0 ] && ok "all stub tasks pass -> exit 0" || bad "exit $rc"
[ "$el" -le 4 ] && ok "six one-second tasks ran concurrently (${el}s)" || bad "tasks did not overlap: ${el}s"
grep -q "ALL TASKS THAT RAN PASSED" "$T/a.txt" && ok "summary says all passed" || bad "summary"
FAKE_nullmut_RC=3 "$K" "$T/b" > "$T/b.txt" 2>&1; rc=$?
[ "$rc" = 1 ] && grep -q "nullmut *FAIL (rc=3)" "$T/b.txt" && grep -q "exloom-suite *PASS" "$T/b.txt" && ok "one failing task fails the bundle, the others still ran" || bad "failure handling: rc=$rc $(cat "$T/b.txt")"
FAKE_stop_depth_MISSING=1 "$K" "$T/c" > "$T/c.txt" 2>&1; rc=$?
[ "$rc" = 0 ] && grep -q "stop-depth *SKIPPED" "$T/c.txt" && ok "a task that is not present is SKIPPED, not failed" || bad "skip: rc=$rc"
ARMCHECK_TASKS="nullmut" "$K" "$T/d" > "$T/d.txt" 2>&1
grep -q "^nullmut *PASS" "$T/d.txt" && ! grep -q "exloom-suite" "$T/d.txt" && ok "ARMCHECK_TASKS selects tasks" || bad "selection: $(cat "$T/d.txt")"
[ "$fail" = 0 ] && echo "all armcheck tests passed" || { echo "SOME TESTS FAILED"; exit 1; }
