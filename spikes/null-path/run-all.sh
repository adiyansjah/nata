#!/bin/bash
# Correctness, mutation check and timing for the word-wise NULL path, in this order, for a run on a QUIET machine
# (a GCP VM, x86-64 and ARM64). Stops at the first correctness failure: a fast wrong answer is not a data point.
#
# Usage: run-all.sh <lineitem.parquet> [rounds=25]
#   env OUT=<dir>          where the logs go (default: ./out-<arch>-<date>)
#   env SKIP_MUTATION=1    skip the mutation check (it rebuilds the crate ~35 times)
#   env DUCKDB=<binary>    DuckDB for the result diff (default: duckdb; skipped with a notice if missing)
#   env RUSTFLAGS          left alone: the default (baseline target features) is what the numbers in the report use.
#                          A second, clearly labelled run with RUSTFLAGS="-C target-cpu=native" is optional.
# Needs: cargo, python3, DuckDB (optional), TPC-H SF1 lineitem.parquet (see docs / scripts that create it).
#
# FOR THE GCP RUN (the timing in section 3 must be taken on a quiet VM, x86-64 and ARM64, not on the shared dev box):
#   expected duration on an idle 4-vCPU VM (a reasoned estimate, not measured on a quiet machine):
#     build both profiles, cold ......... 5-8 min (arrow and parquet compile)
#     1 correctness ..................... 5-8 min (tests ~1 min, check <0.5 min, nullcheck 13 cases per query ~4-6 min, DuckDB diff seconds)
#     2 mutation (62 mutants) ........... 40-70 min (own target dir seeded from a copy of target/release; first mutant copies ~0.9 GB) (one unit-test build per mutant, ~0.5-1 min each; SKIP_MUTATION=1 to skip)
#     3 timing (ladder x2 clocks, overflow-checks A/B/A/B, density, stages) ... 15-25 min with 25 rounds
#   so about 1-1.5 h in total, or 25-40 min with SKIP_MUTATION=1 (mutation has already been run locally; it is not
#   timing-sensitive and need not be repeated on both architectures unless the code changed).
#   Cost: a Spot e2-standard-4 / t2a-standard-4 for ~1.5 h each architecture is a few cents; ask for the human's yes first
#   (docs/way-of-working.md section 8) and record machine type, region and date next to the result.
#   HEAVY=scripts/local-heavy.sh (path relative to the repo) makes every cargo/benchmark command queue behind the shared-
#   machine lock; leave HEAVY empty on a VM of our own.
set -euo pipefail
cd "$(dirname "$0")"
LI=$(realpath "${1:?usage: run-all.sh <lineitem.parquet> [rounds]}")
ROUNDS=${2:-25}
ARCH=$(uname -m)
OUT=${OUT:-./out-$ARCH-$(date +%Y%m%d-%H%M)}
mkdir -p "$OUT"
BIN=./target/release/spike-null-path
NOCHK=./target/nochecks/spike-null-path
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}
H=${HEAVY:-}   # command prefix for heavy commands (see above)
export HEAVY

say() { printf '\n==== %s ====\n' "$*" | tee -a "$OUT/run.log"; }
load() { uptime | sed 's/.*load average/load average/'; }
# Every timed phase starts on a settled machine (ADR 0006 rule 7: 1-minute load below 0.5): the phase before it leaves the 1-minute
# average high (found 2026-11-05: the phases after the first printed loads of 1.4-2.0). SETTLE_MAX is the longest wait in seconds
# per phase; the default 0 only prints the load (a development machine is never quiet), the GCP job sets 600. The line it prints
# says "NOT SETTLED" when the wait ran out, and then the phase that follows is SANITY ONLY.
SETTLE_SH="$PWD/../../scripts/gcp/settle.sh"   # run-all.sh has cd-ed to its own directory (line 26)
settle() { [ -x "$SETTLE_SH" ] && { "$SETTLE_SH" "$1" 0.5 "${SETTLE_MAX:-0}" | tee -a "$OUT/run.log"; }; return 0; }

say "machine"
{
  date -u +%FT%TZ; uname -srm
  lscpu | grep -E 'Model name|^CPU\(s\)|Thread|Flags' | cut -c1-160 || true
  rustc --version; echo "RUSTFLAGS=${RUSTFLAGS:-<unset>}"; uptime
} | tee -a "$OUT/run.log"
L1=$(awk '{print $1}' /proc/loadavg)
if awk -v l="$L1" 'BEGIN{exit !(l >= 0.5)}'; then
  echo "WARNING: 1-minute load average is $L1 (>= 0.5). Timing numbers taken before a SETTLE line says 'settled' are SANITY ONLY." | tee -a "$OUT/run.log"
fi

say "build (release: overflow-checks = true; nochecks: overflow-checks = false)"
$H cargo build --release 2>&1 | tail -2 | tee -a "$OUT/run.log"
$H cargo build --profile nochecks 2>&1 | tail -2 | tee -a "$OUT/run.log"

