#!/bin/bash
# Tests par-mutants.sh with a fake spike: no cargo, no network, runs in seconds.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
PM=$HERE/../par-mutants.sh
T=$(mktemp -d); fails=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fails=$((fails + 1)); fi; }
run() { # run SCENARIO WORKERS [extra env...] -> sets RC and LOG
  local scen=$1 w=$2; shift 2
  LOG=$T/$scen-$w.log
  env FAKE_LIST=$scen PARMUT_FAKE=$HERE/fake-spike "$@" "$PM" --only fake --workers "$w" --out "$T/out-$scen-$w" > "$LOG" 2>&1; RC=$?
  OUTD=$T/out-$scen-$w
}

run clean 8
check "clean: exit 0 and says ALL CAUGHT" "[ $RC -eq 0 ] && grep -q 'ALL CAUGHT' $LOG"
check "clean: 24 mutants + the control, each run exactly once" "[ \$(wc -l < $OUTD/results.tsv) -eq 25 ] && [ \$(cut -f2 $OUTD/results.tsv | sort | uniq -d | wc -l) -eq 0 ]"
check "clean: all 24 CAUGHT and the control OK" "[ \$(grep -c CAUGHT $OUTD/results.tsv) -eq 24 ] && grep -q 'none.CONTROL_OK' $OUTD/results.tsv"
check "clean: the queue is empty at the end" "[ ! -s $OUTD/queue.txt ]"
check "clean: several workers took part" "[ \$(grep -o 'worker [0-9]*' $LOG | sort -u | wc -l) -ge 3 ]"

run clean 1
check "one worker gives the same 25 results" "[ $RC -eq 0 ] && [ \$(wc -l < $OUTD/results.tsv) -eq 25 ]"

run survivor 4
check "a survivor makes the exit status 1 and is listed" "[ $RC -eq 1 ] && grep -q 'survivor1.SURVIVED' $OUTD/results.tsv && grep -q 'not CAUGHT' $LOG"

run invalid 4
check "an invalid mutant is NOT a catch: exit 1" "[ $RC -eq 1 ] && grep -q 'bad1.INVALID' $OUTD/results.tsv"

run empty 2
check "a spike that lists no mutants is an error, not a pass" "[ $RC -eq 1 ] && grep -q 'cannot list its mutants' $LOG"

run prefixed 3
check "a verdict behind a '[ n/N ]' progress prefix is parsed (names with spaces and slashes work)" "[ $RC -eq 0 ] && [ \$(grep -c CAUGHT $OUTD/results.tsv) -eq 6 ] && ! grep -q INVALID $OUTD/results.tsv && [ \$(ls $OUTD/logs/fake | wc -l) -eq 7 ]"

# --revalidate recomputes verdicts from saved logs: damage the verdicts of a finished run, then repair them.
sed -i 's/CAUGHT/INVALID/' "$OUTD/results.tsv"
LOG=$T/reval.log; env FAKE_LIST=prefixed PARMUT_FAKE=$HERE/fake-spike "$PM" --only fake --revalidate "$OUTD" > "$LOG" 2>&1; RC=$?
check "--revalidate restores the verdicts from the logs without running anything" "[ $RC -eq 0 ] && [ \$(grep -c CAUGHT $OUTD/results.revalidated.tsv) -eq 6 ] && grep -q 'ALL CAUGHT' $LOG"

run controlfail 3
check "a failed control makes the spike's CAUGHT verdicts UNTRUSTED (exit 1)" "[ $RC -eq 1 ] && grep -q 'UNTRUSTED' $LOG && ! grep -q 'ALL CAUGHT' $LOG"

# The workers run outside the repository on a machine with no default toolchain: simulate it with a fresh RUSTUP_HOME
# that has the toolchains but no default, and check that the driver pins the repository's channel.
SR=$T/rustup-sterile; mkdir -p "$SR"; ln -s "${RUSTUP_HOME:-$HOME/.rustup}/toolchains" "$SR/toolchains"
LOG=$T/tc.log
( cd "$T" && env -u RUSTUP_TOOLCHAIN RUSTUP_HOME="$SR" cargo --version > "$T/sterile-cargo.txt" 2>&1 ); sterile_rc=$?
check "(precondition) a sterile RUSTUP_HOME has no default toolchain, so plain cargo fails outside the repo" "[ $sterile_rc -ne 0 ]"
env -u RUSTUP_TOOLCHAIN RUSTUP_HOME="$SR" FAKE_LIST=toolchain PARMUT_FAKE=$HERE/fake-spike REPO="$HERE/../../.." "$PM" --only fake --workers 2 --out "$T/out-tc" > "$LOG" 2>&1; RC=$?
check "the driver pins the repository's toolchain: cargo works for workers outside the repo" "[ $RC -eq 0 ] && [ \$(grep -c CAUGHT $T/out-tc/results.tsv) -eq 4 ]"

LOG=$T/dry.log; env FAKE_LIST=clean PARMUT_FAKE=$HERE/fake-spike "$PM" --only fake --dry --out "$T/out-dry" > "$LOG" 2>&1; RC=$?
check "dry run lists 25 items and runs nothing" "[ $RC -eq 0 ] && grep -q 'queue: 25 items' $LOG && [ ! -s $T/out-dry/results.tsv ]"

LOG=$T/hang.log; env FAKE_LIST=clean PARMUT_FAKE=$HERE/fake-spike ITEM_TIMEOUT=1 "$PM" --only fake --workers 2 --out "$T/out-hang" > /dev/null 2>&1
# a timeout is an ERROR verdict: simulate with a spike listing a hanging mutant
mkdir -p "$T/hangspike"; printf '#!/bin/bash\necho hang1\n' > "$T/hangspike/list.sh"; cp "$HERE/fake-spike/run.sh" "$T/hangspike/run.sh"; chmod +x "$T/hangspike/"*.sh
env PARMUT_FAKE=$T/hangspike ITEM_TIMEOUT=1 "$PM" --only fake --workers 2 --out "$T/out-hang2" > "$LOG" 2>&1; RC=$?
check "a mutant that hangs past ITEM_TIMEOUT is an ERROR (exit 1)" "[ $RC -eq 1 ] && grep -q 'hang1.ERROR' $T/out-hang2/results.tsv"

LOG=$T/real.log; "$PM" --only exchange-loom,fairness --dry --out "$T/out-real" > "$LOG" 2>&1; RC=$?
check "the real exchange-loom and fairness spikes list their mutants" "[ $RC -eq 0 ] && grep -q 'spike exchange-loom: 1[0-9] mutants' $LOG && grep -q 'spike fairness: [0-9]* mutants' $LOG"
echo
[ $fails -eq 0 ] && echo "all par-mutants tests passed" || echo "$fails failed"
rm -rf "$T"; exit $fails
