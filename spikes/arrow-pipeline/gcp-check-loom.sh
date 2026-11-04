#!/usr/bin/env bash
# Everything for the loom model of the stop_depth early-finish protocol (loom-model/), for a run on a QUIET machine
# (a clean Debian VM with rust installed; x86-64 or ARM64). Nothing here is meant for the shared development box.
#
#   usage: spikes/arrow-pipeline/gcp-check-loom.sh <summary-file>
#
# Steps (a step that fails does not stop the others; the exit status says whether everything passed):
#   1 build      the model under --cfg nata_loom (compiles the spike's own src/proto/*.rs, src/exchange.rs, src/limit.rs)
#   2 dry        every mutant edit applies exactly once to a private copy (no cargo)
#   3 models     every loom test of loom-model/tests/loom_stop.rs, one test per process, on the UNMUTATED real sources
#                (this is the control). Table: test, rc, executions, preemption bound, wall time.
#   4 stress     the same tests on std threads (plain stress loop, LS_STRESS repetitions each, default 200)
#   5 reuse      mutate-reuse-check.sh: two mutation-driver invocations in a row both pass their baseline, and
#                --demonstrate-failure: with the known-bad (copy2) copy the second baseline FAILS (the check can fail)
#   6 mutants    every mutant on a private copy, never on the real sources (mutants/mutate.py), in parallel shards;
#                each shard first runs its baseline (the unmutated copy must pass the tests that judge the mutants)
#   7 sources    the real sources are byte-identical before and after (sha256 of the files the model compiles)
# The summary file gets plain-text counts; full logs go to <summary-file>.d/. Exit status 0 only if: the build works,
# every model passes, the stress passes, both reuse checks hold, every mutant is CAUGHT (none SURVIVED, none INVALID,
# none ERROR), every shard baseline passed and the sources are unchanged.
#
# LOOM_MAX_PREEMPTIONS: NOT exported by default. Unset, every test uses the preemption bound written in its source
#   (2 or 3, printed in the table and listed in bench/results/20261104-stop-depth-loom.md). Set it to force ONE bound on
#   every test (for example LOOM_MAX_PREEMPTIONS=4 for a deeper, slower run); the summary states which was used.
# Other env: LS_TIMEOUT seconds per loom test (default 3000; a timeout is a FAILURE, never a catch)
#            LS_ONLY / LS_SKIP  egrep on test names (step 3 and 4)    LS_MUTANTS  egrep on mutant names (step 6)
#            LS_WORKERS  parallel mutant shards (default: vCPUs/2, between 1 and 6)
#            LS_WORK  scratch directory for targets and private copies (default: a new mktemp dir, removed at the end)
#            CARGO_BUILD_JOBS (default: vCPUs) for the builds; each loom exploration is single-threaded.
set -u
SUMMARY=${1:?usage: gcp-check-loom.sh <summary-file>}
case $SUMMARY in /*) ;; *) SUMMARY=$PWD/$SUMMARY ;; esac
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
LOGS=$SUMMARY.d
mkdir -p "$LOGS" || exit 2
: > "$SUMMARY"
cd "$HERE" || exit 2   # inside the repository, so rustup finds rust-toolchain.toml

# A clean VM has no default toolchain: name the repository's channel; and find cargo if the shell has no PATH entry.
command -v cargo > /dev/null 2>&1 || { [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"; }
if [ -z "${RUSTUP_TOOLCHAIN:-}" ] && [ -f "$REPO/rust-toolchain.toml" ]; then
  RUSTUP_TOOLCHAIN=$(sed -n 's/^channel *= *"\(.*\)".*/\1/p' "$REPO/rust-toolchain.toml" | head -1)
  [ -n "$RUSTUP_TOOLCHAIN" ] && export RUSTUP_TOOLCHAIN
