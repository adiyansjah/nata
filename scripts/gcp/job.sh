#!/bin/bash
# The job that runs on the Spot VM. Every unit of work is a resumable step (see lib.sh), so the job can be
# started again after a preemption and carries on where it stopped. State lives on the data disk ($WORK).
set -u
unset AI_AGENT
export WORK=${WORK:-/mnt/work}
export TMPDIR="$WORK/tmp"
mkdir -p "$TMPDIR"
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=lib.sh
source "$HERE/lib.sh"
export SRC="$WORK/src"
export CARGO_HOME="$WORK/cargo" RUSTUP_HOME="$WORK/rustup" RESULTS
export PATH="$WORK/cargo/bin:$WORK/bin:$PATH"
JOB=$(cat "$WORK/JOBNAME" 2>/dev/null || echo ci)
BOOT=$(boot_id | cut -c1-8)

run() { step "$@" || { fail_job "$1"; exit 1; }; }
# Timing needs a QUIET machine: a step started right after a heavy one sees a 1-minute load average of 2-3 (found 2026-11-05: every timed
# step of the first timing bundle started at load 2.7-3.3 and the scripts label such numbers SANITY ONLY). settle_before NAME waits (at most
# ten minutes) until the 1-minute load is below 0.5; it is a no-op in the other jobs, whose steps are not timing.
# scripts/gcp/settle.sh prints one SETTLE line (settled, or NOT SETTLED with the load); the job carries on either way so that its other
# results are kept, and the line is the record that the block after a NOT SETTLED is SANITY ONLY.
settle_before() { case "$JOB" in timing|perf|nullpath|fairness-bench|bridge|cpack|verify) run "settle-$1" bash -c 'bash "$SRC/scripts/gcp/settle.sh" "$0" 0.5 600; true' "$1" ;; esac; }

run setup-rust bash -c 'command -v rustup >/dev/null || curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
  cd "$SRC" && { rustup show active-toolchain || rustup toolchain install; }'
run setup-duckdb bash -c 'mkdir -p "$WORK/bin" && cd "$WORK/bin" && command -v duckdb >/dev/null && exit 0
  case "$(uname -m)" in x86_64) a=amd64;; aarch64) a=arm64;; *) exit 1;; esac
  curl -sSfL --retry 4 --retry-delay 5 -o duckdb.tar.gz "https://github.com/duckdb/duckdb/releases/download/v2.0.0/duckdb-cli-linux-$a.tar.gz"
  tar -xzf duckdb.tar.gz && rm duckdb.tar.gz && duckdb --version'
