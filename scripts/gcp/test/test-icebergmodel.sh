#!/bin/bash
# Tests of the `icebergmodel` job: spikes/iceberg-commit/gcp-check.sh in IC_MODE=model (the targeted TLC re-run) end to end, with a fake
# `java` (spikes/iceberg-commit/model/testdata/fake-java) instead of TLC and stub programs that FAIL THE TEST if the full check's other
# tools (cargo, rustup, curl, gradle, python3 -m venv) are called. Needs no network, no JDK, no GCP. Also checks the plan for the job.
#   * model mode runs ONLY tools, model-lint, model-parse-and-edits and model-full; none of the Rust, catalog, spike, interop or mutant phases
#   * the default list is the 15 runs without a result in iceberg-full-arm64; three-store-ser first, live-3 last
#   * a TIMEOUT, a NOT-RUN (budget spent), an ERROR and an unknown run name each make the verdict FAIL (exit 1); all-as-expected is PASS
#   * three-writer runs get IC_TLC_TIMEOUT_BIG, the others IC_TLC_TIMEOUT; workers = nproc; heap scales with RAM (10000 on 16 GB, ~19800 on 32 GB)
#   * a restart with IC_RESUME=1 repeats only the run that had not ended as expected
#   * IC_MODEL_MUTANTS=1 adds the gating model-mutants phase (a surviving mutant fails the verdict); off by default
# Against the old gcp-check.sh (no IC_MODE) the first checks fail: it would run rust-build, catalog-build and the rest.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
SPIKE=${ICM_SPIKE_DIR:-$REPO/spikes/iceberg-commit}   # ICM_SPIKE_DIR: run the tests against another copy (e.g. the code before the model mode)
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fails=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fails=$((fails + 1)); fi; }

# --- a private copy of the check with stub tools and stub unit tests (the real ones run in the model-lint phase on the VM)
TREE=$T/ic; mkdir -p "$TREE"
cp "$SPIKE/gcp-check.sh" "$TREE/"; cp -r "$SPIKE/scripts" "$SPIKE/model" "$TREE/"; rm -rf "$TREE/model/__pycache__"
mkdir -p "$TREE/model/testdata"
cat > "$TREE/scripts/setup-tools.sh" <<STUB
#!/bin/bash
mkdir -p "\$1/jdk/bin"; cp "$SPIKE/model/testdata/fake-java" "\$1/jdk/bin/java"; chmod +x "\$1/jdk/bin/java"; : > "\$1/tla2tools.jar"
echo "stub tools" > "\$1/VERSIONS.txt"; echo "tools stubbed"
STUB
chmod +x "$TREE/scripts/setup-tools.sh"
for f in lint_spec.py test_model_tools.py; do printf 'import sys\nprint("stub %s")\nsys.exit(0)\n' "$f" > "$TREE/model/$f"; done
BIN=$T/bin; mkdir -p "$BIN"
for tool in cargo rustup curl gradle gradlew python3-venv gcloud ssh; do
  printf '#!/bin/bash\necho "%s $*" >> "%s/forbidden-calls.log"\nexit 99\n' "$tool" "$T" > "$BIN/$tool"; chmod +x "$BIN/$tool"
done

n=0
# run_check NAME [VAR=value ...] -> RES (results dir), RC, SUM (summary.txt), FAKE_LOG; each call has its own work and results directories
run_check() {
  n=$((n + 1)); local name=$1; shift
  RES=$T/res$n; WORKD=$T/work$n; export FAKE_LOG=$T/calls$n.log; : > "$FAKE_LOG"
  env PATH="$BIN:$PATH" IC_WORK="$WORKD" IC_MODE=model "$@" bash "$TREE/gcp-check.sh" "$RES" > "$T/out$n.txt" 2>&1
  RC=$?; SUM=$RES/summary.txt
}
phases() { grep -o '^PHASE [a-z-]*' "$SUM" | cut -d' ' -f2 | tr '\n' ' '; }
called() { cut -d' ' -f1 "$FAKE_LOG" | tr '\n' ' '; }
DEFAULT15="three-store-ser three-rest-snap-i4 live-2 canary-Unknown canary-Conflict canary-RetryCommitted canary-CompactCommitted canary-DeleteCommitted canary-ValidationAbort canary-CrashedButCommitted canary-LateOldAttempt canary-DuplicateDelivered canary-OrphanCollected canary-ReaderPinned live-3"

