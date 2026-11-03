#!/bin/bash
# The job that runs on the Spot VM. Every unit of work is a resumable step (see lib.sh), so the job can be
# started again after a preemption and carries on where it stopped. State lives on the data disk ($WORK).
set -u
export WORK=${WORK:-/mnt/work}
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
source "$HERE/lib.sh"
export SRC="$WORK/src"
export CARGO_HOME="$WORK/cargo" RUSTUP_HOME="$WORK/rustup" RESULTS
export PATH="$WORK/cargo/bin:$WORK/bin:$PATH"
JOB=$(cat "$WORK/JOBNAME" 2>/dev/null || echo ci)
BOOT=$(boot_id | cut -c1-8)

run() { step "$@" || { fail_job "$1"; exit 1; }; }

run setup-rust bash -c 'command -v rustup >/dev/null || curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
  cd "$SRC" && { rustup show active-toolchain || rustup toolchain install; }'
run setup-duckdb bash -c 'mkdir -p "$WORK/bin" && cd "$WORK/bin" && command -v duckdb >/dev/null && exit 0
  case "$(uname -m)" in x86_64) a=amd64;; aarch64) a=arm64;; *) exit 1;; esac
  curl -sSfL -o duckdb.tar.gz "https://github.com/duckdb/duckdb/releases/download/v2.0.0/duckdb-cli-linux-$a.tar.gz"
  tar -xzf duckdb.tar.gz && rm duckdb.tar.gz && duckdb --version'
# One record per boot: which machine type each step really ran on (a fallback chain may change it).
run "machine-$BOOT" bash -c '{ date -u; uname -a; nproc; free -g; lscpu | grep -E "Model name|Architecture|^CPU\(s\)|L2|L3"
  curl -s -H "Metadata-Flavor: Google" http://metadata.google.internal/computeMetadata/v1/instance/machine-type; echo
  cat /proc/sys/kernel/random/boot_id; } > "$RESULTS/machine-'"$BOOT"'.txt"'

case "$JOB" in
  ci|all)
    run tests-workspace bash -c 'cd "$SRC" && CARGO_TARGET_DIR="$WORK/target-ws" cargo test --workspace'
    for i in 1 2 3 4 5 6; do
      run "protocol-tests-$i" bash -c 'cd "$SRC/spikes/operator-trait" && RUSTFLAGS= CARGO_TARGET_DIR="$WORK/target-op" cargo test --release'
    done ;;&
  bench|all)
    run gen-sf1 bash -c 'cd "$SRC" && scripts/gen-tpch.sh 1 data/tpch-sf1'
    run build-pipeline bash -c 'cd "$SRC/spikes/pipeline-model" && RUSTFLAGS= CARGO_TARGET_DIR="$WORK/target-pm" cargo build --release'
    # Benchmarks are single steps: if one is interrupted it restarts from the beginning, on one boot.
    for i in 1 2 3; do
      run "kernels-$i" bash -c 'cd "$SRC" && "$WORK/target-pm/release/spike-pipeline-model" data/tpch-sf1/lineitem.parquet kernels > "$RESULTS/kernels-'"$i"'.txt"'
    done
    run pullpush bash -c 'cd "$SRC" && "$WORK/target-pm/release/spike-pipeline-model" data/tpch-sf1/lineitem.parquet pullpush 25 > "$RESULTS/pullpush.txt"'
    run v2 bash -c 'cd "$SRC" && "$WORK/target-pm/release/spike-pipeline-model" data/tpch-sf1/lineitem.parquet v2 > "$RESULTS/v2.txt"' ;;&
  genops)
    run gen-sf1 bash -c 'cd "$SRC" && scripts/gen-tpch.sh 1 data/tpch-sf1'
    run build-genops bash -c 'cd "$SRC/spikes/generic-ops" && RUSTFLAGS= CARGO_TARGET_DIR="$WORK/target-go" cargo build --release'
    run genops-unit-tests bash -c 'cd "$SRC/spikes/generic-ops" && RUSTFLAGS= CARGO_TARGET_DIR="$WORK/target-go" cargo test --release > "$RESULTS/genops-unit-tests.txt" 2>&1'
    # Correctness first: a failing check stops the job before any timing is recorded.
    run genops-check bash -c 'cd "$SRC" && "$WORK/target-go/release/spike-generic-ops" check data/tpch-sf1/lineitem.parquet > "$RESULTS/genops-check.txt"'
    run genops-nullcheck bash -c 'cd "$SRC" && "$WORK/target-go/release/spike-generic-ops" nullcheck data/tpch-sf1/lineitem.parquet > "$RESULTS/genops-nullcheck.txt"'
    # Benchmarks are single steps: if one is interrupted it restarts from the beginning, on one boot.
    for m in bench membench nullbench; do
      run "genops-$m" bash -c 'cd "$SRC" && "$WORK/target-go/release/spike-generic-ops" '"$m"' data/tpch-sf1/lineitem.parquet 25 > "$RESULTS/genops-'"$m"'.txt"'
    done
    run genops-duckdb bash -c 'cd "$SRC" && spikes/generic-ops/duckdb-time.sh data/tpch-sf1/lineitem.parquet > "$RESULTS/genops-duckdb.txt"' ;;
  ci|bench|all) ;;
  *) fail_job "unknown job $JOB"; exit 1 ;;
esac
finish_job
