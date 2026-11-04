#!/usr/bin/env bash
# Test of the mutation driver itself (needs cargo; gcp-check.sh runs it). Checks that
#  1. two invocations in a row, in the same work directory (same copy dir, same cargo target dir), both pass
#     their baseline (a stale build left by the first must not poison the second);
#  2. a baseline run AFTER a mutant run in the same work directory passes (the defect of the week: cargo reused the
#     previous mutant's binary);
#  3. the stale-build defect (reverting with shutil.copy2) is either reproduced and flagged, or at least the fix
#     (shutil.copy, fresh mtime) rebuilds;
#  4. the driver leaves the real sources byte-identical (it says so in every run).
# usage: test_mutants.sh <work-dir>      env: SEEDS (default 1500)
set -u
here=$(cd "$(dirname "$0")" && pwd)
work=${1:?work dir}; seeds=${SEEDS:-1500}
mkdir -p "$work"; fail=0
run() { # label, args...
  local label=$1; shift
  echo "---- $label: run_mutants.py $*"
  python3 -I "$here/run_mutants.py" "$@" > "$work/$label.txt" 2>&1; local rc=$?
  sed 's/^/    /' "$work/$label.txt" | tail -n 25
  echo "---- $label: exit $rc"
  return $rc
}
run inv1 --baseline-only --seeds "$seeds" --work "$work/a" || { echo "TEST 1 FAIL (first invocation)"; fail=1; }
run inv2 --baseline-only --seeds "$seeds" --work "$work/a" || { echo "TEST 1 FAIL (second invocation)"; fail=1; }
[ $fail = 0 ] && echo "TEST 1 PASS: two invocations in a row both passed their baseline"
f2=0
run mut --only window_off_by_one,leak_at_shutdown --seeds "$seeds" --work "$work/b" || f2=1
run after --baseline-only --seeds "$seeds" --work "$work/b" || f2=1
if [ $f2 = 0 ]; then echo "TEST 2 PASS: a baseline after mutants in the same work directory passed"; else echo "TEST 2 FAIL"; fail=1; fi
f3=0
run stale --selftest-stale --work "$work/c" || f3=1
if [ $f3 = 0 ]; then echo "TEST 3 PASS: $(grep -c 'STALE-SELFTEST' "$work/stale.txt") lines; fix works"; else echo "TEST 3 FAIL"; fail=1; fi
for f in inv1 inv2 mut after; do
  grep -q 'real sources UNCHANGED' "$work/$f.txt" || { echo "TEST 4 FAIL: $f did not confirm the real sources unchanged"; fail=1; }
done
[ $fail = 0 ] && echo "TEST 4 PASS: real sources unchanged in every run"
[ $fail = 0 ] && echo "TEST_MUTANTS PASS" || echo "TEST_MUTANTS FAIL"
exit $fail
