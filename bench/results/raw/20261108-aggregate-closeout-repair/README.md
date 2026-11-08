Local x86-64 controls, **UNSETTLED**. Main base `a710ae7`; independent immutable
baseline/candidate binaries, SF1 from `scripts/gen-tpch.sh 1` (6,001,215 rows),
warm filesystem cache, whole CLI including startup/planning/CSV to `/dev/null`.
Value gates write complete CSV and compare typed numeric multisets to DuckDB
before timing. Five alternating before/after/before-twin rounds for each of
1/8 threads and 4 GiB/64 MiB. No GCP run or target verdict.

Reproduce from this checkout, keeping a baseline built from `a710ae7`:

```sh
# BEFORE and AFTER must be immutable (chmod a-w) copies, not Cargo outputs.
scripts/gen-tpch.sh 1 "$DATA"
python3 scripts/bench-aggregate-perf2b-local.py adoption \
  --before "$BEFORE" --after "$AFTER" --data "$DATA" --out "$OUT" \
  --queries high_cardinality_tpch --budgets 4294967296 67108864 --rounds 5
```

`adoption-{gates,samples,summary}.json` retain per-run counters, durations,
load, paired ratios, distributions, input and binary hashes. Reference uses
8 GiB only as the independent value oracle, per the original 2b method;
**Nata is measured at the explicit frozen 4 GiB and spill 64 MiB budgets**.

Focused mutation command (one local Cargo invocation at a time):

```sh
git diff a710ae7 HEAD -- crates/nata-exec/src/aggregate/pipeline.rs > "$PIPELINE_DIFF"
cargo-mutants mutants --cap-lints false --in-place -p nata-exec \
  -f crates/nata-exec/src/aggregate/pipeline.rs -F '(fixed_width_output|resident_merge_needs_spill|accumulate)' \
  --in-diff "$PIPELINE_DIFF" -E 'replace accumulate ->' \
  --jobserver-tasks 4 --cargo-arg=--locked --cargo-arg=-j --cargo-arg=4 \
  --cargo-arg=--lib --cargo-arg=--test --cargo-arg=aggregate_spill \
  --timeout 120 --build-timeout 300 \
  --output "$MUTATION_OUT" --caught --unviable
```

The `accumulate` default-return mutations are excluded because neither `Storage`
nor `Partitions` has `Default` and the guessed I/O constructors are not valid;
these syntactic failures cannot count as catches. Changed-condition mutations
and both new decision functions remain in the cohort.

Independent (untimed-verdict) RSS/CPU/reservation probes, after value gates:

```sh
python3 bench/results/raw/20261108-aggregate-closeout-repair/resources.py \
  --before "$BEFORE" --after "$AFTER" --data "$DATA" --out "$RESOURCE_OUT"
```

Initial mutations: 18 caught, one missed. Added the unprojected string-key
case, then reran exactly that mutation with the same Cargo/test arguments:

```sh
cargo-mutants mutants --cap-lints false --in-place -p nata-exec \
  -f crates/nata-exec/src/aggregate/pipeline.rs \
  -F 'replace \|\| with && in fixed_width_output' --jobserver-tasks 4 \
  --cargo-arg=--locked --cargo-arg=-j --cargo-arg=4 \
  --cargo-arg=--lib --cargo-arg=--test --cargo-arg=aggregate_spill \
  --timeout 120 --build-timeout 300 --output "$RECHECK_OUT" --caught --unviable
```

New baseline passes; 1/1 caught. The two outcome JSON files preserve both runs.
`--in-place` enforces a single mutation scenario/Cargo job at a time.

The final checked rebuild has a different file hash from the measured binary,
but all runtime bytes are identical except the GNU build-id. Reproduce the
read-only image check (it also rejects a flipped instruction byte):

```sh
python3 bench/results/raw/20261108-aggregate-closeout-repair/verify-images.py \
  "$MEASURED_AFTER" "$CHECKED_AFTER"
```
