#!/bin/bash
# Tests of the Apache Polaris support of spikes/iceberg-commit (job `icebergpolaris`): scripts/polaris.sh and IC_MODE=polaris of gcp-check.sh, against a FAKE Polaris
# server (fake-polaris-server.py) that insists on what a careless script gets wrong. No network, no JDK, no GCP, no real Polaris.
#   polaris.sh: the bootstrap credentials in the three-field form, the token request, the order and content of the management calls (catalog with FILE storage and
#     an allowed location, then the grant, then the role assignment), the files and their modes, that the client secret appears nowhere but the two mode-600 files,
#     a failing call makes start fail and leaves no server running, stop stops it.
#   gcp-check.sh IC_MODE=polaris: exactly the Polaris phases run (no model, no Apache fixture), credentials reach the suite and the interop through the environment,
#     a failing suite or a skipped interop is a FAIL (both gating), a secret in the results is a FAIL, an unknown mode is refused.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
SPIKE=${ICP_SPIKE_DIR:-$REPO/spikes/iceberg-commit}
T=$(mktemp -d); trap 'pkill -f "[f]ake-polaris-server" 2>/dev/null; rm -rf "$T"' EXIT
fails=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fails=$((fails + 1)); fi; }

TREE=$T/ic; mkdir -p "$TREE"
cp "$SPIKE/gcp-check.sh" "$TREE/"; cp -r "$SPIKE/scripts" "$SPIKE/model" "$TREE/"; rm -rf "$TREE/model/__pycache__"
# a fake installed distribution: the launcher is the fake server
TOOLS=$T/tools; mkdir -p "$TOOLS/jdk21/bin" "$TOOLS/polaris/bin"
printf '#!/bin/bash\nexit 0\n' > "$TOOLS/jdk21/bin/java"; chmod +x "$TOOLS/jdk21/bin/java"
printf '#!/bin/bash\nexec python3 -I %s\n' "$HERE/fake-polaris-server.py" > "$TOOLS/polaris/bin/server"; chmod +x "$TOOLS/polaris/bin/server"
export FAKE_LOG=$T/requests.log; : > "$FAKE_LOG"

# ------------------------------------------------------------------------------------------------------------------------ polaris.sh
RUN=$T/run1; P1=18181
OUT1=$(unset FAKE_FAIL; "$TREE/scripts/polaris.sh" start "$TOOLS" "$RUN" $P1 2> "$T/err1.txt"); RC=$?
check "start succeeds and prints the REST base URL (and only that) on stdout" "[ $RC -eq 0 ] && [ '$OUT1' = 'http://127.0.0.1:$P1/api/catalog' ]"
SEC=$(cut -d' ' -f2 "$RUN/polaris.creds" 2>/dev/null); CID=$(cut -d' ' -f1 "$RUN/polaris.creds" 2>/dev/null)
check "the client secret is random and 32 hex digits long" "[ \${#SEC} -eq 32 ] && echo '$SEC' | grep -qE '^[0-9a-f]{32}\$'"
check "polaris.creds and polaris.env are mode 600" "[ \"\$(stat -c %a '$RUN/polaris.creds')\" = 600 ] && [ \"\$(stat -c %a '$RUN/polaris.env')\" = 600 ]"
check "polaris.env carries the client id, the secret and the catalog name for the suite" "grep -q '^export IC_REST_CLIENT_ID=$CID\$' '$RUN/polaris.env' && grep -q '^export IC_REST_CLIENT_SECRET=$SEC\$' '$RUN/polaris.env' && grep -q '^export IC_REST_WAREHOUSE=spikecat\$' '$RUN/polaris.env'"
check "the secret is NOT in stdout, stderr, the server log or the properties file" "[ -n '$SEC' ] && ! grep -qaF '$SEC' '$T/err1.txt' && ! grep -qaF '$SEC' '$RUN/polaris.log' && ! grep -qaF '$SEC' '$RUN/polaris.properties' && [ '$OUT1' != *'$SEC'* ]"
check "the properties file allows FILE storage, acknowledges the readiness issues and listens on loopback only" "grep -q 'ALLOW_INSECURE_STORAGE_TYPES\"=true' '$RUN/polaris.properties' && grep -q 'SUPPORTED_CATALOG_STORAGE_TYPES\"=\[\"FILE\"' '$RUN/polaris.properties' && grep -q 'polaris.readiness.ignore-severe-issues=true' '$RUN/polaris.properties' && grep -q 'quarkus.http.host=127.0.0.1' '$RUN/polaris.properties'"
check "the management port is the API port + 1" "grep -q 'quarkus.management.port=$((P1 + 1))' '$RUN/polaris.properties'"
tok=$(grep -n 'oauth/tokens' "$FAKE_LOG" | head -1 | cut -d: -f1); cat_=$(grep -n 'POST /api/management/v1/catalogs' "$FAKE_LOG" | head -1 | cut -d: -f1)
gr=$(grep -n 'PUT /api/management/v1/catalogs/spikecat/catalog-roles/catalog_admin/grants' "$FAKE_LOG" | head -1 | cut -d: -f1); as=$(grep -n 'PUT /api/management/v1/principal-roles/service_admin/catalog-roles/spikecat' "$FAKE_LOG" | head -1 | cut -d: -f1)
check "the calls come in order: token, create catalog, grant, role assignment" "[ -n '$tok' ] && [ -n '$cat_' ] && [ -n '$gr' ] && [ -n '$as' ] && [ '$tok' -lt '$cat_' ] && [ '$cat_' -lt '$gr' ] && [ '$gr' -lt '$as' ]"
PID=$(cat "$RUN/polaris.pid" 2>/dev/null)
check "the server runs after start" "kill -0 '$PID' 2>/dev/null"
"$TREE/scripts/polaris.sh" start "$TOOLS" "$RUN" $P1 > /dev/null 2>&1; RC=$?
check "a second start while one runs is refused" "[ $RC -ne 0 ]"
"$TREE/scripts/polaris.sh" stop "$RUN" > /dev/null 2>&1
check "stop ends the server and removes the pid file" "! kill -0 '$PID' 2>/dev/null && [ ! -e '$RUN/polaris.pid' ]"
"$TREE/scripts/polaris.sh" stop "$RUN" > /dev/null 2>&1; check "stop twice is harmless" "[ $? -eq 0 ]"

