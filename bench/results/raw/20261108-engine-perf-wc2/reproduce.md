# W-C step 2 reproduction

All Cargo builds/tests are serial, four jobs. Existing locked dependencies only.

```sh
export CARGO_TARGET_DIR=/home/dev/perf2b/target CARGO_PROFILE_RELEASE_DEBUG=1 CARGO_BUILD_JOBS=4
cargo test --locked --release -j 4 -p nata-kernels -p nata-exec
cargo fmt --all --check
cargo clippy --locked --release -j 4 -p nata-kernels -p nata-exec --all-targets -- -D warnings
cargo build --locked --release -j 4 -p nata -p nata-check
env -u AI_AGENT scripts/gen-tpch.sh 0.01 /tmp/nata-wc2/gates
scripts/gen-sql-check-data.sh /tmp/nata-wc2/gates
for suite in aggregate aggregate-spill sql scan; do
  opts=(); queries=$suite
  if [ "$suite" = aggregate-spill ]; then queries=aggregate; opts+=(--force-aggregate-spill); fi
  env -u AI_AGENT "$CARGO_TARGET_DIR/release/nata-check" \
    --candidate "$CARGO_TARGET_DIR/release/nata" --data /tmp/nata-wc2/gates \
    --queries "check/queries/$queries" "${opts[@]}"
done
# Read-only binary copies: base=709674d, part1=dictionary, part2=cumulative.
NATA_QUERY_STATS=1 NATA_STATS_DIR="$PWD/bench/results/raw/20261108-engine-perf-wc2/part1-stats" \
  python3 scripts/bench-aggregate-perf2b-local.py part1 --before /tmp/nata-wc2/base \
  --after /tmp/nata-wc2/part1 --data /home/dev/nata-s5-fixes/sf1 \
  --out bench/results/raw/20261108-engine-perf-wc2/part1-screen --rounds 9
```

The baseline was built from an immutable `git archive 709674d` extraction.
After switching the shared target between trees, source mtimes were refreshed
before rebuilding to prevent Cargo reusing an artifact from a different tree.
Exact CSV multiset comparison before/after (using Python csv/Counter, without
DuckDB AVG tolerance) preceded the timing runner's own DuckDB gates.

Proof and negative control, from `spikes/kani-nata-kernels`:

```sh
CARGO_TARGET_DIR=/home/dev/perf2b/kani-target CARGO_BUILD_JOBS=4 \
  cargo kani --exact --harness wc2_dictionary::wc2_codes
```

Negative control: in the actual production `dictionary.rs`, replace
`if *valid.get(i)? {` by `if true {`, rerun the same harness, require exit 1
and `VERIFICATION:- FAILED`, then restore the source. Both logs are retained.
No timeout, compile failure or unreachable cover is counted as a catch.

Focused mutations, from the root:

```sh
CARGO_TARGET_DIR=/home/dev/perf2b/target CARGO_PROFILE_RELEASE_DEBUG=1 \
  /home/dev/perf2b/cargo-mutants mutants --no-config --in-place --jobserver false \
  --cargo-arg=-j4 --cargo-arg=--lib --profile release \
  --output /tmp/nata-wc2/part1-mutants-final -p nata-kernels \
  -f crates/nata-kernels/src/dictionary.rs --timeout 180
```

Kani 0.68 / CBMC 6.11; cargo-mutants 27.1.0. No concurrent Cargo builds/tests.
Cloud plan/run commands and immutable archive preparation are in the main
W-C report. These local numbers have no settled-machine performance verdict.
