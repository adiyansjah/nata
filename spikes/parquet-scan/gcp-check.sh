#!/usr/bin/env bash
# gcp-check.sh <results-dir>
#
# The baseline spike of docs/slices/0001-parquet-scan.md (slice 3): does parquet-rs meet decision rule R1 (decoder), and which morsel size does rule R3 choose?
#   1. build, unit tests (tests/scan.rs), the mutants (mutants/run.sh: each of 3 deliberate bugs must be caught; INVALID is not a catch)
#   2. data: TPC-H SF1 lineitem (DuckDB-written, 49 row groups, no page index); `big1` = the same rows in 4 row groups of about 1.5 million rows
#      (the layout of spikes/object-store-scan/make-layouts.sh); `big1-pi` = big1 rewritten by parquet-rs WITH a Parquet page index
#   3. gate: every arm must return DuckDB's result (no timing is kept before this passes)
#   4. R1: single thread, plain / engine types / engine twin / DuckDB, two projections, interleaved, settled before the block
#   5. R3: morsel = row group or a quarter of one, 1/2/4/8 threads, on big1 (the rule's layout) and on big1-pi (what a page index changes)
#   6. memory: one process per arm, PS_MEM_RUNS runs each, median of the peak above the opened file, at 8 threads
# The script prints the numbers and the per-machine reading of R1 and R3; the rules need BOTH CPU types, so the verdict is made from the two result sets
# (bench/results/<date>-parquet-scan-baseline.md), not here.
# Env: PS_ROUNDS (25), PS_MEM_RUNS (5), PS_SETTLE_MAX (600 s per block, 0 = only print), PS_LINEITEM, PS_DATA (default <spike>/data), PS_MUTANTS (1), PS_QUICK (1: 2 rounds, 2 memory runs, no settle wait).
# Exit status: 0 everything ran and every check passed (a rule that fails is a RESULT, not an error), 1 a check failed, 4 setup failed.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)

# ---- the R3 reading: the quarter is chosen only if its 8-thread throughput gain is at least 1.15 AND its peak memory is at most 25% above the row group's.
# r3_reading GAIN MEM_RG_KIB MEM_QUARTER_KIB -> "quarter" or "row-group" plus the reason
r3_reading() {
  awk -v g="$1" -v r="$2" -v q="$3" 'BEGIN {
    if (g + 0 != g || r + 0 != r || q + 0 != q || r <= 0) { print "invalid input"; exit 1 }
    up = (q - r) / r * 100
    gain_ok = (g >= 1.15); mem_ok = (q <= r * 1.25)
    printf "%s (gain %.2fx %s 1.15x; memory %+.1f%% %s 25%%)\n", (gain_ok && mem_ok) ? "quarter" : "row-group", g, gain_ok ? ">=" : "<", up, mem_ok ? "<=" : ">"
  }'
}
if [ "${1:-}" = "--r3-reading" ]; then r3_reading "${2:?gain}" "${3:?rg kib}" "${4:?quarter kib}"; exit $?; fi

OUT=${1:?usage: gcp-check.sh <results-dir>}
mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)
QUICK=${PS_QUICK:-0}
ROUNDS=${PS_ROUNDS:-25}; MEMRUNS=${PS_MEM_RUNS:-5}; SETTLE_MAX=${PS_SETTLE_MAX:-600}
if [ "$QUICK" = 1 ]; then ROUNDS=2; MEMRUNS=2; SETTLE_MAX=0; fi
DATA=${PS_DATA:-$HERE/data}; mkdir -p "$DATA"
LI=${PS_LINEITEM:-$REPO/data/tpch-sf1/lineitem.parquet}
SUMMARY="$OUT/summary.txt"; : > "$SUMMARY"
say() { printf '\n==== %s ====\n' "$*" | tee -a "$SUMMARY"; }
log() { printf '%s\n' "$*" | tee -a "$SUMMARY"; }
FAILED=0
check() { if [ "$2" = 0 ]; then log "CHECK $1: PASS ${3:-}"; else log "CHECK $1: FAIL ${3:-}"; FAILED=$((FAILED + 1)); fi; }
settle() { if [ -x "$REPO/scripts/gcp/settle.sh" ]; then "$REPO/scripts/gcp/settle.sh" "$1" 0.5 "$SETTLE_MAX" | tee -a "$SUMMARY"; fi; }
BIN="$HERE/target/release/pscan"

{ date -u +%FT%TZ; uname -srm; lscpu | grep -E 'Model name|Architecture|^CPU\(s\)|Thread|Core' || true; free -m | head -2; duckdb --version 2>&1 | head -1
  echo "git: $(git -C "$HERE" rev-parse HEAD 2>/dev/null || echo unknown)"; echo "rounds=$ROUNDS mem_runs=$MEMRUNS quick=$QUICK"; } | tee "$OUT/machine.txt" >> "$SUMMARY"

say "1. build, tests, mutants"
( cd "$HERE" && cargo build --release 2>&1 | tail -3 ) || { log "SETUP FAILED: build"; exit 4; }
[ -x "$BIN" ] || { log "SETUP FAILED: no $BIN"; exit 4; }
command -v duckdb > /dev/null || { log "SETUP FAILED: duckdb is not on PATH"; exit 4; }
( cd "$HERE" && cargo test --release --test scan 2>&1 ) > "$OUT/tests.txt"; trc=$?
check "unit tests" $trc "$(grep -E 'test result' "$OUT/tests.txt" | tail -1)"
if [ "${PS_MUTANTS:-1}" = 1 ]; then
  "$HERE/mutants/run.sh" > "$OUT/mutants.txt" 2>&1; mrc=$?
  check "mutants: all caught" $mrc "$(grep -E '^(CAUGHT|MISSED|INVALID)' "$OUT/mutants.txt" | cut -c1-60 | tr '\n' ';')"
