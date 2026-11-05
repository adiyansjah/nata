#!/usr/bin/env bash
# gcp-check.sh <results-dir> [rounds=5]
#
# Verification-plan item 27 (a gate): the object-store read path with real latency. For a clean Debian VM (ARM64 and x86-64):
#   1. build in release, check that the reused sources are the ones ADR 0002 was tested with, run the unit tests;
#   2. the harness self-test (it must see an injected known latency, bandwidth cap and tail) and the leak-scan self-test;
#   3. generate TPC-H lineitem (scripts/gen-tpch.sh) and the four file layouts with DuckDB, check their shape;
#   4. start the SIMULATED object store (a local HTTP server with a latency distribution, failures and a bandwidth cap),
#      run correctness, memory-bound, cancellation and retry/error tests against it (each also against known-bad versions);
#   5. the throughput / request-pattern measurement against the SIMULATED store, interleaved, with controls, and DuckDB
#      over the same store as the reference;
#   6. the same measurement against a REAL bucket, only if one is configured through environment variables (below);
#      otherwise it says "REAL STORE NOT MEASURED" and does not pretend.
#
# Everything the simulated store produces is labelled SIMULATED. It runs on loopback: no TLS, no slow start, no cross-zone path.
#
# Exit status: 0 all tests and self-tests passed, 1 a test failed, 2 a self-test failed, 3 the benchmark failed (wrong result,
# crash, or the verification before timing), 4 setup failed (build, duckdb, data), 5 NATA_REQUIRE_REAL=1 and the real store was
# not measured, 6 a configured value of the real store was found in the results (leak).
#
# Needs: bash, coreutils (timeout), curl (optional: the VM's zone), duckdb on PATH (with network access once, for
# `INSTALL httpfs`), a C compiler (the S3 client's TLS library builds C code; `cmake` is NOT needed), and the Rust toolchain
# pinned by rust-toolchain.toml at the repo root (no default toolchain exists outside the repo, so this script cd's into it).
# Run it on an otherwise idle VM with at least 4 vCPUs (8 recommended): the CPU pool, the I/O runtime, the simulated server and
# DuckDB share the cores.
#
# THE REAL STORE (mode b) comes ONLY from the environment, is never written to a file or a log, and the results directory is
# scanned for it at the end:
#   NATA_OBJSTORE_ENDPOINT   e.g. https://storage.googleapis.com   (the S3-API endpoint; GCS speaks it through HMAC keys)
#   NATA_OBJSTORE_BUCKET     the bucket (in the same region as the VM)
#   NATA_OBJSTORE_PREFIX     the temporary test prefix; <prefix>/<layout>/part-NNNN.parquet must exist (see `objscan upload`)
#   NATA_OBJSTORE_REGION     optional, default "auto"
#   NATA_OBJSTORE_ACCESS_KEY_ID / NATA_OBJSTORE_SECRET_ACCESS_KEY   optional HMAC key pair; without it, anonymous reads
#   NATA_OBJSTORE_LABEL      optional, NON-SECRET description for the report, e.g. "GCS Standard, us-central1, same region as the VM"
#   NATA_OBJSTORE_HTTP1=1    optional: force HTTP/1.1 (default lets the client negotiate HTTP/2 like the engine would)
# Other knobs: NATA_REQUIRE_REAL=1 (not measuring the real store is a failure), SF (TPC-H scale factor, default 1),
# NATA_SCAN_DATA (where the data goes, default <repo>/data/objscan-sf<SF>; NEVER under the results directory),
# NATA_SCAN_NO_DUCK=1 (skip the DuckDB reference arm: degraded, say so in the report).
#
# Expected duration on an idle 8-vCPU VM at 5 rounds (a reasoned estimate, NOT yet measured): build 5-8 min, unit tests 3-6 min,
# data generation 2-4 min, simulated-store tests 4-8 min, simulated benchmark 15-25 min, real benchmark 15-25 min (when
# configured). About 45-75 min in total, 25-45 min without the real store.
set -u -o pipefail

OUT_ARG=${1:?usage: gcp-check.sh <results-dir> [rounds=5]}
ROUNDS=${2:-5}
SF=${SF:-1}
mkdir -p "$OUT_ARG" || exit 4
OUT=$(cd "$OUT_ARG" && pwd)
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
cd "$HERE" || exit 4

