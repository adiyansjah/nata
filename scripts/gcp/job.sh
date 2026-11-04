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
      run "h-$m-ov-$ov" bash -c 'cd "$SRC/spikes/hash-ops" && { echo "load: $(cat /proc/loadavg)"; '"$B"' '"$m"' "$SRC/data/tpch-sf1" 25; } > "$RESULTS/hashops-'"$m"'-ov-'"$ov"'.txt" 2>&1'
    done; done
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
    run f-overhead bash -c 'cd "$SRC/spikes/fairness" && { echo "load: $(cat /proc/loadavg)"; NATA_NOTE="quiet GCP run" ./target/release/examples/overhead --rounds 20; } > "$RESULTS/fairness-overhead.txt" 2>&1' ;;&
  bridge|cpack)
    # The tokio-to-CPU-pool bridge (ADR 0002): stress with weak memory ordering (ARM64), then the I/O latency under a
    # saturated pool, the memory bound and the idle-pool CPU use, on a quiet machine.
    run b-build bash -c 'cd "$SRC/spikes/async-bridge" && RUSTFLAGS= cargo build --release --examples && RUSTFLAGS= cargo test --release --no-run'
    run b-tests bash -c 'cd "$SRC/spikes/async-bridge" && RUSTFLAGS= cargo test --release > "$RESULTS/bridge-tests.txt" 2>&1'
    run b-stress bash -c 'cd "$SRC/spikes/async-bridge" && ./run_stress.sh > "$RESULTS/bridge-stress.txt" 2>&1'
    for m in "latency --rounds 30" "memory" "idle --rounds 6"; do
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
  nullpath|timing)
    # ADR 0003 item 16: the word-wise NULL path, timed on a QUIET machine (the agent's numbers were taken at load 2-14).
    # spikes/null-path/run-all.sh: correctness first (tests, check, nullcheck, DuckDB diff; it stops at the first failure,
    # so a wrong engine never produces a timing), then the ladder (wall and CPU time), the overflow-checks A/B/A/B, the
    # density table and the stage timings. The mutation check is skipped: it ran locally and is not timing-sensitive.
    run np-gen-sf1 bash -c 'cd "$SRC" && scripts/gen-tpch.sh 1 data/tpch-sf1'
    run np-all bash -c 'cd "$SRC/spikes/null-path" && CARGO_BUILD_JOBS=$(nproc) OUT="$RESULTS/nullpath" SKIP_MUTATION=1 HEAVY= ./run-all.sh "$SRC/data/tpch-sf1/lineitem.parquet" 25 > "$RESULTS/nullpath-run.txt" 2>&1' ;;&

  objscan)
    # verification-plan item 27 (spikes/object-store-scan): the object-store read path against a SIMULATED store (latency tail,
    # failures, bandwidth cap) on loopback, with the correctness, memory-bound, cancellation and retry tests and their known-bad
    # versions. The real store's coordinates, if any, would be read from a file the owner placed on the data disk
    # ($WORK/objstore.env, mode 600, 'export NAME=value' lines); without it the script says REAL STORE NOT MEASURED. The exit
    # status is recorded and does not stop the job, so the results are kept.
    run o-object-store-scan bash -c 'cd "$SRC" && { [ -f "$WORK/objstore.env" ] && set -a && . "$WORK/objstore.env" && set +a; }; spikes/object-store-scan/gcp-check.sh "$RESULTS/object-store-scan" > "$RESULTS/object-store-scan-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/object-store-scan-exit.txt"; true' ;;
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
  failmodel|failmodel-retry)
    # verification-plan item 29 (spikes/failure-model): the QueryLifecycle TLA+ model (TLC), the canaries and the mutants.
    # failmodel-retry: only the configuration the first run did not finish (it was cut short), with a longer TLC timeout
    [ "$JOB" = failmodel-retry ] && export FM_ONLY=live-safe-2x3 FM_TIMEOUT=9000 FM_SKIP_MUTANTS=1
    run fm-all bash -c 'cd "$SRC" && spikes/failure-model/gcp-check.sh "$RESULTS/failure-model" > "$RESULTS/failure-model-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/failure-model-exit.txt"; true' ;;
  opsbeyond)
    # verification-plan item 31 (spikes/operators-beyond): unit tests, the DuckDB differential check (memory and spill), the mutants.
    run ob-gen-sf1 bash -c 'cd "$SRC" && scripts/gen-tpch.sh 1 data/tpch-sf1'
    run ob-all bash -c 'cd "$SRC/spikes/operators-beyond" && ./gcp-check.sh "$RESULTS/operators-beyond" "$SRC/data/tpch-sf1" > "$RESULTS/operators-beyond-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/operators-beyond-exit.txt"; true' ;;
  sqlfuzz)
    # verification-plan item 33 (spikes/sqlfuzz): the fuzzer's tests, `prove` on three seeds, long clean control campaigns.
    run sf-all bash -c 'cd "$SRC/spikes/sqlfuzz" && ./gcp-check.sh "$RESULTS/sqlfuzz" > "$RESULTS/sqlfuzz-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/sqlfuzz-exit.txt"; true' ;;
  parquetwrite)
    # verification-plan item 30 (spikes/parquet-write): tests, every writer configuration verified and read back by DuckDB, DuckDB as writer, known-bad statistics.
    run pw-all bash -c 'cd "$SRC/spikes/parquet-write" && PQW_WORK="$WORK/pqw-work" ./gcp-check.sh "$RESULTS/parquet-write" > "$RESULTS/parquet-write-run.txt" 2>&1; echo "exit status: $?" > "$RESULTS/parquet-write-exit.txt"; true' ;;
  icebergplan)
    # verification-plan items 25 and 26 (spikes/iceberg-plan): tests, 22 mutants, interop with pyiceberg and DuckDB both ways, planning at 10k / 100k / 1M files.
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
    run p-timing bash -c 'cd "$SRC" && B=spikes/generic-ops/target/release/spike-generic-ops && { echo "load: $(cat /proc/loadavg)"; for q in q1 q6; do for arm in generic fused; do a=""; [ $arm = fused ] && a=fused; s=$(date +%s.%N); $B loop data/tpch-sf1/lineitem.parquet 20 $q $a; e=$(date +%s.%N); echo "$q $arm: $(awk -v s=$s -v e=$e "BEGIN{printf \"%.2f\", e-s}") s for 20 iterations incl. one preload"; done; done; } > "$RESULTS/perf-timing.txt" 2>&1'
    for q in q1 q6; do for arm in generic fused; do
      run "p-record-$q-$arm" bash -c 'cd "$SRC" && B=spikes/generic-ops/target/release/spike-generic-ops && a=""; [ '"$arm"' = fused ] && a=fused; perf record -e cpu-clock -F 999 -g -o "$WORK/perf-'"$q-$arm"'.data" -- $B loop data/tpch-sf1/lineitem.parquet 60 '"$q"' $a > /dev/null 2> "$RESULTS/perf-record-'"$q-$arm"'.err"; perf report -i "$WORK/perf-'"$q-$arm"'.data" --stdio --no-children --percent-limit 1.5 -g none 2>/dev/null | grep -v "^#" | grep -v "^$" | head -40 > "$RESULTS/perf-report-'"$q-$arm"'.txt"; perf stat -e task-clock,context-switches,page-faults -- $B loop data/tpch-sf1/lineitem.parquet 10 '"$q"' $a > /dev/null 2> "$RESULTS/perf-stat-'"$q-$arm"'.txt"; true'
    done; done ;;&

  timing)
    # The rest of the timing bundle (after hashops, nullpath and perf above): the compaction and push-cost micro-benchmarks and
    # the Arrow pipeline's correctness gate and benchmarks, on a QUIET machine, one after the other. Correctness first: the
    # arrow-pipeline gate (tests + DuckDB digit-for-digit diff) must pass before its timings are recorded.
    run t-gen-sf1 bash -c 'cd "$SRC" && scripts/gen-tpch.sh 1 data/tpch-sf1'
    run t-compaction bash -c 'cd "$SRC" && spikes/compaction/gcp-check.sh "$RESULTS/compaction" 25 > "$RESULTS/compaction-run.txt" 2>&1'
    run t-pushcost bash -c 'cd "$SRC" && spikes/push-cost/gcp-check.sh "$RESULTS/push-cost" 25 > "$RESULTS/push-cost-run.txt" 2>&1'
    run t-ap-build bash -c 'cd "$SRC/spikes/arrow-pipeline" && RUSTFLAGS= cargo build --release'
    run t-ap-tests bash -c 'cd "$SRC/spikes/arrow-pipeline" && RUSTFLAGS= cargo test --release --no-fail-fast -- --test-threads=1 > "$RESULTS/arrow-pipeline-tests.txt" 2>&1'
    run t-ap-diff bash -c 'cd "$SRC/spikes/arrow-pipeline" && scripts/duckdb_diff.sh "$SRC/data/tpch-sf1/lineitem.parquet" > "$RESULTS/arrow-pipeline-duckdb-diff.txt" 2>&1'
    run t-ap-bench bash -c 'cd "$SRC/spikes/arrow-pipeline" && { echo "load: $(cat /proc/loadavg)"; LINEITEM="$SRC/data/tpch-sf1/lineitem.parquet" target/release/spike-arrow-pipeline bench "$SRC/data/tpch-sf1/lineitem.parquet" 25; echo "load: $(cat /proc/loadavg)"; LINEITEM="$SRC/data/tpch-sf1/lineitem.parquet" target/release/spike-arrow-pipeline membench "$SRC/data/tpch-sf1/lineitem.parquet" 25; } > "$RESULTS/arrow-pipeline-bench.txt" 2>&1' ;;

  ci|bench|all) ;;
  crates|fairness-bench|bridge|cpack|hashtests|armcheck|armcheck-retry|kani-mut-crate|objscan|simharness|iceberg|iceberg-smoke) ;;
  hashops|nullpath|perf) ;;   # the blocks above end with ;;& so that the timing bundle chains them; run alone they end here   # the blocks above end with ;;& so that cpack runs all three
  *) fail_job "unknown job $JOB"; exit 1 ;;
esac
finish_job
