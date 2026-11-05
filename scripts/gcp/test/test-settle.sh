#!/usr/bin/env bash
# Tests of settle.sh with a fake load-average file: settled at once, settled after the load falls, never settled, unreadable
# or non-numeric load, no waiting with MAX_SECONDS=0, the boundary (a load equal to the limit is NOT settled), usage errors.
set -u
S=$(cd "$(dirname "$0")/.." && pwd)/settle.sh; T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fail=0; ok() { echo "ok   $1"; }; bad() { echo "FAIL $1"; fail=1; }
export SETTLE_POLL=1
echo "0.12 0.40 0.80 1/200 123" > "$T/low"
out=$(SETTLE_LOADAVG="$T/low" "$S" a 0.5 10); rc=$?
[ $rc = 0 ] && [ "$out" = "SETTLE a: settled, load 0.12 after 0 s (limit 0.5)" ] && ok "a quiet machine is settled at once, one line" || bad "quiet: rc=$rc out=$out"
echo "3.10 2.00 1.00 1/200 123" > "$T/high"
out=$(SETTLE_LOADAVG="$T/high" "$S" b 0.5 2); rc=$?
[ $rc = 1 ] && echo "$out" | grep -q "^SETTLE b: NOT SETTLED, load 3.10 after 2 s (limit 0.5)$" && ok "a busy machine times out with exit 1 and a NOT SETTLED line" || bad "busy: rc=$rc out=$out"
cp "$T/high" "$T/fall"; ( sleep 2; echo "0.30 1 1 1/2 3" > "$T/fall" ) &
out=$(SETTLE_LOADAVG="$T/fall" "$S" c 0.5 20); rc=$?; wait
[ $rc = 0 ] && echo "$out" | grep -qE "^SETTLE c: settled, load 0.30 after [1-9] s" && ok "waits until the load falls below the limit" || bad "fall: rc=$rc out=$out"
echo "0.50 0.5 0.5 1/2 3" > "$T/eq"
SETTLE_LOADAVG="$T/eq" "$S" d 0.5 0 > "$T/d.txt"; rc=$?
[ $rc = 1 ] && ok "a load equal to the limit is not settled (the rule is 'below')" || bad "boundary: rc=$rc"
start=$(date +%s); SETTLE_LOADAVG="$T/high" "$S" e 0.5 0 > /dev/null; el=$(( $(date +%s) - start ))
[ "$el" -le 1 ] && ok "MAX_SECONDS=0 reads once and does not wait" || bad "waited ${el}s"
out=$(SETTLE_LOADAVG="$T/missing" "$S" f 0.5 1); rc=$?
[ $rc = 1 ] && echo "$out" | grep -q "load unknown" && ok "an unreadable load is never settled" || bad "missing file: rc=$rc out=$out"
echo "garbage" > "$T/junk"
SETTLE_LOADAVG="$T/junk" "$S" g 0.5 0 > /dev/null; rc=$?
[ $rc = 1 ] && ok "a non-numeric load is never settled" || bad "junk: rc=$rc"
"$S" > /dev/null 2>&1; [ $? = 2 ] && ok "no label: usage error, exit 2" || bad "usage"
"$S" h 0.5 abc > /dev/null 2>&1; [ $? = 2 ] && ok "a non-numeric MAX_SECONDS is a usage error" || bad "max check"
"$S" h zero 5 > /dev/null 2>&1; [ $? = 2 ] && ok "a non-numeric LIMIT is a usage error" || bad "limit check"
SETTLE_LOADAVG="$T/low" "$S" i > "$T/i.txt"; grep -q "limit 0.5)" "$T/i.txt" && ok "the default limit is 0.5" || bad "default limit"
# Wiring: the three blocks that were SANITY ONLY on 2026-11-05 must settle before they start (static checks of the call sites).
REPO=$(cd "$(dirname "$S")/../.." && pwd); NP=$REPO/spikes/null-path/run-all.sh; JOB=$REPO/scripts/gcp/job.sh; PC=$REPO/spikes/push-cost/gcp-check.sh
for ph in 3a 3b 3d; do
  grep -q "^settle $ph\$" "$NP" && ok "null-path: settle before phase $ph" || bad "null-path: no settle before phase $ph"
done
grep -q 'settle "3c-' "$NP" && grep -q 'settle "3e-' "$NP" && ok "null-path: settle before every 3c repetition and every 3e stage run" || bad "null-path: 3c/3e settle missing"
awk '/^settle 3a$/{a=NR} /^say "3a\./{b=NR} END{exit !(a && b && a<b)}' "$NP" && ok "null-path: the settle comes before the phase header and its timed command" || bad "null-path: settle is not before phase 3a"
grep -q 'SETTLE_MAX=600 ./run-all.sh' "$JOB" && ok "job.sh gives the null-path step a 600 s settle wait" || bad "job.sh: SETTLE_MAX not passed to run-all.sh"
grep -qE 'settle.sh" ap-membench[^}]*spike-arrow-pipeline membench' "$JOB" && ok "job.sh: settle before the engine-only (membench) process" || bad "job.sh: no settle before membench"
grep -qE 'settle.sh" ap-bench[^}]*spike-arrow-pipeline bench' "$JOB" && ok "job.sh: settle before the end-to-end (bench) process" || bad "job.sh: no settle before bench"
grep -q 'settle.sh" push-cost 0.5' "$PC" && ! grep -q 'l < 1.0' "$PC" && ok "push-cost: settles below 0.5 (the old below-1.0 loop is gone)" || bad "push-cost: still uses the old rule"
! grep -q "SETTLE='for i in" "$JOB" && ok "job.sh: one settle implementation (the inline copy is gone)" || bad "job.sh still has the inline settle"
bash -n "$JOB" && bash -n "$NP" && bash -n "$PC" && ok "the edited scripts are valid bash" || bad "syntax error"
[ $fail = 0 ] && echo "all settle tests passed" || { echo "SETTLE TESTS FAILED"; exit 1; }