[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
if ! rustc --version >/dev/null 2>&1; then
  TC=$(sed -n 's/^channel *= *"\(.*\)".*/\1/p' "$REPO/rust-toolchain.toml" 2>/dev/null | head -1)
  [ -n "$TC" ] && export RUSTUP_TOOLCHAIN="$TC"
fi
export RUSTFLAGS=""          # baseline target features, like every other number in bench/results
export RUST_BACKTRACE=1
# NEVER enable `set -x` or print the environment in this script: the real store's coordinates live there.

DATA=${NATA_SCAN_DATA:-$REPO/data/objscan-sf$SF}
case "$DATA" in "$OUT"|"$OUT"/*) echo "NATA_SCAN_DATA must not be inside the results directory (it is copied back)"; exit 4 ;; esac
BIN="$HERE/target/release/objscan"
SUMMARY="$OUT/SUMMARY.txt"
: > "$SUMMARY"
say()  { printf '\n==== %s ====\n' "$*" | tee -a "$SUMMARY"; }
note() { printf '%s\n' "$*" | tee -a "$SUMMARY"; }
FAIL=0          # worst exit status so far
fail() { [ "$1" -gt "$FAIL" ] && FAIL=$1; }
SERVER_PID=""
cleanup() { [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null; true; }
trap cleanup EXIT

load1() { cut -d' ' -f1 /proc/loadavg 2>/dev/null || echo 0; }
wait_quiet() {   # wait up to $1 s for the 1-minute load average to fall below 2
  local t=0
  while [ "$t" -lt "$1" ] && awk -v l="$(load1)" 'BEGIN{exit !(l>=2)}'; do sleep 5; t=$((t+5)); done
}

# ---------------------------------------------------------------- machine
say "machine"
{
  date -u +%FT%TZ
  uname -srm
  lscpu 2>/dev/null | grep -E 'Model name|^CPU\(s\)|Thread|Core\(s\)|Socket|L1d|L2|L3|Architecture'
  rustc --version; cargo --version; cc --version 2>/dev/null | head -1
  duckdb --version 2>/dev/null || echo "duckdb: NOT FOUND"
  free -m | head -2
  echo "ulimit -n: $(ulimit -n)"
  # the zone and the machine type, stripped of the project number that the metadata server puts in front of them
  for k in zone machine-type; do
    v=$(curl -s -m 2 -H 'Metadata-Flavor: Google' "http://metadata.google.internal/computeMetadata/v1/instance/$k" 2>/dev/null | sed 's|.*/||')
    [ -n "$v" ] && echo "$k: $v"
  done
  echo "RUSTFLAGS=<empty: baseline target features>"
  uptime
} 2>&1 | tee "$OUT/machine.txt" | tee -a "$SUMMARY"
NCPU=$(nproc)
[ "$NCPU" -lt 4 ] && note "WARNING: $NCPU CPU(s) visible: the CPU pool, the I/O runtime, the simulated server and DuckDB oversubscribe the machine; timing is SANITY ONLY."
command -v duckdb >/dev/null || { note "duckdb is not on PATH"; exit 4; }

# ---------------------------------------------------------------- 1. build, reuse check, unit tests
say "reused sources are the ones the bridge was tested with"
REUSE_OK=1
for f in api batch; do
  if cmp -s "$HERE/../operator-trait/src/$f.rs" "$HERE/../async-bridge/src/$f.rs"; then
    note "ok: spikes/operator-trait/src/$f.rs is byte-identical to the snapshot in spikes/async-bridge/src/$f.rs"
  else
    note "DIFFERENT: spikes/operator-trait/src/$f.rs and spikes/async-bridge/src/$f.rs (the bridge's tests were run against the snapshot)"; REUSE_OK=0
  fi
done
[ "$REUSE_OK" = 1 ] || fail 4

say "build (release, with the test binaries)"
cargo build --release --bins --tests 2>&1 | tee "$OUT/build.txt" | tail -5
if [ "${PIPESTATUS[0]}" -ne 0 ] || [ ! -x "$BIN" ]; then note "BUILD FAILED"; exit 4; fi

