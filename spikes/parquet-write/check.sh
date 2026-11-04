#!/bin/bash
# Verification-plan item 30 end to end. Usage: check.sh <results-dir> [rows]   (rows default 300000)
#  1. cargo tests (good files pass the verifier; every known-bad statistic is flagged; controls pass)
#  2. for every writer configuration: write, independent verification, DuckDB reads the same rows (as text), sort order across files, and a predicate
#     suite where DuckDB's pruning on our statistics must give the same counts as a full scan
#  3. DuckDB as the WRITER (several settings): the same verifier and pruning suite on its files (informational: a flag there is a finding about DuckDB or
#     about the verifier, to be read by a person)
#  4. the known-bad statistic files: the verifier must flag every bad one and pass the controls; DuckDB's pruning on the bad ones is reported
# Exit 0 only if every GATING phase passed. Needs cargo, python3, duckdb on PATH.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); OUT=${1:?usage: check.sh <results-dir> [rows]}; ROWS=${2:-300000}
mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd); cd "$HERE" || exit 1
W=${PQW_WORK:-$OUT/work}; rm -rf "$W"; mkdir -p "$W"
SUMMARY=$OUT/summary.txt; : > "$SUMMARY"; FAILED=0
log() { printf '%s\n' "$*" | tee -a "$SUMMARY"; }
gate() { local n=$1; shift; local t0=$SECONDS; if "$@" > "$OUT/$n.txt" 2>&1; then log "PASS $n ($((SECONDS - t0)) s)"; else log "FAIL $n ($((SECONDS - t0)) s), see $n.txt"; FAILED=1; fi; }
info() { local n=$1; shift; local t0=$SECONDS; "$@" > "$OUT/$n.txt" 2>&1; local rc=$?; log "INFO $n: exit $rc ($((SECONDS - t0)) s), see $n.txt"; }
log "host: $(uname -m), $(nproc) vCPU; $(rustc --version); duckdb $(env -u AI_AGENT duckdb --version); rows $ROWS"
gate build cargo build --release --locked
gate cargo-tests cargo test --release --locked
PQW=./target/release/pqw
$PQW gen "$W" --rows "$ROWS" > /dev/null; SRC=$W/src-0000.parquet
log "source: $(du -h "$SRC" | cut -f1)"
for c in default small-groups trunc8 chunk-stats bloom nodict-uncompressed no-truncation sorted-id-desc sorted-u8-id rolling; do
  n=$($PQW rewrite "$SRC" "$W/out" "$c" | wc -l)
  flags="--ids"; [ "$c" = trunc8 ] && flags="--ids --trunc 8"; [ "$c" = bloom ] && flags="--ids --bloom"
  gate "verify-$c" $PQW verify $flags "$W"/out/$c-*.parquet
  gate "content-$c" python3 -I xcheck.py content "$SRC" "$W/out/$c-*.parquet"
  case $c in sorted-id-desc) gate "order-$c" python3 -I xcheck.py order "$W/out/$c-*.parquet" id desc ;; esac
  gate "prune-$c" python3 -I xcheck.py prune "$W/out/$c-*.parquet"
  [ "$c" = rolling ] && log "rolling: $n files, sizes $(ls -l "$W"/out/rolling-*.parquet | awk '{printf "%d ", $5/1024}')KB (target 600 KB; the last file is the remainder)"
done
# the raw parquet-rs behaviour, to show what the mitigation is for
$PQW rewrite "$SRC" "$W/out" unmitigated > /dev/null
info verify-unmitigated $PQW verify "$W"/out/unmitigated-*.parquet
info prune-unmitigated python3 -I xcheck.py prune "$W/out/unmitigated-*.parquet" --cols f64_allnan,f64,f32
# DuckDB as the writer
i=0
for opts in "FORMAT parquet" "FORMAT parquet, ROW_GROUP_SIZE 5000" "FORMAT parquet, COMPRESSION zstd, ROW_GROUP_SIZE 20000" "FORMAT parquet, ROW_GROUP_SIZE 3000, COMPRESSION uncompressed"; do
  i=$((i + 1)); mkdir -p "$W/duck"
  env -u AI_AGENT duckdb -c "COPY (SELECT * FROM read_parquet('$SRC')) TO '$W/duck/duck$i.parquet' ($opts);" > /dev/null 2>&1
  info "verify-duckdb-writer-$i" $PQW verify "$W/duck/duck$i.parquet"
  info "prune-duckdb-writer-$i" python3 -I xcheck.py prune "$W/duck/duck$i.parquet"
done
# known-bad statistics
$PQW faults "$W/faults" > "$W/faults.list"
bad_visible=0; nbad=0
while IFS=$'\t' read -r f kind; do
  name=$(basename "$f" .parquet); $PQW verify "$f" > "$OUT/fault-$name.txt" 2>&1; rc=$?
  if [ "$kind" = control ]; then [ $rc = 0 ] && log "PASS fault-$name: control accepted" || { log "FAIL fault-$name: a control was rejected"; FAILED=1; }
  else nbad=$((nbad + 1)); [ $rc != 0 ] && log "PASS fault-$name: flagged" || { log "FAIL fault-$name: NOT flagged"; FAILED=1; }
       python3 -I xcheck.py prune "$f" > "$OUT/fault-$name-prune.txt" 2>&1 || { bad_visible=$((bad_visible + 1)); log "INFO fault-$name: DuckDB's pruning loses rows on this file ($(grep -c MISMATCH "$OUT/fault-$name-prune.txt") predicates)"; }
  fi
done < "$W/faults.list"
log "known-bad statistic files: $nbad, wrong answers visible through DuckDB's pruning for $bad_visible of them"
[ "$bad_visible" -ge 3 ] && log "PASS end-to-end suite can fail (>= 3 bad files give wrong counts through DuckDB)" || { log "FAIL end-to-end pruning suite did not fail on enough bad files"; FAILED=1; }
rm -rf "$W"
[ $FAILED = 0 ] && log "VERDICT: all gating phases passed" || log "VERDICT: FAILED"
exit $FAILED
