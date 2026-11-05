#!/usr/bin/env bash
# Wait until the 1-minute load average is below a limit, so that a timed block starts on a quiet machine (ADR 0006, rule 7:
# a timing counts only if the load at its start is below 0.5 and is printed next to the result).
#
#   settle.sh LABEL [LIMIT=0.5] [MAX_SECONDS=600]
#
# Prints exactly one line on stdout:
#   SETTLE <label>: settled, load 0.31 after 40 s (limit 0.5)
#   SETTLE <label>: NOT SETTLED, load 1.20 after 600 s (limit 0.5)
# Exit status 0 if settled, 1 if not, 2 for a usage error. A caller that wants to carry on anyway (the job must keep its other
# results) ends the call with `|| true`; the NOT SETTLED line is then the record that the block that follows is SANITY ONLY.
#
# MAX_SECONDS=0 only reads the load once (no waiting): scripts that also run on a development machine use it as their default.
# Test hooks: SETTLE_LOADAVG (file to read instead of /proc/loadavg), SETTLE_POLL (seconds between reads, default 5).
set -u
label=${1:-}; limit=${2:-0.5}; max=${3:-600}
if [ -z "$label" ]; then echo "usage: settle.sh LABEL [LIMIT] [MAX_SECONDS]" >&2; exit 2; fi
case "$max" in ''|*[!0-9]*) echo "settle.sh: MAX_SECONDS must be a whole number, got '$max'" >&2; exit 2 ;; esac
awk -v l="$limit" 'BEGIN { exit !(l + 0 > 0) }' 2>/dev/null || { echo "settle.sh: LIMIT must be a positive number, got '$limit'" >&2; exit 2; }
file=${SETTLE_LOADAVG:-/proc/loadavg}; poll=${SETTLE_POLL:-5}
waited=0
while :; do
  load=$(cut -d' ' -f1 "$file" 2>/dev/null)
  # an unreadable or non-numeric load is never "settled"
  if awk -v l="${load:-x}" -v lim="$limit" 'BEGIN { exit !(l ~ /^[0-9]+(\.[0-9]+)?$/ && l + 0 < lim + 0) }'; then
    echo "SETTLE $label: settled, load ${load} after ${waited} s (limit ${limit})"; exit 0
  fi
  if [ "$waited" -ge "$max" ]; then
    echo "SETTLE $label: NOT SETTLED, load ${load:-unknown} after ${waited} s (limit ${limit})"; exit 1
  fi
  sleep "$poll"; waited=$((waited + poll))
done