say "unit tests (release profile: overflow checks on; one test at a time: they share a simulated store)"
timeout 3000 cargo test --release -- --test-threads=1 --nocapture 2>&1 | "$BIN" redact > "$OUT/unit-tests.txt"
RC=${PIPESTATUS[0]}
grep -E '^test result|^test .*(FAILED|ok)$|panicked|^(PASS|FAIL|CAUGHT|NOT CAUGHT)' "$OUT/unit-tests.txt" | tail -150 | tee -a "$SUMMARY" >/dev/null
if [ "$RC" -ne 0 ]; then note "UNIT TESTS FAILED (exit $RC): see unit-tests.txt; no measurement is run"; exit 1; fi
note "unit tests passed: $(grep -c '^test .* ok$' "$OUT/unit-tests.txt") tests"

# ---------------------------------------------------------------- 2. self-tests
say "harness self-test (an injected 50 ms latency, a 50 MiB/s cap and a tail must be seen; the same checks must fail on the known-bad version)"
timeout 600 "$BIN" selftest 2>&1 | tee "$OUT/selftest.txt" | grep -E '^(PASS|FAIL|RESULT)' | tee -a "$SUMMARY"
[ "${PIPESTATUS[0]}" -eq 0 ] || { note "HARNESS SELF-TEST FAILED: no number below is evidence"; exit 2; }
say "leak-scan self-test (a planted value must be found, a clean directory must not be flagged)"
"$BIN" leakscan --selftest 2>&1 | tee "$OUT/leakscan-selftest.txt" | tee -a "$SUMMARY"
[ "${PIPESTATUS[0]}" -eq 0 ] || { note "LEAK-SCAN SELF-TEST FAILED"; exit 2; }

# ---------------------------------------------------------------- 3. data
say "data: TPC-H lineitem SF$SF (scripts/gen-tpch.sh) and the four layouts (DuckDB COPY ... ROW_GROUP_SIZE)"
LI="$DATA/tpch/lineitem.parquet"
mkdir -p "$DATA/tpch" "$DATA/layouts" "$DATA/expected"
if [ ! -f "$LI" ]; then
  "$REPO/scripts/gen-tpch.sh" "$SF" "$DATA/tpch" > "$OUT/gen-tpch.txt" 2>&1 || { note "gen-tpch.sh failed (it needs network access to install DuckDB's tpch extension)"; exit 4; }
fi
NROWS=$(duckdb -noheader -list -c "SELECT count(*) FROM read_parquet('$LI')")
note "lineitem rows: $NROWS"
if [ ! -f "$DATA/layouts/.done" ]; then
  PER=$(( (NROWS + 99) / 100 ))
  {
    echo "CREATE TABLE li AS SELECT * FROM read_parquet('$LI');"
    echo "COPY li TO '$DATA/layouts/big1/part-0000.parquet' (FORMAT parquet, ROW_GROUP_SIZE 1500000);"
    echo "COPY li TO '$DATA/layouts/mid1/part-0000.parquet' (FORMAT parquet, ROW_GROUP_SIZE 122880);"
    echo "COPY li TO '$DATA/layouts/small1/part-0000.parquet' (FORMAT parquet, ROW_GROUP_SIZE 20000);"
    for i in $(seq 0 99); do
      printf "COPY (SELECT * FROM li WHERE rowid >= %d AND rowid < %d) TO '%s/layouts/files100/part-%04d.parquet' (FORMAT parquet, ROW_GROUP_SIZE 1000000);\n" \
        $((i * PER)) $(((i + 1) * PER)) "$DATA" "$i"
    done
  } > "$OUT/layouts.sql"
  mkdir -p "$DATA/layouts/big1" "$DATA/layouts/mid1" "$DATA/layouts/small1" "$DATA/layouts/files100"
  duckdb -bail < "$OUT/layouts.sql" > "$OUT/layouts-gen.txt" 2>&1 || { note "layout generation failed (see layouts-gen.txt)"; exit 4; }
  touch "$DATA/layouts/.done"
fi
"$BIN" expected --lineitem "$LI" --out "$DATA/expected" 2>&1 | tee "$OUT/expected.txt" | tee -a "$SUMMARY"
[ "${PIPESTATUS[0]}" -eq 0 ] || { note "reference results could not be computed"; exit 4; }
"$BIN" layouts --data "$DATA/layouts" --out "$OUT/layouts.md" 2>&1 | tee -a "$SUMMARY"
[ "${PIPESTATUS[0]}" -eq 0 ] || { note "A LAYOUT DOES NOT HAVE THE SHAPE IT IS NAMED FOR: the layout results would not mean what they say"; exit 4; }
if [ "${NATA_SCAN_NO_DUCK:-0}" != 1 ]; then
  duckdb -c "INSTALL httpfs;" > "$OUT/httpfs-install.txt" 2>&1 || { note "INSTALL httpfs failed (needs network access to DuckDB's extension repository); set NATA_SCAN_NO_DUCK=1 to run without the reference arm"; exit 4; }
