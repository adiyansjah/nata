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
#   IC_MODE            polaris: the commit suite and the pyiceberg interop against Apache Polaris (a PRODUCT catalog, started by scripts/polaris.sh from the
#                      pinned release; JDK 21): polaris-tools, rust-build, polaris-start, spike-polaris-suite, interop-pyiceberg-polaris (both gating). No model,
#                      no Apache fixture, no object-store mode. The catalog is a throwaway on loopback with an in-memory metastore and local-disk storage.
#   IC_MODE            full (default): every phase.  spikes: everything EXCEPT the TLC model (no model-lint, model-parse-and-edits, model-smoke,
#                      model-full, model-mutants): tools, rust-build, rust-tests, catalog-build, catalog-start, spike-rest-catalog,
#                      interop-pyiceberg and spike-object-store, for re-running the code side on another machine without the 2-hour model
#                      phase (the model result does not depend on the architecture of the VM). In this mode interop-pyiceberg is GATING:
#                      the first run skipped it (setup defect) and a skip would otherwise pass unnoticed. The verdict says the model is NOT
#                      covered.  model: the targeted model re-run of the `icebergmodel` GCP job: ONLY the phases
#                      tools, model-lint, model-parse-and-edits and model-full (no Rust, no catalog, no spikes, no interop, no mutants),
#                      model-full restricted to IC_MODEL_ONLY, each run cut to the time left in IC_MODEL_BUDGET. A TIMEOUT, ERROR or
#                      NOT-RUN run, or any result other than the expected one, makes the verdict FAIL. With IC_MODEL_MUTANTS=1 the phase
#                      model-mutants (every mutant of mutate_model.py, as in mode full) runs too, after model-full.
#   IC_MODEL_ONLY      mode model: comma-separated run names, done in this order (default: the 15 runs without a result in the first
#                      GCP run, three-store-ser first and the liveness run live-3 last). `run_model.py --list` prints all names;
#                      an unknown name is an error.
#   IC_MODEL_MUTANTS   mode model: 1 also runs the phase model-mutants (default 0: no mutants, as before). Gating: a mutant that SURVIVED or is
#                      INVALID makes the verdict FAIL. Not limited by IC_MODEL_BUDGET: each TLC run of a mutant has IC_MUTANT_TIMEOUT.
#   IC_TLC_TIMEOUT_BIG mode model: seconds allowed for one three-writer run (three-* and live-3; default 4500); IC_TLC_TIMEOUT
#                      (default 2700) applies to the others. In mode full IC_TLC_TIMEOUT applies to every run, as before.
#   IC_MODEL_HEAP_MB   TLC heap in MB (default: 62% of the machine's RAM, at least 10000: 10000 on the 16 GB n4a-highcpu-8, about 19800
#                      on the 32 GB n4a-highcpu-16). TLC gets one worker per vCPU (nproc) in both modes.
#   IC_MEMINFO         file read instead of /proc/meminfo (tests)
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
MODE=${IC_MODE:-full}
case "$MODE" in full | model | spikes | polaris | gcs) ;; *) echo "IC_MODE must be full, model, spikes, polaris or gcs, not '$MODE'" >&2; exit 2 ;; esac
# The default list of the targeted re-run: the 15 runs that had no result in the first GCP run (iceberg-full-arm64), cheapest guess
# first is NOT knowable (the costs are unknown), so: the near-complete three-store-ser, then the I4 run, the 2-writer liveness run,
# the 11 canaries, and the 3-writer liveness run (the most expensive unknown) last.
MODEL_ONLY_DEFAULT="three-store-ser,three-rest-snap-i4,live-2,canary-Unknown,canary-Conflict,canary-RetryCommitted,canary-CompactCommitted,canary-DeleteCommitted,canary-ValidationAbort,canary-CrashedButCommitted,canary-LateOldAttempt,canary-DuplicateDelivered,canary-OrphanCollected,canary-ReaderPinned,live-3"
MODEL_ONLY=${IC_MODEL_ONLY:-$MODEL_ONLY_DEFAULT}
TLC_TIMEOUT_BIG=${IC_TLC_TIMEOUT_BIG:-4500}
if [ "$MODE" = model ]; then MODEL_BUDGET=${IC_MODEL_BUDGET:-9000}; else MODEL_BUDGET=${IC_MODEL_BUDGET:-7200}; fi
default_heap_mb() { # 62% of RAM, but at least 10000 (the value used on the 16 GB machines)
  local kb mb
  kb=$(awk '/^MemTotal:/ {print $2}' "${IC_MEMINFO:-/proc/meminfo}" 2> /dev/null)
  mb=$(( ${kb:-0} / 1024 * 62 / 100 ))
  if [ "$mb" -gt 10000 ]; then echo "$mb"; else echo 10000; fi
}
HEAP_MB=${IC_MODEL_HEAP_MB:-$(default_heap_mb)}
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
POLARIS_RUN="$WORKBASE/polaris-run"
cleanup() { "$HERE/scripts/catalog.sh" stop "$CATALOG_RUN" > /dev/null 2>&1 || true; "$HERE/scripts/polaris.sh" stop "$POLARIS_RUN" > /dev/null 2>&1 || true; }
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
  local extra=()
  if [ "$MODE" = model ]; then
    extra=(--only "$MODEL_ONLY" --timeout-big "$TLC_TIMEOUT_BIG" --clamp-to-budget)
    [ "${IC_RESUME:-0}" = 1 ] && extra+=(--resume)
  fi
  cd "$HERE/model" && python3 -I run_model.py --jar "$JAR" --java "$JAVA" --out "$OUT/model" --work "$WORKBASE/model-work" \
    --workers "$NPROC" --heap-mb "$HEAP_MB" --timeout "$TLC_TIMEOUT" --budget "$MODEL_BUDGET" ${extra[@]+"${extra[@]}"}
}
ph_model_mutants() {
  cd "$HERE/model" && python3 -I mutate_model.py --baseline --jar "$JAR" --java "$JAVA" --out "$OUT/mutants" --work "$WORKBASE/mutants-work" \
    --workers "$NPROC" --heap-mb 6000 --timeout "$MUTANT_TIMEOUT"
}

