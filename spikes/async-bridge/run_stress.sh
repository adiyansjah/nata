#!/bin/bash
# The 6b stress phases. Output: one summary block per phase. usage: ./run_stress.sh [reps-scale]
cd "$(dirname "$0")"
bin=${STRESS_BIN:-./target/release/examples/stress}
scale=${1:-1}
r() { echo $(( $1 * scale )); }
echo "host: $(nproc) cores; $(date -u +%FT%TZ)"
$bin --reps $(r 1500) --label "A no extra contention, no hooks (machine shared; load in output)" --seed 11
$bin --reps $(r 1500) --contention --label "B 8 busy threads (= cores), no hooks" --seed 12
$bin --reps $(r 600) --hook-us 50 --worker-delay-us 200 --label "C delay hooks 50us + worker delay 200us" --seed 13
$bin --reps $(r 600) --contention --hook-us 50 --worker-delay-us 200 --label "D 8 busy threads + delay hooks" --seed 14
