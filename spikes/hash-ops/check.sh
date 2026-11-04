#!/usr/bin/env bash
# Differential check against DuckDB: run the 7 plans of the spike on the engine (1, 4 and 8 threads) and the
# same SQL (sql/<name>.sql) on DuckDB, normalise both to one text form, and require identical output.
#
# Text form (src/out.rs `lines()`): one line per group, fields joined by '|', key columns first (integers as
# integers, strings raw, dates as days since 1970-01-01, NULL as NULL), then COUNT(*) when the query has one,
# then each SUM as an exact decimal at its scale; lines sorted bytewise (LC_ALL=C). DuckDB prints DECIMAL
# and HUGEINT exactly, so no rounding is involved on either side.
#
# Usage: check.sh [data-dir (default data/tpch-sf1 at the repo root)] [keep-dir]
# Needs `duckdb` and `cargo` on PATH. Exit status 0 only if all 7 queries are identical at 1, 4 and 8 threads.
#
# Disk use: the result files of the big queries are hundreds of MB. The work directory is created with
# `mktemp -d` inside $HASH_OPS_TMP (default: $TMPDIR, else /tmp; point it at disk if /tmp is a RAM-backed
# tmpfs) and is REMOVED on every exit (success, failure, Ctrl-C, kill). Each query's files are deleted as soon
# as that query has been compared, so the peak is one query (about 150 MB), not all seven. If [keep-dir] is
# given the files go there instead and are kept for inspection: then the caller owns the directory.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
DATA="$(cd "${1:-$ROOT/data/tpch-sf1}" && pwd)"
if [ -n "${2:-}" ]; then
  WORK="$2"; OWN=0; mkdir -p "$WORK"
else
  BASE="${HASH_OPS_TMP:-${TMPDIR:-/tmp}}"; mkdir -p "$BASE"
  WORK="$(mktemp -d "$BASE/hash-ops-check.XXXXXX")"; OWN=1
fi
cleanup() { [ "$OWN" = 1 ] && rm -rf "$WORK"; return 0; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
QUERIES=(agg_orderkey agg_partkey agg_modedate agg_comment q3 q12 j3)

for t in customer orders lineitem; do
  [ -f "$DATA/$t.parquet" ] || { echo "missing $DATA/$t.parquet (generate with scripts/gen-tpch.sh 1 data/tpch-sf1)" >&2; exit 2; }
done

cd "$HERE"
cargo build --release --quiet
BIN="${CARGO_TARGET_DIR:-$HERE/target}/release/hash-ops"

SETUP=""
for t in customer orders lineitem; do
  SETUP+="CREATE OR REPLACE VIEW $t AS SELECT * FROM read_parquet('$DATA/$t.parquet');"
done

bad=0
for q in "${QUERIES[@]}"; do
  # engine at 1, 4 and 8 threads (also asserts them equal to each other and to the hand-fused baseline)
  "$BIN" check "$DATA" "$WORK" "$q"
  env -u AI_AGENT -u CLAUDECODE duckdb -noheader -list -separator '|' -c "$SETUP" -c "$(cat "$HERE/sql/$q.sql")" 2>"$WORK/$q.duckdb.err" \
    | LC_ALL=C sort > "$WORK/$q.duckdb.txt" || { echo "duckdb failed on $q:"; cat "$WORK/$q.duckdb.err"; exit 1; }
  [ ! -s "$WORK/$q.duckdb.err" ] || { echo "duckdb wrote to stderr on $q:"; cat "$WORK/$q.duckdb.err"; exit 1; }
  rows=$(wc -l < "$WORK/$q.duckdb.txt")
  for t in 1 4 8; do
    # The engine's files are already sorted by Rust's bytewise String order; sort again with LC_ALL=C to be safe.
    LC_ALL=C sort "$WORK/$q.engine.t$t.txt" > "$WORK/$q.engine.t$t.sorted"
    if [ "$rows" -eq 0 ]; then echo "FAIL $q: DuckDB returned no rows"; bad=1; continue; fi
    if cmp -s "$WORK/$q.duckdb.txt" "$WORK/$q.engine.t$t.sorted"; then
      echo "identical  $q  threads=$t  ($rows rows, sha256 $(sha256sum < "$WORK/$q.duckdb.txt" | cut -c1-12))"
    else
      echo "DIFFERENT  $q  threads=$t"
      diff <(head -20 "$WORK/$q.duckdb.txt") <(head -20 "$WORK/$q.engine.t$t.sorted") | head -10 || true
      bad=1
    fi
  done
  [ "$OWN" = 1 ] && rm -f "$WORK/$q".*   # this query is done
done
[ "$bad" -eq 0 ] && echo "ALL 7 QUERIES IDENTICAL at 1, 4 and 8 threads" || { echo "MISMATCH"; exit 1; }