# 1. defaults, every run ends as expected
run_check defaults
check "model mode: exit 0 and VERDICT PASS when every run ends as expected" "[ $RC -eq 0 ] && grep -q '^VERDICT PASS' '$SUM'"
check "model mode runs ONLY tools, model-lint, model-parse-and-edits, model-full" "[ \"\$(phases)\" = 'tools model-lint model-parse-and-edits model-full ' ]"
check "model mode runs no Rust, catalog, spike, interop, smoke or mutant phase" "! grep -q -E '^PHASE (rust|catalog|spike|interop|model-smoke|model-mutants)' '$SUM'"
check "model mode calls no cargo, rustup, curl, gradle or gcloud" "[ ! -e '$T/forbidden-calls.log' ]"
check "the default list is the 15 runs without a result, three-store-ser first and live-3 last, once each" "[ \"\$(called)\" = '$DEFAULT15 ' ]"
check "the model table is in the same format (header, 15 rows, TOTAL mismatches=0)" "grep -q '^run  *expected  *result  *detail' '$SUM' && grep -q '^TOTAL mismatches=0' '$SUM' && [ \$(grep -c -E '^(three|live|canary)-' '$SUM') -eq 15 ]"
check "the summary has no mutant or spike section and says what the verdict does not cover" "! grep -q -E 'model: mutants|spike: ' '$SUM' && grep -q 'NOT covered by this verdict' '$SUM'"
check "workers = nproc on every TLC call" "! grep -v \"workers=\$(nproc) \" '$FAKE_LOG' | grep -q ."

# 2. heap scales with RAM
printf 'MemTotal:       16384000 kB\n' > "$T/mem16"; printf 'MemTotal:       32768000 kB\n' > "$T/mem32"
run_check heap16 IC_MEMINFO="$T/mem16" IC_MODEL_ONLY=live-2
check "16 GB machine: TLC heap stays 10000 MB (as in the first run)" "grep -q 'xmx=10000\$' '$FAKE_LOG'"
run_check heap32 IC_MEMINFO="$T/mem32" IC_MODEL_ONLY=live-2
check "32 GB machine (n4a-highcpu-16): TLC heap 19800-20000 MB, not the old fixed 10000" "grep -q -E 'xmx=(19[89]|20[0-9])[0-9]{2}\$' '$FAKE_LOG'"
run_check heapset IC_MODEL_HEAP_MB=12345 IC_MODEL_ONLY=live-2
check "IC_MODEL_HEAP_MB overrides the heap" "grep -q 'xmx=12345\$' '$FAKE_LOG'"

# 3. a TIMEOUT makes the verdict FAIL; the other runs still ran
run_check timeout IC_TLC_TIMEOUT_BIG=1 FAKE_SLOW=three-store-ser FAKE_SLOW_SECONDS=30 IC_MODEL_ONLY=three-store-ser,live-2,canary-Unknown
check "a TIMEOUT: exit 1, VERDICT FAIL, the row says TIMEOUT, TOTAL mismatches=1" "[ $RC -eq 1 ] && grep -q '^VERDICT FAIL' '$SUM' && grep -E '^three-store-ser' '$SUM' | grep -q 'MISMATCH *TIMEOUT' && grep -q '^TOTAL mismatches=1' '$SUM'"
check "after the TIMEOUT the remaining runs still ran and ended as expected" "[ \"\$(called)\" = 'three-store-ser live-2 canary-Unknown ' ] && grep -E '^live-2' '$SUM' | grep -q ' OK '"

# 4. NOT-RUN (budget spent) makes the verdict FAIL
run_check budget IC_MODEL_BUDGET=2 IC_MODEL_ONLY=three-store-ser,live-2
check "a spent budget: both runs NOT-RUN, nothing started, exit 1, VERDICT FAIL" "[ $RC -eq 1 ] && [ \$(grep -c 'NOT-RUN' '$SUM') -eq 2 ] && [ ! -s '$FAKE_LOG' ] && grep -q '^VERDICT FAIL' '$SUM'"

# 5. an ERROR in TLC
run_check broken FAKE_BROKEN=live-2 IC_MODEL_ONLY=live-2
check "a TLC error is a mismatch: exit 1, VERDICT FAIL" "[ $RC -eq 1 ] && grep -E '^live-2' '$SUM' | grep -q 'MISMATCH *ERROR' && grep -q '^VERDICT FAIL' '$SUM'"

# 6. an unknown run name is not a quiet pass
run_check typo IC_MODEL_ONLY=three-store-ser,live-22
check "an unknown run name: exit 1, VERDICT FAIL, TLC never started, the error names it" "[ $RC -eq 1 ] && grep -q '^VERDICT FAIL' '$SUM' && [ ! -s '$FAKE_LOG' ] && grep -q 'live-22' '$RES/model-full.txt'"

# 7. a custom list
run_check custom IC_MODEL_ONLY=canary-ReaderPinned,live-2
check "IC_MODEL_ONLY selects exactly those runs, in that order" "[ \"\$(called)\" = 'canary-ReaderPinned live-2 ' ] && [ $RC -eq 0 ]"

# 8. per-config limits: three-writer runs get the big one
run_check limits IC_TLC_TIMEOUT=1 IC_TLC_TIMEOUT_BIG=6 FAKE_SLOW="three-store-ser live-2" FAKE_SLOW_SECONDS=3 IC_MODEL_ONLY=three-store-ser,live-2
check "three-store-ser gets IC_TLC_TIMEOUT_BIG (finishes), live-2 gets IC_TLC_TIMEOUT (TIMEOUT)" "grep -E '^three-store-ser' '$SUM' | grep -q ' OK ' && grep -E '^live-2' '$SUM' | grep -q 'MISMATCH *TIMEOUT'"
check "the limits are written to the summary" "grep -q '1s per run, 6s per three-writer run' '$SUM'"