fi
NCPU=$(nproc 2> /dev/null || echo 2)
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-$NCPU}
LS_TIMEOUT=${LS_TIMEOUT:-3000}
WORKERS=${LS_WORKERS:-$(( NCPU / 2 < 1 ? 1 : (NCPU / 2 > 6 ? 6 : NCPU / 2) ))}
if [ -n "${LS_WORK:-}" ]; then WORK=$LS_WORK; mkdir -p "$WORK"; KEEP=1; else WORK=$(mktemp -d "${TMPDIR:-/tmp}/ap-loom-check.XXXXXX"); KEEP=0; fi
trap '[ "$KEEP" = 1 ] || rm -rf "$WORK"' EXIT
MODEL=$HERE/loom-model
LOOMTARGET=$WORK/target-loom
STDTARGET=$WORK/target-std
FAIL=0
say() { echo "$@" | tee -a "$SUMMARY"; }
bad() { FAIL=$((FAIL + 1)); say "  !! $*"; }

hash_sources() { # sha256 of every file the model compiles
  ( cd "$HERE" && find loom-model/Cargo.toml loom-model/Cargo.lock loom-model/src loom-model/tests src/proto src/exchange.rs src/limit.rs -type f | LC_ALL=C sort | xargs sha256sum )
}
hash_sources > "$WORK/sources.before"

say "== loom model of the stop_depth early-finish protocol (spikes/arrow-pipeline/loom-model)"
say "date (UTC): $(date -u +%Y-%m-%dT%H:%M:%SZ)"
say "host: $(uname -srm), $NCPU vCPUs, $(awk '/MemTotal/ {printf "%.1f GiB", $2/1048576}' /proc/meminfo 2> /dev/null)"
say "rustc: $(rustc --version 2>&1)   cargo: $(cargo --version 2>&1)   python3: $(python3 --version 2>&1)   toolchain: ${RUSTUP_TOOLCHAIN:-none}"
say "source revision: $(git -C "$REPO" rev-parse --short HEAD 2> /dev/null || echo unknown)$(git -C "$REPO" diff --quiet 2> /dev/null || echo ' (+ uncommitted changes)')"
say "LOOM_MAX_PREEMPTIONS: ${LOOM_MAX_PREEMPTIONS:-unset (per-test bound from tests/loom_stop.rs, printed per test below)}   per-test timeout ${LS_TIMEOUT}s   mutant shards: $WORKERS"
say "load average at start: $(cut -d' ' -f1-3 /proc/loadavg 2> /dev/null)"
say ""

# ------------------------------------------------------------------------------------------------ 1 build
say "== 1 build (RUSTFLAGS=--cfg nata_loom)"
cd "$MODEL" || exit 2
if RUSTFLAGS="--cfg nata_loom" CARGO_TARGET_DIR=$LOOMTARGET cargo test --release --no-run > "$LOGS/1-build.log" 2>&1; then say "  build OK"; else bad "build FAILED (see $LOGS/1-build.log)"; tail -20 "$LOGS/1-build.log"; fi
RUSTFLAGS="" CARGO_TARGET_DIR=$STDTARGET cargo test --release --no-run > "$LOGS/1-build-std.log" 2>&1 || bad "std build FAILED (see $LOGS/1-build-std.log)"

# ------------------------------------------------------------------------------------------------ 2 dry
say ""
say "== 2 mutant edits apply exactly once (no cargo)"
if python3 -I mutants/mutate.py --dry > "$LOGS/2-dry.log" 2>&1; then say "  all $(python3 -I mutants/mutate.py --list | wc -l) edits apply exactly once"; else bad "a mutant edit does not apply (see $LOGS/2-dry.log)"; cat "$LOGS/2-dry.log"; fi

# ------------------------------------------------------------------------------------------------ 3 models
tests=$(RUSTFLAGS="--cfg nata_loom" CARGO_TARGET_DIR=$LOOMTARGET cargo test --release --test loom_stop -- --list 2> /dev/null | sed -n 's/: test$//p')
pick() { while read -r t; do
  [ -n "${LS_ONLY:-}" ] && ! echo "$t" | grep -Eq "$LS_ONLY" && continue
  [ -n "${LS_SKIP:-}" ] && echo "$t" | grep -Eq "$LS_SKIP" && continue
  echo "$t"; done; }
