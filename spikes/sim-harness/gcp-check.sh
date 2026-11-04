#!/usr/bin/env bash
# The whole check of spikes/sim-harness on a clean Debian VM (verification-plan item 23). Plain text results in
# <results-dir>; exit status 0 only if every check passed. Nothing here is run on the shared development box.
#
#   spikes/sim-harness/gcp-check.sh <results-dir> [seeds]        (seeds default 20000)
#
# Steps, in order (each writes <results-dir>/<step>.txt):
#   machine           what ran where (arch, cores, load average, toolchain)
#   build-*           release and debug builds of the harness; unit tests (debug)
#   real-runtime      the toy protocol on the REAL runtime, with and without a killed worker
#   determinism       the same seeds 3x in separate processes, release and debug: byte-identical trace hashes;
#                     in-process repeat with another seed in between; distinct seeds give distinct traces.
#                     Writes trace-hashes-<arch>.txt: compare that file (and the HASHDIGEST line) across machines.
#   control-sweep     the unmutated protocol on <seeds> seeds (release) and 2,000 (debug): every seed must pass,
#                     every fault path must have been exercised; a replay of a seed is identical
#   mutants           13 mutants (10 protocol bugs, 3 determinism breaks): each must be caught, the first failing
#                     seed REPLAYED to the identical failure; copies with fresh mtimes; real sources byte-compared
#   test-mutants      the mutation driver's own test: two invocations in a row, a baseline after mutants
#   shuttle-*, madsim-*, turmoil-*   the build-or-buy trials (shuttle: control + a known scheduler bug found and
#                     replayed = checked; madsim and turmoil: results recorded, a failure there is a finding, not a fault)
#   bench             cost of the abstraction on the real runtime, paired and interleaved, with a control arm and a
#                     self-test that must detect an injected known cost
#
# Exit non-zero on any failure, survivor, invalid mutant or invalid benchmark. Timing sections print the load average
# and say TIMING SANITY ONLY when it is >= 2.
set -u
here=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$here/../.." && pwd)
out=${1:?usage: gcp-check.sh <results-dir> [seeds]}
SEEDS=${2:-20000}
mkdir -p "$out"; out=$(cd "$out" && pwd)
# Build products and mutant copies are big: keep them out of the results directory (it is copied back to the orchestrator).
work=${SIM_WORK:-${WORK:-/tmp}/sim-harness-work}; mkdir -p "$work"
# The VM has no default toolchain outside the repo (mutant copies and trial builds may not run inside it).
export RUSTUP_TOOLCHAIN=${RUSTUP_TOOLCHAIN:-$(sed -n 's/^channel *= *"\(.*\)"/\1/p' "$REPO/rust-toolchain.toml")}
cd "$here" || exit 2
export CARGO_TARGET_DIR="$work/target"
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-$(nproc)}
JOBS=${SIM_JOBS:-$(nproc)}
REL="$work/target/release"; DBG="$work/target/debug"
TRIAL_SEEDS=${TRIAL_SEEDS:-400}
SHUTTLE_SEEDS=${SHUTTLE_SEEDS:-$SEEDS}
ARCH=$(uname -m)

FAILED=()
STARTED=$(date +%s)
loadavg() { awk '{print $1}' /proc/loadavg; }
loadlabel() { awk -v l="$(loadavg)" 'BEGIN { if (l + 0 >= 2) printf "loadavg=%s: TIMING SANITY ONLY (load >= 2)", l; else printf "loadavg=%s (quiet enough for timing)", l }'; }

# step <name> <command...>: output to <out>/<name>.txt; a non-zero exit is a failure of the whole check.
step() {
  local name=$1; shift
  local s e rc
  echo "[$(date -u +%H:%M:%S)] START $name"
  s=$(date +%s)
  "$@" > "$out/$name.txt" 2>&1; rc=$?
  e=$(date +%s)
  printf '[%s] END   %-26s rc=%d  %ds  %s\n' "$(date -u +%H:%M:%S)" "$name" "$rc" "$((e - s))" "$(loadlabel)"
  [ $rc -ne 0 ] && FAILED+=("$name")
  return $rc
}
# soft_step: same, but a non-zero exit is a recorded RESULT (a candidate framework that cannot do something is a finding).
SOFT=()
soft_step() {
  local name=$1; shift
  local s e rc
  echo "[$(date -u +%H:%M:%S)] START $name (result, not a pass/fail check)"
  s=$(date +%s)
  "$@" > "$out/$name.txt" 2>&1; rc=$?
  e=$(date +%s)
  printf '[%s] END   %-26s rc=%d  %ds  %s\n' "$(date -u +%H:%M:%S)" "$name" "$rc" "$((e - s))" "$(loadlabel)"
  SOFT+=("$name rc=$rc")
  return 0
}