IC="$WORKBASE/target/release/ic-spike"
URL=""

# ---- Apache Polaris (IC_MODE=polaris): the same commit suite against a product catalog. Credentials reach the suite and pyiceberg only through the
# environment file polaris.sh wrote (mode 600); they are never printed.
ph_polaris_start() { need rust-build && "$HERE/scripts/polaris.sh" start "$TOOLS" "$POLARIS_RUN" "${IC_POLARIS_PORT:-8181}" > "$WORKBASE/polaris-url"; }
ph_polaris_suite() {
  need rust-build polaris-start || return 1
  rm -rf "$WORKBASE/suite-polaris"
  ( set -a; . "$POLARIS_RUN/polaris.env"; set +a
    "$IC" suite --backend rest --url "$URL" --work "$WORKBASE/suite-polaris" --seeds "$SEEDS" ${IC_WRITERS:+--writers "$IC_WRITERS"} )
  local rc=$?
  tar -czf "$OUT/journals-polaris.tgz" -C "$WORKBASE/suite-polaris" journals 2> /dev/null || true
  return $rc
}
ph_polaris_interop() {
  need rust-build polaris-start spike-polaris-suite || return 1
  python3 -m venv "$TOOLS/venv" > /dev/null 2>&1 || { echo "python3 -m venv failed (python3-venv missing, no sudo): SKIPPED"; return 77; }
  "$TOOLS/venv/bin/pip" install --quiet "pyiceberg[pyarrow]==$PYICEBERG_VERSION" || { echo "pip install pyiceberg==$PYICEBERG_VERSION failed: SKIPPED"; return 77; }
  ( set -a; . "$POLARIS_RUN/polaris.env"; set +a
    "$IC" interop-table --backend rest --url "$URL" --work "$WORKBASE/interop-polaris" > "$OUT/interop-polaris.json" || exit 1
    "$TOOLS/venv/bin/python" "$HERE/scripts/interop_check.py" "$URL" "$OUT/interop-polaris.json" )
  local rc=$?
  [ "$rc" -eq 3 ] && return 77
  return $rc
}
if [ "$MODE" = polaris ]; then
  log "MODE polaris: the commit suite and the pyiceberg interop against Apache Polaris $POLARIS_VERSION (JDK 21), nothing else"
  phase polaris-tools yes "$HERE/scripts/setup-polaris.sh" "$TOOLS"
  cp "$TOOLS/POLARIS.txt" "$OUT/polaris-version.txt" 2> /dev/null || true
  phase rust-build yes ph_rust_build
  phase polaris-start yes ph_polaris_start
  URL=$(cat "$WORKBASE/polaris-url" 2> /dev/null || true)
  phase spike-polaris-suite yes ph_polaris_suite
  phase interop-pyiceberg-polaris yes ph_polaris_interop
  "$HERE/scripts/polaris.sh" stop "$POLARIS_RUN" > /dev/null 2>&1 || true
  cp "$POLARIS_RUN/polaris.log" "$OUT/polaris.log" 2> /dev/null || true
  {
    echo
    echo "---- spike: Apache Polaris REST catalog"
    tail -n 4 "$OUT/spike-polaris-suite.txt" 2> /dev/null
    echo "---- interop: pyiceberg reads our manifests through Polaris"
    tail -n 4 "$OUT/interop-pyiceberg-polaris.txt" 2> /dev/null
  } >> "$SUMMARY"
  # the client secret must not be anywhere in the results
  if [ -f "$POLARIS_RUN/polaris.creds" ] && grep -rqaF "$(cut -d' ' -f2 "$POLARIS_RUN/polaris.creds")" "$OUT" 2> /dev/null; then
    log "THE POLARIS CLIENT SECRET IS IN THE RESULTS: delete $OUT before it is copied anywhere"; FAILED=$((FAILED + 1))
  else
    log "secret scan: the Polaris client secret appears nowhere under the results directory"
  fi
  if [ "$FAILED" -eq 0 ]; then log "VERDICT PASS (the commit suite and the pyiceberg interop passed against Apache Polaris $POLARIS_VERSION; not covered: the TLA+ model, other catalogs, a real object store)"; exit 0; fi
  log "VERDICT FAIL ($FAILED gating phase(s) failed; see the PHASE lines above and the per-phase files in $OUT)"
  exit 1
