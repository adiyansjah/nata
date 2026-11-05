#!/usr/bin/env bash
# mutants/run.sh [results-file]: builds the crate with each deliberate bug of the hand-written decoder (a cargo feature) and runs tests/hand.rs.
# CAUGHT = the test run finished and one of the tests named for that bug FAILED. SURVIVED = all passed. INVALID = it did not compile or did not
# finish normally (a crashed test binary is not a catch). MISSED = tests failed, but not the expected one. The baseline must pass twice first.
set -u
cd "$(dirname "$0")/.."
OUT=${1:-/dev/stdout}
declare -A EXPECT=(
  [bug-no-count-guard]="a_huge_count_of_zero_size_items"
  [bug-no-sync-check]="a_corrupted_sync_marker"
  [bug-no-crc]="flipping_any_byte"
  [bug-skip-nan]="matches_the_reference_reader"
  [bug-swap-bounds]="matches_the_reference_reader"
  [bug-no-utf8-check]="flipping_any_byte"
)
ORDER=(bug-no-count-guard bug-no-sync-check bug-no-crc bug-skip-nan bug-swap-bounds bug-no-utf8-check)
fail=0
run() { cargo test --release --test hand "$@" 2>&1; }
{
  for i in 1 2; do
    o=$(run); if echo "$o" | grep -q "test result: ok. 11 passed"; then echo "baseline run $i: PASS"; else echo "baseline run $i: FAIL"; echo "$o" | tail -15; fail=1; fi
  done
  echo "mutant | result | failed tests"
  for m in "${ORDER[@]}"; do
    o=$(run --features "$m")
    if echo "$o" | grep -q -E "^error\[|error: could not compile"; then echo "$m | INVALID (did not compile) |"; fail=1; continue; fi
    if ! echo "$o" | grep -q "test result:"; then echo "$m | INVALID (no test result: the test binary crashed or hung) |"; fail=1; continue; fi
    failed=$(echo "$o" | grep -E "^test .* FAILED$" | sed 's/^test //; s/ \.\.\. FAILED//' | tr '\n' ' ')
    if echo "$o" | grep -q "test result: ok"; then echo "$m | SURVIVED |"; fail=1
    elif echo "$failed" | grep -q -E "${EXPECT[$m]}"; then echo "$m | CAUGHT | $failed"
    else echo "$m | MISSED (failed, but not the expected test) | $failed"; fail=1; fi
  done
  [ $fail = 0 ] && echo "VERDICT PASS (baseline green twice, ${#ORDER[@]} of ${#ORDER[@]} mutants caught)" || echo "VERDICT FAIL"
} | tee "$OUT"
exit $fail