tests=$(echo "$tests" | pick)
ntests=$(echo "$tests" | grep -c .)
say ""
say "== 3 loom models on the UNMUTATED sources = the control ($ntests tests, one process each)"
printf '%-72s %-4s %-14s %-9s %s\n' test rc executions bound wall | tee -a "$SUMMARY"
mpass=0; mfail=0
for t in $tests; do
  s=$(date +%s.%N)
  RUSTFLAGS="--cfg nata_loom" CARGO_TARGET_DIR=$LOOMTARGET timeout "$LS_TIMEOUT" cargo test --release --test loom_stop "$t" -- --exact --nocapture > "$LOGS/3-$t.log" 2>&1
  rc=$?
  e=$(date +%s.%N)
  ex=$(sed -n 's/^\[loom\] [^:]*: \([0-9]*\) executions, preemption bound \(.*\)$/\1/p' "$LOGS/3-$t.log" | head -1)
  bd=$(sed -n 's/^\[loom\] [^:]*: \([0-9]*\) executions, preemption bound \(.*\)$/\2/p' "$LOGS/3-$t.log" | head -1)
  note=""
  if [ $rc -eq 0 ] && [ -n "$ex" ]; then mpass=$((mpass + 1)); else
    mfail=$((mfail + 1)); [ $rc -eq 124 ] && note="TIMEOUT" || note="FAILED: $(grep -A1 -m1 'panicked at' "$LOGS/3-$t.log" | tail -1 | cut -c1-100)"
  fi
  printf '%-72s %-4s %-14s %-9s %.1fs %s\n' "$t" "$rc" "${ex:-none}" "${bd:-none}" "$(awk -v a="$e" -v b="$s" 'BEGIN{print a-b}')" "$note" | tee -a "$SUMMARY"
done
say "  loom models: $mpass passed, $mfail failed, of $ntests"
[ "$mfail" -eq 0 ] && [ "$ntests" -gt 0 ] || bad "the unmutated control does not pass every loom model (or no model ran)"

# ------------------------------------------------------------------------------------------------ 4 stress
say ""
say "== 4 the same tests on std threads (stress loop, LS_STRESS=${LS_STRESS:-200} runs each)"
spass=0; sfail=0
for t in $tests; do
  if RUSTFLAGS="" CARGO_TARGET_DIR=$STDTARGET timeout "$LS_TIMEOUT" cargo test --release --test loom_stop "$t" -- --exact --nocapture > "$LOGS/4-$t.log" 2>&1; then spass=$((spass + 1)); else sfail=$((sfail + 1)); say "  std FAILED: $t"; fi
done
say "  std stress: $spass passed, $sfail failed"
[ "$sfail" -eq 0 ] || bad "std stress failed"

# ------------------------------------------------------------------------------------------------ 5 reuse
say ""
say "== 5 two driver invocations in a row both pass their baseline (and the check can fail)"
if bash mutants/mutate-reuse-check.sh > "$LOGS/5-reuse.log" 2>&1; then say "  $(tail -1 "$LOGS/5-reuse.log")"; else bad "reuse check failed (see $LOGS/5-reuse.log)"; tail -20 "$LOGS/5-reuse.log"; fi
if bash mutants/mutate-reuse-check.sh --demonstrate-failure > "$LOGS/5-reuse-bad.log" 2>&1; then say "  $(tail -1 "$LOGS/5-reuse-bad.log")"; else bad "the reuse check could not be made to fail (see $LOGS/5-reuse-bad.log)"; tail -20 "$LOGS/5-reuse-bad.log"; fi

