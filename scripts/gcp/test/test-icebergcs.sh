#!/bin/bash
# Tests of IC_MODE=gcs of spikes/iceberg-commit/gcp-check.sh (the `icebergcs` job): the catalog-less commit suite with the metadata
# versions in a real GCS bucket, and the probes of what the VM's identity may do. No network, no GCP, no Rust build: a fake curl plays
# the metadata server and the GCS JSON API, a fake spike binary plays the suite, and the build phase is marked as passed (IC_RESUME).
#   * the phases are gcs-config, gcs-rust-build, gcs-probes, spike-object-store-gcs, in that order, and nothing of the full check
#   * the suite gets NATA_STORE_URL=gs://<bucket>/ic-suite/<tag>
#   * each probe that does not give its expected status makes the verdict FAIL (foreign write allowed, create-only not enforced, ...)
#   * a missing gcs.env, a failing suite, or the project id anywhere in the results is a FAIL; the token is never printed
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
SPIKE=${ICS_SPIKE_DIR:-$REPO/spikes/iceberg-commit}
T=$(mktemp -d); [ -n "${KEEP:-}" ] || trap 'rm -rf "$T"' EXIT; echo "tmp $T" >&2
fails=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fails=$((fails + 1)); fi; }
TREE=$T/ic; mkdir -p "$TREE"; cp "$SPIKE/gcp-check.sh" "$TREE/"; cp -r "$SPIKE/scripts" "$TREE/"
BIN=$T/bin; mkdir -p "$BIN"
cat > "$BIN/curl" <<'STUB'
#!/bin/bash
url=""; method=GET; w=0
while [ $# -gt 0 ]; do case "$1" in -X) method=$2; shift 2 ;; -w) w=1; shift 2 ;; -o|-m|-H|--data|--data-binary) shift 2 ;; -s) shift ;; *) url=$1; shift ;; esac; done
case "$url" in
  *computeMetadata*token*) echo '{"access_token":"FAKE-TOKEN-VALUE-0123","expires_in":3599}'; exit 0 ;;
  *computeMetadata*project-id*) printf '%s' "${FAKE_PID:-fakeproj-424242}"; exit 0 ;;
esac
code=000
case "$method $url" in
  "POST "*"/upload/storage/v1/b/gcp-public-data-landsat/"*) code=${FAKE_P3:-403} ;;
  "POST "*"/upload/storage/v1/b/"*"ifGenerationMatch=0"*) n=$(cat "$FAKE_STATE/creates" 2>/dev/null || echo 0); n=$((n + 1)); echo $n > "$FAKE_STATE/creates"
                                                          if [ $n = 1 ]; then code=${FAKE_P1:-200}; else code=${FAKE_P2:-412}; fi ;;
  "POST "*"/storage/v1/b?project="*) code=${FAKE_P4:-403} ;;
  "GET "*"/storage/v1/b?project="*) code=${FAKE_P5:-403} ;;
  "DELETE "*) code=${FAKE_P6:-204} ;;
