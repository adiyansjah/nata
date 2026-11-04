#!/bin/bash
# For each mutation: apply it to a scratch copy, run EVERY loom test in its own process there,
# and record which tests catch it (exit status != 0: deadlock, invariant or assertion failure,
# or an abort after one of those). usage: mutants/loom_mutants.sh [mutation ...]
set -u
here=$(cd "$(dirname "$0")/.." && pwd)
tests=(w1_two_producers_cap1 w1_two_producers_cancel w1_cap1_two_items w1_cap2_one_item w1_close_without_items w1_cancel_races_with_producer w2_cap1_one_item w2_close_without_items w2_cancel_races_with_producer)
names=("$@"); [ ${#names[@]} -eq 0 ] && names=(chan_no_recheck chan_close_no_wake chan_send_no_wake chan_close_rx_no_wake sched_ignore_notified sched_wake_queued_enqueues sched_shutdown_without_lock none)
export RUSTFLAGS="--cfg nata_loom" CARGO_TARGET_DIR=${MUT_TARGET:-/tmp/ab-mut-target}
for n in "${names[@]}"; do
  d=/tmp/ab-mut/$n
  if [ "$n" = none ]; then rm -rf "$d"; mkdir -p "$d"; cp -r "$here/src" "$here/tests" "$here/Cargo.toml" "$here/Cargo.lock" "$d"/; : > /tmp/ab-mut-none.diff
  else python3 -I "$here/mutants/mutate.py" "$n" "$here" "$d" > /tmp/ab-mut-$n.diff || { echo "$n: MUTATION NOT APPLIED"; continue; }; fi
  caught=(); missed=(); why=""
  for t in "${tests[@]}"; do
    out=$(cd "$d" && timeout 900 cargo test --release --test loom_bridge -- --exact "$t" 2>&1); rc=$?
    if [ $rc -ne 0 ]; then
      caught+=("$t"); r=$(echo "$out" | grep -m1 -oE 'deadlock|invariant violated: [a-z ]*|lost or reordered items|is not a prefix|assertion[^\n]{0,40}')
      why="$why|${r:-rc=$rc}"
    else missed+=("$t"); fi
  done
  echo "MUTANT $n: caught_by=${#caught[@]}/${#tests[@]} [${caught[*]}] missed=[${missed[*]}] reasons=[$(echo "$why" | tr '|' '\n' | sort | uniq -c | sort -rn | tr '\n' ';')]"
done
