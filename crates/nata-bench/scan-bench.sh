#!/usr/bin/env bash
# The slice 3 benchmark (docs/slices/0001-parquet-scan.md, rules R2 and the scalability row): `nata scan --count` (the scan without CSV
# rendering) on TPC-H `lineitem`.
#   scan-bench.sh <nata binary> <tpch-sf1 dir> <tpch-sf10 dir or -> <out dir> [rounds=5]
#   1. R2: SF1 `lineitem` rewritten sorted by l_shipdate in 122,880-row groups; the year predicate of the card with and without pruning:
#      row groups skipped (>= 60% required), bytes read pruned / unpruned (<= 40% required), and the row count equal to DuckDB's.
#   2. Scalability: the Q1 columns of `lineitem` (SF1, and SF10 if given) at 1, 2, 4 and 8 threads, window = threads and 2 x threads, with
#      a twin of the 1- and 8-thread arms as the noise control; 8-thread throughput >= 5x the 1-thread figure required.
# Every block waits for a settled machine first (scripts/gcp/settle.sh; its SETTLE line is the record). Wall time of the process; the
# scan's own report (stderr) gives the row groups, bytes read and peak RSS. Exit 1 if a count differs from DuckDB's or a run fails; the
# R2 and 5x verdicts are printed, not gating (they are what the slice's closeout reads).
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
NATA=$(realpath "${1:?nata binary}"); SF1=$(realpath "${2:?tpch-sf1 dir}"); SF10=${3:?tpch-sf10 dir or -}; OUT=${4:?out dir}; ROUNDS=${5:-5}
mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)
SETTLE=$HERE/../../scripts/gcp/settle.sh
settle() { [ -x "$SETTLE" ] && "$SETTLE" "$1" 0.5 "${SETTLE_MAX:-600}" | tee -a "$OUT/settle.txt"; echo "block $1 load at start: $(cut -d' ' -f1-3 /proc/loadavg)" >> "$OUT/settle.txt"; }
CSV=$OUT/runs.csv; echo "block,arm,threads,window,round,secs,rows,row_groups,skipped,bytes_read,peak_rss_kib,exit" > "$CSV"; FAILED=0
one() { # block arm threads window round file args...
  local blk=$1 arm=$2 t=$3 w=$4 r=$5 f=$6; shift 6
  local s0 s1 rc rows rep
  s0=$(date +%s.%N); rows=$("$NATA" scan "$f" --count --threads "$t" --window "$w" "$@" 2> "$OUT/.rep"); rc=$?; s1=$(date +%s.%N)
  rep=$(tail -1 "$OUT/.rep"); kv() { sed -n "s/.* $1=\([^ ]*\).*/\1/p" <<< " $rep"; }
  echo "$blk,$arm,$t,$w,$r,$(awk -v a="$s0" -v b="$s1" 'BEGIN{printf "%.3f", b-a}'),$rows,$(kv row_groups),$(kv skipped),$(kv bytes_read),$(kv peak_rss_kib),$rc" >> "$CSV"
  [ "$rc" = 0 ] || { echo "FAIL $blk $arm t=$t w=$w round $r: exit $rc: $rep"; FAILED=1; }
}
# ---------------------------------------------------------------- R2
# the sorted copy (about 200 MB at SF1) goes to SCAN_WORK, never under the results (which the orchestrator pulls every 2 minutes)
SORTED=${SCAN_WORK:-$OUT}/.lineitem-sorted.parquet; mkdir -p "$(dirname "$SORTED")"
duckdb -c "COPY (SELECT * FROM '$SF1/lineitem.parquet' ORDER BY l_shipdate) TO '$SORTED' (FORMAT parquet, ROW_GROUP_SIZE 122880);" > /dev/null || { echo "could not write the sorted file"; exit 2; }
F1=(--filter "l_shipdate >= 1994-01-01" --filter "l_shipdate < 1995-01-01")
want=$(duckdb -noheader -csv -c "SELECT count(*) FROM '$SORTED' WHERE l_shipdate >= DATE '1994-01-01' AND l_shipdate < DATE '1995-01-01';" | tail -1)
settle r2
for r in $(seq 1 "$ROUNDS"); do
  one r2 pruned 8 8 "$r" "$SORTED" --columns l_orderkey,l_extendedprice,l_shipdate "${F1[@]}"
  one r2 unpruned 8 8 "$r" "$SORTED" --columns l_orderkey,l_extendedprice,l_shipdate "${F1[@]}" --no-prune
