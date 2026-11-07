#!/usr/bin/env bash
# run-reference.sh <step>   (card 0006 part 4: the reference run; each step is one `heavy` job step, see scripts/gcp/README.md)
#
#   build       release build and unit tests
#   tests       the reliability suites of the wrapper: faults, cache+scan, hostile (under ulimit -v), mutants -> $OUT/tests-*.txt
#   preflight   (gcs) one tiny object: upload, read it with DuckDB (bearer token) and with the wrapper, delete it; stops the run early if DuckDB cannot read
#   data        TPC-H SF1 and SF10 lineitem (scripts/gen-tpch.sh) -> layouts (mk-layouts.sh) -> the DuckDB-on-local-files answers (expected.txt)
#   upload      (gcs) the layouts to <bucket>/<prefix>/ (write-test identity)
#   bench-sf1   SF1: 4 layouts x full,proj,sel,meta; DuckDB, control (unmerged), wrapper, cross-row-group wrapper; each with a twin; 5 rounds
#   bench-meta  repeated metadata reads: DuckDB (5 statements in one process), wrapper without a cache, wrapper with the bounded cache
#   bench-sf10  SF10: mid10 and files10 x full,proj,sel; DuckDB, control, wrapper; twins; 5 rounds
#   bench-scale SF10 mid10 full,proj at 1 and 4 CPU workers (8 is in bench-sf10)
#   bench-rt    ADR 0002's open arm: split runtime against a separate tokio CPU runtime, idle and saturated, SF1 mid1/small1
#   bench-rss   the resource envelope: peak RSS of each arm in a FRESH process (one cell, one arm), full scans at SF1 and SF10
#   cleanup     (gcs) delete everything under the prefix and prove the prefix is empty
#   leakscan    no bucket, prefix, service account or project id in the results
#
# The bucket and prefix come from runtime env file next to this script (git-ignored, shipped with the tree) and are never written to a result.
# NATA_OSB_MODE=trial runs the same steps at TPC-H SF0.1/0.2 against an endpoint named by NATA_OBJ_* (the local fake), without upload/cleanup.
set -u -o pipefail
unset AI_AGENT
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
MODE=${NATA_OSB_MODE:-gcs}
WORK_BASE=${WORK:-$HOME/tmp-osb}
DATA=${NATA_OSB_DATA:-$WORK_BASE/osb-data}
OUT=${NATA_OSB_OUT:-${RESULTS:-$HERE/out}/osb}
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$WORK_BASE/target-osb}
export RUSTFLAGS=""
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
if ! rustc --version >/dev/null 2>&1; then
  TC=$(sed -n 's/^channel *= *"\(.*\)".*/\1/p' "$REPO/rust-toolchain.toml" | head -1); [ -n "$TC" ] && export RUSTUP_TOOLCHAIN="$TC"
fi
mkdir -p "$OUT" "$DATA"
OSB=$CARGO_TARGET_DIR/release/osb
export NATA_SETTLE=${NATA_SETTLE-$REPO/scripts/gcp/settle.sh}   # NATA_SETTLE= (empty) skips the wait
if [ "$MODE" = gcs ]; then
  : "${NATA_OBJ_ENV_FILE:?set the private runtime env file path}"
  set -a; . "$NATA_OBJ_ENV_FILE"; set +a
