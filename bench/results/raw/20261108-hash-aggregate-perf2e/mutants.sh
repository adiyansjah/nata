#!/usr/bin/env bash
set -eu
export CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=1
set +e
/home/dev/perf2b/cargo-mutants mutants --no-config --in-place --jobserver false --cargo-arg=-j4 --cargo-arg=--locked --cargo-arg=--lib --profile release --output /tmp/nata-perf2e/mutants-exec-focused -p nata-exec -f crates/nata-exec/src/aggregate.rs --cargo-arg=--test --cargo-arg=aggregate -F 'gather_output|replace deliver_output ->|replace \|\| with && in deliver_output|replace == with != in deliver_output|replace < with <= in deliver_output' --build-timeout 300 --timeout 10 -- output_ > /tmp/perf2e-mutants-exec.log 2>&1
rc=$?
set -e
printf '%s\n' "$rc" > /tmp/nata-perf2e/mutants-exec.exit
[ "$rc" -eq 0 ] || [ "$rc" -eq 2 ] || [ "$rc" -eq 3 ]
set +e
/home/dev/perf2b/cargo-mutants mutants --no-config --in-place --jobserver false --cargo-arg=-j4 --cargo-arg=--locked --cargo-arg=--lib --profile release --output /tmp/nata-perf2e/mutants-csv-focused -p nata-parquet -f crates/nata-parquet/src/csv.rs -F 'decimal_into|BatchWriter::(write_batch|flush|field) ->|in BatchWriter::flush|replace \|\| with && in BatchWriter::field' --build-timeout 300 --timeout 10 -- csv::tests > /tmp/perf2e-mutants-csv.log 2>&1
rc=$?
set -e
printf '%s\n' "$rc" > /tmp/nata-perf2e/mutants-csv.exit
[ "$rc" -eq 0 ] || [ "$rc" -eq 2 ] || [ "$rc" -eq 3 ]
