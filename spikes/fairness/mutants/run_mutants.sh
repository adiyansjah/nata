#!/bin/bash
# For each mutation of the NEW fairness code: apply it to a scratch copy and run
#  (a) every new loom model (y_*), each in its own process (exit status != 0 = caught: deadlock,
#      "invariant violated", assertion, or "exceeded maximum number of branches" for a livelock), and
#  (b) the plain tests tests/fairness.rs and tests/starvation.rs (timeouts are failures).
# Prints one line per mutant. usage: mutants/run_mutants.sh [mutation ...]   (about 25 min for all, when none is caught early)
set -u
here=$(cd "$(dirname "$0")/.." && pwd)
loom_tests=(y_w1_two_tasks y_w2_two_tasks y_w1_cancel_races_with_yield y_w2_cancel_races_with_yield y_w1_consumer_with_yielding_neighbour y_w1_consumer_cancel y_w2_consumer_with_yielding_neighbour)
names=("$@"); [ ${#names[@]} -eq 0 ] && names=(none yield_no_requeue yield_parks yield_as_blocked yield_enqueue_before_state yield_budget_not_reset yield_front_of_queue yield_check_inverted yield_never)
export CARGO_BUILD_JOBS=2
for n in "${names[@]}"; do
  d=/tmp/fair-mut/$n
  if [ "$n" = none ]; then rm -rf "$d"; mkdir -p "$d"; cp -r "$here/src" "$here/tests" "$here/examples" "$here/Cargo.toml" "$here/Cargo.lock" "$d"/
  else python3 -I "$here/mutants/mutate.py" "$n" "$here" "$d" > /tmp/fair-mut-$n.diff || { echo "$n: MUTATION NOT APPLIED"; continue; }; fi
  caught=(); missed=(); why=""
  for t in "${loom_tests[@]}"; do
    out=$(cd "$d" && RUSTFLAGS="--cfg nata_loom" CARGO_TARGET_DIR=${MUT_LOOM_TARGET:-/tmp/fair-mut-loom-target} timeout 900 cargo test --release --offline --test loom_bridge -- --exact "$t" 2>&1); rc=$?
    if [ $rc -ne 0 ]; then
      caught+=("loom:$t"); r=$(echo "$out" | grep -m1 -oE 'deadlock|invariant violated: [a-zA-Z ]*|lost, duplicated or reordered|is not a prefix|exceeded maximum number of branches|assertion[^\n]{0,40}')
      why="$why|${r:-rc=$rc}"
    else missed+=("loom:$t"); fi
  done
  for tf in fairness starvation; do
    out=$(cd "$d" && RUSTFLAGS= CARGO_TARGET_DIR=${MUT_PLAIN_TARGET:-/tmp/fair-mut-plain-target} timeout 60 cargo test --release --offline --test $tf -- --test-threads=1 2>&1); rc=$?
    failed=$(echo "$out" | grep -E '^test .* FAILED' | sed -E 's/^test (\S+) .*/\1/' | tr '\n' ',')
    if [ $rc -ne 0 ]; then caught+=("plain:$tf[${failed:-rc=$rc}]"); else missed+=("plain:$tf"); fi
  done
  echo "MUTANT $n: caught_by=${#caught[@]} [${caught[*]}] missed=[${missed[*]}] loom_reasons=[$(echo "$why" | tr '|' '\n' | sort | uniq -c | sort -rn | tr '\n' ';')]"
done