# 9. bad mode
run_check badmode IC_MODE=nonsense
check "an unknown IC_MODE is refused (exit 2)" "[ $RC -eq 2 ]"

# 10. restart after a failure with IC_RESUME=1 repeats only what did not end as expected
run_check first IC_RESUME=1 IC_TLC_TIMEOUT_BIG=1 FAKE_SLOW=three-store-ser FAKE_SLOW_SECONDS=30 IC_MODEL_ONLY=live-2,three-store-ser
W1=$WORKD; R1=$RES
FAKE_LOG=$T/calls-resume.log; : > "$FAKE_LOG"
env PATH="$BIN:$PATH" IC_WORK="$W1" IC_MODE=model IC_RESUME=1 FAKE_LOG="$FAKE_LOG" IC_MODEL_ONLY=live-2,three-store-ser bash "$TREE/gcp-check.sh" "$R1" > "$T/out-resume.txt" 2>&1; RC=$?
check "resume: only three-store-ser is run again, live-2 is reused; the second verdict is PASS" "[ \"\$(cut -d' ' -f1 '$FAKE_LOG' | tr '\n' ' ')\" = 'three-store-ser ' ] && [ $RC -eq 0 ] && grep -q '(resumed)' '$R1/summary.txt' && tail -n 1 '$R1/summary.txt' | grep -q '^VERDICT PASS'"

# 10b. IC_MODEL_MUTANTS=1 adds the model-mutants phase (gating); without it there are no mutants (checked in test 1)
run_check mutants IC_MODEL_MUTANTS=1 FAKE_MUTANTS_CAUGHT=1 IC_MODEL_ONLY=live-2
check "IC_MODEL_MUTANTS=1: phases are tools, model-lint, model-parse-and-edits, model-full, model-mutants; every mutant CAUGHT: exit 0, VERDICT PASS" "[ \"\$(phases)\" = 'tools model-lint model-parse-and-edits model-full model-mutants ' ] && [ $RC -eq 0 ] && grep -q '^VERDICT PASS' '$SUM' && grep -q '^---- model: mutants' '$SUM' && grep -q 'SUMMARY CAUGHT=' '$RES/model-mutants.txt' && ! grep -q -E 'SURVIVED=[1-9]' '$RES/model-mutants.txt'"
check "IC_MODEL_MUTANTS=1 runs the mutant that reverts to the pairs rule (compaction-sees-new-pairs-only) and still no Rust, catalog or spike phase" "grep -q '^compaction-sees-new-pairs-only  *CAUGHT' '$RES/model-mutants.txt' && ! grep -q -E '^PHASE (rust|catalog|spike|interop|model-smoke)' '$SUM' && [ ! -e '$T/forbidden-calls.log' ]"
run_check survivor IC_MODEL_MUTANTS=1 IC_MODEL_ONLY=live-2
check "IC_MODEL_MUTANTS=1: a mutant that SURVIVES makes the verdict FAIL (exit 1) even though every model run is OK" "[ $RC -eq 1 ] && grep -q '^VERDICT FAIL' '$SUM' && grep -q '^PHASE model-full PASS' '$SUM' && grep -q '^PHASE model-mutants FAIL' '$SUM'"

# 11. the cost plan for the job (nothing is called)
bound=$(. "$REPO/scripts/gcp/prices.env"; awk -v r="$RATE_n4a_highcpu_16" -v h=3.0 -v ip="$IP_USD_PER_HOUR" -v dm="$DISK_USD_PER_GIB_MONTH_hyperdisk" -v b=20 -v d=30 -v x="$HYPERDISK_EXTRA_ALLOWANCE_USD" 'BEGIN { dh = dm / 730; printf "%.3f", r*h + ip*h + b*dh*h + d*dh*(h+0.5) + x }')
PATH="$BIN:$PATH" "$REPO/scripts/gcp/orchestrate.sh" plan --job icebergmodel --machines n4a-highcpu-16 --max-vm-hours 3.0 > "$T/plan.out" 2>&1; prc=$?
check "plan for icebergmodel on n4a-highcpu-16 with 3.0 VM-hours prints the worst case computed from prices.env (\$$bound) and calls nothing" "[ $prc -eq 0 ] && grep -q 'job: icebergmodel' '$T/plan.out' && grep -q 'Worst-case cost.*\\\$$bound' '$T/plan.out' && [ ! -e '$T/forbidden-calls.log' ]"
check "the orchestrator's accepted-job list names icebergmodel" "grep -q 'iceberg-smoke|icebergmodel|' '$REPO/scripts/gcp/orchestrate.sh'"

[ "$fails" = 0 ] && echo "all icebergmodel tests passed" || { echo "$fails icebergmodel test(s) FAILED"; exit 1; }
