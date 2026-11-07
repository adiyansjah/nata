#!/usr/bin/env bash
# Whole-system view of the aggregate spill path (parent, 2026-11-08): wall vs CPU time, kernel and I/O shares, spill counters.
# For each high-cardinality cell (1 and 8 threads; in memory and 64 MiB): `perf stat` (task-clock, context switches, faults),
# `/usr/bin/time -v`, `iostat -x 1` alongside, and a `perf record` with kernel samples sorted by dso and symbol.
set -uo pipefail
: "${WORK:?}" "${RESULTS:?}"
nata=$WORK/target/release/nata; data=$WORK/sf1; out=$RESULTS/spill-whole; mkdir -p "$out" "$WORK/spill"
sql=$(sed "s|{data}|$data|g" check/queries/aggregate/high_cardinality_tpch.sql)
status=0
for threads in 1 8; do for mib in 8192 64; do
  label=hc-${threads}t-${mib}MiB
  cmd=("$nata" sql --csv --report --threads "$threads" --aggregate-memory-bytes $((mib << 20)) --spill-directory "$WORK/spill" --spill-disk-bytes $((16 << 30)) "--command=$sql")
  "${cmd[@]}" >/dev/null 2>"$out/$label-warm.txt" || status=1
  iostat -x 1 > "$out/$label-iostat.txt" & io=$!
  env -u AI_AGENT perf stat -e task-clock,context-switches,cpu-migrations,page-faults,major-faults -o "$out/$label-stat.txt" -- /usr/bin/time -v "${cmd[@]}" >/dev/null 2>"$out/$label-time.txt" || status=1
  kill $io
  env -u AI_AGENT perf record -q -e cpu-clock -F 499 -g -o "$WORK/$label.data" -- "${cmd[@]}" >/dev/null 2>"$out/$label-report-stderr.txt" || status=1
  perf report -i "$WORK/$label.data" --stdio --no-children --sort dso,symbol --percent-limit 0.3 2>/dev/null | c++filt -s rust | grep -v '^$' | head -n 120 > "$out/$label-system.txt"
  perf report -i "$WORK/$label.data" --stdio --no-children --sort dso --percent-limit 0.1 2>/dev/null | grep -v '^#' | grep -v '^$' > "$out/$label-dso.txt"
  rm -f "$WORK/$label.data"
  echo "$label: $(grep -E 'seconds time elapsed|task-clock' "$out/$label-stat.txt" | tr -s ' ' | tr '\n' ' ')"
  grep -m1 '^aggregate:' "$out/$label-time.txt" | cut -c1-400
done; done
exit $status