fi
# scale factors: the real ones, or the trial's
if [ "$MODE" = trial ]; then SF_A=0.1; SF_B=0.2; K_A=trial; else SF_A=1; SF_B=10; K_A=sf1; fi
ROUNDS=${NATA_OSB_ROUNDS:-5}
step=${1:?usage: run-reference.sh <step>}
# a 5,000-row object with the TPC-H column types (BIGINT, DECIMAL(15,2), DATE, VARCHAR): the preflight must read what the real layouts hold
make_tiny() {
  duckdb -c "COPY (SELECT range::BIGINT AS l_orderkey, (range % 200)::BIGINT AS l_partkey, (range % 100)::BIGINT AS l_suppkey, (1 + range % 7)::BIGINT AS l_linenumber, CAST(1 + range % 50 AS DECIMAL(15,2)) AS l_quantity, CAST(range * 1.25 AS DECIMAL(15,2)) AS l_extendedprice, CAST((range % 10) / 100.0 AS DECIMAL(15,2)) AS l_discount, CAST((range % 8) / 100.0 AS DECIMAL(15,2)) AS l_tax, 'R' AS l_returnflag, 'O' AS l_linestatus, DATE '2020-01-01' + CAST(range % 300 AS INTEGER) AS l_shipdate, DATE '2020-02-01' + CAST(range % 200 AS INTEGER) AS l_commitdate, DATE '2020-03-01' + CAST(range % 100 AS INTEGER) AS l_receiptdate, 'DELIVER IN PERSON' AS l_shipinstruct, 'TRUCK' AS l_shipmode, 'comment ' || range::VARCHAR AS l_comment FROM range(5000)) TO '$1' (FORMAT parquet, ROW_GROUP_SIZE 1000)" > /dev/null
}
machine() { { date -u +%FT%TZ; uname -srm; nproc; lscpu | grep -E 'Model name|Architecture'; free -m | head -2; echo "ulimit -n: $(ulimit -n)"; duckdb --version 2>/dev/null; } > "$OUT/machine-$step.txt" 2>&1; }

