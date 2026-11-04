#!/bin/bash
# For each mutation: apply it to a private copy of the crate (/tmp/exl-mut/<name>), run every loom
# test there (targets loom_exchange, loom_channel), each in its own process, plus the std leak tests
# (tests/leak.rs, "leak:<name>"), and report which fail (exit status != 0) and why.
# Output per test is kept in /tmp/exl-mut-out/<name>/<test>.txt.
# usage: mutants/run_mutants.sh [mutation ...]      (default: all, then "none" as the control)
# Env: EXL_ONLY / EXL_SKIP (egrep on test names) restrict the tests; EXL_TIMEOUT per test (default 900 s).
set -u
here=$(cd "$(dirname "$0")/.." && pwd)
names=("$@"); [ ${#names[@]} -eq 0 ] && names=($(python3 -I "$here/mutants/mutate.py" --list) none)
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}
LOOMFLAGS="--cfg nata_loom"; LOOMTARGET=${MUT_TARGET:-/tmp/nata-exl-mut-target}; STDTARGET=${MUT_STD_TARGET:-/tmp/nata-exl-mut-std-target}
list() { (cd "$here" && RUSTFLAGS="$LOOMFLAGS" CARGO_TARGET_DIR=$LOOMTARGET cargo test --release --test "$1" -- --list 2>/dev/null | sed -n 's/: test$//p'); }
sel() { while read -r t; do
  if [ -n "${EXL_ONLY:-}" ] && ! echo "$t" | grep -Eq "$EXL_ONLY"; then continue; fi
  if [ -n "${EXL_SKIP:-}" ] && echo "$t" | grep -Eq "$EXL_SKIP"; then continue; fi
  echo "$t"; done; }
ex_tests=$(list loom_exchange | sel); ch_tests=$(list loom_channel | sel)
leak_tests=$( (cd "$here" && RUSTFLAGS= CARGO_TARGET_DIR=$STDTARGET cargo test --release --test leak -- --list 2>/dev/null | sed -n 's/: test$//p') | sel)
for n in "${names[@]}"; do
  d=/tmp/exl-mut/$n; o=/tmp/exl-mut-out/$n; rm -rf "$o"; mkdir -p "$o"
  if [ "$n" = none ]; then
    rm -rf "$d"; mkdir -p "$d"; cp -r "$here/src" "$here/tests" "$here/Cargo.toml" "$here/Cargo.lock" "$d"/; : > "$o/mutation.diff"
  else
    python3 -I "$here/mutants/mutate.py" "$n" "$here" "$d" > "$o/mutation.diff" || { echo "$n: MUTATION NOT APPLIED"; continue; }
  fi
  (cd "$d" && RUSTFLAGS="$LOOMFLAGS" CARGO_TARGET_DIR=$LOOMTARGET cargo test --release --no-run >"$o/build.txt" 2>&1 && RUSTFLAGS= CARGO_TARGET_DIR=$STDTARGET cargo test --release --test leak --no-run >>"$o/build.txt" 2>&1) || { echo "$n: DOES NOT COMPILE (see $o/build.txt)"; continue; }
  caught=(); missed=()
  run() { # kind target test
    local kind=$1 target=$2 t=$3 label=$3 s e rc msg
    s=$(date +%s.%N)
    if [ "$kind" = loom ]; then
      (cd "$d" && RUSTFLAGS="$LOOMFLAGS" CARGO_TARGET_DIR=$LOOMTARGET timeout ${EXL_TIMEOUT:-900} cargo test --release --test "$target" "$t" -- --exact --nocapture >"$o/$t.txt" 2>&1); rc=$?
    else
      label="leak:$t"
      (cd "$d" && RUSTFLAGS= CARGO_TARGET_DIR=$STDTARGET timeout ${EXL_TIMEOUT:-900} cargo test --release --test leak "$t" -- --exact --nocapture >"$o/$label.txt" 2>&1); rc=$?
    fi
    e=$(date +%s.%N)
    if [ $rc -ne 0 ]; then
      msg=$(grep -A1 -m1 'panicked at' "$o/$label.txt" | tail -1 | cut -c1-110); [ $rc -eq 124 ] && msg="TIMEOUT"
      caught+=("$label"); printf '  %-8s %-62s %6.1fs  %s\n' "$n" "$label" "$(awk -v a="$e" -v b="$s" 'BEGIN{print a-b}')" "$msg"
    else missed+=("$label"); fi
  }
  for t in $ex_tests; do run loom loom_exchange "$t"; done
  for t in $ch_tests; do run loom loom_channel "$t"; done
  for t in $leak_tests; do run std leak "$t"; done
  echo "MUTANT $n: caught_by=${#caught[@]} missed=${#missed[@]} [missed: ${missed[*]}]"
done
