#!/bin/bash
# Run the mutation checks of several spikes IN PARALLEL, one mutant per work item, on one big machine.
#
# Each spike already has a script that applies ONE mutant to a private copy and runs its checks (see the adapters
# below). This driver lists the mutants of every spike, puts all (spike, mutant) items in one queue, and starts N
# workers. A worker has its own directory per spike (a copy of the spike without its target dir, plus its own cargo
# target dir), so mutants never touch the real sources or each other, and a worker re-uses its warm target dir for the
# next mutant of the same spike.
#
# usage: par-mutants.sh [--dry] [--workers N] [--only spike,spike] [--match REGEX] [--out DIR]
#        --match keeps only the queue lines (spike<TAB>mutant) that match the regular expression (for smoke tests)
# env:   REPO (default: this repository), WORKERS (default 12), ITEM_TIMEOUT seconds (default 3000),
#        DATA (default $REPO/data/tpch-sf1), SPIKES (default: exchange-loom fairness arrow-pipeline hash-ops)
# exit:  0 if every mutant is CAUGHT and every control passes; 1 otherwise (survivors, invalid mutants, errors).
#
# Verdicts: CAUGHT, SURVIVED (a hole in the tests), INVALID (the mutant did not apply or compile: not evidence),
#           ERROR (the script failed or timed out), CONTROL_OK / CONTROL_FAIL (the unmutated copy must pass).
set -u
REPO=${REPO:-$(cd "$(dirname "$0")/../.." && pwd)}
WORKERS=${WORKERS:-12}
ITEM_TIMEOUT=${ITEM_TIMEOUT:-3000}
DATA=${DATA:-$REPO/data/tpch-sf1}
SPIKES=${SPIKES:-"exchange-loom fairness arrow-pipeline hash-ops"}
OUT=${OUT:-$REPO/target/par-mutants}
DRY=0
MATCH=
REVAL=
while [ $# -gt 0 ]; do
  case "$1" in
    --dry) DRY=1; shift ;;
    --workers) WORKERS=$2; shift 2 ;;
    --only) SPIKES=${2//,/ }; shift 2 ;;
    --match) MATCH=$2; shift 2 ;;
    --out) OUT=$2; shift 2 ;;
    --revalidate) REVAL=$2; shift 2 ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
done

# A log file name for a mutant: only letters, digits and . _ ( ) - are kept, so names with '/' or other characters cannot
# point into a directory that does not exist (the first parallel run lost the log of 'dense/masked path' that way).
logname() { local n=$1; printf '%s' "${n//[^A-Za-z0-9._()-]/_}"; }

# ---------------------------------------------------------------------------------------------- adapters
# list_<spike>          prints the mutant names, one per line (the unmutated control is added by the driver)
# run_<spike> DIR SLOT NAME   runs one mutant (or 'none') inside DIR (a copy of the spike), SLOT is a scratch dir
#                       unique to the worker; prints the script's raw output
# verdict_<spike> NAME  reads the raw output on stdin and prints CAUGHT / SURVIVED / INVALID / CONTROL_OK / CONTROL_FAIL

# Spikes whose script prints 'MUTANT <name>: caught_by=K ...' (K = number of checks that failed).
verdict_mutantline() {
  local name=$1 out line k
  out=$(cat)
  line=$(printf '%s\n' "$out" | grep -m1 "^MUTANT $name:")
  if printf '%s\n' "$out" | grep -qE "MUTATION NOT APPLIED|DOES NOT COMPILE"; then
    [ "$name" = none ] && echo CONTROL_FAIL || echo INVALID; return
  fi
  [ -n "$line" ] || { [ "$name" = none ] && echo CONTROL_FAIL || echo INVALID; return; }
  k=$(printf '%s' "$line" | sed -n 's/.*caught_by=\([0-9]*\).*/\1/p')
  [ -n "$k" ] || { echo INVALID; return; }
  if [ "$name" = none ]; then [ "$k" -eq 0 ] && echo CONTROL_OK || echo CONTROL_FAIL
  else [ "$k" -ge 1 ] && echo CAUGHT || echo SURVIVED; fi
}
# Spikes whose script prints a line starting with CAUGHT / SURVIVED / INVALID for the mutant.
# These scripts print 'baseline passes ...' at the start of EVERY mutant run (the unmutated private copy must pass its
# checks first), so each mutant carries its own control: a run without that line is INVALID, never a catch.
verdict_statusline() {
  local name=$1 out first
  out=$(cat)
  # The verdict word may follow a progress prefix such as '[ 5/33] ' (arrow-pipeline) or start the line (hash-ops).
  first=$(printf '%s\n' "$out" | grep -E '^(\[ *[0-9]+/[0-9]+\] +)?(CAUGHT|SURVIVED|INVALID)' | head -1 | sed -E 's/^(\[ *[0-9]+\/[0-9]+\] +)?//' | awk '{print $1}')
  if [ "$name" = none ]; then
    printf '%s\n' "$out" | grep -qE '^baseline passes' && echo CONTROL_OK || echo CONTROL_FAIL; return
  fi
  printf '%s\n' "$out" | grep -qE '^baseline passes' || { echo INVALID; return; }
  [ -n "$first" ] && echo "$first" || echo INVALID
}
# Spikes whose script accepts the name 'none' as the unmutated control (the others carry the control in every run).
CONTROL_SPIKES="exchange-loom fairness fake"