for w in grants catalogs; do
  RUN2=$T/run-fail-$w; : > "$FAKE_LOG"
  FAKE_FAIL=$w "$TREE/scripts/polaris.sh" start "$TOOLS" "$RUN2" 18183 > "$T/out-f.txt" 2> "$T/err-f.txt"; RC=$?
  PID2=$(cat "$RUN2/polaris.pid" 2>/dev/null)
  check "a failing management call ($w) makes start fail with a message, prints no URL" "[ $RC -ne 0 ] && [ ! -s '$T/out-f.txt' ] && grep -q 'failed\\|returned' '$T/err-f.txt'"
  check "...and leaves no server running ($w)" "[ -z '$PID2' ] || ! kill -0 '$PID2' 2>/dev/null"
done

# ------------------------------------------------------------------------------------------------------------------------ gcp-check.sh IC_MODE=polaris
BIN=$T/bin; mkdir -p "$BIN"
cat > "$BIN/python3" <<'STUB'
#!/bin/bash
# fake `python3 -m venv DIR` (a venv whose python exits with FAKE_INTEROP_RC, and records that the catalog environment reached it); everything else is the real python3
if [ "$1" = -m ] && [ "$2" = venv ]; then
  [ "${FAKE_VENV_FAILS:-0}" = 1 ] && exit 1
  mkdir -p "$3/bin"; printf '#!/bin/bash\nexit 0\n' > "$3/bin/pip"
  printf '#!/bin/bash\n[ -n "$IC_REST_CLIENT_ID" ] && [ -n "$IC_REST_CLIENT_SECRET" ] && [ -n "$IC_REST_WAREHOUSE" ] && echo interop-env-ok > "%s/interop-env.txt"\necho "interop check stub"\nexit %s\n' "$TESTDIR" "${FAKE_INTEROP_RC:-0}" > "$3/bin/python"
  chmod +x "$3/bin/pip" "$3/bin/python"; exit 0
fi
exec /usr/bin/python3 "$@"
STUB
chmod +x "$BIN/python3"
for tool in cargo rustup gradle gradlew gcloud ssh; do printf '#!/bin/bash\necho "%s $*" >> "%s/forbidden-calls.log"\nexit 99\n' "$tool" "$T" > "$BIN/$tool"; chmod +x "$BIN/$tool"; done
export TESTDIR=$T
# the installer is stubbed (the real one downloads ~630 MB): it only has to leave the fake distribution in place
cat > "$TREE/scripts/setup-polaris.sh" <<STUB
#!/bin/bash
mkdir -p "\$1/jdk21/bin" "\$1/polaris/bin"; cp "$TOOLS/jdk21/bin/java" "\$1/jdk21/bin/java"; cp "$TOOLS/polaris/bin/server" "\$1/polaris/bin/server"
echo "stub polaris installed" > "\$1/POLARIS.txt"; echo "stub"
STUB
chmod +x "$TREE/scripts/setup-polaris.sh"

