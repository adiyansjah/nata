#!/usr/bin/env bash
# What the spill path of the grace hash join costs (verification-plan item 15: "hash-join spill unmeasured").
#   spill-bench.sh <tpch dir> <out dir> [rounds=7] [threads="1 4 8"] [queries="join-inner join-left q13"] [arms=...]
# Per (query, threads) block: wait for a settled machine (scripts/gcp/settle.sh, 1-minute load < 0.5, at most 600 s; the
# SETTLE line is the record, a NOT SETTLED block is SANITY ONLY), then ROUNDS rounds of every arm in a rotating order:
#   mem        in memory (no budget)                   mem2  the same again (the noise control: a twin)
#   g256       grace, 32 partitions, 256 MB budget     g64   grace, 32 partitions, 64 MB budget
#   duckdb     DuckDB on the same Parquet files and SQL (sql/<query>.sql), same thread count, as the reference
# Every run's wall time (process start to end, for every arm alike, so DuckDB and ours compare the same thing; the engine's own
# timer is kept as engine_secs), peak RSS (GNU time) and, for ours, rows / spilled MB / spill files / most open files / leftover files
# go to rounds.csv; spill_summary.py writes summary.md (medians, paired ratios against `mem` with quartiles, a NOISY verdict
# when the twin's spread covers the difference). A run that fails, a row count that differs between arms, or a leftover
# spill file is a FAILURE (exit 1); correctness against DuckDB itself is check.sh's job and is not repeated here.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
DATA=$(cd "${1:?tpch dir}" && pwd); OUT=${2:?out dir}; ROUNDS=${3:-7}; THREADS=${4:-1 4 8}
QUERIES=${5:-join-inner join-left q13}; ARMS=${6:-mem mem2 g256 g64 duckdb}
mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)
# spill files go under SPILL_ROOT (default: the out dir). On GCP it must NOT be under the results, which are pulled every 2 min.
[ -n "${SPILL_ROOT:-}" ] && mkdir -p "$SPILL_ROOT"
BIN=$HERE/target/release/opsbeyond; [ -x "$BIN" ] || { echo "build first: cargo build --release"; exit 2; }
SETTLE=$HERE/../../scripts/gcp/settle.sh
TIMEBIN=/usr/bin/time; [ -x "$TIMEBIN" ] || { echo "GNU time is needed for peak RSS (apt install time)"; exit 2; }
command -v duckdb >/dev/null || ARMS=$(echo "$ARMS" | sed 's/duckdb//')
views="create view customer as select * from '$DATA/customer.parquet'; create view orders as select * from '$DATA/orders.parquet'; create view lineitem as select * from '$DATA/lineitem.parquet'; create view supplier as select * from '$DATA/supplier.parquet'; create view nation as select * from '$DATA/nation.parquet'; create view part as select * from '$DATA/part.parquet'; create view partsupp as select * from '$DATA/partsupp.parquet'; create view region as select * from '$DATA/region.parquet';"
CSV=$OUT/rounds.csv; echo "query,threads,round,arm,secs,engine_secs,peak_rss_mb,rows,spilled_mb,spill_files,max_open_files,leftover,status" > "$CSV"
FAILED=0
args_of() { case "$1" in mem|mem2) echo "" ;; g256) echo "--grace 32 --mem-mb 256" ;; g64) echo "--grace 32 --mem-mb 64" ;; g1) echo "--grace 32 --mem-mb 1" ;; esac; }
one() { # query threads round arm
  local q=$1 t=$2 r=$3 a=$4 m=$OUT/.rss line secs rows sp sf mo lo st
  if [ "$a" = duckdb ]; then
    local s0 s1; s0=$(date +%s.%N)
    "$TIMEBIN" -f %M -o "$m" duckdb -c "set threads=$t; $views select count(*) from ($(cat "$HERE/sql/$q.sql"));" > "$OUT/.duck" 2>&1 && st=ok || st=FAILED
    s1=$(date +%s.%N); secs=$(awk -v a="$s0" -v b="$s1" 'BEGIN{printf "%.3f", b-a}')
    rows=$(grep -Eo '[0-9]+' "$OUT/.duck" | tail -1); esecs=; sp=; sf=; mo=; lo=
  else
    local s0 s1; s0=$(date +%s.%N)
    line=$("$TIMEBIN" -f %M -o "$m" "$BIN" "$q" --data "$DATA" --threads "$t" --spill-dir "${SPILL_ROOT:-$OUT}/spill-$q-$t-$a" $(args_of "$a") 2>&1 | tail -1)
    s1=$(date +%s.%N); secs=$(awk -v a="$s0" -v b="$s1" 'BEGIN{printf "%.3f", b-a}')
    kv() { sed -n "s/.* $1=\([^ ]*\).*/\1/p" <<<" $line"; }
    st=$(kv status); esecs=$(kv secs); rows=$(kv rows); sp=$(kv spilled_mb); sf=$(kv spill_files); mo=$(kv max_open_files)
    # leftover spill files: counted on disk after the process has exited (the binary's own count is not printed when it fails)
    local sd="${SPILL_ROOT:-$OUT}/spill-$q-$t-$a"; lo=0; [ -d "$sd" ] && lo=$(find "$sd" -type f | wc -l); rm -rf "$sd"
    [ "$st" = ok ] || { echo "FAIL $q t=$t $a: $line"; FAILED=1; }
    [ "${lo:-1}" = 0 ] || { echo "FAIL $q t=$t $a: $lo spill files left on disk after the process exited ($st)"; FAILED=1; }
  fi
  echo "$q,$t,$r,$a,${secs:-},${esecs:-},$(( $(tail -1 "$m" 2>/dev/null || echo 0) / 1024 )),${rows:-},${sp:-},${sf:-},${mo:-},${lo:-},${st:-FAILED}" >> "$CSV"
}
for q in $QUERIES; do
  for t in $THREADS; do
    [ -x "$SETTLE" ] && "$SETTLE" "$q-t$t" 0.5 "${SETTLE_MAX:-600}" | tee -a "$OUT/settle.txt"
    echo "block $q threads=$t load at start: $(cut -d' ' -f1-3 /proc/loadavg)" | tee -a "$OUT/settle.txt"
    one "$q" "$t" 0 mem                                   # warm-up (page cache), not recorded as a round
    sed -i '$d' "$CSV"
    set -- $ARMS; n=$#
    for r in $(seq 1 "$ROUNDS"); do
      for i in $(seq 0 $((n - 1))); do arm=$(echo $ARMS | tr ' ' '\n' | sed -n "$(( (r + i) % n + 1 ))p"); one "$q" "$t" "$r" "$arm"; done
    done
  done
done
rm -f "$OUT/.rss" "$OUT/.duck"
# the row count must be the same for every arm of a query (ours and DuckDB's count)
python3 -I "$HERE/spill_summary.py" "$CSV" > "$OUT/summary.md" || FAILED=1
cat "$OUT/summary.md"
exit $FAILED
