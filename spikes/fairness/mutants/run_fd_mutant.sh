#!/bin/bash
# The open-spill-file mutant (`spill_keep_open` in mutate.py): the tests that must catch it, then a
# paired alternating timing of the fix against the mutant (the cost of open-append-close).
# Run it through scripts/local-heavy.sh. About 2-3 minutes.
set -u
here=$(cd "$(dirname "$0")/.." && pwd)
export CARGO_BUILD_JOBS=2
m=/tmp/fair-mut/spill_keep_open
python3 -I "$here/mutants/mutate.py" spill_keep_open "$here" "$m" > /tmp/fair-mut-spill_keep_open.diff || exit 1
echo "load $(cut -d' ' -f1-3 /proc/loadavg)"
echo "== the new tests against the mutant (each must FAIL)"
cd "$m"
export CARGO_TARGET_DIR=/tmp/fair-mut-fd-target
cargo test --release --offline --test spill_fds -- --nocapture 2>&1 | grep -E "PEAK|test result|panicked at|spill failed"
cargo test --release --offline --test spill_fd_limit 2>&1 | grep -E "^test |test result|spill failed"
(ulimit -n 1024; cargo test --release --offline --test protocol finalize_spills -- --test-threads=1 2>&1 | grep -E "test result|spill failed")
echo "== paired timing, alternating (wall ms, opens, peak open files); default slice"
cargo build --release --offline --example spill_cost 2>&1 | grep -E "^error" ; cp "$CARGO_TARGET_DIR/release/examples/spill_cost" /tmp/spill_cost_keepopen
cd "$here"; unset CARGO_TARGET_DIR
cargo build --release --offline --example spill_cost 2>&1 | grep -E "^error"; cp target/release/examples/spill_cost /tmp/spill_cost_fixed
for i in $(seq 1 15); do
  echo "fixed    $(/tmp/spill_cost_fixed)"
  echo "keepopen $(/tmp/spill_cost_keepopen)"
done
echo DONE