# ------------------------------------------------------------------------------------------------ machine
machine() {
  date -u; uname -a; echo "ARCH=$ARCH"; nproc; free -g | head -2
  lscpu | grep -E "Model name|Architecture|^CPU\(s\)|L2|L3" || true
  curl -s -m 3 -H "Metadata-Flavor: Google" http://metadata.google.internal/computeMetadata/v1/instance/machine-type 2>/dev/null; echo
  echo "load average at start: $(cat /proc/loadavg)  [$(loadlabel)]"
  echo "toolchain: $RUSTUP_TOOLCHAIN"; rustc --version; cargo --version
  echo "seeds=$SEEDS trial_seeds=$TRIAL_SEEDS shuttle_seeds=$SHUTTLE_SEEDS jobs=$JOBS"
  python3 --version
  git -C "$REPO" rev-parse HEAD 2>/dev/null || echo "(not a git checkout: source tarball)"
}
step machine machine

# ---------------------------------------------------------------------------------------------- builds
step build-release cargo build --release -p nata-sim --bins || { echo "cannot continue without a release build"; }
step build-debug cargo build -p nata-sim --bins || { echo "cannot continue without a debug build"; }
step unit-tests cargo test -p nata-sim --no-fail-fast -- --test-threads=4
step info-release "$REL/simctl" info
step info-debug "$DBG/simctl" info

# ---------------------------------------------------------------------------------------- real runtime
real_runtime() {
  local rc=0
  "$REL/simctl" real || rc=1
  "$REL/simctl" real --kill || rc=1
  return $rc
}
step real-runtime real_runtime

# -------------------------------------------------------------------------------------------- determinism
determinism() {
  local n=${DET_SEEDS:-100} bad=0 prof i f
  for prof in release debug; do
    for i in 1 2 3; do
      local bin="$REL/simctl"; [ $prof = debug ] && bin="$DBG/simctl"
      "$bin" hashes 0 "$n" > "$work/hashes-$prof-$i.txt" 2>&1 || { echo "hashes $prof run $i exited non-zero:"; tail -5 "$work/hashes-$prof-$i.txt"; bad=1; }
      grep -E '^(HASH|INPROCESS_MISMATCH|COLLISION)' "$work/hashes-$prof-$i.txt" > "$work/hashes-$prof-$i.lines"
    done
  done
  for f in release-2 release-3 debug-1 debug-2 debug-3; do
    if cmp -s "$work/hashes-release-1.lines" "$work/hashes-$f.lines"; then echo "IDENTICAL release-1 vs $f ($n seeds)"; else echo "DIFFERENT release-1 vs $f"; diff "$work/hashes-release-1.lines" "$work/hashes-$f.lines" | head -5; bad=1; fi
  done
  grep -q -E 'INPROCESS_MISMATCH|COLLISION' "$work/hashes-release-1.lines" && { echo "in-process mismatch or hash collision"; bad=1; }
  grep -E '^HASHES' "$work/hashes-release-1.txt"
  grep '^HASH ' "$work/hashes-release-1.lines" > "$out/trace-hashes-$ARCH.txt"
  echo "first hashes (seed, FNV-1a 64 of the whole event trace):"; head -5 "$out/trace-hashes-$ARCH.txt"
  echo "HASHDIGEST arch=$ARCH seeds=$n sha256=$(sha256sum < "$out/trace-hashes-$ARCH.txt" | cut -d' ' -f1)"
  echo "(compare trace-hashes-*.txt across machines: they must be byte-identical)"
  return $bad
}
step determinism determinism

