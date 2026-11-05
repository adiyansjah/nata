#!/bin/bash
# Tests of IC_MODE=spikes of spikes/iceberg-commit/gcp-check.sh (the `icebergspikes` job): the code side of the Iceberg commit check on
# another machine WITHOUT the 2-hour TLA+ model phase. Needs no network, no JDK, no GCP: the expensive phases are marked as already passed
# (the checks's own IC_RESUME mechanism), the pyiceberg interop phase runs for real against a fake python3 and a fake spike binary.
#   * no model phase runs (model-lint, model-parse-and-edits, model-smoke, model-full, model-mutants) and TLC (fake java) is never called
#   * the code phases run in the same order as in full mode, and interop-pyiceberg is GATING here (a skip or a failure makes the verdict FAIL)
#   * the summary and the verdict say that the model was NOT run
#   * a mode the script does not know is refused
# Against the code before the spikes mode these checks fail (IC_MODE=spikes is refused with exit 2).
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../../.." && pwd)
SPIKE=${ICS_SPIKE_DIR:-$REPO/spikes/iceberg-commit}
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fails=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fails=$((fails + 1)); fi; }

TREE=$T/ic; mkdir -p "$TREE"
cp "$SPIKE/gcp-check.sh" "$TREE/"; cp -r "$SPIKE/scripts" "$SPIKE/model" "$TREE/"; rm -rf "$TREE/model/__pycache__"
cat > "$TREE/scripts/setup-tools.sh" <<STUB
#!/bin/bash
mkdir -p "\$1/jdk/bin"; cp "$SPIKE/model/testdata/fake-java" "\$1/jdk/bin/java"; chmod +x "\$1/jdk/bin/java"; : > "\$1/tla2tools.jar"
echo "stub tools" > "\$1/VERSIONS.txt"; echo "tools stubbed"
STUB
chmod +x "$TREE/scripts/setup-tools.sh"
BIN=$T/bin; mkdir -p "$BIN"
# fake python3: `-m venv DIR` creates a venv whose python exits with FAKE_INTEROP_RC (default 0); FAKE_VENV_FAILS=1 makes `-m venv` fail
cat > "$BIN/python3" <<'STUB'
#!/bin/bash
if [ "$1" = -m ] && [ "$2" = venv ]; then
  [ "${FAKE_VENV_FAILS:-0}" = 1 ] && exit 1
  mkdir -p "$3/bin"; printf '#!/bin/bash\nexit 0\n' > "$3/bin/pip"
  printf '#!/bin/bash\necho "interop check stub"\nexit %s\n' "${FAKE_INTEROP_RC:-0}" > "$3/bin/python"; chmod +x "$3/bin/pip" "$3/bin/python"; exit 0
fi
echo "python3 $*" >> "$FORBIDDEN"; exit 99
STUB
chmod +x "$BIN/python3"
for tool in cargo rustup curl gradle gradlew gcloud ssh; do
  printf '#!/bin/bash\necho "%s $*" >> "%s/forbidden-calls.log"\nexit 99\n' "$tool" "$T" > "$BIN/$tool"; chmod +x "$BIN/$tool"
done
export FORBIDDEN=$T/forbidden-calls.log

n=0
run_check() {
  n=$((n + 1)); local name=$1; shift
  RES=$T/res$n; WORKD=$T/work$n; export FAKE_LOG=$T/calls$n.log; : > "$FAKE_LOG"
  mkdir -p "$WORKD/state" "$WORKD/target/release"
  for p in rust-build rust-tests catalog-build catalog-start spike-rest-catalog spike-object-store; do : > "$WORKD/state/$p.pass"; done
  printf '#!/bin/bash\necho "{}"\n' > "$WORKD/target/release/ic-spike"; chmod +x "$WORKD/target/release/ic-spike"
  echo "http://127.0.0.1:1" > "$WORKD/catalog-url"
  env PATH="$BIN:$PATH" IC_WORK="$WORKD" IC_RESUME=1 IC_MODE=spikes "$@" bash "$TREE/gcp-check.sh" "$RES" > "$T/out$n.txt" 2>&1
  RC=$?; SUM=$RES/summary.txt
}
phases() { grep -o '^PHASE [a-z-]*' "$SUM" | cut -d' ' -f2 | tr '\n' ' '; }

run_check pass
check "spikes mode: exit 0 and VERDICT PASS when the code phases and the interop pass" "[ $RC -eq 0 ] && grep -q '^VERDICT PASS' '$SUM'"
check "it runs the code phases in the order of the full check, and no model phase" "[ \"\$(phases)\" = 'tools rust-build rust-tests catalog-build catalog-start spike-rest-catalog interop-pyiceberg spike-object-store ' ]"
check "no model phase appears (lint, parse, smoke, full, mutants)" "! grep -q -E '^PHASE model' '$SUM'"
check "TLC (fake java) was never called" "[ ! -s '$FAKE_LOG' ]"
check "interop-pyiceberg really ran (not resumed) and is gating" "grep -q '^PHASE interop-pyiceberg PASS .*gating=yes' '$SUM'"
check "the summary says the model was NOT run, and the verdict says so too" "grep -q 'model: NOT RUN in mode spikes' '$SUM' && grep -q '^VERDICT PASS.*NOT run' '$SUM'"
check "the summary has the interop section and no model table" "grep -q 'interop: pyiceberg reads our manifests' '$SUM' && ! grep -q 'unmutated runs' '$SUM'"
check "no tool that the stubs forbid was called" "[ ! -e '$FORBIDDEN' ]"

run_check venvfail FAKE_VENV_FAILS=1
check "interop SKIPPED because python3 -m venv failed: the verdict is FAIL (gating in this mode)" "[ $RC -eq 1 ] && grep -q '^PHASE interop-pyiceberg FAIL (exit 77, .*gating=yes' '$SUM' && grep -q '^VERDICT FAIL' '$SUM'"
run_check skip3 FAKE_INTEROP_RC=3
check "interop check exit 3 (pyiceberg not usable) is FAIL, not a quiet skip" "[ $RC -eq 1 ] && grep -q '^PHASE interop-pyiceberg FAIL (exit 77' '$SUM'"
run_check interopfail FAKE_INTEROP_RC=1
check "interop check exit 1 (pyiceberg reads a different table) is FAIL" "[ $RC -eq 1 ] && grep -q '^PHASE interop-pyiceberg FAIL (exit 1' '$SUM'"
run_check badmode IC_MODE=nonsense
check "an unknown IC_MODE is refused (exit 2) and the message lists spikes" "[ $RC -eq 2 ] && grep -q 'full, model or spikes' '$T/out$n.txt'"
check "in the full mode interop-pyiceberg is still not gating (unchanged)" "grep -q 'phase interop-pyiceberg no ph_interop' '$TREE/gcp-check.sh'"

# The job runs as the unprivileged user `nata`, so python3-venv (for the pyiceberg interop) must be installed by vm-startup.sh for every job that
# runs the interop. Found 2026-11-05: icebergspikes was missing there, its interop would have been SKIPPED = FAIL on both VMs.
VS=$REPO/scripts/gcp/vm-startup.sh
for j in iceberg iceberg-smoke icebergspikes icebergpolaris; do
  grep 'python3-venv' "$VS" | grep -qE "in ([a-z-]+\|)*$j(\|[a-z-]+)*\)" && ok_=1 || ok_=0
  check "vm-startup.sh installs python3-venv for the job $j" "[ $ok_ = 1 ]"
done
[ "$fails" = 0 ] && echo "all icebergspikes tests passed" || { echo "ICEBERGSPIKES TESTS FAILED ($fails)"; exit 1; }
