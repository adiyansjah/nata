#!/bin/bash
# Verification-plan item 18 on a clean (GCP) VM: build in release, run the unit tests, run the harness self-test, then the
# benchmark. Writes markdown/CSV results into the directory given as $1. Exits non-zero if the build, a test or the
# self-test fails (a fast wrong answer, or a harness that cannot see a known extra cost, is not a data point: the benchmark
# is not started then).
#
#   usage: spikes/compaction/gcp-check.sh <results-dir> [rounds=25]
#
# Needs a Debian VM with a C toolchain (build-essential) and network access to crates.io. rust-toolchain.toml at the repo
# root pins the toolchain; there is no default toolchain outside the repo, so the script cd's into the repo and also exports
# RUSTUP_TOOLCHAIN. If rustup is missing it is installed (minimal profile, no default toolchain).
#
# Expected duration (a reasoned estimate, NOT measured: nothing was run on the dev box): cold build of arrow in release plus
# the test binary 3-6 min on 4-8 vCPUs; unit tests 1-3 min (single test thread, so the timing self-test inside them is not
# disturbed); self-test < 10 s; benchmark 270 cells x 26 rounds x 9 arms, I expect 1-4 min; the second, no-NULL pass < 1 min.
# About 10 min in total on an idle 8-vCPU VM. Cost: a few cents for a Spot VM of that size; ask for the human's yes first
# (docs/way-of-working.md, "Cloud spend").
#
# Output files in <results-dir>: machine.txt, build.txt, tests.txt, selftest.md, compaction.md + compaction.csv (10% NULLs,
# all 270 cells), compaction-nonull.md + compaction-nonull.csv (no NULLs, 8192 rows), summary.txt, and SANITY-ONLY.txt if the
# load average was 2 or more at any point of a timed phase.
set -euo pipefail
cd "$(dirname "$0")"
HERE=$(pwd)
REPO=$(git rev-parse --show-toplevel 2> /dev/null || (cd ../.. && pwd))
cd "$REPO"   # rust-toolchain.toml lives here
OUT=${1:?usage: gcp-check.sh <results-dir> [rounds=25]}
ROUNDS=${2:-25}
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
cd "$HERE"

say() { printf '\n==== %s ====\n' "$*" | tee -a "$OUT/summary.txt"; }
load1() { awk '{print $1}' /proc/loadavg; }
is_high() { awk -v l="$1" 'BEGIN { exit !(l >= 2.0) }'; }   # load average >= 2
SANITY=0
check_load() {   # check_load <label>: print the load average; a load >= 2 labels every number SANITY ONLY
  local l; l=$(load1)
  echo "load average (1 min) $1: $l" | tee -a "$OUT/summary.txt"
  if is_high "$l"; then
    SANITY=1
    echo "WARNING: load average $l >= 2 at '$1'. Timing numbers from this run are SANITY ONLY (not evidence)." | tee -a "$OUT/summary.txt" | tee "$OUT/SANITY-ONLY.txt"
  fi
}
: > "$OUT/summary.txt"

# ---------------------------------------------------------------------------------------------- toolchain
say "toolchain"
if ! command -v cc > /dev/null; then
  echo "no C compiler: install build-essential (apt-get install -y build-essential curl ca-certificates)" >&2
  exit 2
fi
export PATH="$HOME/.cargo/bin:$PATH"
if ! command -v rustup > /dev/null; then
  curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none | tail -3
  export PATH="$HOME/.cargo/bin:$PATH"
fi
cd "$REPO"
TC=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml)
[ -n "$TC" ] || { echo "cannot read the channel from $REPO/rust-toolchain.toml" >&2; exit 2; }
export RUSTUP_TOOLCHAIN="$TC"
rustup toolchain list | grep -q "^$TC" || rustup toolchain install "$TC" --profile minimal
cd "$HERE"
{
  date -u +%FT%TZ; uname -srm
  lscpu | grep -E 'Model name|Architecture|^CPU\(s\)|Thread|L2|L3' || true
  free -g | head -2
  rustc --version; cargo --version
  echo "git: $(git -C "$REPO" rev-parse HEAD 2> /dev/null || echo unknown)"
  echo "Cargo.lock sha256: $(sha256sum Cargo.lock | cut -d' ' -f1)"
  echo "RUSTFLAGS=${RUSTFLAGS:-<unset>}  (default target features: the numbers are for baseline x86-64 / aarch64 code)"
} | tee "$OUT/machine.txt" | tee -a "$OUT/summary.txt"
check_load "start"