fi

# ---- Real Google Cloud Storage (IC_MODE=gcs): the commit point of the catalog-less store mode (PutMode::Create of metadata/vN,
# GCS ifGenerationMatch=0) in a REAL bucket, the dedicated write-test bucket, with the VM's attached service account (object
# access on that one bucket, no project roles). The bucket name comes from gcs.env next to this script (git-ignored, uploaded
# with the source): NATA_WTEST_BUCKET=<name>. Probes print HTTP status codes only, never a token or the project id.
gcs_token() { curl -s -m 5 -H 'Metadata-Flavor: Google' http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["access_token"])'; }
gcs_code() { curl -s -o /dev/null -w '%{http_code}' -m 20 -H "Authorization: Bearer $(gcs_token)" "$@"; }
ph_gcs_config() {
  [ -f "$HERE/gcs.env" ] || { echo "gcs.env is missing next to gcp-check.sh (NATA_WTEST_BUCKET=<bucket>)"; return 1; }
  . "$HERE/gcs.env"; [ -n "${NATA_WTEST_BUCKET:-}" ] || { echo "NATA_WTEST_BUCKET is empty in gcs.env"; return 1; }
  gcs_token > /dev/null 2>&1 || { echo "no token from the metadata server: is a service account attached?"; return 1; }
  echo "bucket configured; a token is available"
}
ph_gcs_rust_build() { rust_env && cd "$HERE" && cargo build --release --locked --bins --tests --features gcs; }
ph_gcs_probes() {
  need gcs-config || return 1
  . "$HERE/gcs.env"; local b=$NATA_WTEST_BUCKET obj="ic-probe/$GCS_TAG/create-only" up="https://storage.googleapis.com/upload/storage/v1/b" bad=0 c
  local pid; pid=$(curl -s -m 5 -H 'Metadata-Flavor: Google' http://metadata.google.internal/computeMetadata/v1/project/project-id)
  c=$(gcs_code -X POST --data-binary probe "$up/$b/o?uploadType=media&name=$obj&ifGenerationMatch=0")
  echo "P1 create-only write to our bucket: HTTP $c (expected 200)"; [ "$c" = 200 ] || bad=1
  c=$(gcs_code -X POST --data-binary probe2 "$up/$b/o?uploadType=media&name=$obj&ifGenerationMatch=0")
  echo "P2 the same create-only write again: HTTP $c (expected 412, the precondition the commit relies on)"; [ "$c" = 412 ] || bad=1
  c=$(gcs_code -X POST --data-binary probe "$up/gcp-public-data-landsat/o?uploadType=media&name=nata-probe-$GCS_TAG")
  echo "P3 write to a bucket that is not ours (Google's public landsat bucket): HTTP $c (expected 403)"; [ "$c" = 403 ] || bad=1
  c=$(gcs_code -X POST -H 'Content-Type: application/json' --data "{\"name\":\"nata-probe-$GCS_TAG\"}" "https://storage.googleapis.com/storage/v1/b?project=$pid")
  echo "P4 create a bucket in the project: HTTP $c (expected 403)"; [ "$c" = 403 ] || bad=1
  c=$(gcs_code "https://storage.googleapis.com/storage/v1/b?project=$pid")
  echo "P5 list the project's buckets: HTTP $c (expected 403)"; [ "$c" = 403 ] || bad=1
  c=$(gcs_code -X DELETE "https://storage.googleapis.com/storage/v1/b/$b/o/$(python3 -I -c 'import sys,urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$obj")")
  echo "P6 delete our probe object: HTTP $c (expected 204)"; [ "$c" = 204 ] || bad=1
  return $bad
}
ph_gcs_suite() {
  need gcs-rust-build gcs-probes || return 1
  . "$HERE/gcs.env"
  rm -rf "$WORKBASE/suite-gcs"
  NATA_STORE_URL="gs://$NATA_WTEST_BUCKET/ic-suite/$GCS_TAG" "$IC" suite --backend store --work "$WORKBASE/suite-gcs" --seeds "$SEEDS" ${IC_WRITERS:+--writers "$IC_WRITERS"}
  local rc=$?
  tar -czf "$OUT/journals-gcs.tgz" -C "$WORKBASE/suite-gcs" journals 2> /dev/null || true
  return $rc
}
if [ "$MODE" = gcs ]; then
  GCS_TAG="$(date -u +%Y%m%d-%H%M%S)-$(od -An -N3 -tx1 /dev/urandom | tr -d ' \n')"
  log "MODE gcs: the commit suite of the catalog-less store mode with the metadata versions in a real GCS bucket (objects under ic-suite/$GCS_TAG, deleted by the bucket's one-day lifecycle), and the probes of what the VM's identity may do; nothing else"
  phase gcs-config yes ph_gcs_config
  phase gcs-rust-build yes ph_gcs_rust_build
  phase gcs-probes yes ph_gcs_probes
  phase spike-object-store-gcs yes ph_gcs_suite
  { echo; echo "---- probes of the VM identity"; cat "$OUT/gcs-probes.txt" 2> /dev/null; echo "---- spike: catalog-less commit on real GCS"; tail -n 4 "$OUT/spike-object-store-gcs.txt" 2> /dev/null; } >> "$SUMMARY"
  # neither the access token nor the project id may be in the results
  pid=$(curl -s -m 5 -H 'Metadata-Flavor: Google' http://metadata.google.internal/computeMetadata/v1/project/project-id 2> /dev/null)
  if [ -n "$pid" ] && grep -rqaF "$pid" "$OUT" 2> /dev/null; then log "THE PROJECT ID IS IN THE RESULTS: remove it before they are copied anywhere"; FAILED=$((FAILED + 1)); else log "scan: the project id appears nowhere under the results directory"; fi
  if [ "$FAILED" -eq 0 ]; then log "VERDICT PASS (probes as expected and the store commit suite passed with the metadata versions on real GCS; manifests and data files on the VM's disk; not covered: S3, Azure, other regions, a product catalog)"; exit 0; fi
  log "VERDICT FAIL ($FAILED gating phase(s) failed; see the PHASE lines above and the per-phase files in $OUT)"
  exit 1
fi

# 1. tools
phase tools yes "$HERE/scripts/setup-tools.sh" "$TOOLS"
cp "$TOOLS/VERSIONS.txt" "$OUT/tool-versions.txt" 2> /dev/null || true
# 2. the model: static checks, parse of every mutant
[ "$MODE" = spikes ] || phase model-lint yes ph_model_lint
[ "$MODE" = spikes ] || phase model-parse-and-edits yes ph_model_parse
if [ "$MODE" = model ]; then
  if [ "${IC_MODEL_MUTANTS:-0}" = 1 ]; then
    log "MODE model (targeted re-run): only tools, model-lint, model-parse-and-edits, model-full and model-mutants; runs: $MODEL_ONLY"
  else
    log "MODE model (targeted re-run): only tools, model-lint, model-parse-and-edits and model-full; runs: $MODEL_ONLY"
  fi
  log "limits: ${TLC_TIMEOUT}s per run, ${TLC_TIMEOUT_BIG}s per three-writer run (three-*, live-3), ${MODEL_BUDGET}s in total (each run is cut to the time left); TLC heap ${HEAP_MB} MB, ${NPROC} workers"
else
# 3. smoke: does TLC evaluate the spec?
[ "$MODE" = spikes ] || phase model-smoke yes ph_model_smoke
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
if [ "$MODE" = spikes ]; then phase interop-pyiceberg yes ph_interop; else phase interop-pyiceberg no ph_interop; fi
cp "$CATALOG_RUN/catalog.log" "$OUT/catalog.log" 2> /dev/null || true
cleanup
# 6. the object-store mode
phase spike-object-store yes ph_store_suite
fi
# 7. the model: the full unmutated run (every configuration, liveness, canaries), then every mutant
if [ "$MODE" != spikes ]; then
  phase model-full yes ph_model_full
  cp "$OUT/model/model-summary.txt" "$OUT/model-summary.txt" 2> /dev/null || true
  if [ "$MODE" != model ] || [ "${IC_MODEL_MUTANTS:-0}" = 1 ]; then phase model-mutants yes ph_model_mutants; fi
fi

# ------------------------------------------------------------------------------------------------ verdict
{
  echo
  if [ "$MODE" = spikes ]; then
    echo "---- model: NOT RUN in mode spikes"
  else
    echo "---- model: unmutated runs"
    cat "$OUT/model-summary.txt" 2> /dev/null || echo "(no model summary)"
  fi
  if [ "$MODE" = full ] || { [ "$MODE" = model ] && [ "${IC_MODEL_MUTANTS:-0}" = 1 ]; }; then
    echo
    echo "---- model: mutants"
    sed -n '/^mutant /,$p' "$OUT/model-mutants.txt" 2> /dev/null || echo "(no mutant table)"
    echo
  fi
  if [ "$MODE" != model ]; then
    echo "---- spike: REST reference catalog"
    tail -n 4 "$OUT/spike-rest-catalog.txt" 2> /dev/null
    echo "---- spike: object store"
    tail -n 4 "$OUT/spike-object-store.txt" 2> /dev/null
    echo "---- interop: pyiceberg reads our manifests"
    tail -n 4 "$OUT/interop-pyiceberg.txt" 2> /dev/null
  fi
} >> "$SUMMARY"
if [ "$FAILED" -eq 0 ]; then
  [ "$MODE" = model ] && log "VERDICT PASS (every requested model run ended as expected; only the model was re-run: the other phases of the full check are NOT covered by this verdict)"
  [ "$MODE" = spikes ] && log "VERDICT PASS (every phase of the code side passed, including the pyiceberg interop; the TLA+ model was NOT run in this mode: its result is in the model runs, not here)"
  [ "$MODE" = full ] && log "VERDICT PASS (every gating phase passed)"
  exit 0
fi
log "VERDICT FAIL ($FAILED gating phase(s) failed; see the PHASE lines above and the per-phase files in $OUT)"
exit 1