done
bad=$(awk -F, -v w="$want" '$1=="r2" && $7!=w' "$CSV" | wc -l); [ "$bad" = 0 ] || { echo "FAIL r2: $bad run(s) returned a count other than DuckDB's $want"; FAILED=1; }
# ---------------------------------------------------------------- scalability
Q1COLS=l_returnflag,l_linestatus,l_quantity,l_extendedprice,l_discount,l_tax,l_shipdate
for sf in sf1 sf10; do
  dir=$([ $sf = sf1 ] && echo "$SF1" || echo "$SF10"); [ "$dir" = - ] && continue; F=$dir/lineitem.parquet; [ -f "$F" ] || continue
  want=$(duckdb -noheader -csv -c "SELECT count(*) FROM '$F';" | tail -1)
  "$NATA" scan "$F" --count --columns "$Q1COLS" > /dev/null 2>&1    # warm-up (page cache)
  for t in 1 2 4 8; do
    for w in "$t" $((2 * t)); do
      settle "$sf-t$t-w$w"
      for r in $(seq 1 "$ROUNDS"); do
        one "$sf" scan "$t" "$w" "$r" "$F" --columns "$Q1COLS"
        if [ "$w" = "$t" ] && { [ "$t" = 1 ] || [ "$t" = 8 ]; }; then one "$sf" twin "$t" "$w" "$r" "$F" --columns "$Q1COLS"; fi
      done
    done
  done
  bad=$(awk -F, -v b="$sf" -v w="$want" '$1==b && $7!=w' "$CSV" | wc -l); [ "$bad" = 0 ] || { echo "FAIL $sf: $bad run(s) returned a count other than DuckDB's $want"; FAILED=1; }
done
rm -f "$SORTED" "$OUT/.rep"
python3 -I - "$CSV" > "$OUT/summary.md" <<'PY'
import csv, statistics as st, sys
from collections import defaultdict
rows = [r for r in csv.DictReader(open(sys.argv[1])) if r["exit"] == "0"]
g = defaultdict(list)
for r in rows: g[(r["block"], r["arm"], int(r["threads"]), int(r["window"]))].append(r)
def med(k, f): return st.median(float(x[f]) for x in g[k]) if g.get(k) else float("nan")
print("## R2: pruning on SF1 lineitem sorted by l_shipdate, 122,880-row groups, the 1994 predicate, 8 threads")
p, u = ("r2", "pruned", 8, 8), ("r2", "unpruned", 8, 8)
if g.get(p) and g.get(u):
    rg, sk = int(g[p][0]["row_groups"]), int(g[p][0]["skipped"])
    bp, bu = med(p, "bytes_read"), med(u, "bytes_read")
    print(f"row groups skipped: {sk} of {rg} ({sk/rg:.1%}; R2 requires at least 60%): {'MET' if sk/rg >= 0.6 else 'NOT MET'}")
    print(f"bytes read pruned / unpruned: {bp:.0f} / {bu:.0f} = {bp/bu:.1%} (the card requires at most 40%): {'MET' if bp/bu <= 0.4 else 'NOT MET'}")
    print(f"wall: pruned {med(p,'secs'):.3f} s, unpruned {med(u,'secs'):.3f} s; rows returned {g[p][0]['rows']} (equal to DuckDB's in every run)")
for sf in ("sf1", "sf10"):
    ks = sorted(k for k in g if k[0] == sf)
    if not ks: continue
    print(f"\n## Scalability, {sf}, the 7 Q1 columns, --count (median of the rounds; the twin is the same arm again)")
    print("| threads | window | median s | rows/s (M) | speed-up vs 1 thread | efficiency | peak RSS MB | twin / arm |")
    print("|---:|---:|---:|---:|---:|---:|---:|---:|")
    b = med((sf, "scan", 1, 1), "secs"); n = int(g[(sf, "scan", 1, 1)][0]["rows"])
    for k in [k for k in ks if k[1] == "scan"]:
        s = med(k, "secs"); tw = (sf, "twin", k[2], k[3])
        twin = f"{med(tw,'secs')/s:.2f}" if g.get(tw) else "-"
        print(f"| {k[2]} | {k[3]} | {s:.3f} | {n/s/1e6:.1f} | {b/s:.2f} | {b/s/k[2]:.2f} | {med(k,'peak_rss_kib')/1024:.0f} | {twin} |")
    s8 = min(med((sf, "scan", 8, 8), "secs"), med((sf, "scan", 8, 16), "secs"))
    print(f"\n8-thread speed-up (best window): {b/s8:.2f}x (the card requires at least 5x): {'MET' if b/s8 >= 5 else 'NOT MET'}")
PY
cat "$OUT/summary.md"
[ "$(grep -c ',0$' "$CSV")" -gt 0 ] || { echo "nothing ran"; FAILED=1; }
exit $FAILED
