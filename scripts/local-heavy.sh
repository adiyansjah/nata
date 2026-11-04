#!/bin/bash
# Run ONE heavy command at a time on this machine (builds, tests, loom, model checking, mutation runs, benchmarks).
# Every heavy command takes the same lock, so two of them never run together, whoever started them.
#   usage: scripts/local-heavy.sh <command> [args...]
# A command that waits for the lock prints who holds it. Anything expected to run longer than about 10 minutes does not
# belong here: write down the command and its expected duration, and it goes to a GCP VM (scripts/gcp/).
LOCK=${NATA_HEAVY_LOCK:-/tmp/nata-heavy.lock}
exec 9> "$LOCK" || exit 2
if ! flock -n 9; then
  echo "[local-heavy] waiting for the heavy-command lock ($LOCK); held by: $(cat "$LOCK.owner" 2>/dev/null || echo unknown)" >&2
  flock -w "${NATA_HEAVY_WAIT:-7200}" 9 || { echo "[local-heavy] gave up waiting" >&2; exit 3; }
fi
printf '%s (pid %s) since %s\n' "$*" "$$" "$(date -u +%H:%M:%S)" > "$LOCK.owner"
"$@"; rc=$?
rm -f "$LOCK.owner"
exit $rc