# ---------------------------------------------------------------- 1. correctness
say "1a. unit and property tests (kernels against scalar references; overflow checks on)"
$H cargo test --release 2>&1 | grep -E '^test result|^test .*FAILED|panicked' | tee -a "$OUT/run.log"

say "1b. check: no NULLs, generic-ops == hand-fused == every word-wise step, at 1 and 4 threads"
$H $BIN check "$LI" | tee "$OUT/check.txt" | tee -a "$OUT/run.log"

say "1c. nullcheck: 1 3 5 10 25 50 75 95 100% NULLs (8192-row batches) and 5 50% (1000- and 97-row batches) == independent scalar reference, old engine and every step, 1 and 4 threads"
$H $BIN nullcheck "$LI" | tee -a "$OUT/run.log"

say "1d. the same two queries in DuckDB, diffed against the engine's output"
DUCKDB=${DUCKDB:-duckdb}
if command -v "$DUCKDB" >/dev/null; then
  Q1="SELECT l_returnflag, l_linestatus, sum(l_quantity), sum(l_extendedprice), sum(l_extendedprice*(1-l_discount)), count(*) FROM '$LI' WHERE l_shipdate <= DATE '1998-09-02' GROUP BY 1,2 ORDER BY 1,2;"
  Q6="SELECT sum(l_extendedprice*l_discount) FROM '$LI' WHERE l_shipdate >= DATE '1994-01-01' AND l_shipdate < DATE '1995-01-01' AND l_discount BETWEEN 0.05 AND 0.07 AND l_quantity < 24;"
  env -u AI_AGENT "$DUCKDB" -csv -noheader -c "$Q1" > "$OUT/duckdb-q1.csv"
  env -u AI_AGENT "$DUCKDB" -csv -noheader -c "$Q6" > "$OUT/duckdb-q6.csv"
  awk '/^== q1/{f=1;next} /^== q6/{f=0} /^ok /{next} f' "$OUT/check.txt" > "$OUT/engine-q1.csv"
  awk '/^== q6/{f=1;next} /^ok /{next} f' "$OUT/check.txt" > "$OUT/engine-q6.csv"
  diff "$OUT/duckdb-q1.csv" "$OUT/engine-q1.csv" && diff "$OUT/duckdb-q6.csv" "$OUT/engine-q6.csv" && echo "ok   Q1 and Q6 identical to DuckDB" | tee -a "$OUT/run.log"
else
  echo "NOTICE: DuckDB not found; the DuckDB diff was skipped (the scalar reference and generic-ops checks above still ran)" | tee -a "$OUT/run.log"
fi

# ---------------------------------------------------------------- 2. mutation
if [ -z "${SKIP_MUTATION:-}" ]; then
  say "2. mutation check on a private copy (each mutant must be caught)"
  python3 -I mutate.py "$LI" 2>&1 | tee "$OUT/mutation.txt" | tee -a "$OUT/run.log"
else
  say "2. mutation check skipped (SKIP_MUTATION set)"
fi

# ---------------------------------------------------------------- 3. timing
settle 3a
say "3a. ladder, wall clock ($ROUNDS rounds): old engine and each step, 0% vs 10% NULLs, interleaved. $(load)"
$H $BIN ladder "$LI" "$ROUNDS" | tee "$OUT/ladder-wall.md" | tee -a "$OUT/run.log"
echo "$(load)" | tee -a "$OUT/run.log"

settle 3b
say "3b. ladder, process CPU time (same arms). $(load)"
NATA_CLOCK=cpu $H $BIN ladder "$LI" "$ROUNDS" | tee "$OUT/ladder-cpu.md" | tee -a "$OUT/run.log"
echo "$(load)" | tee -a "$OUT/run.log"

say "3c. cost of overflow-checks: release (on) and nochecks (off), alternated A B A B. $(load)"
for rep in 1 2; do
  for b in "$BIN" "$NOCHK"; do
    settle "3c-$(basename "$(dirname "$b")")-rep$rep"
    echo "--- $b (rep $rep) $(load)" | tee -a "$OUT/run.log"
    $H $b ladder "$LI" "$((ROUNDS / 2 + 1))" | tee "$OUT/ladder-$(basename "$(dirname "$b")")-$rep.md" | grep -E '^###|^\| (0|3|6) ' | tee -a "$OUT/run.log"
  done
done

settle 3d
say "3d. NULL density 0 / 1 / 10 / 50 / 100%, old engine and the final step, process CPU time. $(load)"
NATA_CLOCK=cpu $H $BIN density "$LI" "$ROUNDS" | tee "$OUT/density.md" | tee -a "$OUT/run.log"

say "3e. where the time goes: per-stage minimum over passes, one thread, per-row masks (3a) / subtract NULL rows (3) / final (6). $(load)"
for q in q1 q6; do for st in 3a 3 6; do
  settle "3e-$q-$st"
  echo "--- $q step $st $(load)" | tee -a "$OUT/run.log"
  $H $BIN stages "$LI" 15 $q $st | tee -a "$OUT/stages.md" | tee -a "$OUT/run.log"
done; done

say "done; logs in $OUT. Load at the start was $L1."