list_exchange-loom() { (cd "$1" && python3 -I mutants/mutate.py --list); }
run_exchange-loom() { # DIR SLOT NAME
  (cd "$1" && MUT_TARGET="$2/target" CARGO_BUILD_JOBS=2 mutants/run_mutants.sh "$3"); }
verdict_exchange-loom() { verdict_mutantline "$1"; }

list_fairness() { # the default list inside run_mutants.sh (names=(none a b c)), without the control
  sed -n 's/^names=("\$@"); \[ \${#names\[@\]} -eq 0 \] && names=(\(.*\))$/\1/p' "$1/mutants/run_mutants.sh" | tr ' ' '\n' | grep -vx none | grep -v '^$'; }
run_fairness() {
  (cd "$1" && MUT_LOOM_TARGET="$2/loom" MUT_PLAIN_TARGET="$2/plain" CARGO_BUILD_JOBS=2 mutants/run_mutants.sh "$3"); }
verdict_fairness() { verdict_mutantline "$1"; }

list_arrow-pipeline() { (cd "$1" && python3 -I scripts/mutate.py --list); }
run_arrow-pipeline() {
  (cd "$1" && MUT_ROOT="$2/ap" LINEITEM="$DATA/lineitem.parquet" CARGO_BUILD_JOBS=2 python3 -I scripts/mutate.py "$3"); }
verdict_arrow-pipeline() { verdict_statusline "$1"; }

list_hash-ops() { (cd "$1" && python3 -I mutate.py --list); }
run_hash-ops() {
  (cd "$1" && HASH_OPS_MUT_ROOT="$2/ho" HASH_OPS_DATA="$DATA" CARGO_BUILD_JOBS=2 python3 -I mutate.py "$3"); }
verdict_hash-ops() { verdict_statusline "$1"; }

# A fake spike used only by the driver's own tests (PARMUT_FAKE=dir).
list_fake() { "$1/list.sh"; }
run_fake() { "$1/run.sh" "$3"; }
verdict_fake() { verdict_statusline "$1"; }

# The workers run in copies OUTSIDE the repository, where rustup does not see rust-toolchain.toml, and a fresh VM has no
# default toolchain ('rustup could not choose a version of cargo'): name the repository's channel explicitly.
if [ -z "${RUSTUP_TOOLCHAIN:-}" ] && [ -f "$REPO/rust-toolchain.toml" ]; then
  RUSTUP_TOOLCHAIN=$(sed -n 's/^channel *= *"\(.*\)".*/\1/p' "$REPO/rust-toolchain.toml" | head -1)
  [ -n "$RUSTUP_TOOLCHAIN" ] && export RUSTUP_TOOLCHAIN
fi
export DATA
for _f in $(compgen -A function | grep '^run_'); do export -f "$_f"; done

# ---------------------------------------------------------------------------------------------- revalidate
# --revalidate DIR: recompute every verdict of a finished run from its saved logs with the CURRENT parsers (for example
# after fixing a parser bug), without running anything. Writes DIR/results.revalidated.tsv and prints the summary.
if [ -n "$REVAL" ]; then
  : > "$REVAL/results.revalidated.tsv"
  while IFS=$'\t' read -r spike name old secs; do
    f="$REVAL/logs/$spike/$(logname "$name").log"; [ -f "$f" ] || f="$REVAL/logs/$spike/${name// /_}.log"   # older runs only replaced spaces
    if [ -f "$f" ] && declare -F "verdict_$spike" > /dev/null; then v=$("verdict_$spike" "$name" < "$f"); else v=$old; fi
    printf '%s\t%s\t%s\t%s\n' "$spike" "$name" "$v" "$secs" >> "$REVAL/results.revalidated.tsv"
  done < "$REVAL/results.tsv"
  RES=$REVAL/results.revalidated.tsv; total=$(wc -l < "$RES"); problems=0
  SUMMARY_ONLY=1
fi