n=0
run_check() {
  n=$((n + 1)); local name=$1; shift
  RES=$T/res$n; WORKD=$T/work$n; : > "$FAKE_LOG"; rm -f "$T/suite-env.txt" "$T/interop-env.txt"
  mkdir -p "$WORKD/state" "$WORKD/target/release"; : > "$WORKD/state/rust-build.pass"
  cat > "$WORKD/target/release/ic-spike" <<'STUB'
#!/bin/bash
# fake spike: records that the catalog credentials reached it through the environment (not their values)
case "$1" in
  suite) [ -n "$IC_REST_CLIENT_ID" ] && [ -n "$IC_REST_CLIENT_SECRET" ] && [ "$IC_REST_WAREHOUSE" = spikecat ] && echo "suite-env-ok $5" > "$TESTDIR/suite-env.txt"
         [ "${FAKE_LEAK:-0}" = 1 ] && echo "oops $IC_REST_CLIENT_SECRET"
         [ "${FAKE_SUITE_FAIL:-0}" = 1 ] && { echo "SUITE FAIL"; exit 1; }
         echo "SUMMARY backend=rest forced=18 (known-bad writers caught 5/5) crash-matrix=12 stress-correct=6/6 failures=0"; echo "SUITE PASS"; exit 0 ;;
  interop-table) echo '{}'; exit 0 ;;
esac
exit 0
STUB
  chmod +x "$WORKD/target/release/ic-spike"
  env PATH="$BIN:$PATH" IC_WORK="$WORKD" IC_RESUME=1 IC_MODE=polaris IC_POLARIS_PORT=18185 "$@" bash "$TREE/gcp-check.sh" "$RES" > "$T/out$n.txt" 2>&1
  RC=$?; SUM=$RES/summary.txt
}
phases() { grep -o '^PHASE [a-z-]*' "$SUM" | cut -d' ' -f2 | tr '\n' ' '; }

run_check pass
check "polaris mode: exit 0 and VERDICT PASS" "[ $RC -eq 0 ] && grep -q '^VERDICT PASS.*Apache Polaris' '$SUM'"
check "exactly the Polaris phases run, in order, and no model or Apache-fixture phase" "[ \"\$(phases)\" = 'polaris-tools rust-build polaris-start spike-polaris-suite interop-pyiceberg-polaris ' ]"
check "both the suite and the interop are gating" "grep -q '^PHASE spike-polaris-suite PASS .*gating=yes' '$SUM' && grep -q '^PHASE interop-pyiceberg-polaris PASS .*gating=yes' '$SUM'"
check "the suite got the catalog URL and the credentials through the environment" "grep -q 'suite-env-ok http://127.0.0.1:18185/api/catalog' '$T/suite-env.txt'"
check "the interop (pyiceberg side) got the credentials through the environment" "[ -s '$T/interop-env.txt' ]"
check "the summary has the Polaris sections and the secret scan line" "grep -q 'spike: Apache Polaris REST catalog' '$SUM' && grep -q 'secret scan: the Polaris client secret appears nowhere' '$SUM'"
check "the server is stopped at the end" "! pgrep -f '[f]ake-polaris-server' > /dev/null"
check "no forbidden tool (cargo, gradle, ...) was called" "[ ! -e '$T/forbidden-calls.log' ]"
check "the Polaris version file is copied to the results" "[ -s '$RES/polaris-version.txt' ]"

run_check suitefail FAKE_SUITE_FAIL=1
check "a failing suite is a FAIL (gating)" "[ $RC -eq 1 ] && grep -q '^PHASE spike-polaris-suite FAIL' '$SUM' && grep -q '^VERDICT FAIL' '$SUM'"
run_check venvfail FAKE_VENV_FAILS=1
check "a skipped interop (python3 -m venv failed) is a FAIL, not a quiet skip" "[ $RC -eq 1 ] && grep -q '^PHASE interop-pyiceberg-polaris FAIL (exit 77' '$SUM'"
run_check interopfail FAKE_INTEROP_RC=1
check "pyiceberg reading a different table is a FAIL" "[ $RC -eq 1 ] && grep -q '^PHASE interop-pyiceberg-polaris FAIL (exit 1' '$SUM'"
run_check leak FAKE_LEAK=1
check "the client secret in the results makes the run FAIL, and says so" "[ $RC -eq 1 ] && grep -q 'THE POLARIS CLIENT SECRET IS IN THE RESULTS' '$SUM' && grep -q '^VERDICT FAIL' '$SUM'"
run_check badmode IC_MODE=nonsense
check "an unknown IC_MODE is refused (exit 2) and the message lists polaris" "[ $RC -eq 2 ] && grep -q 'full, model, spikes or polaris' '$T/out$n.txt'"

[ "$fails" = 0 ] && echo "all icebergpolaris tests passed" || { echo "ICEBERGPOLARIS TESTS FAILED ($fails)"; exit 1; }
