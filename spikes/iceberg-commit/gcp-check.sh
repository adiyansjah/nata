#!/bin/bash
# Verification-plan item 24 on a clean Debian VM (GCP): is the Iceberg commit atomic and safe under concurrent writers,
# retries and crashes?  Part 1: the TLA+ model (TLC), the unmutated model, every mutant, liveness, coverage canaries.
# Part 2: the concurrent-commit spike against the Apache Iceberg REST reference fixture (built from the verified source
# release, no Docker) and against conditional creates in an object store, with injected faults and known-bad writers.
#
#   usage: spikes/iceberg-commit/gcp-check.sh <results-dir>
#
# Needs: Debian with bash, curl, tar, python3, git, a C toolchain (build-essential), network access to GitHub, Maven Central,
# the Gradle services, crates.io and static.rust-lang.org. Everything else is downloaded and checksum-verified
# (scripts/tools.env): Temurin JDK 17, tla2tools.jar, the Iceberg 1.12.0 source release. No sudo is needed.
#
# Plain text results go to <results-dir>: summary.txt (one PASS/FAIL/SKIPPED line per phase, then the verdict), one file per
# phase, model/ (TLC output of every run), mutants/ (TLC output per mutant and configuration), catalog logs, journals.
# Exit status: 0 only if every gating phase passed. Any failure, SURVIVED or INVALID mutant, unreached canary or missed
# known-bad writer makes it non-zero. Phases run to the end even after a failure so one run shows everything.
#
# Environment (all optional):
#   IC_WORK            scratch directory (default $WORK/iceberg-commit-work on the orchestrator VM, else ~/iceberg-commit-work)
#   IC_TOOLS           where the pinned tools are installed (default $IC_WORK/tools)
#   IC_RESUME=1        skip phases that already passed in $IC_WORK/state (for the orchestrator's restart after a preemption)
#   IC_TLC_TIMEOUT     seconds allowed for one baseline TLC run (default 2700); a run that hits it is TIMEOUT = failure
#   IC_MODEL_BUDGET    seconds for all baseline runs together (default 7200)
#   IC_MUTANT_TIMEOUT  seconds per TLC run of a mutant (default 900)
#   IC_WRITERS         writer processes per stress run (default: 4, 8, 16 in turn); IC_SEEDS stress seeds (default 6)
#   IC_PORT            port of the catalog (default 8181)
#
# Expected duration, an ESTIMATE that nothing has confirmed (neither TLC nor the catalog was ever run on the dev box):
#   tools 2-3 min; lint/unit/parse 1 min; Rust build 4-8 min (ARM64, cold) and tests 2-5 min; Gradle build of the fixture
#   8-20 min (cold); both suites 3-10 min; full unmutated model 20-120 min (the state counts are unknown until TLC runs);
#   mutants 5-30 min (counterexamples are found early); interop 2-4 min.   About 1.5-3.5 hours in total on 8 vCPU / 16 GB.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
OUT=${1:?usage: gcp-check.sh <results-dir>}
mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)
# shellcheck source=scripts/lib.sh
. "$HERE/scripts/lib.sh"
# shellcheck source=scripts/tools.env
. "$HERE/scripts/tools.env"