# ------------------------------------------------------------------------------------------------ 6 mutants
names=$(python3 -I mutants/mutate.py --list | grep -E "${LS_MUTANTS:-.}")
nm=$(echo "$names" | grep -c .)
say ""
say "== 6 mutants ($nm, on private copies, $WORKERS parallel shard(s); each shard's baseline = the unmutated copy passing the tests that judge its mutants)"
for ((i = 0; i < WORKERS; i++)); do : > "$WORK/shard$i.names"; done
i=0; for n in $names; do echo "$n" >> "$WORK/shard$((i % WORKERS)).names"; i=$((i + 1)); done
pids=()
for ((i = 0; i < WORKERS; i++)); do
  [ -s "$WORK/shard$i.names" ] || continue
  mapfile -t shard_names < "$WORK/shard$i.names"
  ( MUT_ROOT="$WORK/shard$i" MUT_TESTS=aimed MUT_TEST_TIMEOUT=$LS_TIMEOUT CARGO_BUILD_JOBS=2 python3 -I mutants/mutate.py "${shard_names[@]}" > "$LOGS/6-shard$i.log" 2>&1; echo $? > "$WORK/shard$i.rc" ) &
  pids+=($!)
done
for p in "${pids[@]}"; do wait "$p"; done
caught=0; survived=0; invalid=0; shard_bad=0
for ((i = 0; i < WORKERS; i++)); do
  [ -s "$WORK/shard$i.names" ] || continue
  log=$LOGS/6-shard$i.log; rc=$(cat "$WORK/shard$i.rc" 2> /dev/null || echo 99)
  grep -q '^baseline passes' "$log" || { shard_bad=$((shard_bad + 1)); say "  shard $i: no passing baseline (rc=$rc), see $log"; }
  grep -q '^real sources unchanged' "$log" || { shard_bad=$((shard_bad + 1)); say "  shard $i: the real sources were not confirmed unchanged, see $log"; }
  [ "$rc" = 0 ] || [ "$rc" = 1 ] || { shard_bad=$((shard_bad + 1)); say "  shard $i: driver exit status $rc, see $log"; }
done
for n in $names; do
  line=$(grep -h -E "^(CAUGHT|SURVIVED|INVALID) +$n[ :]" "$LOGS"/6-shard*.log | head -1)
  case $line in
    CAUGHT*) caught=$((caught + 1)) ;;
    SURVIVED*) survived=$((survived + 1)) ;;
    INVALID*) invalid=$((invalid + 1)) ;;
    *) line="MISSING (no verdict for $n)"; shard_bad=$((shard_bad + 1)) ;;
  esac
  say "  $line" | cut -c1-230
done
say "  mutants: $caught CAUGHT, $survived SURVIVED, $invalid INVALID, of $nm; shard problems: $shard_bad"
{ [ "$caught" -eq "$nm" ] && [ "$survived" -eq 0 ] && [ "$invalid" -eq 0 ] && [ "$shard_bad" -eq 0 ] && [ "$nm" -gt 0 ]; } || bad "mutants: not every mutant was CAUGHT"

# ------------------------------------------------------------------------------------------------ 7 sources
say ""
say "== 7 the real sources are unchanged"
hash_sources > "$WORK/sources.after"
if cmp -s "$WORK/sources.before" "$WORK/sources.after"; then say "  byte-identical before and after ($(wc -l < "$WORK/sources.after") files hashed)"; else bad "the real sources CHANGED during the run"; diff "$WORK/sources.before" "$WORK/sources.after" | head; fi

say ""
say "load average at end: $(cut -d' ' -f1-3 /proc/loadavg 2> /dev/null)"
if [ "$FAIL" -eq 0 ]; then say "RESULT: PASS (models $mpass/$ntests, std $spass/$ntests, mutants $caught/$nm CAUGHT, control OK, reuse checks OK, sources unchanged)"; else say "RESULT: FAIL ($FAIL problem(s) above)"; fi
exit $(( FAIL > 0 ? 1 : 0 ))
