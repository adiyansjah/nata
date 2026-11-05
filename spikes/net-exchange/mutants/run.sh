#!/usr/bin/env bash
# mutants/run.sh [results-file]: builds the crate with each deliberate bug (a cargo feature) and runs tests/net.rs against it.
# CAUGHT = the test run finished and one of the tests named for that bug FAILED. Anything else is NOT a catch:
#   SURVIVED = all tests passed;  INVALID = it did not compile or the test run did not finish normally;  MISSED = tests failed, but not the expected one.
# The baseline (no feature) must pass first, twice in a row (a mutation script that reuses stale builds would pass the second time only by luck).
# Exit status 0 only if the baseline passes and every mutant is CAUGHT.
set -u
cd "$(dirname "$0")/.."
OUT=${1:-/dev/stdout}
declare -A EXPECT=(
  [bug-no-credit]="backpressure_keeps_buffered|a_sender_that_ignores|a_four_node_shuffle|a_shuffle_with_a_slow"
  [bug-no-cancel]="cancel_stops_a_sender|when_a_consumer_cancels"
  [bug-no-seq-check]="a_gap_a_repeat_or_a_reorder"
  [bug-no-liveness]="a_silent_peer_with_an_open_connection"
  [bug-eof-is-eos]="a_sender_that_crashes_mid_stream|when_one_node_crashes"
  [bug-credit-not-returned]="a_stream_arrives_complete|backpressure_keeps_buffered|a_four_node_shuffle"
  [bug-eos-count-unchecked]="an_end_of_stream_with_the_wrong_batch_count"
  [bug-no-size-limit]="wire_rejects_an_oversize_length"
)
ORDER=(bug-no-credit bug-no-cancel bug-no-seq-check bug-no-liveness bug-eof-is-eos bug-credit-not-returned bug-eos-count-unchecked bug-no-size-limit)
fail=0
run() { cargo test --release --test net "$@" 2>&1; }
{
  for i in 1 2; do
    o=$(run); if echo "$o" | grep -q "test result: ok. 2[0-9] passed"; then echo "baseline run $i: PASS"; else echo "baseline run $i: FAIL"; echo "$o" | tail -20; fail=1; fi
  done
  echo "mutant | result | failed tests"
  for m in "${ORDER[@]}"; do
    o=$(run --features "$m")
    if echo "$o" | grep -q -E "^error\[|error: could not compile"; then echo "$m | INVALID (did not compile) |"; fail=1; continue; fi
    if ! echo "$o" | grep -q "test result:"; then echo "$m | INVALID (no test result) |"; fail=1; continue; fi
    failed=$(echo "$o" | grep -E "^test .* FAILED$" | sed 's/^test //; s/ \.\.\. FAILED//' | tr '\n' ' ')
    if echo "$o" | grep -q "test result: ok"; then echo "$m | SURVIVED |"; fail=1
    elif echo "$failed" | grep -q -E "${EXPECT[$m]}"; then echo "$m | CAUGHT | $failed"
    else echo "$m | MISSED (failed, but not the expected test) | $failed"; fail=1; fi
  done
  [ $fail = 0 ] && echo "VERDICT PASS (baseline green twice, ${#ORDER[@]} of ${#ORDER[@]} mutants caught)" || echo "VERDICT FAIL"
} | tee "$OUT"
exit $fail