fi
DUCKFLAG=""; [ "${NATA_SCAN_NO_DUCK:-0}" = 1 ] && DUCKFLAG="--no-duck" && note "WARNING: NATA_SCAN_NO_DUCK=1: no DuckDB reference in any result"

# ---------------------------------------------------------------- 4. the simulated store and its tests
say "simulated object store (SIMULATED): started as a separate process on 127.0.0.1"
# A restart after a preemption finds the previous incarnation's sim.addr on the persistent disk: waiting for "a non-empty file" would
# accept that stale port, and every request would go to a dead server (found by the first GCP run that was preempted, 2026-11-05).
rm -f "$OUT/sim.addr" "$OUT/sim-server.txt"
"$BIN" serve --root "$DATA/layouts" --bucket sim --profile zero --port-file "$OUT/sim.addr" > "$OUT/sim-server.txt" 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 50); do [ -s "$OUT/sim.addr" ] && break; sleep 0.2; done
[ -s "$OUT/sim.addr" ] || { note "the simulated store did not start"; exit 4; }
ADDR=$(cat "$OUT/sim.addr")
grep -q "^LISTEN $ADDR " "$OUT/sim-server.txt" || { note "the address in sim.addr ($ADDR) is not the one the server reported: $(head -1 "$OUT/sim-server.txt")"; exit 4; }
"$BIN" ctl --addr "$ADDR" --stats > /dev/null 2>&1 || { note "the simulated store does not answer at $ADDR"; exit 4; }
note "SIMULATED store listening on $ADDR"
EXP="$DATA/expected"

run_scn() {   # run_scn <name> <objscan args...>   (exit status 1 = a check failed)
  local name=$1; shift
  say "$name (SIMULATED store)"
  timeout 3000 "$BIN" "$@" 2>&1 | tee "$OUT/$name.txt" | grep -E '^(PASS|FAIL|CAUGHT|NOT CAUGHT|RESULT)' | tee -a "$SUMMARY"
  local rc=${PIPESTATUS[0]}
  [ "$rc" -eq 0 ] || { note "$name: FAILED (exit $rc)"; fail 1; }
  return "$rc"
}
run_scn correctness check --addr "$ADDR" --bucket sim --expected "$EXP" $DUCKFLAG
CORRECT_RC=$?
run_scn membound membound --addr "$ADDR" --bucket sim --expected "$EXP" --layout small1
run_scn cancellation cancel --addr "$ADDR" --bucket sim --layout mid1
run_scn faults faults --addr "$ADDR" --bucket sim --expected "$EXP" --layout files100 --victim part-0042
if [ "$CORRECT_RC" -ne 0 ]; then note "correctness failed: a fast wrong answer is not a data point; no timing is run"; exit 3; fi

# ---------------------------------------------------------------- 5. simulated benchmark
say "timing: quiet machine?"
wait_quiet 180
L=$(load1)
SANITY=""
if awk -v l="$L" 'BEGIN{exit !(l>=2)}' || [ "$NCPU" -lt 4 ]; then
  SANITY="--sanity-only"
  note "load average before timing: $L (>= 2 or fewer than 4 CPUs): ALL TIMING BELOW IS SANITY ONLY"
else
  note "load average before timing: $L (< 2): timing is usable, subject to the noise the report states"
fi
mkdir -p "$OUT/sim"
say "throughput and request pattern, SIMULATED store ('gcs' latency profile), $ROUNDS rounds"
timeout 7000 "$BIN" bench --mode sim --addr "$ADDR" --bucket sim --profile gcs --data "$DATA/layouts" --expected "$EXP" \
  --out "$OUT/sim" --rounds "$ROUNDS" $DUCKFLAG $SANITY 2>&1 | tee "$OUT/sim-bench-log.txt" | grep -E '^RESULT|NOT OK' | tee -a "$SUMMARY"
BRC=${PIPESTATUS[0]}
if [ "$BRC" -ne 0 ]; then note "simulated benchmark FAILED (exit $BRC)"; fail 3; fi

