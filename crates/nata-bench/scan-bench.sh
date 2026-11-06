#!/usr/bin/env bash
# The slice 3 benchmark (docs/slices/0001-parquet-scan.md, rules R2 and the scalability row): `nata scan --count` (the scan without CSV
# rendering) on TPC-H `lineitem`.
#   scan-bench.sh <nata binary> <tpch-sf1 dir> <tpch-sf10 dir or -> <out dir> [rounds=5]
#   1. R2: SF1 `lineitem` rewritten sorted by l_shipdate in 122,880-row groups; the year predicate of the card with and without pruning:
#      row groups skipped (>= 60% required), bytes read pruned / unpruned (<= 40% required), and the row count equal to DuckDB's.
#   2. Scalability: the Q1 columns of `lineitem` (SF1, and SF10 if given) at 1, 2, 4 and 8 threads, window = threads and 2 x threads, with
#      a twin of the 1- and 8-thread arms as the noise control; 8-thread throughput >= 5x the 1-thread figure required.
#   3. (slice 4, step 4a; BLOCKS=late) P1 late materialization: `nata scan --count --no-prune` on the Q1 columns of `lineitem` with a filter that keeps
#      10% and 100% of the rows, once on a column that is not projected (l_partkey) and once on a projected one (l_shipdate), at 8 threads, window 16:
#      arms `early` (decode every column, then filter), `late` (`--late-materialization`) and `twin` (early again, the noise control), alternating
#      early, late, twin in every round. Rule of the card (docs/slices/0002-select-where-limit.md part 5): keep it only if at 10% it is at least 1.2x
#      faster and at 100% no slower than 1.05x, on SF1 and SF10 and both CPU types.
#   4. (BLOCKS=footers) P2 footers read ahead and P10: 1,000 small files (generated here from SF1 `lineitem`, 4,000 rows and one row group each, never
#      committed; under SCAN_WORK) scanned by `nata scan --count` at 8 threads with `--footers-ahead` 1 (slice 3), 8 (the configuration of the card),
#      and 4 and 16 (information), twin of the 1; and `nata sql` over the same glob with the slice-4 binder (`--bind-all-footers`, ahead 1) against the
#      P10 binder (first footer only, ahead 8). Rule: a scan of 1,000 small files at least 1.3x faster at 8 threads.
#   5. (BLOCKS=crc) R4 page checksums, the cost: SF1 (and SF10) `lineitem` copied by pyarrow WITH page checksums (crates/nata-bench/checksum-copy.py; needs
#      PYARROW_PYTHON, a python that has pyarrow), scanned by two builds of the same code, `nata` (verification on, built with --features page-crc) and NATA_NOCRC (built with
#      `cargo build --release -p nata --no-default-features`), alternating, plus a twin of the verifying build; and the same two builds on the original file
#      WITHOUT checksums (the control: the verification costs nothing where there is no checksum). Rule: on a file with checksums the explicitly CRC-enabled build must
#      be at most 1.05x the other, or the verification becomes an option.
#   BLOCKS (default "r2 scale") selects the blocks: any of r2, scale, late, footers, crc. Exit 1 if a count differs from DuckDB's.
# Every block waits for a settled machine first (scripts/gcp/settle.sh; its SETTLE line is the record). Wall time of the process; the
# scan's own report (stderr) gives the row groups, bytes read and peak RSS. Exit 1 if a count differs from DuckDB's or a run fails; the
# R2 and 5x verdicts are printed, not gating (they are what the slice's closeout reads).
set -uo pipefail
unset AI_AGENT     # the duckdb CLI switches to its agent mode (extra output) when it is set
BLOCKS=" ${BLOCKS:-r2 scale} "; want() { case "$BLOCKS" in *" $1 "*) return 0 ;; *) return 1 ;; esac; }
HERE=$(cd "$(dirname "$0")" && pwd)
NATA=$(realpath "${1:?nata binary}"); SF1=$(realpath "${2:?tpch-sf1 dir}"); SF10=${3:?tpch-sf10 dir or -}; OUT=${4:?out dir}; ROUNDS=${5:-5}
mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)
SETTLE=$HERE/../../scripts/gcp/settle.sh
settle() { [ -x "$SETTLE" ] && "$SETTLE" "$1" 0.5 "${SETTLE_MAX:-600}" | tee -a "$OUT/settle.txt"; echo "block $1 load at start: $(cut -d' ' -f1-3 /proc/loadavg)" >> "$OUT/settle.txt"; }
CSV=$OUT/runs.csv; echo "block,arm,threads,window,round,secs,rows,row_groups,skipped,bytes_read,peak_rss_kib,exit,peak_footers,peak_open_files" > "$CSV"; FAILED=0
timed() { # block arm threads window round command... : the command prints the row count on stdout and the scan report (last line) on stderr
  local blk=$1 arm=$2 t=$3 w=$4 r=$5; shift 5
  local s0 s1 rc rows rep
  s0=$(date +%s.%N); rows=$("$@" 2> "$OUT/.rep"); rc=$?; s1=$(date +%s.%N)
  rep=$(tail -1 "$OUT/.rep"); kv() { sed -n "s/.* $1=\([^ ]*\).*/\1/p" <<< " $rep"; }
  echo "$blk,$arm,$t,$w,$r,$(awk -v a="$s0" -v b="$s1" 'BEGIN{printf "%.3f", b-a}'),$rows,$(kv row_groups),$(kv skipped),$(kv bytes_read),$(kv peak_rss_kib),$rc,$(kv peak_footers),$(kv peak_open_files)" >> "$CSV"
  [ "$rc" = 0 ] || { echo "FAIL $blk $arm t=$t w=$w round $r: exit $rc: $rep"; FAILED=1; }
}
one() { # block arm threads window round file args...
  local blk=$1 arm=$2 t=$3 w=$4 r=$5 f=$6; shift 6
  timed "$blk" "$arm" "$t" "$w" "$r" "$NATA" scan "$f" --count --threads "$t" --window "$w" "$@"
}
unset NATA_LATE_MATERIALIZATION     # the arms say what they mean by flags, never by the environment
Q1COLS=l_returnflag,l_linestatus,l_quantity,l_extendedprice,l_discount,l_tax,l_shipdate
if want r2; then
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
fi
if want scale; then
# ---------------------------------------------------------------- scalability
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
fi
# ---------------------------------------------------------------- P1 late materialization (BLOCKS=late)
if want late; then
  for sf in sf1 sf10; do
    dir=$([ $sf = sf1 ] && echo "$SF1" || echo "$SF10"); [ "$dir" = - ] && continue; F=$dir/lineitem.parquet; [ -f "$F" ] || continue
    total=$(duckdb -noheader -csv -c "SELECT count(*) FROM '$F';" | tail -1)
    # the thresholds that keep 10% of the rows (from DuckDB), and filters that keep every row; neither column is sorted in the file, so no row group is
    # pruned (and --no-prune says so): the arms differ in the decoding only
    pk10=$(duckdb -noheader -csv -c "SELECT quantile_disc(l_partkey, 0.1) FROM '$F';" | tail -1)
    sd10=$(duckdb -noheader -csv -c "SELECT strftime(quantile_disc(l_shipdate, 0.1), '%Y-%m-%d') FROM '$F';" | tail -1)
    want10_pk=$(duckdb -noheader -csv -c "SELECT count(*) FROM '$F' WHERE l_partkey <= $pk10;" | tail -1)
    want10_sd=$(duckdb -noheader -csv -c "SELECT count(*) FROM '$F' WHERE l_shipdate <= DATE '$sd10';" | tail -1)
    "$NATA" scan "$F" --count --columns "$Q1COLS" > /dev/null 2>&1    # warm-up (page cache)
    # column "pk": the filter column is NOT projected; column "sd": it is (l_shipdate is one of the Q1 columns, so the late arm decodes it twice)
    for combo in "pk 10 l_partkey<=$pk10 $want10_pk" "pk 100 l_partkey>=0 $total" "sd 10 l_shipdate<=$sd10 $want10_sd" "sd 100 l_shipdate>=1900-01-01 $total"; do
      set -- $combo; col=$1; sel=$2; flt=$(sed -E 's/(<=|>=)/ \1 /' <<< "$3"); want_rows=$4; blk="late-$sf-$col-$sel"
      settle "$blk"
      for r in $(seq 1 "$ROUNDS"); do
        one "$blk" early 8 16 "$r" "$F" --columns "$Q1COLS" --filter "$flt" --no-prune
        one "$blk" late 8 16 "$r" "$F" --columns "$Q1COLS" --filter "$flt" --no-prune --late-materialization
        one "$blk" twin 8 16 "$r" "$F" --columns "$Q1COLS" --filter "$flt" --no-prune
      done
      bad=$(awk -F, -v b="$blk" -v w="$want_rows" '$1==b && $7!=w' "$CSV" | wc -l); [ "$bad" = 0 ] || { echo "FAIL $blk: $bad run(s) returned a count other than DuckDB's $want_rows"; FAILED=1; }
    done
  done
