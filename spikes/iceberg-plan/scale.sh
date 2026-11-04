#!/bin/bash
# Planning at scale: metadata-only tables of 10k, 100k and 1M data files (one position delete per 100 files), clustered by day and scattered, planned with 1 and N
# threads. One process per query, so the peak RSS is that query's. usage: scale.sh <workdir> <results.jsonl> [sizes...]   (default sizes: 10000 100000 1000000)
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd); W=${1:?workdir}; OUT=${2:?results file}; shift 2
SIZES=${*:-10000 100000 1000000}; ICX=$HERE/target/release/icx; N=$(nproc)
mkdir -p "$W"; : > "$OUT"
for layout in clustered scattered; do
  for size in $SIZES; do
    [ "$layout" = scattered ] && [ "$size" -lt 100000 ] && continue
    d=$W/s-$layout-$size; rm -rf "$d"
    info=$("$ICX" synth "$d" "$size" "$layout" 10000); echo "$info" >&2
    for th in 1 "$N"; do
      for q in point week id name_prefix k_range nonselective all; do
        "$ICX" bench-one "$d" "$th" 3 "$q" | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); d.update(files=$size, layout='$layout'); print(json.dumps(d))" >> "$OUT"
      done
    done
    rm -rf "$d"
  done
done
python3 - "$OUT" <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
print(f"{'layout':10} {'files':>8} {'query':13} {'thr':>3} {'planned':>8} {'ms (min of 3)':>14} {'manifests read/total':>21} {'entries':>9} {'peak RSS MB':>11}")
for r in rows:
    print(f"{r['layout']:10} {r['files']:>8} {r['query']:13} {r['threads']:>3} {r['planned_files']:>8} {r['ms_min']:>14.0f} {r['manifests'] - r['manifests_pruned']:>10}/{r['manifests']:<10} {r['entries_examined']:>9} {r['peak_rss_mb']:>11}")
PY
