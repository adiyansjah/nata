#!/usr/bin/env bash
# Runs the tests against each deliberately broken build (a cargo feature of the library). Every mutant must make at least one test FAIL.
# A build error or a test run that did not start is INVALID, not a catch (ADR 0006 rule 1). Exit 0 only if the clean build passes and every mutant is caught.
set -u
cd "$(dirname "$0")/.."
MUTANTS=${MUTANTS:-"bug-split-off-by-one bug-drop-last-thread bug-dict-skips-last-key"}
bad=0
out=$(cargo test --release --test scan 2>&1); rc=$?
if [ $rc -eq 0 ] && echo "$out" | grep -q "test result: ok. [1-9]"; then echo "clean build: tests pass"; else echo "clean build: TESTS FAIL (rc=$rc), mutant results would mean nothing"; echo "$out" | tail -15; exit 1; fi
for m in $MUTANTS; do
  out=$(cargo test --release --test scan --features "$m" 2>&1); rc=$?
  if echo "$out" | grep -qE "^error\[|could not compile"; then echo "INVALID  $m: did not compile"; bad=1
  elif ! echo "$out" | grep -q "test result:"; then echo "INVALID  $m: no test result"; bad=1
  elif [ $rc -ne 0 ]; then echo "CAUGHT   $m: $(echo "$out" | grep -E '^test .* FAILED' | sed 's/^test //; s/ \.\.\. FAILED//' | tr '\n' ' ')"
  else echo "MISSED   $m: all tests passed with the bug in"; bad=1; fi
done
# cargo test with a feature relinks target/release/pscan with the mutant in it: rebuild the clean binary so that no mutant is left behind
cargo build --release > /dev/null 2>&1 || { echo "INVALID: the clean rebuild failed"; bad=1; }
[ $bad = 0 ] && echo "RESULT mutants: all caught" || echo "RESULT mutants: NOT ALL CAUGHT"
exit $bad
