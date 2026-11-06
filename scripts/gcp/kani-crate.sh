#!/usr/bin/env bash
# Run the Kani harnesses of spikes/kani-nata-kernels on one big machine, P at a time (CBMC is single-threaded; on the
# 64 GB VM a handful fit). One result file per harness, so a restart after a preemption skips what is done.
#   kani-crate.sh <phase> <out-dir> [parallel (default 4)] [cap seconds]
#   phase all      every harness, default cap 900
#   phase solvers  the harnesses in KC_HARNESSES (default: the five 64/32-bit division proofs that timed out with the default
#                  solver on 2026-11-04) once per solver in KC_SOLVERS (default "kissat z3 cvc5"), default cap 2400; one
#                  result file per harness and solver (<h>.solvers-<solver>.txt)
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
  solvers) cap=${cap:-2400}; todo=${KC_HARNESSES:-"primitive_checked_div_i32_is_exact div_exact_matches_the_spec_rows1 div_exact_matches_the_spec_rows2 div_const_left_exact_matches_the_spec_rows1 div_const_left_exact_matches_the_spec_rows2"}; todo=$(printf '%s\n' $todo) ;;
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
  d="${TMPDIR:-/tmp}/kc-$h${KANI_SOLVER:+-$KANI_SOLVER}"; rm -rf "$d"; mkdir -p "$d"
  # copy the spike and the crate sources it includes by #[path] (../../crates/nata-kernels/...), keeping the layout
  mkdir -p "$d/spikes" "$d/crates"; cp -r "$KC_SPIKE" "$d/spikes/kani-nata-kernels"; rm -rf "$d/spikes/kani-nata-kernels/target"
  cp -r "$KC_REPO/crates/nata-kernels" "$d/crates/nata-kernels"; rm -rf "$d/crates/nata-kernels/target"
  [ -f "$KC_REPO/rust-toolchain.toml" ] && cp "$KC_REPO/rust-toolchain.toml" "$d/"
  line=$(cd "$d/spikes/kani-nata-kernels" && CARGO_TARGET_DIR="$d/target" "./$KC_RUN1" "$h" "$KC_CAP" 2>&1 | tail -n1)
  printf '%s\n' "$line" > "$f.tmp" && mv "$f.tmp" "$f"
  printf '%s%s\n' "${KANI_SOLVER:+[$KANI_SOLVER] }" "$line" >> "$KC_OUT/table-$KC_PHASE.txt"
  rm -rf "$d"
}
export -f one
n=$(printf '%s\n' "$todo" | grep -c .)
if [ "$phase" = solvers ]; then
  # every (harness, solver) pair is one job; the solver is passed to run1.sh through KANI_SOLVER
  pairs=$(for sv in ${KC_SOLVERS:-kissat z3 cvc5}; do printf '%s\n' "$todo" | grep . | sed "s/\$/ $sv/"; done)
  echo "phase solvers: $(printf '%s\n' "$pairs" | grep -c .) runs ($n harnesses x solvers ${KC_SOLVERS:-kissat z3 cvc5}), $par at a time, cap ${cap}s, $(date -u +%H:%M:%S)"
  printf '%s\n' "$pairs" | xargs -r -P "$par" -L1 bash -c 'KC_SUFFIX="solvers-$1" KANI_SOLVER="$1" one "$0"'
else
  echo "phase $phase: $n harnesses, $par at a time, cap ${cap}s, $(date -u +%H:%M:%S)"
  printf '%s\n' "$todo" | grep . | xargs -r -P "$par" -I{} bash -c 'one {}'
fi
echo "phase $phase done $(date -u +%H:%M:%S)"
sort "$out/table-$phase.txt" 2>/dev/null | cut -c1-230
