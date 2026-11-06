#!/usr/bin/env bash
# Verification-plan item 9: the worker pool of `nata scan` sizes itself to the CPUs the process may really use, not to the machine's cores.
#   cgroup-check.sh <nata binary> <parquet file> <out dir>
# Runs `nata scan --count` without --threads (so the pool takes its default, std::thread::available_parallelism) inside a systemd scope
# with a CPU quota (cgroup v2 cpu.max) of 100%, 200%, 350% and 600%, under an affinity mask (taskset) of 3 CPUs, under both together, and
# without limits, and reads the thread count from the scan's report. Pass: for a quota of Q percent the pool has floor(Q/100) or
# ceil(Q/100) threads (at least 1); under a mask of k CPUs at most k; without limits all of nproc; the row count is the same in every case.
# Needs root (sudo) for systemd-run --scope with a quota. Exit 0 all pass, 1 a check failed, 4 setup failed.
set -uo pipefail
NATA=$(realpath "${1:?nata}"); F=$(realpath "${2:?parquet file}"); OUT=${3:?out dir}; mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)
S=$OUT/summary.txt; : > "$S"; FAILED=0; N=$(nproc)
log() { echo "$*" | tee -a "$S"; }
sudo -n true 2> /dev/null || { log "SETUP FAIL: no passwordless sudo"; exit 4; }
log "machine: $N CPUs; $(grep -m1 -i 'model name' /proc/cpuinfo 2>/dev/null || lscpu | grep -m1 'Model name'); cgroup: $(stat -fc %T /sys/fs/cgroup)"
run() { # label prefix... -> sets T (threads) and R (rows)
  local out; out=$("$@" "$NATA" scan "$F" --count --columns l_orderkey 2> "$OUT/.err"); R=$out
  T=$(grep -o 'threads=[0-9]*' "$OUT/.err" | cut -d= -f2); [ -n "$T" ] || T=none
}
run env; base_rows=$R; log "no limit: threads=$T rows=$R"; [ "$T" = "$N" ] || { log "CHECK no-limit FAIL (expected $N)"; FAILED=1; }
for q in 100 200 350 600; do
  run sudo systemd-run --scope -q --uid="$(id -u)" --gid="$(id -g)" -p CPUQuota=${q}% --setenv=PATH="$PATH"
  lo=$((q / 100)); hi=$(( (q + 99) / 100 )); [ "$lo" -ge 1 ] || lo=1
  if [ "$T" != none ] && [ "$T" -ge "$lo" ] && [ "$T" -le "$hi" ] && [ "$R" = "$base_rows" ]; then log "CHECK quota-${q} PASS threads=$T (allowed $lo..$hi)"; else log "CHECK quota-${q} FAIL threads=$T rows=$R (allowed $lo..$hi, rows $base_rows)"; FAILED=1; fi
done
run taskset -c 0-2
if [ "$T" = 3 ] && [ "$R" = "$base_rows" ]; then log "CHECK affinity-3 PASS threads=$T"; else log "CHECK affinity-3 FAIL threads=$T"; FAILED=1; fi
run sudo systemd-run --scope -q --uid="$(id -u)" --gid="$(id -g)" -p CPUQuota=200% taskset -c 0-5
if [ "$T" != none ] && [ "$T" -le 2 ] && [ "$T" -ge 1 ]; then log "CHECK quota-200-with-affinity-6 PASS threads=$T"; else log "CHECK quota-200-with-affinity-6 FAIL threads=$T"; FAILED=1; fi
# an explicit --threads is obeyed even above the quota (the user asked); it is not the default
out=$(sudo systemd-run --scope -q --uid="$(id -u)" --gid="$(id -g)" -p CPUQuota=100% "$NATA" scan "$F" --count --columns l_orderkey --threads 4 2>&1 >/dev/null | grep -o 'threads=[0-9]*')
log "explicit --threads 4 under a 100% quota: $out (informational)"
rm -f "$OUT/.err"
[ "$FAILED" = 0 ] && { log "VERDICT PASS"; exit 0; }; log "VERDICT FAIL"; exit 1