# ---------------------------------------------------------------- 6. the real store
say "REAL object store"
REAL_STATUS=$("$BIN" real-status 2>&1); REAL_RC=$?
if [ "$REAL_RC" -eq 3 ]; then
  MSG="REAL STORE NOT MEASURED: none of NATA_OBJSTORE_ENDPOINT, NATA_OBJSTORE_BUCKET, NATA_OBJSTORE_PREFIX is set. The throughput and tail-latency numbers of this run are SIMULATED only."
  echo "$MSG" | tee "$OUT/REAL-STORE-NOT-MEASURED.txt" | tee -a "$SUMMARY"
  if [ "${NATA_REQUIRE_REAL:-0}" = 1 ]; then note "NATA_REQUIRE_REAL=1: failing"; fail 5; fi
elif [ "$REAL_RC" -ne 0 ]; then
  note "REAL STORE NOT MEASURED: the configuration is incomplete ($REAL_STATUS)"; fail 5
else
  note "real store: $REAL_STATUS (endpoint, bucket and prefix are never written to a file)"
  "$BIN" real-check 2>&1 | "$BIN" redact | tee "$OUT/real-check.txt" | tee -a "$SUMMARY"
  if [ "${PIPESTATUS[0]}" -ne 0 ]; then
    echo "REAL STORE NOT MEASURED: the layouts are not readable under the prefix. Upload them first: objscan upload --data <layouts dir> (needs write credentials in the environment)." | tee "$OUT/REAL-STORE-NOT-MEASURED.txt" | tee -a "$SUMMARY"
    fail 5
  else
    mkdir -p "$OUT/real"
    wait_quiet 120
    RSAN=""; awk -v l="$(load1)" 'BEGIN{exit !(l>=2)}' && RSAN="--sanity-only"
    say "throughput and request pattern, REAL store, $ROUNDS rounds"
    timeout 7000 "$BIN" bench --mode real --data "$DATA/layouts" --expected "$EXP" --out "$OUT/real" --rounds "$ROUNDS" $DUCKFLAG $RSAN 2>&1 \
      | "$BIN" redact | tee "$OUT/real-bench-log.txt" | grep -E '^RESULT|NOT OK|NOT MEASURED' | tee -a "$SUMMARY"
    RB=${PIPESTATUS[0]}
    [ "$RB" -eq 0 ] || { note "real benchmark FAILED (exit $RB)"; fail 3; }
  fi
fi

# ---------------------------------------------------------------- 7. results, leak scan
say "leak scan: no configured endpoint, bucket, prefix or key may appear under the results directory"
"$BIN" leakscan --dir "$OUT" 2>&1 | tee -a "$SUMMARY"
[ "${PIPESTATUS[0]}" -eq 0 ] || { note "A CONFIGURED VALUE OF THE REAL STORE IS IN THE RESULTS: delete $OUT before it is copied anywhere"; exit 6; }

{
  echo "# Object-store scan, verification-plan item 27: results of one run"
  echo
  echo "Machine: see machine.txt. Load average before the simulated timing: $L${SANITY:+ (SANITY ONLY)}. Rounds: $ROUNDS. TPC-H SF$SF."
  [ -f "$OUT/REAL-STORE-NOT-MEASURED.txt" ] && { echo; echo "**$(cat "$OUT/REAL-STORE-NOT-MEASURED.txt")**"; }
  echo; echo "## Layouts"; cat "$OUT/layouts.md"
  echo; echo "## Tests (SIMULATED store)"; echo '```'; grep -E '^(PASS|FAIL|CAUGHT|NOT CAUGHT|RESULT)' "$OUT"/correctness.txt "$OUT"/membound.txt "$OUT"/cancellation.txt "$OUT"/faults.txt "$OUT"/selftest.txt 2>/dev/null | sed 's|^[^:]*/||'; echo '```'
  [ -f "$OUT/sim/simulated-summary.md" ] && { echo; cat "$OUT/sim/simulated-summary.md"; }
  [ -f "$OUT/real/real-summary.md" ] && { echo; cat "$OUT/real/real-summary.md"; }
} > "$OUT/RESULTS.md"

say "done"
note "load average now: $(load1)"
note "exit status $FAIL (0 = everything passed; real store: $([ -f "$OUT/real/real-summary.md" ] && echo measured || echo NOT MEASURED))"
exit "$FAIL"
