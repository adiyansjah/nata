#!/bin/bash
# The whole push-cost check, for a clean Debian VM (ARM64 and x86-64): build in release, run the unit tests, run a smoke
# pass of the benchmark, then the benchmark itself (which starts with the harness self-test).
#
#   usage: spikes/push-cost/gcp-check.sh <out-dir> [rounds=25]
#
# Writes into <out-dir>:  machine.txt  build.txt  unit-tests.txt  smoke.txt  results.md  rounds.csv  bench-stderr.txt  SUMMARY.txt
# Exit status: 0 only if the build, every unit test, the smoke pass, the harness self-test and the benchmark all succeed.
#   1  a unit test or the build failed (no benchmark is run: a fast wrong answer is not a data point)
#   2  the harness self-test failed (the harness cannot see an injected cost: no number is evidence)
#   3  the benchmark itself failed (a wrong checksum in some arm, a crash or a timeout)
#
# Needs: bash, coreutils (timeout), and the Rust toolchain pinned by rust-toolchain.toml at the repo root. No default
# toolchain exists outside the repo, so this script runs from inside it (or exports RUSTUP_TOOLCHAIN from that file).
# Run it on an otherwise idle VM with at least 4 vCPUs (8 recommended): the 4-thread cells and the 3-thread branching cells
# need free cores, and anything else running makes the numbers SANITY ONLY (the benchmark says so itself).
#
# Expected duration on an idle 8-vCPU VM at 25 rounds (a reasoned estimate, not yet measured): build ~0.5 min, unit tests
# 1-3 min, smoke <1 min, self-test 1-2 min, benchmark ~10-15 min. About 15-25 min in total. No network is needed after the
# toolchain is installed; the crate has no dependencies. No project ids, accounts or keys are read or written.
set -u -o pipefail

OUT_ARG=${1:?usage: gcp-check.sh <out-dir> [rounds=25]}
ROUNDS=${2:-25}
mkdir -p "$OUT_ARG" || exit 1
OUT=$(cd "$OUT_ARG" && pwd)
HERE=$(cd "$(dirname "$0")" && pwd)
cd "$HERE" || exit 1

[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
if ! rustc --version >/dev/null 2>&1; then
  TC=$(sed -n 's/^channel *= *"\(.*\)".*/\1/p' "$HERE/../../rust-toolchain.toml" 2>/dev/null | head -1)
  if [ -n "$TC" ]; then export RUSTUP_TOOLCHAIN="$TC"; fi
fi
# Baseline target features, like every other number in bench/results (no -C target-cpu=native).
export RUSTFLAGS=""
export RUST_BACKTRACE=1

SUMMARY="$OUT/SUMMARY.txt"
: > "$SUMMARY"
say() { printf '\n==== %s ====\n' "$*" | tee -a "$SUMMARY"; }
note() { printf '%s\n' "$*" | tee -a "$SUMMARY"; }

say "machine"
{
  date -u +%FT%TZ
  uname -srm
  lscpu 2>/dev/null | grep -E 'Model name|^CPU\(s\)|Thread|Core\(s\)|Socket|L1d|L2|L3'
  rustc --version
  cargo --version
  free -m | head -2
  uptime
  echo "RUSTFLAGS=<empty: baseline target features>"
} 2>&1 | tee "$OUT/machine.txt" | tee -a "$SUMMARY"
NCPU=$(nproc)
if [ "$NCPU" -lt 4 ]; then
  note "WARNING: $NCPU CPU(s) visible. The 4-thread cells and the 3-thread branching cells oversubscribe the machine and are marked SANITY ONLY."
fi

say "build (release, with the examples and the test binaries)"
cargo build --release --examples --tests 2>&1 | tee "$OUT/build.txt" | tail -5
if [ "${PIPESTATUS[0]}" -ne 0 ]; then
  note "BUILD FAILED"
  exit 1
fi

say "unit tests (release profile: overflow checks on)"
timeout 1800 cargo test --release -- --test-threads=1 2>&1 | tee "$OUT/unit-tests.txt" | grep -E '^test result|^test .*(FAILED|ok)$|panicked|error' | tee -a "$SUMMARY"
RC=${PIPESTATUS[0]}
if [ "$RC" -ne 0 ]; then
  note "UNIT TESTS FAILED (exit $RC): the benchmark is not run"
  exit 1
fi
note "unit tests passed"

BIN=./target/release/examples/push_cost
[ -x "$BIN" ] || { note "missing $BIN"; exit 1; }

say "smoke pass (mechanics only: a tiny grid, 3 rounds, no numbers to keep)"
timeout 900 "$BIN" --smoke --csv "$OUT/smoke.csv" > "$OUT/smoke.txt" 2>&1
RC=$?
if [ "$RC" -ne 0 ]; then
  tail -20 "$OUT/smoke.txt" | tee -a "$SUMMARY"
  note "SMOKE PASS FAILED (exit $RC): the benchmark is not run"
  exit 1
fi
note "smoke pass ok"

say "wait for the machine to be quiet (the build and the tests just ran): 1-minute load below 0.5, at most PC_SETTLE_MAX s"
# Was: below 1.0 for at most 300 s, and the harness called 0.72 "quiet" (found 2026-11-05). Same rule as every other timed block (ADR 0006 rule 7).
if settle_line=$("$HERE/../../scripts/gcp/settle.sh" push-cost 0.5 "${PC_SETTLE_MAX:-600}"); then settled=1; else settled=0; fi
note "$settle_line"
note "1-minute load average now: $(cut -d' ' -f1 /proc/loadavg) (settled below 0.5: $settled)"

say "benchmark: harness self-test, then the grid ($ROUNDS rounds)"
timeout 7200 "$BIN" --rounds "$ROUNDS" --csv "$OUT/rounds.csv" > "$OUT/results.md" 2> "$OUT/bench-stderr.txt"
RC=$?
if [ "$RC" -eq 1 ]; then
  grep -A12 'Harness self-test' "$OUT/results.md" | tee -a "$SUMMARY"
  note "HARNESS SELF-TEST FAILED: no number from this run is evidence (see $OUT/results.md)"
  exit 2
elif [ "$RC" -ne 0 ]; then
  tail -20 "$OUT/bench-stderr.txt" | tee -a "$SUMMARY"
  note "BENCHMARK FAILED (exit $RC; a wrong checksum, a crash or a timeout)"
  exit 3
fi
grep -E 'Harness self-test:|SANITY ONLY|load average at start|\*\*quiet\*\*' "$OUT/results.md" | tee -a "$SUMMARY"
note "done: $OUT/results.md  $OUT/rounds.csv"
exit 0
