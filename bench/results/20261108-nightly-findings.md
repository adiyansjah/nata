# Nightly findings, 2026-11-08

Run [42465596007](https://github.com/adiyansjah/nata/actions/runs/42465596007),
nightly commit `a8710d0`; investigated from local main `c90a434`.
Rules and ADRs are unchanged. Local checks use x86-64; no portability or speed
claim is made. No dependencies, push, merge or cloud work.

## 1. SQL: a listed type difference, not an arithmetic bug

**Rule.** Decimal rules §§2–3, 9 and ADR 0003 decisions 3–4 require the tight
remainder type, the multiplication width cap, and overflow only on selected,
valid rows. As with card 0004 item D, an error following those rules is a listed
difference, even when DuckDB answers.

For the shrunk query `(-12345678901234567890.12345 % a) * d`:

| Step | Derivation / exact value |
|---|---|
| Literal | DECIMAL(25,5) |
| `a INTEGER` in decimal arithmetic | DECIMAL(10,0) |
| `%` | S = 5; P = min(20,10) + 5 = 15; DECIMAL(15,5). Operand alignment needs 25 digits, within 38 |
| `* d DECIMAL(10,2)` | S = 7; both precisions ≤18, so P = min(15+10,18) = 18; checked DECIMAL(18,7) |
| Failing row | `a = -2147483648`, `d = 12345.67` |
| Remainder | `-1797196498.12345` |
| Exact product | `-22187594890987.7329615`, exceeding the 11 integer digits allowed by DECIMAL(18,7) |

**Evidence.** Python Decimal at precision 80 independently gives these exact
values. DuckDB 2.0.0 gives the same values but types `%` as DECIMAL(25,5) and the
product as DECIMAL(35,7). Nata correctly reports a decimal multiplication
overflow naming DECIMAL(18,7).
The downloaded artifact is
`/tmp/nf/13611698712/subset-seed20261108-b1868-q39.sql`.

**Historical replay (measured).** Built `bb946ab` from `git archive` in scratch
with the same Cargo settings, and replayed both the header's original query and
the downloaded shrunk artifact through `nata_adapter.py` against that binary and
the current-main binary. Both commits report the same DECIMAL(18,7)
multiplication overflow. DuckDB answers the original query with
`-22191189283983.9798615`; the extra negation and `0 - (d + id)` explain its
difference from the shrunk row's exact product above.

**Path check (source evidence).** The W-C typed decimal kernels (`e755653` /
`73e561e`, present on main as `70da510` / `9da9f5a`) are used by aggregate
expressions; this is a scalar projection using `eval::arith_into`. Between
`bb946ab` and current main, `eval.rs` changes only the visibility of
`dec_overflow` and `kernel_op`, not arithmetic. Step 2c (`75e2d93`) was already
present at `bb946ab`; its `with_precisions` admission is explicitly restricted to Add/Sub; the shrunk expression has only
Mod/Mul and cannot take that shortcut. The original expression's add/sub also
does not change the replay verdict. No regression from those optimizations was
found on this path.

**Cause and change.** The previous tight-remainder classifier recognized only
`(w % 99999999.99) * CAST(d AS DECIMAL(10,2))`. It did not recognize this
literal/remainder/integer shape. The new class requires the exact unguarded AST,
operand types, multiplication diagnostic and an actual overflowing row. Filters,
limits, offsets, grouping, missing witnesses, NULL-only inputs, zero divisors,
wrong operand types and different errors do not qualify. Dialect §9 and a
Nata-only differential query document the difference. No engine semantics change.

**Tests.** `decimal_integer_remainder_product_uses_the_tight_checked_type`
asserts both bound types, the remainder, the error, the exact widened product,
a selected small product and a NULL result. A mutant replacing the 18-digit
multiplication cap with 38 is caught by the new bound-type assertion (actual DECIMAL(25,7), expected DECIMAL(18,7)); the rule
was restored before the passing full suite. The Python classifier regression
fails with the previous classifier and passes with the new class; negative cases
remain BUG?. The new `d9_decimal_integer_remainder_product_overflow.sql` requires
the DECIMAL(18,7) error.

Implementation commit: `70141ff40f24b3e9266383aefe69b265453c5ac1`.

## 2. Bridge: missing tool in the verdict script

**Rule.** The clean control must pass; a mutant counts as caught only with exit
101/134 and a deadlock/assertion diagnostic. Build failures and timeouts are
INVALID. A real race belongs in production code, an uncaught mutant in the
model, and a time-box failure in the workflow. None of those three occurred here.

**Evidence.** The job log was retrieved with
`gh api --allow-escape-sequences repos/adiyansjah/nata/actions/jobs/128684253746/logs`.
The overall run was still in progress when inspected. The configured clean control
passed at bound 3, including `io_completion_before_or_after_park_is_not_lost`
(129.38 s). Then the dropped-wake mutant panicked with a loom deadlock in
`parked_core::two_workers_wake_and_cancel_two_parked_jobs`. At 09:07:12 UTC the
script printed `rg: command not found`, then `bridge-drop-wake INVALID`, exit 1.
The dropped-cancel mutant was never reached. This was neither a timeout, a clean
concurrency failure, nor a surviving mutant: the verdict harness could not read
the caught mutant's diagnostic.

**Fix.** Use the runner's standard `grep -E` instead of an undeclared ripgrep
dependency. Preserve the accepted exit statuses and diagnostic requirement,
models, mutants, bounds and time limits. Run the new verdict-harness regression
before the real models in the nightly job.

**Tests.** `bash crates/nata-sched/test_bridge_mutants.sh` shadows ripgrep with
an unavailable command and checks caught exits 101/134, SURVIVED, timeout,
compile failure, missing command and failed clean control. Restoring the old
`rg` line makes this test fail with INVALID on a deadlock. The fixed harness
passes. The real bridge loom at bound 2 passed; the full mutant script at both
bounds 2 and 3 passed all ten configured control tests and caught both mutants
with exit 101. The bound-3 I/O-completion control finished in 401.27 s locally,
inside the unchanged 1,200 s model limit.

Implementation commit: `d649394f05350ebf88572648733a3fd24e417bce`.

## Verification

- Python subset suite: 16 tests, PASS, one optional prove test skipped. Core
  sqlfuzz suite: 14 tests, PASS. Verdict-harness regression and `bash -n`: PASS.
- Local subset smoke, seed 20261108: 20,000 queries, 47,804 compared rows,
  1,757 oracle errors, 242 optimizer-dependent queries, 11 unsupported CASEs;
  six differences listed by message and all seven remaining differences shrunk
  into existing listed classes. Exit 0, no BUG?. A fresh shrink of batch 1868,
  query 39 independently reaches the downloaded shrunk query and the new class.
- Differential gates: SQL **252/252**, aggregate **65/65**, forced aggregate
  spill **65/65**, SQL differences **56/56**, all PASS with zero mismatch,
  error, unsupported or pending entries.
- `cargo test --locked -j 4 -p nata-exec -p nata-sched`: PASS, including the new
  regression; one existing opt-in bridge test ignored in the normal build.
- `cargo fmt --all --check`: PASS.
- `cargo clippy --locked -j 4 --workspace --all-targets -- -D warnings`: PASS.
- Standalone bridge loom at bound 2: PASS, all ten configured control tests.
  Mutant script at bound 2: ten controls PASS, both mutants CAUGHT (exit 101).
  Mutant script at bound 3: ten controls PASS, both mutants CAUGHT (exit 101).
  The opt-in combined five-thread model belongs to the other nightly job and
  was not enabled here.

Commands (repo root unless specified):

```sh
# Saved current-main binaries, with an explicit candidate after the old build:
export NATA_BIN=/tmp/nata-nightly/nata-main NATA_CHECK=/tmp/nata-nightly/nata-check-main
export DATA=/tmp/nata-nightly/data
python3 -I spikes/sqlfuzz/tests/test_subset.py
python3 -I spikes/sqlfuzz/tests/test_sqlfuzz.py
bash crates/nata-sched/test_bridge_mutants.sh
cargo test --locked -j 4 -p nata-exec -p nata-sched
cargo fmt --all --check
cargo clippy --locked -j 4 --workspace --all-targets -- -D warnings
# Separate target for cfg(nata_loom); no concurrent Cargo invocations:
export CARGO_TARGET_DIR=/tmp/nata-nightly/loom CARGO_BUILD_JOBS=4 BRIDGE_MODEL_TIMEOUT=1200
LOOM_MAX_PREEMPTIONS=2 bash crates/nata-sched/run_bridge_loom.sh
LOOM_MAX_PREEMPTIONS=2 bash crates/nata-sched/check_bridge_mutants.sh
LOOM_MAX_PREEMPTIONS=3 bash crates/nata-sched/check_bridge_mutants.sh
"$NATA_CHECK" --candidate "$NATA_BIN" --data "$DATA" --queries check/queries/sql
"$NATA_CHECK" --candidate "$NATA_BIN" --data "$DATA" --queries check/queries/aggregate
"$NATA_CHECK" --candidate "$NATA_BIN" --data "$DATA" --queries check/queries/aggregate --force-aggregate-spill
"$NATA_CHECK" --candidate "$NATA_BIN" --data "$DATA" --queries check/queries/sql-differences
# From spikes/sqlfuzz, NATA_BIN set to the freshly built nata:
python3 -I fuzz.py replay-subset --seed 20261108 --batch 1868 --query 39 --target-cmd 'python3 -I adapters/nata_adapter.py'
python3 -I fuzz.py run-subset --seed 20261108 --cases 20000 --jobs 2 --target-cmd 'python3 -I adapters/nata_adapter.py' --out /tmp/nata-nightly/fuzz-smoke
```

Data: existing local TPC-H SF0.01 copied to scratch, with SQL fixtures regenerated
by `scripts/gen-sql-check-data.sh`. Cargo uses one invocation at a time,
`CARGO_BUILD_JOBS=4`, `CARGO_INCREMENTAL=0`, dev/test debug info disabled,
`CARGO_TARGET_DIR=/tmp/nata-nightly/target`, and TMPDIR/NATA_TEST_TMP on disk.
Dev binaries, not performance measurements. Free disk checks remained above 20 GB.
Local logs, saved replay binaries and generated gate data remain in
`/tmp/nata-nightly/` for inspection; they are scratch evidence, not committed
artifacts. Build caches, transient test data and the historical source archive
were removed after verification. `scripts/check-leaks.sh --staged` passed before
each implementation commit and the results-record commit.
