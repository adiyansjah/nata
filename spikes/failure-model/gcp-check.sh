#!/bin/bash
# Verification-plan item 29 on a clean Debian VM (GCP): the QueryLifecycle model (TLC): every real configuration, the coverage canaries
# and the mutants. Reuses the pinned tools of the iceberg-commit spike (Temurin JDK 17, tla2tools.jar, checksum-verified).
#   usage: spikes/failure-model/gcp-check.sh <results-dir>
# Environment (optional): FM_ONLY comma list of configurations to run instead of all (the canaries still run); FM_WORK scratch dir; FM_TIMEOUT seconds per TLC run (default 3600); FM_HEAP_MB (default 24000).
# Exit status 0 only if the real configurations pass, every canary is violated and every mutant is CAUGHT.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
OUT=${1:?usage: gcp-check.sh <results-dir>}; mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)
IC=$HERE/../iceberg-commit/scripts
. "$IC/lib.sh"; . "$IC/tools.env"
WORKBASE=${FM_WORK:-${WORK:+$WORK/failure-model-work}}; WORKBASE=${WORKBASE:-$HOME/failure-model-work}; mkdir -p "$WORKBASE"
TOOLS=$WORKBASE/tools; mkdir -p "$TOOLS"
FAILED=0; SUMMARY=$OUT/summary.txt; : > "$SUMMARY"
log() { printf '%s\n' "$*" | tee -a "$SUMMARY"; }
t=$(arch_tag); url=JDK_URL_$t; sha=JDK_SHA256_$t
if [ ! -x "$TOOLS/jdk/bin/java" ]; then
  fetch_verified "${!url}" "${!sha}" "$TOOLS/jdk.tgz" && mkdir -p "$TOOLS/jdk" && tar -xzf "$TOOLS/jdk.tgz" -C "$TOOLS/jdk" --strip-components=1 || { log "FAIL tools: JDK"; exit 1; }
fi
fetch_verified "$TLA_URL" "$TLA_SHA256" "$TOOLS/tla2tools.jar" || { log "FAIL tools: tla2tools.jar"; exit 1; }
log "tools ok: $("$TOOLS/jdk/bin/java" -version 2>&1 | head -1); host $(uname -m), $(nproc) vCPU, $(awk '/MemTotal/{printf "%d MiB",$2/1024}' /proc/meminfo)"
cd "$HERE/model" || exit 1
C="python3 -I check.py"; A="--java $TOOLS/jdk/bin/java --jar $TOOLS/tla2tools.jar --workers $(nproc) --timeout ${FM_TIMEOUT:-3600} --heap-mb ${FM_HEAP_MB:-24000}"
python3 -I -c "import ast,sys; ast.parse(open('check.py').read())" || { log "FAIL python syntax"; exit 1; }
log "== model (real configurations and canaries)"
$C model $A ${FM_ONLY:+--only $FM_ONLY} --out "$OUT/model" 2>&1 | tee -a "$SUMMARY"; [ "${PIPESTATUS[0]}" = 0 ] && log "PASS model" || { log "FAIL model"; FAILED=1; }
log "== mutants"
$C mutants $A --out "$OUT/mutants" 2>&1 | tee -a "$SUMMARY"; [ "${PIPESTATUS[0]}" = 0 ] && log "PASS mutants" || { log "FAIL mutants"; FAILED=1; }
[ $FAILED = 0 ] && log "VERDICT: all gating phases passed" || log "VERDICT: FAILED (see above)"
exit $FAILED
