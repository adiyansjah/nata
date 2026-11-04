#!/usr/bin/env bash
# Run the Kani harnesses of spikes/kani-nata-kernels on one big machine, P at a time (CBMC is single-threaded; on the
# 64 GB VM a handful fit). One result file per harness, so a restart after a preemption skips what is done.
#   kani-crate.sh <phase> <out-dir> [parallel (default 4)] [cap seconds]
#   phase all      every harness, default cap 900
#   phase timeouts every harness whose `all` result is a TIMEOUT or NO RESULT (CBMC killed, usually out of memory), again
#                  with the (larger) cap, default 3600; run it with a smaller P so that big ones do not compete for memory
# Each harness gets its own copy of the spike and its own CARGO_TARGET_DIR (cargo and Kani lock the target directory),
# removed afterwards. A line per harness is also appended to <out-dir>/table-<phase>.txt (unordered; sort it).
# Env: SPIKE (default spikes/kani-nata-kernels of this repository), RUN1 (the script that runs one harness; tests replace it).
set -u
phase=${1:?phase}; out=${2:?out-dir}; par=${3:-4}; cap=${4:-}
here=$(cd "$(dirname "$0")" && pwd); repo=$(cd "$here/../.." && pwd)
spike=${SPIKE:-$repo/spikes/kani-nata-kernels}
run1=${RUN1:-run1.sh}
mkdir -p "$out"
list_all() { awk '/#\[kani::proof\]/{p=1;next} p && /^ *fn /{sub(/^ *fn /,"");sub(/\(.*/,"");print;p=0}' "$spike/src/lib.rs"; }
case "$phase" in
  all) cap=${cap:-900}; todo=$(list_all) ;;
  timeouts) cap=${cap:-3600}; todo=$(grep -l -E 'TIMEOUT after|NO RESULT' "$out"/*.all.txt 2>/dev/null | xargs -r -n1 basename | sed 's/\.all\.txt$//') ;;
  *) echo "unknown phase $phase" >&2; exit 2 ;;
esac
suffix=$phase
export KC_SPIKE=$spike KC_OUT=$out KC_CAP=$cap KC_RUN1=$run1 KC_SUFFIX=$suffix KC_REPO=$repo KC_PHASE=$phase
# (prefixed on purpose: run1.sh keeps Kani's whole output, possibly hundreds of KB, in a shell variable called `out`;
# an exported variable of that name goes into the environment of every child and every exec then fails with
# 'Argument list too long' -- found on the first GCP run, 2026-11-04)
one() {
  h=$1; f="$KC_OUT/$h.$KC_SUFFIX.txt"
  [ -s "$f" ] && return 0                       # done before a restart
  d="${TMPDIR:-/tmp}/kc-$h"; rm -rf "$d"; mkdir -p "$d"
  # copy the spike and the crate sources it includes by #[path] (../../crates/nata-kernels/...), keeping the layout
  mkdir -p "$d/spikes" "$d/crates"; cp -r "$KC_SPIKE" "$d/spikes/kani-nata-kernels"; rm -rf "$d/spikes/kani-nata-kernels/target"
  cp -r "$KC_REPO/crates/nata-kernels" "$d/crates/nata-kernels"; rm -rf "$d/crates/nata-kernels/target"
  [ -f "$KC_REPO/rust-toolchain.toml" ] && cp "$KC_REPO/rust-toolchain.toml" "$d/"
  line=$(cd "$d/spikes/kani-nata-kernels" && CARGO_TARGET_DIR="$d/target" "./$KC_RUN1" "$h" "$KC_CAP" 2>&1 | tail -n1)
  printf '%s\n' "$line" > "$f.tmp" && mv "$f.tmp" "$f"
  printf '%s\n' "$line" >> "$KC_OUT/table-$KC_PHASE.txt"
  rm -rf "$d"
}
export -f one
n=$(printf '%s\n' "$todo" | grep -c .)
echo "phase $phase: $n harnesses, $par at a time, cap ${cap}s, $(date -u +%H:%M:%S)"
printf '%s\n' "$todo" | grep . | xargs -r -P "$par" -I{} bash -c 'one {}'
echo "phase $phase done $(date -u +%H:%M:%S)"
sort "$out/table-$phase.txt" 2>/dev/null | cut -c1-230
