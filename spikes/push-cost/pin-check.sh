#!/usr/bin/env bash
# pin-check.sh <results-dir>
#
# Why does the scheduler of spikes/operator-trait cost much more with FOUR threads on the x86-64 `c3d-highcpu-8` VM (65, 47, 48 percentage points of `dyn` over
# `static` at 1,024 / 8,192 / 65,536 rows) than on ARM64 (23, 7, 3) and on a quiet Intel desktop machine (21, 8, 22)? That VM has 4 cores with 2 hardware threads each. This
# runs the SAME cells (the cheap kernel, one stage, T4) three ways with `taskset`:
#   unpinned   the operating system chooses (what was measured before)
#   distinct   four threads on four DIFFERENT physical cores (the first hardware thread of each core)
#   siblings   four threads on the two hardware threads of TWO cores (they share execution units and caches)
# If the gap collapses when the threads are on distinct cores and grows on siblings, the cost is hardware-thread sharing, not the scheduler's design. If it stays, the
# scheduler (one Mutex<VecDeque> run queue and a Condvar notified per wake) is the cause on this machine.
# The CPU sets are computed from `lscpu -p` (never assumed from the numbering). A machine with one hardware thread per core cannot run `siblings`: it says so.
#
# Env: PIN_ROUNDS (25), PIN_TOTAL_LOG2 (25), PIN_TOPOLOGY (a file in `lscpu -p=CPU,CORE` format instead of the machine's own; for tests), PIN_SETTLE_MAX (300 s per run, 0 = only print).
# Exit status: 0 the runs completed, 1 a run failed (its own validation or a crash), 4 setup failed.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)

# pin_sets <file in `lscpu -p=CPU,CORE` format> -> prints   threads_per_core=N / distinct=LIST / siblings=LIST|none
pin_sets() {
  grep -v '^#' "$1" | awk -F, '
    { cpu[NR]=$1; core[NR]=$2; if (!($2 in seen)) { seen[$2]=1; order[++nc]=$2 } n[$2]++; members[$2]=members[$2] (members[$2] == "" ? "" : " ") $1 }
    END {
      tpc = 0; for (c in n) if (n[c] > tpc) tpc = n[c]
      printf "threads_per_core=%d\n", tpc
      d = ""; k = 0
      for (i = 1; i <= nc && k < 4; i++) { split(members[order[i]], m, " "); d = d (d == "" ? "" : ",") m[1]; k++ }
      printf "distinct=%s\n", d
      if (tpc < 2) { print "siblings=none"; exit }
      s = ""; k = 0
      for (i = 1; i <= nc && k < 4; i++) { split(members[order[i]], m, " "); for (j = 1; j in m && k < 4; j++) { s = s (s == "" ? "" : ",") m[j]; k++ } }
      printf "siblings=%s\n", s
    }'
}
# gap_row <name> <results.md> -> one table row. Columns of the summary table: | cell | control ns/row | static | dyn-direct | dyn | static-in-sched | twin | quiet |
gap_row() {
  awk -v name="$1" -F'|' '
    function num(x) { gsub(/[ ?+]/, "", x); return x + 0 }
    /^\| cheap L1 B[0-9]+ T4 / { split($2, a, " "); b = a[3]; sub(/^B/, "", b); st[b] = num($4); dy[b] = num($6); gap[b] = dy[b] - st[b]; q = $9; gsub(/ /, "", q); quiet = quiet (quiet == "" ? "" : "/") q }
    END { printf "| %s | %.1f | %.1f | %.1f | %.1f | %.1f | %s |\n", name, gap[1024], gap[8192], gap[65536], st[8192], dy[8192], quiet }' "$2"
}
if [ "${1:-}" = "--gap-row" ]; then gap_row "${2:?name}" "${3:?results.md}"; exit 0; fi
if [ "${1:-}" = "--sets" ]; then pin_sets "${2:?topology file}"; exit 0; fi   # used by the test of this script