# ------------------------------------------------------------------------------------------ control sweep
control_sweep() {
  local rc=0
  echo "== release, $SEEDS seeds, $JOBS threads  [$(loadlabel)]"
  "$REL/simctl" sweep 0 "$SEEDS" --jobs "$JOBS" --require-coverage || rc=1
  echo "== debug (overflow checks and debug assertions on), 2000 seeds"
  "$DBG/simctl" sweep 0 2000 --jobs "$JOBS" --require-coverage || rc=1
  echo "== replay is identical"
  local a b
  a=$("$REL/simctl" run 12345 | sed 's/^RUN //'); b=$("$REL/simctl" replay 12345 | sed 's/^REPLAY //')
  if [ "$a" = "$b" ]; then echo "replay of seed 12345 identical: $a"; else echo "REPLAY MISMATCH: $a / $b"; rc=1; fi
  return $rc
}
step control-sweep control_sweep

# ------------------------------------------------------------------------------------------------ mutants
step mutants python3 -I "$here/mutants/run_mutants.py" --seeds "$SEEDS" --jobs "$JOBS" --work "$work/mutants"
mkdir -p "$out/mutants-detail" && cp "$work"/mutants/sweep-*.txt "$work"/mutants/build-*.log "$out/mutants-detail/" 2>/dev/null
step test-mutants env SEEDS=1500 bash "$here/mutants/test_mutants.sh" "$work/mutants-test"

# ------------------------------------------------------------------------------------------------ shuttle
shuttle_bug() {
  # the scheduler's known bug (spikes/operator-trait feature bug-lost-shutdown) must be found by a seed sweep under
  # shuttle and the failing seed must replay to the same failure
  local bin="$work/target-shuttle-bug/release/shuttle-trial" rc=0 first seed rep
  CARGO_TARGET_DIR="$work/target-shuttle-bug" cargo build --release -p shuttle-trial --features bug-lost-shutdown || return 1
  "$bin" sweep 0 "$SHUTTLE_SEEDS"; rc=$?
  if [ $rc -ne 1 ]; then echo "SHUTTLE MUTANT bug-lost-shutdown: NOT CAUGHT (sweep exit $rc)"; return 1; fi
  first=$("$bin" sweep 0 "$SHUTTLE_SEEDS" | sed -n 's/^SHUTTLE FIRST_FAIL //p'); seed=$(echo "$first" | sed -n 's/^seed=\([0-9]*\).*/\1/p')
  rep=$("$bin" replay "$seed" | sed -n 's/^SHUTTLE REPLAY //p')
  echo "first failure: $first"; echo "replay       : $rep"
  [ "$rep" = "$first" ] && echo "SHUTTLE MUTANT bug-lost-shutdown: CAUGHT at seed $seed, replay IDENTICAL" || { echo "SHUTTLE MUTANT: replay MISMATCH"; return 1; }
}
step shuttle-build cargo build --release -p shuttle-trial
step shuttle-control "$REL/shuttle-trial" sweep 0 "$SHUTTLE_SEEDS"
step shuttle-mutant shuttle_bug

# ------------------------------------------------------------------------------------------------ madsim
MAD="$work/target-madsim/release/madsim-trial"
step madsim-build env RUSTFLAGS="--cfg madsim" CARGO_TARGET_DIR="$work/target-madsim" cargo build --release -p madsim-trial
soft_step madsim-probe-thread "$MAD" probe-thread
soft_step madsim-probe-thread-allowed env MADSIM_ALLOW_SYSTEM_THREAD=1 "$MAD" probe-thread
soft_step madsim-sweep "$MAD" sweep 0 "$TRIAL_SEEDS"
madsim_hashes() { local i; for i in 1 2 3; do "$MAD" hashes 0 20 | grep '^MADSIM HASH' > "$work/madsim-hashes-$i.txt"; done; cmp -s "$work/madsim-hashes-1.txt" "$work/madsim-hashes-2.txt" && cmp -s "$work/madsim-hashes-2.txt" "$work/madsim-hashes-3.txt" && echo "MADSIM determinism: 3 runs in separate processes, 20 seeds: IDENTICAL" || { echo "MADSIM determinism: DIFFERENT between runs"; return 1; }; head -3 "$work/madsim-hashes-1.txt"; }
soft_step madsim-determinism madsim_hashes