# ---------------------------------------------------------------------------------------------- queue
if [ -z "${SUMMARY_ONLY:-}" ]; then
mkdir -p "$OUT"
Q=$OUT/queue.txt; : > "$Q"
RES=$OUT/results.tsv; : > "$RES"
problems=0
for s in $SPIKES; do
  dir=$REPO/spikes/$s; [ "$s" = fake ] && dir=${PARMUT_FAKE:?}
  if [ ! -d "$dir" ]; then echo "spike $s: directory $dir not found" >&2; problems=1; continue; fi
  if ! names=$("list_$s" "$dir" 2>"$OUT/list-$s.err") || [ -z "$names" ]; then
    echo "spike $s: cannot list its mutants (see $OUT/list-$s.err); it needs a --list flag" >&2
    printf '%s\t%s\t%s\t%s\n' "$s" "(list)" ERROR 0 >> "$RES"; problems=1; continue
  fi
  n=0
  while IFS= read -r m; do printf '%s\t%s\n' "$s" "$m" >> "$Q"; n=$((n + 1)); done <<< "$names"
  case " $CONTROL_SPIKES " in
    *" $s "*) printf '%s\t%s\n' "$s" none >> "$Q"; echo "spike $s: $n mutants + the unmutated control" ;;
    *) echo "spike $s: $n mutants (each run checks its own baseline)" ;;
  esac
done
if [ -n "$MATCH" ]; then grep -E "$MATCH" "$Q" > "$Q.f"; mv "$Q.f" "$Q"; fi
total=$(wc -l < "$Q")
echo "queue: $total items, $WORKERS workers, output in $OUT"
if [ "$DRY" = 1 ]; then cat "$Q" | cut -c1-110; [ "$problems" = 0 ]; exit $?; fi

# ---------------------------------------------------------------------------------------------- workers
worker() { # worker ID
  local id=$1
  local slot=$OUT/slot$id item spike name dir sl t0 raw v
  mkdir -p "$slot"
  while :; do
    item=$(flock "$Q.lock" bash -c 'l=$(head -n1 "$1"); if [ -n "$l" ]; then sed -i 1d "$1"; fi; printf "%s" "$l"' _ "$Q")
    [ -n "$item" ] || break
    spike=${item%%$'\t'*}; name=${item#*$'\t'}
    if [ "$spike" = fake ]; then dir=$PARMUT_FAKE
    else
      dir=$slot/$spike
      [ -d "$dir" ] || { mkdir -p "$dir"; tar --exclude=target --exclude=.venv --exclude=__pycache__ -cf - -C "$REPO/spikes/$spike" . | tar -xf - -C "$dir"; }
    fi
    sl=$slot/scratch-$spike; mkdir -p "$sl"
    t0=$SECONDS
    # `timeout` cannot run a shell function: run it in a child bash that inherits the exported adapter functions.
    raw=$(timeout "$ITEM_TIMEOUT" bash -c '"$@"' _ "run_$spike" "$dir" "$sl" "$name" 2>&1); rc=$?
    mkdir -p "$OUT/logs/$spike"; printf '%s\n' "$raw" > "$OUT/logs/$spike/$(logname "$name").log"
    if [ "$rc" = 124 ]; then v=ERROR
    else v=$(printf '%s\n' "$raw" | "verdict_$spike" "$name"); fi
    flock "$RES.lock" bash -c 'printf "%s\t%s\t%s\t%s\n" "$1" "$2" "$3" "$4" >> "$5"' _ "$spike" "$name" "$v" "$((SECONDS - t0))" "$RES"
    echo "[$(date -u +%H:%M:%S)] worker $id: $spike / $name -> $v ($((SECONDS - t0)) s)"
  done
}
for i in $(seq 1 "$WORKERS"); do worker "$i" & done
wait

fi   # end of the queue and workers (skipped by --revalidate)

# ---------------------------------------------------------------------------------------------- summary
echo
echo "=== summary ($(wc -l < "$RES") items)"
awk -F'\t' '{c[$1" "$3]++} END {for (k in c) print c[k], k}' "$RES" | sort -k2,2 -k3,3
# A spike whose unmutated control did not pass proves nothing about its mutants (a broken environment makes every
# check fail, which looks like a catch): its CAUGHT verdicts become UNTRUSTED.
for s in $(awk -F'\t' '$2=="none" && $3!="CONTROL_OK" {print $1}' "$RES" | sort -u); do
  echo "spike $s: the unmutated control did NOT pass, so its CAUGHT verdicts are UNTRUSTED"
  awk -F'\t' -v s="$s" 'BEGIN{OFS="\t"} $1==s && $3=="CAUGHT" {$3="UNTRUSTED"} {print}' "$RES" > "$RES.tmp" && mv "$RES.tmp" "$RES"
done
bad=$(awk -F'\t' '$3!="CAUGHT" && $3!="CONTROL_OK"' "$RES")
done_n=$(wc -l < "$RES")
# Every queued item must have produced a verdict: an empty or partial run is never a pass.
if [ "$done_n" -lt "$total" ]; then bad="$bad
(incomplete: $done_n of $total items have a verdict)"; problems=1; fi
if [ -n "$bad" ] || [ "$problems" != 0 ]; then
  echo "--- not CAUGHT / not control-OK:"; printf '%s\n' "$bad"
  exit 1
fi
echo "ALL CAUGHT, all controls pass"
