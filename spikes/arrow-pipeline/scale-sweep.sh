#!/usr/bin/env bash
# Thread sweep of Q1 and Q6 on the protocol scheduler at a large scale factor, against DuckDB (verification-plan item 32).
#   scale-sweep.sh <lineitem.parquet> <out dir> [threads="1 2 4 8 16 32"] [rounds=5]
# Per (query, threads): wait for a settled machine (scripts/gcp/settle.sh; the SETTLE line is the record), then ROUNDS alternations of
# ours (`csv`, wall time of the process, Parquet decode included) and DuckDB (`SET threads`, same SQL, same file, wall time). EVERY output
# of ours must equal DuckDB's digit for digit (exit 1 otherwise). summary.md: medians, speed-up over 1 thread and parallel efficiency
# (speed-up / threads) for both engines.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
F=$(realpath "${1:?lineitem.parquet}"); OUT=${2:?out dir}; THREADS=${3:-1 2 4 8 16 32}; ROUNDS=${4:-5}
case "$ROUNDS" in ""|*[!0-9]*) echo "ROUNDS must be a whole number, got '$ROUNDS'"; exit 2 ;; esac
mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)
BIN=$HERE/target/release/spike-arrow-pipeline; [ -x "$BIN" ] || { echo "build first: cargo build --release"; exit 2; }
SETTLE=$HERE/../../scripts/gcp/settle.sh
Q1="SELECT l_returnflag, l_linestatus, sum(l_quantity), sum(l_extendedprice), sum(l_extendedprice*(1-l_discount)), count(*) FROM '$F' WHERE l_shipdate <= DATE '1998-09-02' GROUP BY 1,2 ORDER BY 1,2;"
Q6="SELECT sum(l_extendedprice*l_discount) FROM '$F' WHERE l_shipdate >= DATE '1994-01-01' AND l_shipdate < DATE '1995-01-01' AND l_discount BETWEEN 0.05 AND 0.07 AND l_quantity < 24;"
CSV=$OUT/rounds.csv; echo "query,threads,round,engine,secs,identical" > "$CSV"; FAILED=0
wall() { local s0 s1; s0=$(date +%s.%N); "$@"; local rc=$?; s1=$(date +%s.%N); awk -v a="$s0" -v b="$s1" 'BEGIN{printf "%.3f", b-a}' > "$OUT/.secs"; return $rc; }
for q in q1 q6; do
  sql=$([ $q = q1 ] && echo "$Q1" || echo "$Q6")
  want=$(env -u AI_AGENT duckdb -noheader -csv -c "$sql") || { echo "duckdb failed"; exit 2; }
  for t in $THREADS; do
    [ -x "$SETTLE" ] && "$SETTLE" "$q-t$t" 0.5 "${SETTLE_MAX:-600}" | tee -a "$OUT/settle.txt"
    echo "block $q threads=$t load at start: $(cut -d' ' -f1-3 /proc/loadavg)" >> "$OUT/settle.txt"
    "$BIN" csv "$F" "$q" "$t" > /dev/null 2>&1                         # warm-up (page cache)
    for r in $(seq 1 "$ROUNDS"); do
      wall "$BIN" csv "$F" "$q" "$t" > "$OUT/.got" 2> /dev/null; s=$(cat "$OUT/.secs")
      if [ "$(cat "$OUT/.got")" == "$want" ]; then same=yes; else same=NO; FAILED=1; echo "DIFF $q threads=$t round $r"; diff <(echo "$want") "$OUT/.got" | head -5; fi
      echo "$q,$t,$r,ours,$s,$same" >> "$CSV"
      wall env -u AI_AGENT duckdb -noheader -csv -c "SET threads=$t; $sql" > /dev/null 2>&1; echo "$q,$t,$r,duckdb,$(cat "$OUT/.secs"),-" >> "$CSV"
    done
  done
done
rm -f "$OUT/.got" "$OUT/.secs"
python3 -I - "$CSV" > "$OUT/summary.md" <<'PY'
import csv, statistics, sys
from collections import defaultdict
rows = list(csv.DictReader(open(sys.argv[1])))
med = defaultdict(list)
for r in rows: med[(r["query"], int(r["threads"]), r["engine"])].append(float(r["secs"]))
print("| query | threads | ours median s | DuckDB median s | ours / DuckDB | ours speed-up vs 1 thread | ours efficiency | DuckDB speed-up | DuckDB efficiency |")
print("|---|---:|---:|---:|---:|---:|---:|---:|---:|")
for q in ("q1", "q6"):
    ts = sorted({t for (qq, t, e) in med if qq == q})
    if not ts: continue
    b_o = statistics.median(med[(q, ts[0], "ours")]); b_d = statistics.median(med[(q, ts[0], "duckdb")])
    for t in ts:
        o = statistics.median(med[(q, t, "ours")]); d = statistics.median(med[(q, t, "duckdb")])
        print(f"| {q} | {t} | {o:.3f} | {d:.3f} | {o/d:.2f} | {b_o/o:.2f} | {b_o/o/t*ts[0]:.2f} | {b_d/d:.2f} | {b_d/d/t*ts[0]:.2f} |")
mism = sum(1 for r in rows if r["identical"] == "NO")
print(f"\nOutputs compared with DuckDB: {sum(1 for r in rows if r['engine']=='ours')}, mismatches: {mism}")
PY
cat "$OUT/summary.md"
[ "$(grep -c ",ours," "$CSV")" -gt 0 ] || { echo "nothing was compared"; FAILED=1; }
exit $FAILED