# ---------------------------------------------------------------------------------------------- build
say "build (release, locked)"
{ cargo build --release --locked --bins && cargo test --release --locked --no-run; } 2>&1 | tee "$OUT/build.txt" | tail -5

# ---------------------------------------------------------------------------------------------- tests
say "unit tests (release build, overflow checks on for this crate; ONE test thread so the timing self-test inside is not disturbed)"
set +e
cargo test --release --locked -- --test-threads=1 2>&1 | tee "$OUT/tests.txt" | grep -E '^test |^test result|panicked|FAILED' | tail -40
TEST_RC=${PIPESTATUS[0]}
set -e
echo "cargo test exit status: $TEST_RC" | tee -a "$OUT/summary.txt"
if [ "$TEST_RC" -ne 0 ]; then
  echo "UNIT TESTS FAILED: no benchmark is run. See $OUT/tests.txt" | tee -a "$OUT/summary.txt"
  exit 1
fi

# ---------------------------------------------------------------------------------------------- let our own build load decay
# The 1-minute load average still contains the compile and the test run; wait (at most 5 min) until it is below 0.5 so that the
# load printed at the start of the timed phase is the machine's, not ours. The benchmark itself is single-threaded and adds 1.0.
for _ in $(seq 1 60); do
  l=$(load1)
  awk -v l="$l" 'BEGIN { exit !(l < 0.5) }' && break
  sleep 5
done

BIN="$HERE/target/release/compaction-bench"

# ---------------------------------------------------------------------------------------------- self-test
say "harness self-test: an arm with an injected extra cost must be detected as slower by that amount"
check_load "before the self-test"
set +e
"$BIN" selftest 41 2>&1 | tee "$OUT/selftest.md"
ST_RC=${PIPESTATUS[0]}
set -e
echo "self-test exit status: $ST_RC" | tee -a "$OUT/summary.txt"
if [ "$ST_RC" -ne 0 ]; then
  echo "HARNESS SELF-TEST FAILED: the timing harness did not see a known extra cost; no benchmark is run." | tee -a "$OUT/summary.txt"
  exit 1
fi

# ---------------------------------------------------------------------------------------------- benchmark
say "benchmark, 10% NULLs, 5 types x 3 selection shapes x 6 selectivities x 3 batch sizes = 270 cells, $ROUNDS rounds"
check_load "before the benchmark"
"$BIN" bench --rounds "$ROUNDS" --nulls 10 --csv "$OUT/compaction.csv" > "$OUT/compaction.md"
check_load "after the benchmark"
say "benchmark, no NULLs, 8192 rows (the NULL-free control), $ROUNDS rounds"
"$BIN" bench --rounds "$ROUNDS" --nulls 0 --sizes 8192 --csv "$OUT/compaction-nonull.csv" > "$OUT/compaction-nonull.md"
check_load "after the second benchmark"

# The binary labels its own tables from the load it saw between cells; make the script's verdict visible in the files too.
if [ "$SANITY" -eq 1 ]; then
  for f in "$OUT/compaction.md" "$OUT/compaction-nonull.md" "$OUT/selftest.md"; do
    { echo "> **SANITY ONLY: the load average was >= 2 at some point of this run (see summary.txt). Not evidence.**"; echo; cat "$f"; } > "$f.tmp" && mv "$f.tmp" "$f"
  done
fi

say "done"
{
  echo "results in $OUT"
  [ "$SANITY" -eq 1 ] && echo "NUMBERS ARE SANITY ONLY (load average >= 2)" || echo "load stayed below 2 at every check"
  echo "Break-even tables: $OUT/compaction.md (section 'Break-even selectivity'); raw numbers: $OUT/compaction.csv"
} | tee -a "$OUT/summary.txt"
exit 0