fi
# ---------------------------------------------------------------- P2 footers ahead and P10 (BLOCKS=footers)
if want footers; then
  N=${FOOTER_FILES:-1000}; FROWS=${FOOTER_ROWS:-2048}
  FDIR=${SCAN_WORK:-$OUT}/.small-files; rm -rf "$FDIR"; mkdir -p "$(dirname "$FDIR")"
  # N small files of FROWS rows (2,048: DuckDB rounds a row group up to its vector size) and one row group each, cut from SF1 lineitem (never committed: they live under SCAN_WORK and are deleted below)
  duckdb -c "SET threads=1; COPY (SELECT l_orderkey, l_partkey, l_quantity, l_extendedprice, l_discount, l_shipdate FROM '$SF1/lineitem.parquet' LIMIT $(((N + 8) * FROWS))) TO '$FDIR' (FORMAT parquet, ROW_GROUP_SIZE $FROWS, ROW_GROUPS_PER_FILE 1);" > /dev/null || { echo "could not write the small files"; exit 2; }
  # DuckDB may cut one file short; make exactly N, in the order of the names
  ls "$FDIR" | sort -V | tail -n +$((N + 1)) | while read -r extra; do rm -f "$FDIR/$extra"; done
  FILES=("$FDIR"/*.parquet)
  [ "${#FILES[@]}" = "$N" ] || { echo "FAIL footers: expected $N files, found ${#FILES[@]}"; FAILED=1; }
  want_rows=$(duckdb -noheader -csv -c "SELECT count(*) FROM '$FDIR/*.parquet' WHERE l_quantity < 2;" | tail -1)
  SQLQ="SELECT l_orderkey, l_extendedprice FROM '$FDIR/*.parquet' WHERE l_quantity < 2"
  sql_rows() { "$NATA" sql --csv --report "$@" -c "$SQLQ" | awk 'END{print NR-1}'; }     # the row count; the scan report is the last line of stderr
  scan_files() { "$NATA" scan "${FILES[@]}" --count --columns l_orderkey,l_extendedprice --filter "l_quantity < 2" "$@"; }
  for f in 1 8; do scan_files --footers-ahead $f > /dev/null 2>&1; done     # warm-up (page cache)
  settle footers
  for r in $(seq 1 "$ROUNDS"); do
    # arms alternate in every round; 8 = the threads: the configuration the card's rule is about; 4 and 16 are information; twin = the first arm again
    for armahead in ahead1:1 ahead8:8 twin1:1 ahead4:4 ahead16:16; do
      timed footers-scan "${armahead%%:*}" 8 16 "$r" scan_files --threads 8 --window 16 --footers-ahead "${armahead##*:}"
    done
    # nata sql over the same glob: the slice-4 binder (every footer read at bind time, one file at a time) against the P10 binder (the first footer only)
    timed footers-sql old 8 16 "$r" sql_rows --threads 8 --window 16 --footers-ahead 1 --bind-all-footers
    timed footers-sql new 8 16 "$r" sql_rows --threads 8 --window 16 --footers-ahead 8
    timed footers-sql twin 8 16 "$r" sql_rows --threads 8 --window 16 --footers-ahead 1 --bind-all-footers
  done
  for blk in footers-scan footers-sql; do
    bad=$(awk -F, -v b="$blk" -v w="$want_rows" '$1==b && $7!=w' "$CSV" | wc -l); [ "$bad" = 0 ] || { echo "FAIL $blk: $bad run(s) returned a count other than DuckDB's $want_rows"; FAILED=1; }
  done
  rm -rf "$FDIR"
fi
# ---------------------------------------------------------------- R4 page checksums, the cost (BLOCKS=crc)
if want crc; then
  : "${NATA_NOCRC:?BLOCKS=crc needs NATA_NOCRC, a nata built with --no-default-features}" "${PYARROW_PYTHON:?BLOCKS=crc needs PYARROW_PYTHON, a python with pyarrow}"
  NOCRC=$(realpath "$NATA_NOCRC")
  for sf in sf1 sf10; do
    dir=$([ $sf = sf1 ] && echo "$SF1" || echo "$SF10"); [ "$dir" = - ] && continue; F=$dir/lineitem.parquet; [ -f "$F" ] || continue
    CK=${SCAN_WORK:-$OUT}/.lineitem-$sf-checksummed.parquet; mkdir -p "$(dirname "$CK")"
    $PYARROW_PYTHON "$HERE/checksum-copy.py" "$F" "$CK" || { echo "could not write the checksummed copy"; exit 2; }
    total=$(duckdb -noheader -csv -c "SELECT count(*) FROM '$F';" | tail -1)
    crcscan() { local bin=$1; shift; "$bin" scan "$@" --count --columns "$Q1COLS" --threads 8 --window 16; }
    for f in "$CK" "$F"; do crcscan "$NATA" "$f" > /dev/null 2>&1; crcscan "$NOCRC" "$f" > /dev/null 2>&1; done     # warm-up (page cache)
    for kind in ck plain; do
      f=$([ $kind = ck ] && echo "$CK" || echo "$F"); blk="crc-$sf-$kind"
      settle "$blk"
      for r in $(seq 1 "$ROUNDS"); do
        timed "$blk" crc 8 16 "$r" crcscan "$NATA" "$f"
        timed "$blk" nocrc 8 16 "$r" crcscan "$NOCRC" "$f"
        timed "$blk" twin 8 16 "$r" crcscan "$NATA" "$f"
      done
      bad=$(awk -F, -v b="$blk" -v w="$total" '$1==b && $7!=w' "$CSV" | wc -l); [ "$bad" = 0 ] || { echo "FAIL $blk: $bad run(s) returned a count other than DuckDB's $total"; FAILED=1; }
    done
    rm -f "$CK"
  done
fi
rm -f "${SORTED:-}" "$OUT/.rep"
python3 -I - "$CSV" > "$OUT/summary.md" <<'PY'
import csv, statistics as st, sys
from collections import defaultdict
rows = [r for r in csv.DictReader(open(sys.argv[1])) if r["exit"] == "0"]
g = defaultdict(list)
for r in rows: g[(r["block"], r["arm"], int(r["threads"]), int(r["window"]))].append(r)
def med(k, f): return st.median(float(x[f]) for x in g[k]) if g.get(k) else float("nan")
p, u = ("r2", "pruned", 8, 8), ("r2", "unpruned", 8, 8)
if g.get(p) and g.get(u):
    print("## R2: pruning on SF1 lineitem sorted by l_shipdate, 122,880-row groups, the 1994 predicate, 8 threads")
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

late = sorted({k[0] for k in g if k[0].startswith("late-")})
if late:
    print("\n## P1 late materialization: `nata scan --count --no-prune`, the 7 Q1 columns, 8 threads, window 16 (median of the rounds; twin = the early arm again)")
    print("| block | rows | early s | late s | early / late | twin / early | bytes read early | bytes read late | verdict |")
    print("|---|---:|---:|---:|---:|---:|---:|---:|---|")
    for b in late:
        e, l, t = (b, "early", 8, 16), (b, "late", 8, 16), (b, "twin", 8, 16)
        if not (g.get(e) and g.get(l)): continue
        se, sl = med(e, "secs"), med(l, "secs"); ratio = se / sl
        sel = b.rsplit("-", 1)[1]; col = b.split("-")[2]
        if sel == "10": v = "speed-up %s (card: at least 1.2x%s)" % ("MET" if ratio >= 1.2 else "NOT MET", "" if col == "pk" else "; judged on the not-projected column, reported here")
        else: v = "slowdown %s (card: late/early at most 1.05)" % ("MET" if sl / se <= 1.05 else "NOT MET")
        tw = f"{med(t,'secs')/se:.2f}" if g.get(t) else "-"
        print(f"| {b} | {g[e][0]['rows']} | {se:.3f} | {sl:.3f} | {ratio:.2f} | {tw} | {med(e,'bytes_read'):.0f} | {med(l,'bytes_read'):.0f} | {v} |")

if g.get(("footers-scan", "ahead1", 8, 16)):
    print("\n## P2 footers read ahead, P10: %s small files, 8 threads, window 16 (median of the rounds)" % "1,000")
    print("| block | arm | median s | min s | vs first arm | peak footers / open files (report) | verdict |")
    print("|---|---|---:|---:|---:|---|---|")
    base = med(("footers-scan", "ahead1", 8, 16), "secs")
    for arm in ("ahead1", "twin1", "ahead4", "ahead8", "ahead16"):
        k = ("footers-scan", arm, 8, 16)
        if not g.get(k): continue
        s_ = med(k, "secs"); v = ""
        if arm == "ahead8": v = "speed-up %.2fx: %s (card: at least 1.3x)" % (base / s_, "MET" if base / s_ >= 1.3 else "NOT MET")
        pf = "%d / %d" % (max(int(x["peak_footers"] or 0) for x in g[k]), max(int(x["peak_open_files"] or 0) for x in g[k]))
        print(f"| footers-scan | {arm} | {s_:.3f} | {min(float(x['secs']) for x in g[k]):.3f} | {base/s_:.2f}x | {pf} | {v} |")
    if g.get(("footers-sql", "old", 8, 16)) and g.get(("footers-sql", "new", 8, 16)):
        o, n = med(("footers-sql", "old", 8, 16), "secs"), med(("footers-sql", "new", 8, 16), "secs")
        tw = med(("footers-sql", "twin", 8, 16), "secs")
        print(f"| footers-sql | old (all footers at bind, ahead 1) | {o:.3f} | - | 1.00x | - | |")
        print(f"| footers-sql | new (first footer at bind, ahead 8) | {n:.3f} | - | {o/n:.2f}x | - | information (twin/old {tw/o:.2f}) |")

crcb = sorted({k[0] for k in g if k[0].startswith("crc-")})
if crcb:
    print("\n## R4 page checksums, the cost: `nata scan --count`, the 7 Q1 columns, 8 threads, window 16; the verifying build against the build without the check (median of the rounds)")
    print("| block | rows | verifying s | without s | verifying / without | twin / verifying | verdict |")
    print("|---|---:|---:|---:|---:|---:|---|")
    for b in crcb:
        c, n, t = (b, "crc", 8, 16), (b, "nocrc", 8, 16), (b, "twin", 8, 16)
        if not (g.get(c) and g.get(n)): continue
        sc, sn = med(c, "secs"), med(n, "secs")
        v = ("on a file WITH checksums: %s (card: at most 1.05)" % ("MET" if sc / sn <= 1.05 else "NOT MET -> becomes an option")) if b.endswith("-ck") else "control, a file without checksums"
        tw = f"{med(t,'secs')/sc:.2f}" if g.get(t) else "-"
        print(f"| {b} | {g[c][0]['rows']} | {sc:.3f} | {sn:.3f} | {sc/sn:.2f} | {tw} | {v} |")
PY
cat "$OUT/summary.md"
[ "$(awk -F, 'NR>1 && $12==0' "$CSV" | wc -l)" -gt 0 ] || { echo "nothing ran"; FAILED=1; }
exit $FAILED
