#!/usr/bin/env bash
# The ARM correctness bundle: independent checks run CONCURRENTLY on one 16-vCPU VM (they need correctness, not quiet),
# one log and one exit code per task, and a summary table at the end. Not gating: every task runs to its end.
#   armcheck.sh <out-dir>            all tasks that exist
#   ARMCHECK_TASKS="exloom-suite nullmut"   only these
# Tasks (a task whose script is missing is SKIPPED, not failed):
#   exloom-suite    spikes/exchange-loom run_loom.sh: all loom models on the fixed exchange, bound 3
#   exloom-mutants  par-mutants.sh --only exchange-loom: every mutant and the control
#   hash-layers     hash-ops: the DuckDB-diff layer and the nullcheck layer must FAIL on known-bad engines
#   nullmut         null-path mutate.py: all mutants, each caught by the unit tests or end to end
#   stop-depth      arrow-pipeline gcp-check-loom.sh (loom model of stop_depth + mutants), if present
#   data-shapes     data-shapes gcp-check.sh (skewed / string-heavy / many-NULL data against DuckDB), if present
#   arrow-regress   arrow-pipeline correctness after the loom changes (F1 port, LIMIT moved): tests, DuckDB diff digit for digit,
#                   check, nullcheck, the long stress and 10,000 LIMIT cases. RUNS FIRST AND ALONE (its stress/latency tests
#                   must not compete with the other tasks for CPU); the others start when it has finished
#   arrow-mutants   arrow-pipeline: all 33 mutants (par-mutants.sh --only arrow-pipeline)
# Env: SRC (repo root; default: this script's repo), DATA (TPC-H SF1 dir; default $SRC/data/tpch-sf1), WORKERS (exloom-mutants, 6),
#      ARMCHECK_FAKE=1 (tests: tasks are stubs controlled by FAKE_<name>_RC / FAKE_<name>_MISSING)
set -u
here=$(cd "$(dirname "$0")" && pwd); SRC=${SRC:-$(cd "$here/../.." && pwd)}; DATA=${DATA:-$SRC/data/tpch-sf1}
out=${1:?out-dir}; mkdir -p "$out"; out=$(cd "$out" && pwd)
ALL="arrow-regress exloom-suite exloom-mutants hash-layers nullmut stop-depth data-shapes arrow-mutants"
FIRST="arrow-regress"
TASKS=${ARMCHECK_TASKS:-$ALL}
WORKERS=${WORKERS:-6}
# Private copies, build directories and generated data go HERE, never under the results directory: the orchestrator pulls
# state/results/logs every two minutes over ssh, and a 3.7 GB results directory made the pull time out, so the orchestrator
# called the healthy VM "stuck" and recreated it (found 2026-11-04). Only small result files are copied back into $out.
WORKDIR=${ARMCHECK_WORK:-${TMPDIR:-/tmp}/armcheck-work-$$}
mkdir -p "$WORKDIR"
export RUSTUP_TOOLCHAIN=${RUSTUP_TOOLCHAIN:-$(sed -n 's/^channel *= *"\(.*\)"/\1/p' "$SRC/rust-toolchain.toml" 2>/dev/null)}

