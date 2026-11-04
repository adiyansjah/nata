#!/bin/bash
# Run every loom test of this crate (targets loom_exchange and loom_channel), one test per process
# (one exploration at a time), print a row per test: name, rc, executions, preemption bound, wall.
# Env: LOOM_MAX_PREEMPTIONS=<n> overrides the bounds; EXL_ONLY / EXL_SKIP = egrep on test names;
# EXL_TIMEOUT = per-test seconds (default 3000).
set -u
cd "$(dirname "$0")"
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2} RUSTFLAGS="--cfg nata_loom" CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-/tmp/nata-exl-target}
cargo test --release --no-run 2>&1 | tail -1
for target in loom_exchange loom_channel; do
  for t in $(cargo test --release --test $target -- --list 2>/dev/null | sed -n 's/: test$//p'); do
    if [ -n "${EXL_ONLY:-}" ] && ! echo "$t" | grep -Eq "$EXL_ONLY"; then continue; fi
    if [ -n "${EXL_SKIP:-}" ] && echo "$t" | grep -Eq "$EXL_SKIP"; then continue; fi
    s=$(date +%s.%N)
    out=$(timeout ${EXL_TIMEOUT:-3000} cargo test --release --test $target "$t" -- --exact --nocapture 2>&1); rc=$?
    e=$(date +%s.%N)
    line=$(echo "$out" | grep -m1 '^\[loom\]' | sed 's/^\[loom\] [^:]*: //')
    printf '%-70s rc=%s  %s  wall=%.1fs\n' "$t" "$rc" "${line:-no result}" "$(awk -v a="$e" -v b="$s" 'BEGIN{print a-b}')"
  done
done