# ----------------------------------------------------------------------------------------------- turmoil
TUR="$work/target-turmoil/release/turmoil-trial"
step turmoil-build env CARGO_TARGET_DIR="$work/target-turmoil" cargo build --release -p turmoil-trial
soft_step turmoil-sweep "$TUR" sweep 0 "$TRIAL_SEEDS"
turmoil_hashes() { local i; for i in 1 2 3; do "$TUR" hashes 0 20 | grep '^TURMOIL HASH' > "$work/turmoil-hashes-$i.txt"; done; cmp -s "$work/turmoil-hashes-1.txt" "$work/turmoil-hashes-2.txt" && cmp -s "$work/turmoil-hashes-2.txt" "$work/turmoil-hashes-3.txt" && echo "TURMOIL determinism: 3 runs in separate processes, 20 seeds: IDENTICAL" || { echo "TURMOIL determinism: DIFFERENT between runs"; return 1; }; head -3 "$work/turmoil-hashes-1.txt"; }
soft_step turmoil-determinism turmoil_hashes

# ------------------------------------------------------------------------------------------------- bench
bench() {
  echo "load average before: $(cat /proc/loadavg)  [$(loadlabel)]"
  "$REL/simbench"; local rc=$?
  echo "load average after:  $(cat /proc/loadavg)  [$(loadlabel)]"
  echo "NOTE: every timing above is labelled by the load average at the moment of the label; a label that says SANITY ONLY means the numbers show the order of magnitude, not a result."
  return $rc
}
step bench bench

# ------------------------------------------------------------------------------------- size of each approach
loc() {
  echo "lines of code (wc -l):"
  for f in sim/src/exec.rs sim/src/env.rs sim/src/sim.rs sim/src/real.rs sim/src/rng.rs sim/src/trace.rs sim/src/frame.rs sim/src/protocol.rs sim/src/scenario.rs sim/src/engine.rs sim/src/hooks.rs sim/src/trialsupport.rs \
           trials/shuttle-trial/src/lib.rs trials/shuttle-trial/src/ops.rs trials/shuttle-trial/src/main.rs \
           trials/madsim-trial/src/lib.rs trials/madsim-trial/src/main.rs trials/turmoil-trial/src/lib.rs trials/turmoil-trial/src/main.rs \
           mutants/run_mutants.py mutants/test_mutants.sh gcp-check.sh; do
    printf '  %6d  %s\n' "$(wc -l < "$here/$f")" "$f"
  done
  echo "adapter sizes: hand-written Env over the executor (sim.rs SimEnv + exec.rs) vs madsim Env (trials/madsim-trial/src/lib.rs) vs turmoil Env (trials/turmoil-trial/src/lib.rs)"
}
step loc loc

# ------------------------------------------------------------------------------------------------ summary
{
  echo "=== sim-harness gcp-check summary  arch=$ARCH  $(date -u +%FT%TZ)  elapsed $(( $(date +%s) - STARTED )) s"
  echo "--- control sweep:";  grep -E '^(SWEEP|COVERAGE_REQUIRED|FIRST_FAIL)|identical' "$out/control-sweep.txt" | head -12
  echo "--- determinism:";    grep -E '^(IDENTICAL|DIFFERENT|HASHES|HASHDIGEST)' "$out/determinism.txt"
  echo "--- mutants:";        grep -E '^(MUTANT |MUTANTS SUMMARY|MUTANTS real|    replay)' "$out/mutants.txt"
  echo "--- mutation driver test:"; grep -E '^TEST|TEST_MUTANTS' "$out/test-mutants.txt"
  echo "--- shuttle trial:";  grep -E '^SHUTTLE' "$out/shuttle-control.txt" "$out/shuttle-mutant.txt"
  echo "--- madsim trial:";   grep -h -E '^MADSIM' "$out"/madsim-*.txt 2>/dev/null | head -12
  echo "--- turmoil trial:";  grep -h -E '^TURMOIL' "$out"/turmoil-*.txt 2>/dev/null | head -8
  echo "--- bench:";          grep -E 'SELFTEST|NOISE_FLOOR|^BENCH|load average' "$out/bench.txt"
  echo "--- soft (recorded) results: ${SOFT[*]:-none}"
  if [ ${#FAILED[@]} -eq 0 ]; then echo "GCP-CHECK PASS"; else echo "GCP-CHECK FAIL: ${FAILED[*]}"; fi
} | tee "$out/summary.txt"
[ ${#FAILED[@]} -eq 0 ]