task_cmd() { # prints the shell command of a task, or nothing if it cannot run here
  local t=$1 u=${1//-/_}
  if [ "${ARMCHECK_FAKE:-}" = 1 ]; then
    local miss="FAKE_${u}_MISSING" rc="FAKE_${u}_RC"
    [ -n "${!miss:-}" ] && return 0
    echo "sleep 1; echo fake $t; exit ${!rc:-0}"; return 0
  fi
  case "$t" in
    exloom-suite)   echo "cd $SRC/spikes/exchange-loom && CARGO_BUILD_JOBS=2 ./run_loom.sh" ;;
    exloom-mutants) echo "cd $SRC && DATA=$DATA WORKERS=$WORKERS scripts/gcp/par-mutants.sh --only exchange-loom --out $WORKDIR/exloom-mutants; rc=\$?; mkdir -p $out/exloom-mutants && cp -r $WORKDIR/exloom-mutants/results.tsv $WORKDIR/exloom-mutants/logs $out/exloom-mutants/ 2>/dev/null; exit \$rc" ;;
    hash-layers)
      echo "cd $SRC/spikes/hash-ops && export HASH_OPS_DATA=$DATA CARGO_BUILD_JOBS=2 && \
        HASH_OPS_MUT_ROOT=$WORKDIR/hl-1 HASH_OPS_TMP=$WORKDIR/hl-1-tmp python3 -I mutate.py --matrix --layer duckdb 'fan-out: only the first match of a chain is emitted' \"merge loses the last thread's partial table\" && \
        HASH_OPS_MUT_ROOT=$WORKDIR/hl-2 HASH_OPS_TMP=$WORKDIR/hl-2-tmp python3 -I mutate.py --matrix --layer nullcheck 'SUM adds the values of NULL rows' 'NULL group dropped from the output'" ;;
    nullmut)        echo "cd $SRC/spikes/null-path && HEAVY= CARGO_BUILD_JOBS=2 python3 -I mutate.py $DATA/lineitem.parquet" ;;
    stop-depth)     [ -f "$SRC/spikes/arrow-pipeline/gcp-check-loom.sh" ] && echo "cd $SRC/spikes/arrow-pipeline && ./gcp-check-loom.sh $out/stop-depth-summary.txt" ;;
    arrow-regress)
      echo "cd $SRC/spikes/arrow-pipeline && export CARGO_BUILD_JOBS=\$(nproc) LINEITEM=$DATA/lineitem.parquet && cargo build --release && \
        cargo test --release --no-fail-fast -- --test-threads=1 && scripts/duckdb_diff.sh $DATA/lineitem.parquet && \
        target/release/spike-arrow-pipeline check $DATA/lineitem.parquet && target/release/spike-arrow-pipeline nullcheck $DATA/lineitem.parquet && \
        STRESS_REPS=3000 STRESS_EXTRA_BUSY=4 WATCHDOG_SECS=60 cargo test --release --test stress -- --nocapture --test-threads=1 && \
        LIMIT_CASES=10000 cargo test --release --test limit -- --nocapture --test-threads=1" ;;
    arrow-mutants)  echo "cd $SRC && DATA=$DATA WORKERS=$WORKERS scripts/gcp/par-mutants.sh --only arrow-pipeline --out $WORKDIR/arrow-mutants; rc=\$?; mkdir -p $out/arrow-mutants && cp -r $WORKDIR/arrow-mutants/results.tsv $WORKDIR/arrow-mutants/logs $out/arrow-mutants/ 2>/dev/null; exit \$rc" ;;
    data-shapes)    [ -f "$SRC/spikes/data-shapes/gcp-check.sh" ] && echo "cd $SRC/spikes/data-shapes && ./gcp-check.sh $out/data-shapes $WORKDIR/data-shapes" ;;
  esac
}
if [ "${ARMCHECK_PRINT:-}" = 1 ]; then   # test aid: print the command of every task and stop
  for t in $TASKS; do echo "TASK $t: $(task_cmd "$t")"; done; exit 0
fi
declare -A PID
started=""
start_task() {
  local t=$1 cmd; cmd=$(task_cmd "$t")
  if [ -z "$cmd" ]; then echo "SKIPPED" > "$out/$t.rc"; echo "[$(date -u +%H:%M:%S)] $t: skipped (not present)"; return; fi
  ( bash -c "$cmd" > "$out/$t.log" 2>&1; echo $? > "$out/$t.rc" ) &
  PID[$t]=$!; started="$started $t"; echo "[$(date -u +%H:%M:%S)] $t: started"
}
wait_started() { local t; for t in $started; do wait "${PID[$t]}"; echo "[$(date -u +%H:%M:%S)] $t: finished rc=$(cat "$out/$t.rc")"; done; started=""; }
# tasks in FIRST run alone, before everything else (timing-sensitive tests must not compete for CPU)
for t in $TASKS; do case " $FIRST " in *" $t "*) start_task "$t" ;; esac; done
wait_started
for t in $TASKS; do case " $FIRST " in *" $t "*) ;; *) start_task "$t" ;; esac; done
wait_started
echo; echo "=== armcheck summary"
bad=0
for t in $TASKS; do
  rc=$(cat "$out/$t.rc" 2>/dev/null || echo MISSING)
  case "$rc" in 0) v=PASS ;; SKIPPED) v=SKIPPED ;; *) v="FAIL (rc=$rc)"; bad=1 ;; esac
  printf '%-16s %s\n' "$t" "$v"
done
[ "$bad" = 0 ] && echo "ALL TASKS THAT RAN PASSED" || echo "SOME TASKS FAILED: read the logs"
exit $bad