# One record per boot: which machine type each step really ran on (a fallback chain may change it).
run "machine-$BOOT" bash -c '{ date -u; uname -a; nproc; free -g; lscpu | grep -E "Model name|Architecture|^CPU\(s\)|L2|L3"
  curl -s -H "Metadata-Flavor: Google" http://metadata.google.internal/computeMetadata/v1/instance/machine-type | sed "s|.*/||"; echo
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
  verify)
    # A clean-machine reproduction of the verification (docs/decisions/verification-plan.md). Everything here was
    # first run by hand; this job only repeats it from a clean tarball on a quiet machine. Each step stops the job
    # on failure, so a red step never yields a table of timings.
    run v-workspace-fmt bash -c 'cd "$SRC" && cargo fmt --check'
    run v-workspace-clippy bash -c 'cd "$SRC" && CARGO_TARGET_DIR="$WORK/target-ws" cargo clippy --workspace --all-targets -- -D warnings'
    run v-workspace-tests bash -c 'cd "$SRC" && CARGO_TARGET_DIR="$WORK/target-ws" cargo test --workspace'
    run v-lint-fixtures bash -c 'cd "$SRC" && CARGO_TARGET_DIR="$WORK/target-lint" python3 -I crates/nata-kernels/lint-fixtures/run.py > "$RESULTS/lint-fixtures.txt"'
    run v-guard-diff bash -c 'cd "$SRC" && python3 -I crates/nata-kernels/scripts/guard_diff.py > "$RESULTS/guard-diff.txt"'
    run v-guard-mutations bash -c 'cd "$SRC" && CARGO_TARGET_DIR="$WORK/target-ws" python3 -I crates/nata-kernels/scripts/mutate_guards.py > "$RESULTS/guard-mutations.txt"'
    # Scheduler: default build and tests, then the loom model, then each deliberate bug must be CAUGHT by loom.
    run v-sched-tests bash -c 'cd "$SRC/spikes/operator-trait" && RUSTFLAGS= CARGO_TARGET_DIR="$WORK/target-op" cargo test --release'
    run v-loom-fixed bash -c 'cd "$SRC/spikes/operator-trait" && RUSTFLAGS="--cfg loom" CARGO_TARGET_DIR="$WORK/target-loom" cargo test --release --test loom_sched -- --nocapture > "$RESULTS/loom-fixed.txt" 2>&1'
    for f in bug-lost-shutdown bug-wake-ignored-while-running bug-park-ignores-notified bug-wake-enqueues-running bug-cancel-does-not-wake; do
      run "v-loom-mutant-$f" bash -c 'cd "$SRC/spikes/operator-trait" && RUSTFLAGS="--cfg loom" CARGO_TARGET_DIR="$WORK/target-loom" "$SRC/scripts/gcp/expect-fail.sh" "deadlock|panicked|invariant" cargo test --release --features '"$f"' --test loom_sched > "$RESULTS/loom-mutant-'"$f"'.txt" 2>&1'
    done
    # ADR 0003 item 21: what does overflow-checks cost the generic-operator experiment? Build both ways, check the
    # results are unchanged with the checks on, then alternate the two binaries so drift shows up as drift.
    run v-gen-gen-tpch bash -c 'cd "$SRC" && scripts/gen-tpch.sh 1 data/tpch-sf1'
    for ov in false true; do
      run "v-genops-build-ov-$ov" bash -c 'cd "$SRC/spikes/generic-ops" && CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS='"$ov"' RUSTFLAGS= CARGO_TARGET_DIR="$WORK/target-go-'"$ov"'" cargo build --release'
    done
    run v-genops-check-ov-true bash -c 'cd "$SRC" && B="$WORK/target-go-true/release/spike-generic-ops" && "$B" check data/tpch-sf1/lineitem.parquet > "$RESULTS/genops-check-ov-true.txt" && "$B" nullcheck data/tpch-sf1/lineitem.parquet >> "$RESULTS/genops-check-ov-true.txt"'
    for i in 1 2; do for ov in false true; do for m in membench bench; do
      settle_before "v-genops-$m-ov-$ov-$i"
      run "v-genops-$m-ov-$ov-$i" bash -c 'cd "$SRC" && { echo "load: $(cat /proc/loadavg)"; "$WORK/target-go-'"$ov"'/release/spike-generic-ops" '"$m"' data/tpch-sf1/lineitem.parquet 25; } > "$RESULTS/genops-'"$m"'-ov-'"$ov"'-'"$i"'.txt"'
    done; done; done ;;
  hashops|timing)
    # The hash join and general hash aggregate spike (ADR 0003 item 15), from a clean tarball on a quiet machine.
    # Each part of spikes/hash-ops/run-all.sh is its own resumable step. Correctness first: a red step stops the job
    # before any timing is recorded.
    export CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true   # correctness runs use the product configuration
    run gen-sf1 bash -c 'cd "$SRC" && scripts/gen-tpch.sh 1 data/tpch-sf1'
    run h-build bash -c 'cd "$SRC/spikes/hash-ops" && RUSTFLAGS= cargo build --release && RUSTFLAGS= cargo test --release --no-run'
    run h-tests bash -c 'cd "$SRC/spikes/hash-ops" && RUSTFLAGS= cargo test --release > "$RESULTS/hashops-tests.txt" 2>&1'
    run h-check bash -c 'cd "$SRC/spikes/hash-ops" && ./check.sh "$SRC/data/tpch-sf1" > "$RESULTS/hashops-check.txt" 2>&1'
    run h-nullcheck bash -c 'cd "$SRC/spikes/hash-ops" && target/release/hash-ops nullcheck "$SRC/data/tpch-sf1" > "$RESULTS/hashops-nullcheck.txt" 2>&1'
    run h-rss bash -c 'cd "$SRC/spikes/hash-ops" && for spec in "agg_orderkey engine" "agg_orderkey fused" "j3 engine" "j3 fused"; do set -- $spec; /usr/bin/time -v target/release/hash-ops rss "$SRC/data/tpch-sf1" "$1" "$2" 4 2>&1 | grep -E "groups|Maximum resident|Elapsed"; done > "$RESULTS/hashops-rss.txt"'
    # Benchmarks with overflow-checks ON (the product configuration, ADR 0003 decision 6) and OFF (to see its cost).
    run h-build-ov-off bash -c 'cd "$SRC/spikes/hash-ops" && CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=false RUSTFLAGS= CARGO_TARGET_DIR="$WORK/target-ho-off" cargo build --release'
    for m in membench bench; do for ov in on off; do
      B='target/release/hash-ops'; [ "$ov" = off ] && B="$WORK/target-ho-off/release/hash-ops"
      settle_before "h-$m-ov-$ov"
      run "h-$m-ov-$ov" bash -c 'cd "$SRC/spikes/hash-ops" && { echo "load: $(cat /proc/loadavg)"; '"$B"' '"$m"' "$SRC/data/tpch-sf1" 25; } > "$RESULTS/hashops-'"$m"'-ov-'"$ov"'.txt" 2>&1'
    done; done
    settle_before h-duckdb
    run h-duckdb bash -c 'cd "$SRC/spikes/hash-ops" && { echo "load: $(cat /proc/loadavg)"; ./duckdb-time.sh "$SRC/data/tpch-sf1" 16; } > "$RESULTS/hashops-duckdb.txt" 2>&1'
    # Last, and slowest (about 45 minutes): the mutation check of the TESTS. It validates the test suite, not the
    # engine, so the correctness steps above (tests, DuckDB diff, nullcheck) gate the timings and this one does not.
    # The step still fails the job when a mutant survives.
    # (the `timing` bundle skips it: 30 of 30 mutants were caught on ARM64 in the parmut-hashops run)
    [ "$JOB" = hashops ] && run h-mutate bash -c 'cd "$SRC/spikes/hash-ops" && HASH_OPS_DATA="$SRC/data/tpch-sf1" python3 mutate.py > "$RESULTS/hashops-mutate.txt" 2>&1'
    unset CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS   # it must not leak into the builds of the blocks that follow in the timing bundle
    : ;;&
  decimal)
    # The exhaustive decimal verification (ADR 0003, docs/decimal-rules.md) in its FULL profile, with every mutant.
    # Python 3 and DuckDB only; the DuckDB answers are cached on the data disk, so a restart after a preemption
    # does not ask DuckDB again.
    run d-verify-full bash -c 'cd "$SRC/spikes/decimal-verify" && python3 -I verify.py --full --mutants all --cache-dir "$WORK/dv-cache" --json "$RESULTS/decimal-full.json" > "$RESULTS/decimal-full.txt" 2>&1' ;;
  kani)
    # Kani proofs of the generic-ops kernels (ADR 0003 item 11). Locally CBMC ran out of memory; this needs a
    # high-memory machine. One harness at a time, each with a time cap; the table is the result, so a harness that
    # fails or times out does not stop the job.
    # The VM has no default toolchain, and this step does not run inside the repository (where rust-toolchain.toml would
    # choose one), so name it: the same cause as the lint-fixture failure of the first clean-machine runs.
    run k-install bash -c 'export RUSTUP_TOOLCHAIN=$(sed -n "s/^channel *= *\"\\(.*\\)\"/\\1/p" "$SRC/rust-toolchain.toml"); echo "toolchain: $RUSTUP_TOOLCHAIN"; command -v cargo-kani >/dev/null || { cargo install --locked kani-verifier && cargo kani setup; }; cargo kani --version'
    run k-harnesses bash -c 'cd "$SRC/spikes/generic-ops/kani" && { free -g | head -2; ./run.sh 900; true; } > "$RESULTS/kani-harnesses.txt" 2>&1' ;;
  kani-mut)
    # The mutation check of the Kani proofs: each of the 15 deliberate breakages of spikes/generic-ops/mutate.py is
    # applied to a temp copy of kernels.rs and the relevant harnesses are run, cheapest first, until one fails
    # verification. A breakage that no harness catches is a hole in the proofs. Same setup as the kani job.
    run k-install bash -c 'export RUSTUP_TOOLCHAIN=$(sed -n "s/^channel *= *\"\\(.*\\)\"/\\1/p" "$SRC/rust-toolchain.toml"); echo "toolchain: $RUSTUP_TOOLCHAIN"; command -v cargo-kani >/dev/null || { cargo install --locked kani-verifier && cargo kani setup; }; cargo kani --version'
    run k-mutations bash -c 'cd "$SRC/spikes/generic-ops/kani" && { free -g | head -2; python3 -I mutate_kani.py; true; } > "$RESULTS/kani-mutations.txt" 2>&1' ;;
  parmut)
    # The mutation checks of several spikes, sharded over many workers on one big machine (scripts/gcp/par-mutants.sh).
    # Every (spike, mutant) item runs on a private copy; a verdict other than CAUGHT fails the step and the job.
    run gen-sf1 bash -c 'cd "$SRC" && scripts/gen-tpch.sh 1 data/tpch-sf1'
    run pm-dry bash -c 'cd "$SRC" && DATA="$SRC/data/tpch-sf1" scripts/gcp/par-mutants.sh --dry --out "$RESULTS/parmut-dry" > "$RESULTS/parmut-dry.txt" 2>&1'
    run pm-run bash -c 'cd "$SRC" && DATA="$SRC/data/tpch-sf1" WORKERS="${PARMUT_WORKERS:-12}" scripts/gcp/par-mutants.sh --out "$WORK/parmut" > "$RESULTS/parmut-summary.txt" 2>&1; rc=$?; cp -r "$WORK/parmut/results.tsv" "$WORK/parmut/logs" "$RESULTS/" 2>/dev/null; exit $rc' ;;
  parmut-hashops)
    # Only the hash-ops mutants (30), after a fix to the unmutated baseline. Same driver as `parmut`.
    run gen-sf1 bash -c 'cd "$SRC" && scripts/gen-tpch.sh 1 data/tpch-sf1'
    run pm-run bash -c 'cd "$SRC" && DATA="$SRC/data/tpch-sf1" WORKERS="${PARMUT_WORKERS:-12}" scripts/gcp/par-mutants.sh --only hash-ops --out "$WORK/parmut" > "$RESULTS/parmut-summary.txt" 2>&1; rc=$?; cp -r "$WORK/parmut/results.tsv" "$WORK/parmut/logs" "$RESULTS/" 2>/dev/null; exit $rc' ;;
  crates|cpack)
    # ADR 0002 open check (3): object_store, crossbeam-deque and the compression crates, built and TESTED on this
    # architecture (not only compiled): codec round trips on 64 MiB, work-stealing exactly-once stress, concurrent range reads.
    run c-build bash -c 'cd "$SRC/spikes/crates-check" && RUSTFLAGS= cargo test --release --no-run'
    run c-codecs bash -c 'cd "$SRC/spikes/crates-check" && { uname -m; echo "load: $(cat /proc/loadavg)"; RUSTFLAGS= cargo test --release --test codecs -- --test-threads=1 --nocapture; } > "$RESULTS/crates-codecs.txt" 2>&1'
    run c-deque bash -c 'cd "$SRC/spikes/crates-check" && { echo "cores: $(nproc)"; RUSTFLAGS= cargo test --release --test deque -- --nocapture; } > "$RESULTS/crates-deque.txt" 2>&1'
    run c-store bash -c 'cd "$SRC/spikes/crates-check" && RUSTFLAGS= cargo test --release --test store -- --nocapture > "$RESULTS/crates-store.txt" 2>&1' ;;&
  fairness-bench|cpack)
    # The overhead of the cooperative time slice on a QUIET machine (the agent's table was sanity only at load 13-18),
    # and the latency-bound tests without any slack factor.
    run f-build bash -c 'cd "$SRC/spikes/fairness" && RUSTFLAGS= cargo build --release --example overhead && RUSTFLAGS= cargo test --release --no-run'
    run f-tests bash -c 'cd "$SRC/spikes/fairness" && { echo "load: $(cat /proc/loadavg)"; RUSTFLAGS= cargo test --release --test starvation --test fairness -- --test-threads=1 --nocapture; } > "$RESULTS/fairness-tests.txt" 2>&1'
    # The older protocol and shutdown suites on the sliced scheduler, with the file-descriptor limit of a fresh VM. They are
    # RECORDED, not gating: a failure here is a finding (the first run found one: spill files held open across yields) and
    # must not stop the overhead measurement that follows.
    run f-compat bash -c 'cd "$SRC/spikes/fairness" && { echo "ulimit -n: $(ulimit -n)"; RUSTFLAGS= cargo test --release --test protocol --test shutdown --test spill_fds --test spill_fd_limit -- --test-threads=1 --nocapture; echo "exit status: $?"; } > "$RESULTS/fairness-compat.txt" 2>&1; true'
    settle_before f-overhead
    run f-overhead bash -c 'cd "$SRC/spikes/fairness" && { echo "load: $(cat /proc/loadavg)"; NATA_NOTE="quiet GCP run" ./target/release/examples/overhead --rounds 20; } > "$RESULTS/fairness-overhead.txt" 2>&1' ;;&
  bridge|cpack)
    # The tokio-to-CPU-pool bridge (ADR 0002): stress with weak memory ordering (ARM64), then the I/O latency under a
    # saturated pool, the memory bound and the idle-pool CPU use, on a quiet machine.
    run b-build bash -c 'cd "$SRC/spikes/async-bridge" && RUSTFLAGS= cargo build --release --examples && RUSTFLAGS= cargo test --release --no-run'
    run b-tests bash -c 'cd "$SRC/spikes/async-bridge" && RUSTFLAGS= cargo test --release > "$RESULTS/bridge-tests.txt" 2>&1'
    run b-stress bash -c 'cd "$SRC/spikes/async-bridge" && ./run_stress.sh > "$RESULTS/bridge-stress.txt" 2>&1'
    for m in "latency --rounds 30" "memory" "idle --rounds 6"; do
      settle_before "b-measure-${m%% *}"
      run "b-measure-${m%% *}" bash -c 'cd "$SRC/spikes/async-bridge" && { echo "load: $(cat /proc/loadavg)"; ./target/release/examples/measure '"$m"'; } > "$RESULTS/bridge-measure-'"${m%% *}"'.txt" 2>&1'
    done ;;&
  hashtests)
    # Diagnostic (cheap): the hash-ops tests on this architecture, with a backtrace and the exact dependency versions.
    # The baseline of parmut-hashops failed one test on ARM64 that never fails on x86. Every step records its output and
    # succeeds, so that one run shows everything; the evidence is in the files.
    run ht-versions bash -c 'cd "$SRC/spikes/hash-ops" && { uname -m; sha256sum Cargo.lock; cargo tree --locked -e normal --prefix none 2>&1 | grep -E "^(arrow|arrow-[a-z]+|foldhash|hashbrown) v" | sort -u; } > "$RESULTS/hashtests-versions.txt" 2>&1; true'
    run ht-release bash -c 'cd "$SRC/spikes/hash-ops" && { RUST_BACKTRACE=1 cargo test --release --locked --test null_slots; echo "exit $?"; } > "$RESULTS/hashtests-release.txt" 2>&1; true'
    run ht-repeat bash -c 'cd "$SRC/spikes/hash-ops" && B=$(ls -t target/release/deps/null_slots-* | grep -v "\.d$" | head -1) && f=0 && for i in $(seq 1 20); do "$B" a_join_on_dictionary_keys_and_payloads_ignores_keys_under_null_slots > /dev/null 2>&1 || f=$((f+1)); done; echo "single test, 20 runs: failures=$f" > "$RESULTS/hashtests-repeat.txt"; true'
    run ht-all bash -c 'cd "$SRC/spikes/hash-ops" && { RUST_BACKTRACE=1 cargo test --release --locked; echo "exit $?"; } > "$RESULTS/hashtests-all.txt" 2>&1; true'
    run ht-debug bash -c 'cd "$SRC/spikes/hash-ops" && { RUST_BACKTRACE=1 cargo test --locked --test null_slots; echo "exit $?"; } > "$RESULTS/hashtests-debug.txt" 2>&1; true' ;;
  kani-crate)
    # Kani proofs of the REAL crates/nata-kernels kernels (spikes/kani-nata-kernels, 105 harnesses). CBMC is single-threaded,
    # so a few harnesses run at once on the big machine (scripts/gcp/kani-crate.sh), each with its own target directory.
    # The table is the result: a harness that fails or times out does not stop the job. Phase 1: every harness, 900 s cap.
    # Phase 2: the timeouts and killed runs again with a 3600 s cap, two at a time (the big ones need many GB each).
    run k-install bash -c 'export RUSTUP_TOOLCHAIN=$(sed -n "s/^channel *= *\"\\(.*\\)\"/\\1/p" "$SRC/rust-toolchain.toml"); echo "toolchain: $RUSTUP_TOOLCHAIN"; command -v cargo-kani >/dev/null || { cargo install --locked kani-verifier && cargo kani setup; }; cargo kani --version'
    run kc-all bash -c 'export RUSTUP_TOOLCHAIN=$(sed -n "s/^channel *= *\"\\(.*\\)\"/\\1/p" "$SRC/rust-toolchain.toml"); { free -g | head -2; TMPDIR="$WORK/kc-tmp" bash -c "mkdir -p \$TMPDIR && $SRC/scripts/gcp/kani-crate.sh all $RESULTS/kc ${KANI_PAR:-3} 900"; true; } > "$RESULTS/kani-crate-all.txt" 2>&1'
    run kc-timeouts bash -c 'export RUSTUP_TOOLCHAIN=$(sed -n "s/^channel *= *\"\\(.*\\)\"/\\1/p" "$SRC/rust-toolchain.toml"); { TMPDIR="$WORK/kc-tmp" bash -c "mkdir -p \$TMPDIR && $SRC/scripts/gcp/kani-crate.sh timeouts $RESULTS/kc 2 3600"; true; } > "$RESULTS/kani-crate-timeouts.txt" 2>&1' ;;
  kani-solvers)
    # (1) The step-6 sum bound of the NULL-path spike (spikes/kani-wrap-bound: an induction proof on the REAL
    # spikes/null-path/src/wrapadd.rs) and its mutation check (a negative control and three mutants, each must be CAUGHT).
    # (2) The five division proofs that timed out with the default CaDiCaL back end on 2026-11-04, once per other solver
    # (kissat, z3, cvc5; z3 and cvc5 are installed by vm-startup.sh for this job), 2400 s cap, a few at a time.
    run k-install bash -c 'export RUSTUP_TOOLCHAIN=$(sed -n "s/^channel *= *\"\\(.*\\)\"/\\1/p" "$SRC/rust-toolchain.toml"); echo "toolchain: $RUSTUP_TOOLCHAIN"; command -v cargo-kani >/dev/null || { cargo install --locked kani-verifier && cargo kani setup; }; cargo kani --version || { echo "k-install: cargo kani does not run (wrong image? Kani needs glibc 2.39, Debian 13)"; exit 1; }; z3 --version || echo "z3: NOT FOUND"; cvc5 --version 2>/dev/null | head -1 || echo "cvc5: NOT FOUND"'
    run kw-proofs bash -c 'export RUSTUP_TOOLCHAIN=$(sed -n "s/^channel *= *\"\\(.*\\)\"/\\1/p" "$SRC/rust-toolchain.toml"); cd "$SRC/spikes/kani-wrap-bound" && { ./run.sh 1200; echo "exit status: $?"; } > "$RESULTS/kani-wrap-bound.txt" 2>&1; true'
    run kw-mutants bash -c 'export RUSTUP_TOOLCHAIN=$(sed -n "s/^channel *= *\"\\(.*\\)\"/\\1/p" "$SRC/rust-toolchain.toml"); cd "$SRC/spikes/kani-wrap-bound" && { ./mutate.sh 1200; echo "exit status: $?"; } > "$RESULTS/kani-wrap-bound-mutants.txt" 2>&1; true'
    run kc-solvers bash -c 'export RUSTUP_TOOLCHAIN=$(sed -n "s/^channel *= *\"\\(.*\\)\"/\\1/p" "$SRC/rust-toolchain.toml"); { free -g | head -2; TMPDIR="$WORK/kc-tmp" bash -c "mkdir -p \$TMPDIR && $SRC/scripts/gcp/kani-crate.sh solvers $RESULTS/kc ${KANI_PAR:-5} 2400"; true; } > "$RESULTS/kani-crate-solvers.txt" 2>&1' ;;
  nullpath|timing)
    # ADR 0003 item 16: the word-wise NULL path, timed on a QUIET machine (the agent's numbers were taken at load 2-14).
    # spikes/null-path/run-all.sh: correctness first (tests, check, nullcheck, DuckDB diff; it stops at the first failure,
    # so a wrong engine never produces a timing), then the ladder (wall and CPU time), the overflow-checks A/B/A/B, the
    # density table and the stage timings. The mutation check is skipped: it ran locally and is not timing-sensitive.
    run np-gen-sf1 bash -c 'cd "$SRC" && scripts/gen-tpch.sh 1 data/tpch-sf1'
    settle_before np-all
    run np-all bash -c 'cd "$SRC/spikes/null-path" && CARGO_BUILD_JOBS=$(nproc) OUT="$RESULTS/nullpath" SKIP_MUTATION=1 HEAVY= SETTLE_MAX=600 ./run-all.sh "$SRC/data/tpch-sf1/lineitem.parquet" 25 > "$RESULTS/nullpath-run.txt" 2>&1' ;;&

  objscan)
    # verification-plan item 27 (spikes/object-store-scan): the object-store read path against a SIMULATED store (latency tail,
    # failures, bandwidth cap) on loopback, with the correctness, memory-bound, cancellation and retry tests and their known-bad
    # versions. The real store's coordinates, if any, would be read from a file the owner placed on the data disk
    # ($WORK/objstore.env, mode 600, 'export NAME=value' lines); without it the script says REAL STORE NOT MEASURED. The exit
    # status is recorded and does not stop the job, so the results are kept.
    run o-object-store-scan bash -c 'cd "$SRC" && { for f in "$WORK/objstore.env" "$SRC/spikes/object-store-scan/objstore.env"; do [ -f "$f" ] && set -a && . "$f" && set +a; done; }; spikes/object-store-scan/gcp-check.sh "$RESULTS/object-store-scan" > "$RESULTS/object-store-scan-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/object-store-scan-exit.txt"; true' ;;
  simharness)
    # spikes/sim-harness (verification-plan item 23): build, unit tests, determinism (trace hashes, compare them across machines),
    # the unmutated control sweep, 13 mutants, the shuttle/madsim/turmoil trials and the abstraction-cost benchmark. The exit status
    # is recorded and does not stop the job, so the results are kept.
    run sim-harness bash -c 'cd "$SRC/spikes/sim-harness" && ./gcp-check.sh "$RESULTS/sim-harness" "${SIM_SEEDS:-20000}" > "$RESULTS/sim-harness-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/sim-harness-exit.txt"; true' ;;
  iceberg|iceberg-smoke)
    # verification-plan item 24 (spikes/iceberg-commit): the TLA+ model (TLC, every mutant) and the commit spike against the Apache
    # REST reference fixture. `iceberg-smoke` is the cheap shake-out with short timeouts (it exercises the tool setup, the Rust
    # build, the fixture build, the suites and the interop check, and the long model runs time out by design); the full model run
    # is `iceberg`. The exit status is recorded and does not stop the job, so the results are kept.
    export IC_WORK="$WORK/iceberg-commit-work" IC_RESUME=1
    [ "$JOB" = iceberg-smoke ] && export IC_TLC_TIMEOUT=300 IC_MODEL_BUDGET=900 IC_MUTANT_TIMEOUT=180 IC_SEEDS=2
    run ic-all bash -c 'cd "$SRC" && spikes/iceberg-commit/gcp-check.sh "$RESULTS/iceberg-commit" > "$RESULTS/iceberg-commit-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/iceberg-commit-exit.txt"; true' ;;
  netexchsim)
    # verification-plan item 28, stage A (spikes/net-exchange): a real TCP exchange between N node processes in Linux network namespaces with
    # tc netem delay, loss and rate. Unit tests, the mutants, correctness and throughput under 6 profiles, backpressure, a credit sweep, cancel,
    # kill -9, a partition and the process-exit race. SIMULATED NETWORK on one VM; stage B (separate VMs) is not this job. Needs sudo (vm-startup.sh).
    # The exit status is recorded and does not stop the job, so the results are kept.
    run nx-all bash -c 'cd "$SRC" && spikes/net-exchange/gcp-check.sh "$RESULTS/net-exchange" > "$RESULTS/net-exchange-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/net-exchange-exit.txt"; true' ;;
  pushpin)
    # ADR 0001 open check (verification-plan item 5): why does the scheduler of spikes/operator-trait cost far more with FOUR threads on the x86-64 c3d VM (4 cores x 2
    # hardware threads) than on ARM64? The same cells run unpinned, on four distinct physical cores and on two cores' worth of sibling threads (spikes/push-cost/pin-check.sh;
    # it settles before each run and computes the CPU sets from lscpu). Intended machines: c3d-highcpu-8, with n4a-highcpu-8 as the control (one thread per core).
    run pp-build bash -c 'cd "$SRC/spikes/push-cost" && RUSTFLAGS= cargo build --release --examples'
    run pp-check bash -c 'cd "$SRC" && spikes/push-cost/pin-check.sh "$RESULTS/pushpin" > "$RESULTS/pushpin-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/pushpin-exit.txt"; true' ;;
  icebergpolaris)
    # verification-plan item 24, ADR 0005 rule 8: the commit suite (18 forced interleavings with known-bad writers, 12 crash points, stress seeds) and the pyiceberg
    # interop against Apache POLARIS, a product catalog, instead of the Apache test fixture. IC_MODE=polaris: JDK 21 and the pinned Polaris release are downloaded
    # and verified, the server runs as a throwaway on loopback with an in-memory metastore and local-disk storage (scripts/polaris.sh). No model, no other catalog.
    # Intended machines: n4a-highcpu-8 and c3d-highcpu-8. The exit status is recorded and does not stop the job, so the results are kept.
    export IC_WORK="$WORK/iceberg-polaris-work" IC_RESUME=1 IC_MODE=polaris
    run ic-polaris bash -c 'cd "$SRC" && spikes/iceberg-commit/gcp-check.sh "$RESULTS/iceberg-polaris" > "$RESULTS/iceberg-polaris-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/iceberg-polaris-exit.txt"; true' ;;
  icebergcs)
    # verification-plan item 24 (real-store gap) and ADR 0005 rule 8: the catalog-less commit with its commit point (PutMode::Create of
    # metadata/vN, GCS ifGenerationMatch=0) in a REAL bucket, and probes that the VM identity can write only there (IC_MODE=gcs). Needs
    # spikes/iceberg-commit/gcs.env (git-ignored) and a VM run with --vm-service-account <the write-test account> --vm-storage-scope read-write.
    run ic-gcs bash -c 'cd "$SRC/spikes/iceberg-commit" && IC_WORK="$WORK/ic-gcs-work" IC_RESUME=1 IC_MODE=gcs ./gcp-check.sh "$RESULTS/iceberg-gcs" > "$RESULTS/iceberg-gcs-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/iceberg-gcs-exit.txt"; true' ;;
  icebergspikes)
    # verification-plan items 24, 25, 30: the CODE side of the Iceberg commit check (Rust build and tests, the Apache REST reference fixture,
    # the commit spike against it and against conditional create, and the pyiceberg interop that the first run skipped) on another machine,
    # WITHOUT the TLA+ model (IC_MODE=spikes; the model result is in the `iceberg` and `icebergmodel` runs and does not depend on the VM's
    # architecture). interop-pyiceberg is gating in this mode, so a skip is a FAIL. Intended machines: n4a-highcpu-8 and c3d-highcpu-8.
    export IC_WORK="$WORK/iceberg-spikes-work" IC_RESUME=1 IC_MODE=spikes
    run ic-spikes bash -c 'cd "$SRC" && spikes/iceberg-commit/gcp-check.sh "$RESULTS/iceberg-spikes" > "$RESULTS/iceberg-spikes-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/iceberg-spikes-exit.txt"; true' ;;
  icebergmodel)
    # verification-plan item 24, targeted re-run of the TLC runs that had no result in `iceberg-full-arm64` (three-store-ser timed out; the
    # I4 run, both liveness runs and the 11 canaries were never started): ONLY tools, model-lint, model-parse-and-edits and model-full, for the
    # runs in IC_MODEL_ONLY (default: those 15). No Rust build, REST catalog, spikes, interop or mutants. Intended machine: n4a-highcpu-16
    # (16 TLC workers). Defaults: 4500 s per three-writer run (three-*, live-3), 2700 s per other run, 9000 s in all (each run is cut to
    # the time left). A TIMEOUT or NOT-RUN run makes the verdict FAIL. The orchestrator forwards no environment to the VM, so to change the
    # IC_* variables put 'export NAME=value' lines in spikes/iceberg-commit/icebergmodel.env before the run (uploaded with the source, kept
    # out of git, copied to the results). The exit status is recorded and does not stop the job, so the results are kept.
    export IC_WORK="$WORK/iceberg-model-work" IC_RESUME=1 IC_MODE=model
    run ic-model bash -c 'cd "$SRC" && { f=spikes/iceberg-commit/icebergmodel.env; if [ -f "$f" ]; then cp "$f" "$RESULTS/iceberg-model-env.txt"; set -a; . "./$f"; set +a; fi; }; spikes/iceberg-commit/gcp-check.sh "$RESULTS/iceberg-model" > "$RESULTS/iceberg-model-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/iceberg-model-exit.txt"; true' ;;
  failmodel|failmodel-retry)
    # verification-plan item 29 (spikes/failure-model): the QueryLifecycle TLA+ model (TLC), the canaries and the mutants.
    # failmodel-retry: only the configuration the first run did not finish (it was cut short), with a longer TLC timeout
    [ "$JOB" = failmodel-retry ] && export FM_ONLY=live-safe-2x3 FM_TIMEOUT=9000 FM_SKIP_MUTANTS=1
    run fm-all bash -c 'cd "$SRC" && spikes/failure-model/gcp-check.sh "$RESULTS/failure-model" > "$RESULTS/failure-model-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/failure-model-exit.txt"; true' ;;
  opsbeyond)
    # verification-plan item 31 (spikes/operators-beyond): unit tests, the DuckDB differential check (memory and spill), the mutants.
    run ob-gen-sf1 bash -c 'cd "$SRC" && scripts/gen-tpch.sh 1 data/tpch-sf1'
    run ob-all bash -c 'cd "$SRC/spikes/operators-beyond" && ./gcp-check.sh "$RESULTS/operators-beyond" "$SRC/data/tpch-sf1" > "$RESULTS/operators-beyond-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/operators-beyond-exit.txt"; true' ;;
  spillbench)
    # verification-plan item 15: what the grace hash join's spill path costs (spikes/operators-beyond/spill-bench.sh): in memory,
    # its twin, grace with a 256 MB and a 64 MB budget, and DuckDB, interleaved, a settle before every block. SF1 at 1, 4 and 8
    # threads; SF10 at 8 threads (bigger than the budgets by far). Correctness against DuckDB is the opsbeyond job's check.
    run sb-gen-sf1 bash -c 'cd "$SRC" && scripts/gen-tpch.sh 1 data/tpch-sf1'
    run sb-gen-sf10 bash -c 'cd "$SRC" && scripts/gen-tpch.sh 10 data/tpch-sf10'
    run sb-build bash -c 'cd "$SRC/spikes/operators-beyond" && RUSTFLAGS= cargo build --release && ./test-spill-summary.sh > "$RESULTS/spill-summary-test.txt" 2>&1'
    run sb-sf1 bash -c 'cd "$SRC/spikes/operators-beyond" && { SPILL_ROOT="$WORK/spill-sf1" ./spill-bench.sh "$SRC/data/tpch-sf1" "$RESULTS/spill-sf1" 7 "1 4 8"; echo "exit status: $?"; } > "$RESULTS/spill-sf1-run.txt" 2>&1; rm -rf "$WORK/spill-sf1"; true'
    run sb-sf10 bash -c 'cd "$SRC/spikes/operators-beyond" && { SPILL_ROOT="$WORK/spill-sf10" ./spill-bench.sh "$SRC/data/tpch-sf10" "$RESULTS/spill-sf10" 3 "8"; echo "exit status: $?"; } > "$RESULTS/spill-sf10-run.txt" 2>&1; rm -rf "$WORK/spill-sf10"; true' ;;
  netexchreal)
    # verification-plan item 28, STAGE B: the TCP exchange between SEPARATE VMs (spikes/net-exchange/real-check.sh). Needs a multi-VM run:
    # orchestrate.sh run --job netexchreal --peers 3 (4 nodes); this VM is the driver and starts the nodes on the peers over the private network.
    run nr-all bash -c 'cd "$SRC/spikes/net-exchange" && ./real-check.sh "$RESULTS/net-exchange-real" > "$RESULTS/net-exchange-real-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/net-exchange-real-exit.txt"; true' ;;
  scale-sf10|scale-sf100|scale-cores)
    # verification-plan item 32: the existing pipelines at a large scale. The Arrow pipeline's Q1/Q6 thread sweep against DuckDB, digit for
    # digit (spikes/arrow-pipeline/scale-sweep.sh), and the operators-beyond DuckDB check in its SPILL modes under a 1,024-descriptor limit.
    #   scale-sf10   n4a-highcpu-32, --data-gb 80: SF10, threads 1-32, all spill modes (budget 1 GB, 128 partitions)
    #   scale-sf100  n4a-highcpu-32, --data-gb 300: SF100 (generated in steps on disk), threads 8-32, spill modes only (budget 4 GB, 512 partitions)
    #   scale-cores  n4a-highcpu-64, --data-gb 80: SF10, Q1/Q6 at 1 to 64 threads (no operators check)
    case "$JOB" in scale-sf100) SF=100 ;; *) SF=10 ;; esac
    run sc-gen bash -c 'cd "$SRC" && scripts/gen-tpch.sh '"$SF"' data/tpch-sf'"$SF"' > "$RESULTS/gen-sf'"$SF"'.txt" 2>&1'
    run sc-build bash -c 'cd "$SRC/spikes/arrow-pipeline" && RUSTFLAGS= cargo build --release && cd "$SRC/spikes/operators-beyond" && RUSTFLAGS= cargo build --release'
    case "$JOB" in scale-sf10) TH="1 2 4 8 16 32"; RN=5 ;; scale-sf100) TH="8 16 32"; RN=3 ;; scale-cores) TH="1 8 16 32 48 64"; RN=5 ;; esac
    run sc-sweep bash -c 'cd "$SRC/spikes/arrow-pipeline" && { ./scale-sweep.sh "$SRC/data/tpch-sf'"$SF"'/lineitem.parquet" "$RESULTS/scale-sweep" "'"$TH"'" '"$RN"'; echo "exit status: $?"; } > "$RESULTS/scale-sweep-run.txt" 2>&1; true'
    if [ "$JOB" != scale-cores ]; then
      case "$JOB" in scale-sf100) MEM=4096; GR=512 ;; *) MEM=1024; GR=128 ;; esac
      run sc-ob bash -c 'ulimit -n 1024; cd "$SRC/spikes/operators-beyond" && { echo "ulimit -n: $(ulimit -n)"; OB_SPILL_ONLY='"$([ "$JOB" = scale-sf100 ] && echo 1 || echo 0)"' OB_THREADS=32 OB_MEM_MB='"$MEM"' OB_GRACE='"$GR"' ./check.sh "$SRC/data/tpch-sf'"$SF"'" "$WORK/ob-scale"; echo "exit status: $?"; } > "$RESULTS/operators-scale-run.txt" 2>&1; cp "$WORK/ob-scale/summary.txt" "$RESULTS/operators-scale-summary.txt" 2>/dev/null; rm -rf "$WORK/ob-scale"; true'
    fi ;;
  scanbench)
    # slice 3 (docs/slices/0001-parquet-scan.md): R2 pruning on SF1 lineitem sorted by l_shipdate, and the 1-8 thread scalability of
    # `nata scan --count` on SF1 and SF10 (crates/nata-bench/scan-bench.sh). Intended machines: n4a-highcpu-8 and c3d-highcpu-8.
    run sb-gen-sf1 bash -c 'cd "$SRC" && scripts/gen-tpch.sh 1 data/tpch-sf1'
    run sb-gen-sf10 bash -c 'cd "$SRC" && scripts/gen-tpch.sh 10 data/tpch-sf10'
    run sb-build bash -c 'cd "$SRC" && CARGO_TARGET_DIR="$WORK/target-ws" cargo build --release -p nata'
    run sb-bench bash -c 'cd "$SRC" && { SCAN_WORK="$WORK/scan-work" crates/nata-bench/scan-bench.sh "$WORK/target-ws/release/nata" data/tpch-sf1 data/tpch-sf10 "$RESULTS/scan-bench" 5; echo "exit status: $?"; } > "$RESULTS/scan-bench-run.txt" 2>&1; rm -rf "$WORK/scan-work"; true' ;;
  pgwirebench|item35)
    run pw-build bash -c 'cd "$SRC" && CARGO_TARGET_DIR="$WORK/target-ws" cargo build --release -p nata -p nata-check'
    if [ "$JOB" = item35 ]; then
      run pw-data bash -c 'cd "$SRC" && scripts/gen-tpch.sh 0.01 data/client-check && scripts/gen-sql-check-data.sh data/client-check'
      run pw-oracle bash -c 'cd "$SRC" && "$WORK/target-ws/release/nata-check" --candidate "$WORK/target-ws/release/nata" --data data/client-check --queries check/queries/sql > "$RESULTS/item35-oracle.txt" 2>&1'
      run pw-clients bash -c 'bash "$SRC/crates/nata-bench/pgwire-job.sh" clients'
    else
      run pw-bench bash -c 'bash "$SRC/crates/nata-bench/pgwire-job.sh" bench > "$RESULTS/pgwire-bench.txt" 2>&1'
    fi ;;
  pgwirecheck)
    # Local step-4 replay only: prepared here, never launches cloud resources itself.
    run pgwire-protocol bash -c 'cd "$SRC" && CARGO_TARGET_DIR="$WORK/target-ws" cargo test --release -p nata-pgwire > "$RESULTS/pgwire-protocol.txt" 2>&1'
    run pgwire-live bash -c 'cd "$SRC" && CARGO_TARGET_DIR="$WORK/target-ws" NATA_PGWIRE_SEED=322661912839 NATA_PGWIRE_SCENARIOS=2000 NATA_PGWIRE_LIVE_CASES=10000 NATA_PGWIRE_CONTENTION=$(nproc) cargo test --release -p nata-pgwire --test reliability -- --nocapture > "$RESULTS/pgwire-live.txt" 2>&1'
    run pgwire-mutants bash -c 'cd "$SRC" && python3 crates/nata-pgwire/mutate.py --work "$WORK/pgwire-mutants" > "$RESULTS/pgwire-mutants.txt" 2>&1' ;;
  slice4bench)
    # slice 4, step 4b (docs/slices/0002-select-where-limit.md part 5; record bench/results/20261106-slice4-carried-in.md): the paired benchmarks of the carried-in
    # improvements, all in crates/nata-bench/scan-bench.sh: P1 late materialization against the early read (BLOCKS=late), P2 footers read ahead and P10 on 1,000
    # small files (footers), R4 the cost of the page-checksum verification (crc: two builds of the same code, the second with --no-default-features, and a
    # checksummed copy of lineitem written by pyarrow in a venv). Intended machines: n4a-highcpu-8 and c3d-highcpu-8, one run each; the owner approves the cost first.
    run s4-gen-sf1 bash -c 'cd "$SRC" && scripts/gen-tpch.sh 1 data/tpch-sf1'
    run s4-gen-sf10 bash -c 'cd "$SRC" && scripts/gen-tpch.sh 10 data/tpch-sf10'
    run s4-build bash -c 'cd "$SRC" && CARGO_TARGET_DIR="$WORK/target-ws" cargo build --release -p nata --features page-crc && CARGO_TARGET_DIR="$WORK/target-nocrc" cargo build --release -p nata --no-default-features'
    run s4-pyarrow bash -c 'python3 -m venv "$WORK/venv" && "$WORK/venv/bin/pip" install --quiet pyarrow && "$WORK/venv/bin/python" -c "import pyarrow; print(pyarrow.__version__)"'
    run s4-bench bash -c 'cd "$SRC" && { BLOCKS="late footers crc" NATA_NOCRC="$WORK/target-nocrc/release/nata" PYARROW_PYTHON="$WORK/venv/bin/python" SCAN_WORK="$WORK/scan-work" crates/nata-bench/scan-bench.sh "$WORK/target-ws/release/nata" data/tpch-sf1 data/tpch-sf10 "$RESULTS/slice4-bench" 5; echo "exit status: $?"; } > "$RESULTS/slice4-bench-run.txt" 2>&1; rm -rf "$WORK/scan-work"; true' ;;
  slice4close)
    run s4c-gen-sf1 bash -c 'cd "$SRC" && scripts/gen-tpch.sh 1 data/tpch-sf1'
    run s4c-gen-sf10 bash -c 'cd "$SRC" && scripts/gen-tpch.sh 10 data/tpch-sf10'
    run s4c-build bash -c 'cd "$SRC" && CARGO_TARGET_DIR="$WORK/target-ws" cargo build --release -p nata -p nata-bench -p nata-check && CARGO_TARGET_DIR="$WORK/target-ws" cargo test --release -p nata-sql -p nata-exec -p nata --no-run'
    run s4c-fixtures bash -c 'cd "$SRC" && scripts/gen-sql-check-data.sh data/tpch-sf1'
    run s4c-python bash -c 'python3 -m venv "$WORK/venv" && "$WORK/venv/bin/pip" install --quiet duckdb==2.0.0'
    run s4c-correct bash -c 'cd "$SRC" && CARGO_TARGET_DIR="$WORK/target-ws" cargo test --release -p nata-sql -p nata-exec -p nata > "$RESULTS/s4c-tests.txt" 2>&1'
    run s4c-contention bash -c 'cd "$SRC" && NATA_COMPETITORS=1 NATA_SCENARIOS=3000 CARGO_TARGET_DIR="$WORK/target-ws" cargo test --release -p nata-exec --test scenarios -- --nocapture > "$RESULTS/s4c-contention.txt" 2>&1'
    run s4c-process bash -c 'cd "$SRC" && NATA_COMPETITORS=1 NATA_PROC_SCENARIOS=2000 CARGO_TARGET_DIR="$WORK/target-ws" cargo test --release -p nata --test sql_reliability -- --nocapture > "$RESULTS/s4c-process.txt" 2>&1'
    run s4c-diff bash -c 'cd "$SRC" && for q in scan sql sql-differences; do "$WORK/target-ws/release/nata-check" --candidate "$WORK/target-ws/release/nata" --data data/tpch-sf1 --queries check/queries/"$q" || exit; done > "$RESULTS/s4c-diff.txt" 2>&1'
    run s4c-bench bash -c 'cd "$SRC" && "$WORK/venv/bin/python" crates/nata-bench/sql-bench.py "$WORK/target-ws/release/nata" "$WORK/target-ws/release/sql-plan" data/tpch-sf1 data/tpch-sf10 "$WORK/sql-work" "$RESULTS/slice4-close" > "$RESULTS/s4c-bench.txt" 2>&1'
    run s4c-scan-bench bash -c 'cd "$SRC" && SCAN_WORK="$WORK/scan-work" crates/nata-bench/scan-bench.sh "$WORK/target-ws/release/nata" data/tpch-sf1 data/tpch-sf10 "$RESULTS/scan-close" 5 > "$RESULTS/s4c-scan-bench.txt" 2>&1'
    run s4c-leaks bash -c 'cd "$SRC" && git init --bare "$WORK/leak-index" >/dev/null && export GIT_DIR="$WORK/leak-index" GIT_WORK_TREE="$SRC" && git add --all && scripts/check-leaks.sh --tree > "$RESULTS/s4c-leaks.txt" 2>&1' ;;
  cgroupcheck)
    # verification-plan item 9: the scan's default pool size under cgroup CPU quotas and affinity masks (scripts/cgroup-check.sh, needs
    # sudo for systemd-run --scope; vm-startup.sh grants it on this throwaway VM for this job only).
    run cg-gen bash -c 'cd "$SRC" && scripts/gen-tpch.sh 1 data/tpch-sf1'
    run cg-build bash -c 'cd "$SRC" && CARGO_TARGET_DIR="$WORK/target-ws" cargo build --release -p nata'
    run cg-check bash -c 'cd "$SRC" && { scripts/cgroup-check.sh "$WORK/target-ws/release/nata" data/tpch-sf1/lineitem.parquet "$RESULTS/cgroup"; echo "exit status: $?"; } > "$RESULTS/cgroup-run.txt" 2>&1; true' ;;
  eng-checks)
    # verification-plan item 34 (slice 4 card, part 2): the engineering checks on the real crates. Each check records its exit status in
    # $RESULTS/eng-*-exit.txt and does not stop the job, so one failure does not hide the others.
    NIGHTLY=nightly-2026-08-21
    run ec-tools bash -c 'rustup toolchain install '"$NIGHTLY"' --profile minimal --component rust-src,miri > /dev/null && cargo install --locked cargo-deny cargo-audit > /dev/null 2>&1; cargo deny --version; cargo audit --version; rustup run '"$NIGHTLY"' rustc --version'
    run ec-deny bash -c 'cd "$SRC" && { cargo deny check 2>&1; echo "exit status: $?"; } > "$RESULTS/eng-deny.txt"; tail -1 "$RESULTS/eng-deny.txt" > "$RESULTS/eng-deny-exit.txt"; true'
    run ec-audit bash -c 'cd "$SRC" && { cargo audit 2>&1; echo "exit status: $?"; } > "$RESULTS/eng-audit.txt"; tail -1 "$RESULTS/eng-audit.txt" > "$RESULTS/eng-audit-exit.txt"; true'
    run ec-unsafe bash -c 'cd "$SRC" && { for p in nata nata-parquet nata-sched nata-scan nata-kernels nata-sql nata-exec; do echo "== $p"; python3 -I scripts/unsafe-inventory.py "$p" 2>&1 | tail -n 15; done; } > "$RESULTS/eng-unsafe.txt" 2>&1; true'
    T=$(rustc -vV | sed -n "s/^host: //p")
    for san in address thread; do
      run "ec-$san" bash -c 'cd "$SRC" && { RUSTFLAGS="-Zsanitizer='"$san"'" RUSTDOCFLAGS="-Zsanitizer='"$san"'" CARGO_TARGET_DIR="$WORK/target-'"$san"'" cargo +'"$NIGHTLY"' test -Zbuild-std --target '"$T"' -p nata-sched -p nata-parquet -p nata-scan -p nata-kernels -p nata-sql -p nata-exec -p nata --lib --tests --no-fail-fast -- --skip peak_memory_is_bounded_by_the_window_times_the_largest_row_group 2>&1 | grep -vE "^(   Compiling|    Finished|     Running)"; echo "exit status: ${PIPESTATUS[0]}"; } > "$RESULTS/eng-'"$san"'.txt"; tail -1 "$RESULTS/eng-'"$san"'.txt" > "$RESULTS/eng-'"$san"'-exit.txt"; true'
    done
    # RSS budgets describe the normal allocator, not sanitizer quarantine/shadow metadata.
    # Keep default sanitizer detection settings; execute both RSS tests without instrumentation.
    run ec-rss bash -c 'cd "$SRC" && { CARGO_TARGET_DIR="$WORK/target-rss" cargo test --release -p nata --test resource_bounds --test sql_resource_bounds 2>&1; echo "exit status: $?"; } > "$RESULTS/eng-rss.txt"; tail -1 "$RESULTS/eng-rss.txt" > "$RESULTS/eng-rss-exit.txt"'
    run ec-miri bash -c 'cd "$SRC" && { MIRIFLAGS=-Zmiri-disable-isolation CARGO_TARGET_DIR="$WORK/target-miri" cargo +'"$NIGHTLY"' miri test -p nata-scan --test rules 2>&1 | tail -n 30; r1=${PIPESTATUS[0]}; CARGO_TARGET_DIR="$WORK/target-miri" cargo +'"$NIGHTLY"' miri test -p nata-kernels --lib 2>&1 | tail -n 30; r2=${PIPESTATUS[0]}; echo "exit status: $(( r1 | r2 ))"; } > "$RESULTS/eng-miri.txt"; tail -1 "$RESULTS/eng-miri.txt" > "$RESULTS/eng-miri-exit.txt"; true'
    run ec-miri-sql bash -c 'cd "$SRC" && { MIRIFLAGS="-Zmiri-env-set=NATA_PUSHDOWN_N=64" CARGO_TARGET_DIR="$WORK/target-miri" cargo +'"$NIGHTLY"' miri test -p nata-sql --test pushdown 2>&1 | tail -n 30; echo "exit status: ${PIPESTATUS[0]}"; } > "$RESULTS/eng-miri-sql.txt"; tail -1 "$RESULTS/eng-miri-sql.txt" > "$RESULTS/eng-miri-sql-exit.txt"'
    run ec-miri-exec bash -c 'cd "$SRC" && { CARGO_TARGET_DIR="$WORK/target-miri" cargo +'"$NIGHTLY"' miri test -p nata-exec --lib 2>&1 | tail -n 30; echo "exit status: ${PIPESTATUS[0]}"; } > "$RESULTS/eng-miri-exec.txt"; tail -1 "$RESULTS/eng-miri-exec.txt" > "$RESULTS/eng-miri-exec-exit.txt"'
    run ec-leaks bash -c 'cd "$SRC" && git init --bare "$WORK/leak-index" >/dev/null && export GIT_DIR="$WORK/leak-index" GIT_WORK_TREE="$SRC" && git add --all && scripts/check-leaks.sh --tree > "$RESULTS/eng-leaks.txt" 2>&1'
    run ec-summary bash -c 'cd "$RESULTS" || exit; failed=0; for f in eng-*-exit.txt; do echo "${f%-exit.txt}: $(cat "$f")"; grep -qx "exit status: 0" "$f" || failed=1; done > eng-summary.txt; cat eng-summary.txt; exit "$failed"' ;;
  sqlfuzz)
    # verification-plan item 33 (spikes/sqlfuzz): the fuzzer's tests, `prove` on three seeds, long clean control campaigns.
    run sf-all bash -c 'cd "$SRC/spikes/sqlfuzz" && ./gcp-check.sh "$RESULTS/sqlfuzz" > "$RESULTS/sqlfuzz-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/sqlfuzz-exit.txt"; true' ;;
  parquetwrite)
    # verification-plan item 30 (spikes/parquet-write): tests, every writer configuration verified and read back by DuckDB, DuckDB as writer, known-bad statistics.
    run pw-all bash -c 'cd "$SRC/spikes/parquet-write" && PQW_WORK="$WORK/pqw-work" ./gcp-check.sh "$RESULTS/parquet-write" > "$RESULTS/parquet-write-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/parquet-write-exit.txt"; true' ;;
  icebergplan|icebergplan-mut)
    # verification-plan items 25 and 26 (spikes/iceberg-plan): tests, 22 mutants, interop with pyiceberg and DuckDB both ways, planning at 10k / 100k / 1M files.
    # icebergplan-mut: only the mutation phase (the first icebergplan runs failed it at once on a missing toolchain pin that was fixed after the upload)
    [ "$JOB" = icebergplan-mut ] && export IP_ONLY=mutants
    run ip-all bash -c 'cd "$SRC/spikes/iceberg-plan" && IP_WORK="$WORK/iceberg-plan-work" ./gcp-check.sh "$RESULTS/iceberg-plan" > "$RESULTS/iceberg-plan-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/iceberg-plan-exit.txt"; true' ;;
  armcheck|armcheck-retry)
    # The ARM correctness bundle (scripts/gcp/armcheck.sh): exchange-loom suite and mutants, the hash-ops DuckDB/nullcheck layers
    # shown to fail on known-bad engines, the null-path mutation check, and (when present) the stop_depth loom model and the
    # data-shape checks, concurrently on one 16-vCPU machine. Not gating: the summary lists PASS/FAIL per task.
    run ac-gen-sf1 bash -c 'cd "$SRC" && scripts/gen-tpch.sh 1 data/tpch-sf1'
    # armcheck-retry: only the five tasks that did not finish on the first run (arrow-regress, exloom-suite and stop-depth passed). Two
    # failed on script defects (null-path mutation seed, a data-shape check), three were lost when the orchestrator recreated the VM
    # because the results directory had grown to 3.7 GB; ARMCHECK_TASKS is read by armcheck.sh
    [ "$JOB" = armcheck-retry ] && export ARMCHECK_TASKS="exloom-mutants hash-layers nullmut data-shapes arrow-mutants"
    export ARMCHECK_WORK="$WORK/armcheck-work"   # private copies and generated data stay out of the results directory (the orchestrator pulls it)
    run ac-all bash -c 'cd "$SRC" && scripts/gcp/armcheck.sh "$RESULTS/armcheck" > "$RESULTS/armcheck-summary.txt" 2>&1; true' ;;
  kani-mut-crate)
    # The mutation check of the Kani proofs of the REAL crate (spikes/kani-nata-kernels/mutate_kani.py): each deliberate
    # breakage of kernels.rs is applied to a temp copy and the relevant harnesses run, cheapest first, until one fails.
    run k-install bash -c 'export RUSTUP_TOOLCHAIN=$(sed -n "s/^channel *= *\"\\(.*\\)\"/\\1/p" "$SRC/rust-toolchain.toml"); echo "toolchain: $RUSTUP_TOOLCHAIN"; command -v cargo-kani >/dev/null || { cargo install --locked kani-verifier && cargo kani setup; }; cargo kani --version'
    run kmc bash -c 'export RUSTUP_TOOLCHAIN=$(sed -n "s/^channel *= *\"\\(.*\\)\"/\\1/p" "$SRC/rust-toolchain.toml"); cd "$SRC/spikes/kani-nata-kernels" && { free -g | head -2; python3 -I mutate_kani.py --rows 4 --cap 900 --heavy; true; } > "$RESULTS/kani-mut-crate.txt" 2>&1' ;;
  perf|timing)
    # ADR 0003 item 19: why is Q1 further from the hand-fused loop on ARM64? perf record (software cpu-clock, a VM may
    # not expose hardware counters) of the generic engine and of the fused baseline, one thread, batches in memory, Q1 and Q6.
    # perf itself is installed by vm-startup.sh (root) for this job.
    run p-gen-sf1 bash -c 'cd "$SRC" && scripts/gen-tpch.sh 1 data/tpch-sf1'
    run p-build bash -c 'cd "$SRC/spikes/generic-ops" && CARGO_PROFILE_RELEASE_DEBUG=line-tables-only RUSTFLAGS="-C force-frame-pointers=yes" cargo build --release'
    run p-machine bash -c '{ lscpu | grep -E "Model name|Architecture|L1d|L2|L3"; perf --version; cat /proc/sys/kernel/perf_event_paranoid; } > "$RESULTS/perf-machine.txt" 2>&1'
    settle_before p-timing
    run p-timing bash -c 'cd "$SRC" && B=spikes/generic-ops/target/release/spike-generic-ops && { echo "load: $(cat /proc/loadavg)"; for q in q1 q6; do for arm in generic fused; do a=""; [ $arm = fused ] && a=fused; s=$(date +%s.%N); $B loop data/tpch-sf1/lineitem.parquet 20 $q $a; e=$(date +%s.%N); echo "$q $arm: $(awk -v s=$s -v e=$e "BEGIN{printf \"%.2f\", e-s}") s for 20 iterations incl. one preload"; done; done; } > "$RESULTS/perf-timing.txt" 2>&1'
    for q in q1 q6; do for arm in generic fused; do
      run "p-record-$q-$arm" bash -c 'cd "$SRC" && B=spikes/generic-ops/target/release/spike-generic-ops && a=""; [ '"$arm"' = fused ] && a=fused; perf record -e cpu-clock -F 999 -g -o "$WORK/perf-'"$q-$arm"'.data" -- $B loop data/tpch-sf1/lineitem.parquet 60 '"$q"' $a > /dev/null 2> "$RESULTS/perf-record-'"$q-$arm"'.err"; perf report -i "$WORK/perf-'"$q-$arm"'.data" --stdio --no-children --percent-limit 1.5 -g none 2>/dev/null | grep -v "^#" | grep -v "^$" | head -40 > "$RESULTS/perf-report-'"$q-$arm"'.txt"; perf stat -e task-clock,context-switches,page-faults -- $B loop data/tpch-sf1/lineitem.parquet 10 '"$q"' $a > /dev/null 2> "$RESULTS/perf-stat-'"$q-$arm"'.txt"; true'
    done; done ;;&

  timing)
    # The rest of the timing bundle (after hashops, nullpath and perf above): the compaction and push-cost micro-benchmarks and
    # the Arrow pipeline's correctness gate and benchmarks, on a QUIET machine, one after the other. Correctness first: the
    # arrow-pipeline gate (tests + DuckDB digit-for-digit diff) must pass before its timings are recorded.
    run t-gen-sf1 bash -c 'cd "$SRC" && scripts/gen-tpch.sh 1 data/tpch-sf1'
    settle_before t-compaction
    run t-compaction bash -c 'cd "$SRC" && spikes/compaction/gcp-check.sh "$RESULTS/compaction" 25 > "$RESULTS/compaction-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/compaction-exit.txt"; true'
    settle_before t-pushcost
    run t-pushcost bash -c 'cd "$SRC" && spikes/push-cost/gcp-check.sh "$RESULTS/push-cost" 25 > "$RESULTS/push-cost-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/push-cost-exit.txt"; true'
    run t-ap-build bash -c 'cd "$SRC/spikes/arrow-pipeline" && RUSTFLAGS= cargo build --release'
    run t-ap-tests bash -c 'cd "$SRC/spikes/arrow-pipeline" && RUSTFLAGS= cargo test --release --no-fail-fast -- --test-threads=1 > "$RESULTS/arrow-pipeline-tests.txt" 2>&1'
    run t-ap-diff bash -c 'cd "$SRC/spikes/arrow-pipeline" && scripts/duckdb_diff.sh "$SRC/data/tpch-sf1/lineitem.parquet" > "$RESULTS/arrow-pipeline-duckdb-diff.txt" 2>&1'
    settle_before t-ap-bench
    run t-ap-bench bash -c 'cd "$SRC/spikes/arrow-pipeline" && { bash "$SRC/scripts/gcp/settle.sh" ap-bench 0.5 600; echo "load: $(cat /proc/loadavg)"; LINEITEM="$SRC/data/tpch-sf1/lineitem.parquet" target/release/spike-arrow-pipeline bench "$SRC/data/tpch-sf1/lineitem.parquet" 25; bash "$SRC/scripts/gcp/settle.sh" ap-membench 0.5 600; echo "load: $(cat /proc/loadavg)"; LINEITEM="$SRC/data/tpch-sf1/lineitem.parquet" target/release/spike-arrow-pipeline membench "$SRC/data/tpch-sf1/lineitem.parquet" 25; } > "$RESULTS/arrow-pipeline-bench.txt" 2>&1' ;;

  ci|bench|all) ;;
  crates|fairness-bench|bridge|cpack|hashtests|armcheck|armcheck-retry|kani-mut-crate|objscan|simharness|iceberg|iceberg-smoke|icebergmodel|icebergspikes|icebergpolaris|pushpin|netexchsim) ;;
  hashops|nullpath|perf) ;;   # the blocks above end with ;;& so that the timing bundle chains them; run alone they end here   # the blocks above end with ;;& so that cpack runs all three
  *) fail_job "unknown job $JOB"; exit 1 ;;
esac
finish_job
