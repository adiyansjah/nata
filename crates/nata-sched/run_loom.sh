#!/bin/bash
# Run every loom model of nata-sched, one model per process (one exploration at a time), and print a row per model:
# name, rc, executions, preemption bound, wall.   Env: LOOM_MAX_PREEMPTIONS=<n> overrides the bounds of all models;
# SCHED_ONLY / SCHED_SKIP = egrep on model names; SCHED_TIMEOUT = seconds per model (default 600; a model that does not finish in time FAILS the run);
# SCHED_FAIL_FAST=1 stops at the first model that fails (to show quickly that a known-bad scheduler is caught). Exit status: 0 only if every model passed.
# usage (from the repository root):  scripts/local-heavy.sh crates/nata-sched/run_loom.sh
set -u
cd "$(dirname "$0")/../.."
export RUSTFLAGS="--cfg nata_loom" CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-target/loom} CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}
# --offline by default (the sandbox has no network); the nightly CI job sets SCHED_OFFLINE="" so that cargo may fetch
cargo test ${SCHED_OFFLINE---offline} --release -p nata-sched --test loom_sched --no-run 2>&1 | tail -1
[ "${PIPESTATUS[0]}" = 0 ] || { echo "the loom test binary did not build"; exit 2; }
bin=$(ls -t "$CARGO_TARGET_DIR"/release/deps/loom_sched-* 2>/dev/null | grep -v '\.d$' | head -1)
fail=0
for t in $("$bin" --list 2>/dev/null | sed -n 's/: test$//p'); do
  if [ -n "${SCHED_ONLY:-}" ] && ! echo "$t" | grep -Eq "$SCHED_ONLY"; then continue; fi
  if [ -n "${SCHED_SKIP:-}" ] && echo "$t" | grep -Eq "$SCHED_SKIP"; then continue; fi
  s=$(date +%s.%N)
  out=$(timeout "${SCHED_TIMEOUT:-600}" "$bin" "$t" --exact --nocapture 2>&1); rc=$?
  e=$(date +%s.%N)
  line=$(echo "$out" | grep -m1 '^\[loom\]' | sed 's/^\[loom\] [^:]*: //')
  [ "$rc" -ne 0 ] && fail=1
  printf '%-90s rc=%s  %s  total=%.1fs\n' "$t" "$rc" "${line:-no result}" "$(awk -v a="$e" -v b="$s" 'BEGIN{print a-b}')"
  if [ "$rc" -ne 0 ]; then
    echo "$out" | grep -E "panicked|assert|deadlock|Deadlock|FAILED" | head -3
    [ -n "${SCHED_FAIL_FAST:-}" ] && break
  fi
done
exit $fail