WORKBASE=${IC_WORK:-${WORK:+$WORK/iceberg-commit-work}}
WORKBASE=${WORKBASE:-$HOME/iceberg-commit-work}
mkdir -p "$WORKBASE"; WORKBASE=$(cd "$WORKBASE" && pwd)
TOOLS=${IC_TOOLS:-$WORKBASE/tools}
STATE=$WORKBASE/state; mkdir -p "$STATE"
[ "${IC_RESUME:-0}" = 1 ] || rm -f "$STATE"/*.pass
PORT=${IC_PORT:-8181}
SEEDS=${IC_SEEDS:-6}
TLC_TIMEOUT=${IC_TLC_TIMEOUT:-2700}
MODEL_BUDGET=${IC_MODEL_BUDGET:-7200}
MUTANT_TIMEOUT=${IC_MUTANT_TIMEOUT:-900}
NPROC=$(nproc)
JAVA="$TOOLS/jdk/bin/java"
JAR="$TOOLS/tla2tools.jar"
SUMMARY="$OUT/summary.txt"
FAILED=0
[ "${IC_RESUME:-0}" = 1 ] && [ -s "$SUMMARY" ] || : > "$SUMMARY"
mkdir -p "$OUT/model" "$OUT/mutants"

log() { printf '%s\n' "$*" | tee -a "$SUMMARY"; }

# phase NAME GATING(yes|no) COMMAND...  (the command's output goes to NAME.txt; exit 77 = SKIPPED, only allowed when not gating)
phase() {
  local name=$1 gating=$2; shift 2
  if [ "${IC_RESUME:-0}" = 1 ] && [ -f "$STATE/$name.pass" ]; then log "PHASE $name PASS (resumed from an earlier boot)"; return 0; fi
  local t0=$SECONDS rc status
  say "phase $name: start" >&2
  ( "$@" ) > "$OUT/$name.txt" 2>&1
  rc=$?
  case $rc in
    0) status=PASS; : > "$STATE/$name.pass" ;;
    77) if [ "$gating" = yes ]; then status=FAIL; FAILED=$((FAILED + 1)); else status=SKIPPED; fi ;;
    *) status=FAIL; if [ "$gating" = yes ]; then FAILED=$((FAILED + 1)); fi ;;
  esac
  log "PHASE $name $status (exit $rc, $((SECONDS - t0))s, gating=$gating)"
  [ "$status" = PASS ]
}
need() { # need PHASE...: succeed only if those phases passed (a missing prerequisite is a failure, not a skip)
  local p
  for p in "$@"; do [ -f "$STATE/$p.pass" ] || { echo "prerequisite phase $p did not pass" >&2; return 1; }; done
}

CATALOG_RUN="$WORKBASE/catalog-run"
cleanup() { "$HERE/scripts/catalog.sh" stop "$CATALOG_RUN" > /dev/null 2>&1 || true; }
trap cleanup EXIT

# ------------------------------------------------------------------------------------------------ 0. the machine
{
  date -u +%FT%TZ; uname -srm
  lscpu | grep -E 'Model name|Architecture|^CPU\(s\)' || true
  free -g | head -2
  df -h "$WORKBASE" | tail -1
  echo "git: $(git -C "$HERE" rev-parse HEAD 2> /dev/null || echo unknown)"
  echo "IC_WORK=$WORKBASE IC_TOOLS=$TOOLS"
  sha256sum "$HERE/model/IcebergCommit.tla" "$HERE/model/MC.tla" "$HERE/Cargo.lock" 2> /dev/null
} | tee "$OUT/machine.txt" >> "$SUMMARY"

# Each phase body is a function (phase runs it in a subshell, so it sees the variables set above).

ph_model_lint() { cd "$HERE/model" && python3 -I lint_spec.py && python3 -I test_model_tools.py -v; }

# Parse every mutant with SANY and check that every edit applies exactly once. Twice in a row: both must pass their baseline.
ph_model_parse() {
  cd "$HERE/model" || return 1
  local i
  for i in 1 2; do
    echo "--- invocation $i"
    python3 -I mutate_model.py --dry --jar "$JAR" --java "$JAVA" --out "$WORKBASE/dry" --work "$WORKBASE/dry-work" || return 1
  done
}

# Does TLC evaluate the spec at all? Two small configurations first, so a spec bug shows up in minutes.
ph_model_smoke() {
  cd "$HERE/model" && python3 -I run_model.py --jar "$JAR" --java "$JAVA" --out "$OUT/model-smoke" --work "$WORKBASE/model-work" \
    --workers "$NPROC" --timeout 900 --only smoke-rest,appends2-rest
}

rust_env() {
  export PATH="$HOME/.cargo/bin:$PATH"
  if ! command -v rustup > /dev/null; then
    curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none | tail -3 || return 1
    export PATH="$HOME/.cargo/bin:$PATH"
  fi
  local repo tc
  repo=$(git -C "$HERE" rev-parse --show-toplevel 2> /dev/null || (cd "$HERE/../.." && pwd))
  tc=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' "$repo/rust-toolchain.toml")
  [ -n "$tc" ] || { echo "cannot read the channel from $repo/rust-toolchain.toml" >&2; return 1; }
  export RUSTUP_TOOLCHAIN="$tc" CARGO_TARGET_DIR="$WORKBASE/target"
  (cd "$repo" && { rustup toolchain list | grep -q "^$tc" || rustup toolchain install "$tc" --profile minimal; }) || return 1
  rustc --version; cargo --version
}
ph_rust_build() { rust_env && cd "$HERE" && cargo build --release --locked --bins --tests; }
ph_rust_tests() {
  rust_env || return 1
  cd "$HERE" || return 1
  cargo test --release --locked -- --nocapture > "$OUT/rust-tests-full.txt" 2>&1
  local rc=$?
  grep -E '^test |test result|panicked|FAILED|failures:' "$OUT/rust-tests-full.txt"
  return $rc
}

ph_catalog_start() { need tools catalog-build && "$HERE/scripts/catalog.sh" start "$TOOLS" "$CATALOG_RUN" "$PORT" > "$WORKBASE/catalog-url"; }

ph_rest_suite() {
  need rust-build catalog-start || return 1
  rm -rf "$WORKBASE/suite-rest"
  "$IC" suite --backend rest --url "$URL" --work "$WORKBASE/suite-rest" --seeds "$SEEDS" ${IC_WRITERS:+--writers "$IC_WRITERS"}
  local rc=$?
  tar -czf "$OUT/journals-rest.tgz" -C "$WORKBASE/suite-rest" journals 2> /dev/null || true
  return $rc
}

# An independent reader (pyiceberg) must find the same files in the manifests we wrote. Not gating: it needs python3-venv,
# which a minimal Debian image may lack and which cannot be installed without sudo.
ph_interop() {
  need rust-build catalog-start spike-rest-catalog || return 1
  python3 -m venv "$TOOLS/venv" > /dev/null 2>&1 || { echo "python3 -m venv failed (python3-venv missing, no sudo): SKIPPED"; return 77; }
  "$TOOLS/venv/bin/pip" install --quiet "pyiceberg[pyarrow]==$PYICEBERG_VERSION" || { echo "pip install pyiceberg==$PYICEBERG_VERSION failed: SKIPPED"; return 77; }
  "$IC" interop-table --backend rest --url "$URL" --work "$WORKBASE/interop" > "$OUT/interop.json" || return 1
  "$TOOLS/venv/bin/python" "$HERE/scripts/interop_check.py" "$URL" "$OUT/interop.json"
  local rc=$?
  [ "$rc" -eq 3 ] && return 77
  return $rc
}

ph_store_suite() {
  need rust-build || return 1
  rm -rf "$WORKBASE/suite-store"
  "$IC" suite --backend store --work "$WORKBASE/suite-store" --seeds "$SEEDS" ${IC_WRITERS:+--writers "$IC_WRITERS"}
  local rc=$?
  tar -czf "$OUT/journals-store.tgz" -C "$WORKBASE/suite-store" journals 2> /dev/null || true
  return $rc
}

ph_model_full() {
  cd "$HERE/model" && python3 -I run_model.py --jar "$JAR" --java "$JAVA" --out "$OUT/model" --work "$WORKBASE/model-work" \
    --workers "$NPROC" --heap-mb 10000 --timeout "$TLC_TIMEOUT" --budget "$MODEL_BUDGET"
}
ph_model_mutants() {
  cd "$HERE/model" && python3 -I mutate_model.py --baseline --jar "$JAR" --java "$JAVA" --out "$OUT/mutants" --work "$WORKBASE/mutants-work" \
    --workers "$NPROC" --heap-mb 6000 --timeout "$MUTANT_TIMEOUT"
}

IC="$WORKBASE/target/release/ic-spike"
URL=""

# 1. tools
phase tools yes "$HERE/scripts/setup-tools.sh" "$TOOLS"
cp "$TOOLS/VERSIONS.txt" "$OUT/tool-versions.txt" 2> /dev/null || true
# 2. the model: static checks, parse of every mutant
phase model-lint yes ph_model_lint
phase model-parse-and-edits yes ph_model_parse
# 3. smoke: does TLC evaluate the spec?
phase model-smoke yes ph_model_smoke
# 4. Rust build and tests (mock catalog + object-store mode; no Java needed)
phase rust-build yes ph_rust_build
phase rust-tests yes ph_rust_tests
# 5. the reference REST catalog and the spike against it
phase catalog-build yes "$HERE/scripts/build-catalog.sh" "$TOOLS" "$WORKBASE/catalog-build"
phase catalog-start yes ph_catalog_start
URL=$(cat "$WORKBASE/catalog-url" 2> /dev/null || true)
cp "$TOOLS/CATALOG.txt" "$OUT/catalog-version.txt" 2> /dev/null || true
cp "$CATALOG_RUN/config.json" "$OUT/catalog-config.json" 2> /dev/null || true
phase spike-rest-catalog yes ph_rest_suite
phase interop-pyiceberg no ph_interop
cp "$CATALOG_RUN/catalog.log" "$OUT/catalog.log" 2> /dev/null || true
cleanup
# 6. the object-store mode
phase spike-object-store yes ph_store_suite
# 7. the model: the full unmutated run (every configuration, liveness, canaries), then every mutant
phase model-full yes ph_model_full
cp "$OUT/model/model-summary.txt" "$OUT/model-summary.txt" 2> /dev/null || true
phase model-mutants yes ph_model_mutants

# ------------------------------------------------------------------------------------------------ verdict
{
  echo
  echo "---- model: unmutated runs"
  cat "$OUT/model-summary.txt" 2> /dev/null || echo "(no model summary)"
  echo
  echo "---- model: mutants"
  sed -n '/^mutant /,$p' "$OUT/model-mutants.txt" 2> /dev/null || echo "(no mutant table)"
  echo
  echo "---- spike: REST reference catalog"
  tail -n 4 "$OUT/spike-rest-catalog.txt" 2> /dev/null
  echo "---- spike: object store"
  tail -n 4 "$OUT/spike-object-store.txt" 2> /dev/null
} >> "$SUMMARY"
if [ "$FAILED" -eq 0 ]; then
  log "VERDICT PASS (every gating phase passed)"
  exit 0
fi
log "VERDICT FAIL ($FAILED gating phase(s) failed; see the PHASE lines above and the per-phase files in $OUT)"
exit 1