case "$step" in
  build)
    duckdb -c "INSTALL httpfs;" > "$OUT/httpfs-install.txt" 2>&1 || { echo "INSTALL httpfs failed"; cat "$OUT/httpfs-install.txt"; }
    cd "$HERE" && { cargo build --release --locked 2>&1 | tail -5; cargo test --release --locked 2>&1 | grep -E "^test result|FAILED|panicked" ; } | tee "$OUT/build.txt"
    ls -la "$OSB" ;;
  tests)
    machine
    for t in faults cache hostile-all mutants; do
      echo "== osb $t"; "$OSB" $t > "$OUT/tests-$t.txt" 2>&1; echo "exit $?" | tee -a "$OUT/tests-$t.txt"; tail -3 "$OUT/tests-$t.txt"
    done ;;
  make-tiny) make_tiny "${2:?path}" ;;   # used by the local trial of the preflight
  preflight)
    machine
    T=$DATA/pre; rm -rf "$T"; mkdir -p "$T/sf0/tiny"
    make_tiny "$T/sf0/tiny/part-0000.parquet"
    ORIG_PREFIX=$NATA_OBJ_PREFIX; export NATA_OBJ_PREFIX="$ORIG_PREFIX/preflight"
    SERVE_PID=""
    if [ "$MODE" = gcs ]; then
      "$OSB" upload --dir "$T" || { echo "PREFLIGHT FAILED: upload"; exit 4; }
    else   # trial: the same bench lines against a throwaway fake endpoint serving the tiny object
      rm -rf "$T/root"; mkdir -p "$T/root/$NATA_OBJ_PREFIX"; cp -r "$T/sf0" "$T/root/$NATA_OBJ_PREFIX/"
      "$OSB" serve --dir "$T/root" --bucket "$NATA_OBJ_BUCKET" --port-file "$T/port" > /dev/null 2>&1 & SERVE_PID=$!
      for _ in $(seq 1 50); do [ -s "$T/port" ] && break; sleep 0.1; done
      export NATA_OBJ_ENDPOINT=$(cat "$T/port")
    fi
    "$OSB" expected --name sf0 --sf 0.001 --glob "$T/sf0/tiny/part-0000.parquet" --out "$T/expected.txt" > /dev/null || exit 4
    NATA_SF_OVERRIDE=0.001 "$OSB" bench --cells 'sf0/tiny:1' --queries full,proj,sel,meta --arms duck,ctl,wrap,wrapT --rounds 1 --workers 2 --expected "$T/expected.txt" --out "$OUT/preflight.csv" 2>&1 | tee "$OUT/preflight.txt"
    RC=${PIPESTATUS[0]}
    if [ "$MODE" = gcs ]; then "$OSB" delete; else kill "$SERVE_PID" 2>/dev/null; fi
    export NATA_OBJ_PREFIX=$ORIG_PREFIX
    if [ "$RC" -eq 0 ] && ! awk -F, 'NR>1 && $6==0' "$OUT/preflight.csv" | grep -q .; then touch "$DATA/preflight.ok"; echo "PREFLIGHT OK"; else echo "PREFLIGHT FAILED (rc $RC): see preflight.csv"; exit 4; fi ;;
  data)
    machine
    cd "$REPO"
    for sf in A B; do
      eval "S=\$SF_$sf"
      d=$DATA/src-$sf
      if [ ! -f "$d/lineitem.parquet" ]; then
        if [ "$sf" = B ] && [ "$MODE" = gcs ]; then GEN_CHILDREN=2 scripts/gen-tpch.sh "$S" "$d" > "$OUT/gen-$sf.txt" 2>&1 || exit 4; else scripts/gen-tpch.sh "$S" "$d" > "$OUT/gen-$sf.txt" 2>&1 || exit 4; fi
        find "$d" -maxdepth 1 -name '*.parquet' ! -name lineitem.parquet -delete
      fi
    done
    LAY=$DATA/lay; rm -rf "$LAY"; mkdir -p "$LAY/$NATA_OBJ_PREFIX"
    if [ "$MODE" = trial ]; then
      "$HERE/mk-layouts.sh" "$DATA/src-A/lineitem.parquet" "$LAY/$NATA_OBJ_PREFIX" trial
      "$HERE/mk-layouts.sh" "$DATA/src-B/lineitem.parquet" "$LAY/$NATA_OBJ_PREFIX" sf10
    else
      "$HERE/mk-layouts.sh" "$DATA/src-A/lineitem.parquet" "$LAY" sf1 && "$HERE/mk-layouts.sh" "$DATA/src-B/lineitem.parquet" "$LAY" sf10 || exit 4
    fi
    rm -f "$OUT/expected.txt"
    "$OSB" expected --name sf1 --sf "$SF_A" --glob "$DATA/src-A/lineitem.parquet" --out "$OUT/expected.txt" || exit 4
    "$OSB" expected --name sf10 --sf "$SF_B" --glob "$DATA/src-B/lineitem.parquet" --out "$OUT/expected.txt" || exit 4
    # the layouts must have the shape they are named for
    for f in $(cd "$LAY" && find . -name 'part-0000.parquet' | sort); do
      duckdb -noheader -list -c "SELECT '$f' AS layout_file, count(DISTINCT row_group_id) AS row_groups, max(row_group_num_rows) AS rows_per_group FROM parquet_metadata('$LAY/$f')" 2>&1 | sed "s|$NATA_OBJ_PREFIX/||"
    done | tee "$OUT/layouts.txt"
    du -sh "$LAY"/* | sed "s|$NATA_OBJ_PREFIX||" ;;
  upload)
    [ "$MODE" = gcs ] || { echo "trial: nothing to upload"; exit 0; }
    [ -f "$DATA/preflight.ok" ] || { echo "no preflight.ok: not uploading"; exit 4; }
    echo "start $(date -u +%T)"; "$OSB" upload --dir "$DATA/lay" || exit 5; echo "end $(date -u +%T)"
    "$OSB" list ;;
  bench-sf1)
    [ "$MODE" = trial ] && export NATA_SF_OVERRIDE=$SF_A
    machine
    "$OSB" bench --cells 'sf1/big1:1,sf1/mid1:1,sf1/small1:1,sf1/files100:100' --queries full,proj,sel,meta --arms duck,ctl,wrap,wrapx8 --rounds "$ROUNDS" --workers 8 --io-threads 2 --expected "$OUT/expected.txt" --out "$OUT/sf1-w8.csv" 2>&1 | tee "$OUT/bench-sf1.txt" | tail -8 ;;
  bench-meta)
    [ "$MODE" = trial ] && export NATA_SF_OVERRIDE=$SF_A
    "$OSB" bench --cells 'sf1/files100:100,sf1/small1:1' --queries meta --arms duck,wrap,wrapc --rounds "$ROUNDS" --workers 8 --io-threads 2 --expected "$OUT/expected.txt" --out "$OUT/meta-w8.csv" 2>&1 | tee "$OUT/bench-meta.txt" | tail -8 ;;
  bench-sf10)
    [ "$MODE" = trial ] && export NATA_SF_OVERRIDE=$SF_B
    "$OSB" bench --cells 'sf10/mid10:1,sf10/files10:10' --queries full,proj,sel --arms duck,ctl,wrap --rounds "$ROUNDS" --workers 8 --io-threads 2 --expected "$OUT/expected.txt" --out "$OUT/sf10-w8.csv" 2>&1 | tee "$OUT/bench-sf10.txt" | tail -8 ;;
  bench-scale)
    [ "$MODE" = trial ] && export NATA_SF_OVERRIDE=$SF_B
    for w in 1 4; do
      "$OSB" bench --cells 'sf10/mid10:1' --queries full,proj --arms duck,wrap --rounds "${NATA_OSB_SCALE_ROUNDS:-3}" --workers $w --io-threads 2 --expected "$OUT/expected.txt" --out "$OUT/scale-w$w.csv" 2>&1 | tee "$OUT/bench-scale-w$w.txt" | tail -4
    done ;;
  bench-rt)
    # network-bound cells (small1, mid1) and CPU-bound cells (big1: 4 row groups, files100: one row group per file, SF10 files10)
    for sat in "" "--saturate"; do
      n=idle; [ -n "$sat" ] && n=saturated
      for cell in 'sf1/small1:1' 'sf1/mid1:1' 'sf1/big1:1' 'sf1/files100:100' 'sf10/files10:10'; do
        case "$cell" in sf1/*) [ "$MODE" = trial ] && export NATA_SF_OVERRIDE=$SF_A ;; *) [ "$MODE" = trial ] && export NATA_SF_OVERRIDE=$SF_B ;; esac
        "$OSB" bench --cells "$cell" --queries full,proj --arms wrap,wrapT --rounds "$ROUNDS" --workers 8 --io-threads 2 $sat --expected "$OUT/expected.txt" --out "$OUT/rt-$n.csv" 2>&1 | tee -a "$OUT/bench-rt-$n.txt" | grep -E "RESULT|NOT OK"
      done
    done ;;
  bench-rss)
    for arm in duck ctl wrap; do
      for cell in 'sf1/small1:1' 'sf1/files100:100' 'sf10/mid10:1' 'sf10/files10:10'; do
        case "$cell" in sf1/*) [ "$MODE" = trial ] && export NATA_SF_OVERRIDE=$SF_A ;; *) [ "$MODE" = trial ] && export NATA_SF_OVERRIDE=$SF_B ;; esac
        NATA_SETTLE= NATA_NO_HWM_RESET=1 "$OSB" bench --cells "$cell" --queries full --arms $arm --rounds 1 --workers 8 --io-threads 2 --expected "$OUT/expected.txt" --out "$OUT/rss.csv" 2>&1 | grep -E "idle runtime|RESULT|NOT OK" | sed "s|^|$arm $cell: |"
      done
    done ;;
  cleanup)
    [ "$MODE" = gcs ] || { echo "trial: nothing to delete"; exit 0; }
    "$OSB" delete | tee "$OUT/cleanup.txt"; RC=${PIPESTATUS[0]}
    "$OSB" list | tee -a "$OUT/cleanup.txt"
    [ "$RC" -eq 0 ] && echo "CLEANUP OK: every object this run uploaded was deleted and the prefix is empty" | tee -a "$OUT/cleanup.txt" ;;
  leakscan)
    bad=0
    for v in "${NATA_OBJ_BUCKET:-}" "${NATA_OBJ_PREFIX:-}" homelab- .iam.gserviceaccount.com; do
      [ -n "$v" ] || continue
      if grep -rIlF -- "$v" "${RESULTS:-$OUT}" 2>/dev/null | grep -q .; then echo "LEAK: a configured value appears in the results"; bad=1; fi
    done
    [ "$bad" = 0 ] && echo "leak scan clean"; exit "$bad" ;;
  *) echo "unknown step $step"; exit 2 ;;
esac