OUT=${1:?usage: pin-check.sh <results-dir>}
mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)
ROUNDS=${PIN_ROUNDS:-25}; LOG2=${PIN_TOTAL_LOG2:-25}
SETTLE_SH="$HERE/../../scripts/gcp/settle.sh"
SUMMARY="$OUT/SUMMARY.txt"; : > "$SUMMARY"
note() { printf '%s\n' "$*" | tee -a "$SUMMARY"; }

TOPO=${PIN_TOPOLOGY:-}
if [ -z "$TOPO" ]; then TOPO="$OUT/lscpu-p.txt"; lscpu -p=CPU,CORE > "$TOPO" 2>/dev/null || { note "SETUP FAILED: lscpu -p did not work"; exit 4; }; fi
{ lscpu 2>/dev/null | grep -E "Model name|Thread|Core|Socket|^CPU\(s\)"; echo "kernel: $(uname -r)"; echo "git: $(git -C "$HERE" rev-parse HEAD 2>/dev/null || echo unknown)"; } > "$OUT/machine.txt"
cat "$OUT/machine.txt" | tee -a "$SUMMARY"
eval "$(pin_sets "$TOPO" | sed 's/^/PS_/')"
note "threads per core: $PS_threads_per_core; distinct-core set: $PS_distinct; sibling set: $PS_siblings"
command -v taskset > /dev/null || { note "SETUP FAILED: taskset is not installed"; exit 4; }

# the benchmark example (built by the caller, or here)
BIN="$HERE/target/release/examples/push_cost"
if [ ! -x "$BIN" ]; then ( cd "$HERE" && RUSTFLAGS= cargo build --release --examples 2>&1 | tail -3 ) || { note "SETUP FAILED: build"; exit 4; }; fi
[ -x "$BIN" ] || { note "SETUP FAILED: no $BIN"; exit 4; }

FAILED=0
run_set() { # run_set NAME CPULIST|""   (empty = unpinned)
  local name=$1 cpus=$2
  if [ -x "$SETTLE_SH" ]; then "$SETTLE_SH" "pin-$name" 0.5 "${PIN_SETTLE_MAX:-300}" | tee -a "$SUMMARY"; fi
  note "== $name ${cpus:+(taskset -c $cpus)}"
  local cmd=("$BIN" --rounds "$ROUNDS" --total-log2 "$LOG2" --threads 4 --lens 1 --batches 1024,8192,65536 --kernels cheap --no-branch --csv "$OUT/$name.csv")
  if [ -n "$cpus" ]; then taskset -c "$cpus" "${cmd[@]}" > "$OUT/$name.md" 2> "$OUT/$name.err"; else "${cmd[@]}" > "$OUT/$name.md" 2> "$OUT/$name.err"; fi
  local rc=$?
  if [ $rc -ne 0 ]; then note "RUN FAILED ($name): exit $rc: $(tail -3 "$OUT/$name.err" | tr '\n' ' ')"; FAILED=1; fi
  grep -E "load average at start|SANITY ONLY" "$OUT/$name.md" | head -2 | tee -a "$SUMMARY"
}
run_set unpinned ""
run_set distinct "$PS_distinct"
if [ "$PS_siblings" != none ]; then run_set siblings "$PS_siblings"; else note "== siblings: not run (one hardware thread per core on this machine)"; fi

# the table: dyn minus static, in percentage points, per batch size and pin set
note ""
note "dyn minus static (percentage points over the hand-fused control), cheap kernel, one stage, four threads:"
note "| pin set | B1024 | B8192 | B65536 | static B8192 (+%) | dyn B8192 (+%) | quiet |"
note "|---|---:|---:|---:|---:|---:|---|"
for name in unpinned distinct siblings; do
  [ -f "$OUT/$name.md" ] || continue
  gap_row "$name" "$OUT/$name.md" | tee -a "$SUMMARY"
done
[ $FAILED = 0 ] && note "RESULT pin-check: the runs completed (the table above is the finding; this script has no pass/fail on the numbers)" || note "RESULT pin-check: A RUN FAILED"
exit $FAILED