esac
[ $w = 1 ] && printf '%s' "$code"; exit 0
STUB
chmod +x "$BIN/curl"
for tool in cargo rustup gcloud ssh java; do printf '#!/bin/bash\necho "%s $*" >> "%s/forbidden-calls.log"\nexit 99\n' "$tool" "$T" > "$BIN/$tool"; chmod +x "$BIN/$tool"; done
FORBIDDEN=$T/forbidden-calls.log
n=0
run_check() {
  n=$((n + 1)); shift; local noenv=0 a; for a in "$@"; do [ "$a" = NO_ENV=1 ] && noenv=1; done; RES=$T/res$n; WORKD=$T/work$n; export FAKE_STATE=$T/state$n; mkdir -p "$FAKE_STATE" "$WORKD/state" "$WORKD/target/release"
  : > "$WORKD/state/gcs-rust-build.pass"
  # the fake suite records the URL it got and exits with FAKE_SUITE_RC; FAKE_SUITE_PRINT is printed (to plant a leak)
  printf '#!/bin/bash\necho "NATA_STORE_URL=$NATA_STORE_URL" > "%s/suite-env"\necho "${FAKE_SUITE_PRINT:-suite stub}"\nexit ${FAKE_SUITE_RC:-0}\n' "$FAKE_STATE" > "$WORKD/target/release/ic-spike"
  chmod +x "$WORKD/target/release/ic-spike"
  if [ "$noenv" = 1 ]; then rm -f "$TREE/gcs.env"; else echo "NATA_WTEST_BUCKET=fake-wtest-bucket" > "$TREE/gcs.env"; fi
  env PATH="$BIN:$PATH" IC_WORK="$WORKD" IC_RESUME=1 IC_MODE=gcs "$@" bash "$TREE/gcp-check.sh" "$RES" > "$T/out$n.txt" 2>&1
  RC=$?; SUM=$RES/summary.txt
}
phases() { grep -o '^PHASE [a-z-]*' "$SUM" | cut -d' ' -f2 | tr '\n' ' '; }

run_check pass
check "gcs mode: exit 0 and VERDICT PASS when the probes and the suite are as expected" "[ $RC -eq 0 ] && grep -q '^VERDICT PASS' '$SUM'"
check "the phases are gcs-config, gcs-rust-build, gcs-probes, spike-object-store-gcs and nothing else" "[ \"\$(phases)\" = 'gcs-config gcs-rust-build gcs-probes spike-object-store-gcs ' ]"
check "the suite got NATA_STORE_URL=gs://<bucket>/ic-suite/<tag>" "grep -qE '^NATA_STORE_URL=gs://fake-wtest-bucket/ic-suite/[0-9]{8}-[0-9]{6}-[0-9a-f]{6}$' '$FAKE_STATE/suite-env'"
check "the six probes are in the summary with their HTTP codes" "[ \$(grep -cE '^P[1-6] .*HTTP [0-9]{3}' '$SUM') -eq 6 ]"
check "the access token is nowhere in the results or the output" "! grep -rqF FAKE-TOKEN-VALUE '$RES' '$T/out$n.txt'"
check "the project id is nowhere in the results" "! grep -rqF fakeproj-424242 '$RES'"
check "no forbidden tool was called (no cargo, gcloud, ssh, java)" "[ ! -e '$FORBIDDEN' ]"
run_check foreign FAKE_P3=200
check "a write to a bucket that is not ours succeeding (P3 200) makes the verdict FAIL" "[ $RC -eq 1 ] && grep -q '^PHASE gcs-probes FAIL' '$SUM' && grep -q '^VERDICT FAIL' '$SUM'"
run_check nocas FAKE_P2=200
check "a second create-only write that is NOT refused (P2 200) makes the verdict FAIL" "[ $RC -eq 1 ] && grep -q '^PHASE gcs-probes FAIL' '$SUM'"
run_check mkbucket FAKE_P4=200
check "being allowed to create a bucket (P4 200) makes the verdict FAIL" "[ $RC -eq 1 ] && grep -q '^PHASE gcs-probes FAIL' '$SUM'"
run_check noenv NO_ENV=1
check "a missing gcs.env fails gcs-config and the verdict" "[ $RC -eq 1 ] && grep -q '^PHASE gcs-config FAIL' '$SUM'"
run_check suitefail FAKE_SUITE_RC=1
check "a failing commit suite on GCS makes the verdict FAIL" "[ $RC -eq 1 ] && grep -q '^PHASE spike-object-store-gcs FAIL' '$SUM'"
run_check leak FAKE_SUITE_PRINT=fakeproj-424242
check "the project id in the results is caught and makes the verdict FAIL" "[ $RC -eq 1 ] && grep -q 'THE PROJECT ID IS IN THE RESULTS' '$SUM'"
[ "$fails" -eq 0 ] && echo "all icebergcs tests passed" || { echo "ICEBERGCS TESTS FAILED ($fails)"; exit 1; }