else log "mutants: SKIPPED (PS_MUTANTS=0)"; fi
[ -x "$BIN" ] || { log "SETUP FAILED: no clean binary after the mutants"; exit 4; }

say "2. data"
if [ ! -f "$LI" ]; then
  log "generating TPC-H SF1 into $REPO/data/tpch-sf1"
  ( cd "$REPO" && scripts/gen-tpch.sh 1 data/tpch-sf1 ) > "$OUT/gen.txt" 2>&1 || { log "SETUP FAILED: no lineitem and gen-tpch.sh failed"; exit 4; }
  LI=$REPO/data/tpch-sf1/lineitem.parquet
fi
[ -f "$LI" ] || { log "SETUP FAILED: no $LI"; exit 4; }
BIG1=$DATA/big1.parquet; PI=$DATA/big1-pi.parquet
rm -f "$BIG1" "$PI"
duckdb -bail -c "COPY (SELECT * FROM read_parquet('$LI')) TO '$BIG1' (FORMAT parquet, ROW_GROUP_SIZE 1500000)" > "$OUT/make-big1.txt" 2>&1 || { log "SETUP FAILED: big1"; exit 4; }
"$BIN" mkpageindex "$BIG1" "$PI" | tee -a "$SUMMARY"; check "big1-pi has a page index and the same rows" ${PIPESTATUS[0]}
log "lineitem: $(du -h "$LI" | cut -f1); big1: $(du -h "$BIG1" | cut -f1); big1-pi: $(du -h "$PI" | cut -f1)"

say "3. gate (every arm returns DuckDB's result)"
"$BIN" gate "$LI" 2>&1 | tee "$OUT/gate-lineitem.txt" | tee -a "$SUMMARY"; check "gate on lineitem" ${PIPESTATUS[0]}
"$BIN" gate "$BIG1" 2>&1 | tee "$OUT/gate-big1.txt" | tee -a "$SUMMARY"; check "gate on big1" ${PIPESTATUS[0]}
if [ "$FAILED" != 0 ]; then log "RESULT parquet-scan: a CHECK FAILED before any timing; no timing is kept"; exit 1; fi

say "4. R1: single-thread decode against DuckDB"
settle r1
"$BIN" r1 "$LI" --rounds "$ROUNDS" > "$OUT/r1.md" 2>&1; check "r1 ran (every timed run was compared with the reference)" $?
cat "$OUT/r1.md" | tee -a "$SUMMARY"

say "5. R3: morsel size"
settle r3-big1
"$BIN" r3 "$BIG1" --rounds "$ROUNDS" > "$OUT/r3-big1.md" 2>&1; check "r3 on big1 ran" $?
settle r3-big1-pi
"$BIN" r3 "$PI" --rounds "$ROUNDS" > "$OUT/r3-big1-pi.md" 2>&1; check "r3 on big1-pi ran" $?
cat "$OUT/r3-big1.md" "$OUT/r3-big1-pi.md" | tee -a "$SUMMARY"

say "6. memory (peak above the opened file, 8 threads, one process per run, $MEMRUNS runs, median)"
median() { sort -n | awk '{ a[NR] = $1 } END { print a[int((NR + 1) / 2)] }'; }
declare -A MEMMED
for f in big1 big1-pi; do
  file=$BIG1; [ $f = big1-pi ] && file=$PI
  settle "mem-$f"
  : > "$OUT/mem-$f-rg.txt"; : > "$OUT/mem-$f-quarter.txt"
  for i in $(seq 1 "$MEMRUNS"); do
    for arm in rg quarter; do "$BIN" mem "$file" $arm 8 >> "$OUT/mem-$f-$arm.txt" 2>&1; done
  done
  for arm in rg quarter; do MEMMED[$f-$arm]=$(sed -n 's/.*above_kib=\([0-9]*\).*/\1/p' "$OUT/mem-$f-$arm.txt" | median); done
  log "mem $f: row group median ${MEMMED[$f-rg]:-?} KiB, quarter median ${MEMMED[$f-quarter]:-?} KiB (runs: rg $(sed -n 's/.*above_kib=\([0-9]*\).*/\1/p' "$OUT/mem-$f-rg.txt" | tr '\n' ' ')| quarter $(sed -n 's/.*above_kib=\([0-9]*\).*/\1/p' "$OUT/mem-$f-quarter.txt" | tr '\n' ' '))"
done

say "7. the per-machine reading of the rules (the rules need both CPU types)"
log "R1 (ii) on this machine: $(grep -E '^R1 \(ii\) on this machine' "$OUT/r1.md" | cut -d: -f2-)"
for f in big1 big1-pi; do
  gain=$(sed -n "s/.*the quarter's 8-thread gain is \([0-9.]*\)x.*/\1/p" "$OUT/r3-$f.md")
  log "R3 on $f: $(r3_reading "${gain:-x}" "${MEMMED[$f-rg]:-0}" "${MEMMED[$f-quarter]:-0}")"
done
log "R3 is decided on big1 (the rule's layout) when the verdicts of both CPU types agree; big1-pi shows what a page index changes and is reported next to it, not instead of it."
if [ "$FAILED" != 0 ]; then log "RESULT parquet-scan: $FAILED CHECK(S) FAILED"; exit 1; fi
log "RESULT parquet-scan: everything ran and the checks passed (R1 and R3 above are the findings)"
