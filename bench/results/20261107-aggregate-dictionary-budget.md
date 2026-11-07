# Aggregate dictionary scratch admission — 2026-11-07

The aggregate string-expression reservation was made after scan tasks could
borrow the remaining budget for partial tables. A denied or conflicting
request was treated as proof of an oversized minimum whenever the requesting
task had no local groups. Peer tables or in-flight writes could still own the
memory. `scan_error` then wrapped the resource message as an internal error.
The short dictionary values themselves do not require more than 16 MiB.

**Measured reproduction:** both recorded VMs exposed 16 CPUs; the local machine
exposes eight. Fresh and existing local `sqlcheck/big.parquet` are byte-identical
(SHA-256 `a13683acf8b2cffcd5a4a6fb68b0ff4c83b78c7084e71efbaa7780357ef35c0a`).
With freshly generated data, the unmodified skeleton (`e198d08`)
passes all 65 queries at eight requested threads, but at 16 requested threads
passes **64/65**, failing only `dictionary_partials` with the recorded internal
resource error. The existing local performance release also reproduces the
exact row-group-1 failure at 16 threads. Fixture regeneration alone is therefore
insufficient to reproduce it on an eight-CPU host: task concurrency matters.
A diagnostic-only copy of the old failure branch records a **20,484-byte**
scratch request rejected as **`RequestChanged`**, with one pending request and
16,773,120 held bytes plus 4,096 progress bytes (B = 16,777,216). The shared
operator already had another request size queued; the requesting task's empty
local table did not establish a query-wide minimum failure. This trace changes
only logging after denial and is not part of the production patch.

The fix follows [ADR 0009](../../docs/decisions/0009-query-memory-manager.md)'s up-front non-revocable workspace rule. Each task
reading VARCHAR admits the existing 64 KiB scratch floor plus 128 bytes of
chunk bookkeeping before any table can borrow. Concurrency accounts for that
65,664-byte charge together with cursor/decode allowances, after spill registry,
codec and I/O reservations. The existing 1 MiB table-progress allowance remains.
The checked admission minimum is the already-held fixed workspace plus one
task's decode/scratch allowance plus that table allowance. A smaller cap fails
with the numeric minimum and cap at an aggregate SQL position. The gate stays
at **16 MiB**, with no query skips or budget increase.

Following [ADR 0010](../../docs/decisions/0010-hash-aggregation.md#spill-and-termination),
string batches exceeding admitted capacity are evaluated in smaller prefixes;
the remainder remains under the cursor/decode reservation. This preserves
WHERE, argument, HAVING and finalization semantics while bounding dictionary
expansion. A single oversized expression retains the existing fallible growth
path and reports a positioned resource error with its minimum byte requirement.
Resource evaluator errors map to PostgreSQL **53200**, rather than XX000 or a
generic data exception. No accepted ADR decision, dependency or result type
changes; this is workspace admission and bounded evaluation, not performance work.

Regression coverage includes the generated fixture's shape (100,000 rows,
13 short dictionary values, 6,144-row groups, integer partial sums and HAVING)
at 1/8/16 requested threads, ordinary/forced spill, checked against independently
calculated exact rows. The same native regression fails against the original
skeleton with the dictionary scratch error and passes with the fix. A deterministic test admits scratch before a competing table takes
all remaining credit, then requires dictionary evaluation to proceed with no
pending request and complete grant cleanup. The admission-skipped mutant removes
that reservation and is **CAUGHT by the fresh-shape regression**, with a passing
private control, zero survivors and zero invalid runs. Tiny-cap and wire checks require a
numeric, positioned resource error and SQLSTATE 53200.

**All required checks PASS:** fmt; workspace/all-target clippy with warnings
denied; workspace tests; cargo-deny; all 16 memory loom models at bound 2
(**1,009 executions**). The staged leak scan is recorded with the commit.
Checks run sequentially with disk TMPDIR and two Cargo/Rust-test workers.

Both fixture sets pass the complete gate at **16 requested threads**:

| Fixture | Scan | SQL | SQL differences | Aggregate | Forced spill |
|---|---:|---:|---:|---:|---:|
| Fresh generators | 5/5 | 252/252 | 55/55 | 65/65 | 65/65 |
| Existing local | 5/5 | 252/252 | 55/55 | 65/65 | 65/65 |

Zero mismatches, unsupported queries, unexpected errors or pending queries.
Forced mode still requires nonzero spill counters for successful nonempty results.
The fixture step also generated the separate SF1 directory as specified; this
fix's query gates use the SF0.01/sqlcheck directory, exactly as the heavy recipe.
No GCP, push or merge; no performance verdict or ARM64 execution is claimed.

Evidence: [reproduction and denial trace](raw/20261107-aggregate-dictionary-budget/reproduction.txt),
[check record](raw/20261107-aggregate-dictionary-budget/checks.txt),
[mutant](raw/20261107-aggregate-dictionary-budget/mutant.md).

Reproduce locally from the repo root (unset `AI_AGENT` for DuckDB):

```sh
mkdir -p "$HOME/tmp-dictfix/tmp"
export TMPDIR="$HOME/tmp-dictfix/tmp" CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 RUST_TEST_THREADS=2
unset AI_AGENT
scripts/gen-tpch.sh 0.01 "$HOME/tmp-dictfix/fresh"
scripts/gen-tpch.sh 1 "$HOME/tmp-dictfix/sf1"
scripts/gen-sql-check-data.sh "$HOME/tmp-dictfix/fresh"
cargo build --locked -p nata -p nata-check
export DICTFIX_CANDIDATE="$PWD/target/debug/nata"
cat > "$HOME/tmp-dictfix/candidate-16" <<'SH'
#!/bin/bash
exec "$DICTFIX_CANDIDATE" "$@" --threads 16
SH
chmod +x "$HOME/tmp-dictfix/candidate-16"
target/debug/nata-check --candidate "$HOME/tmp-dictfix/candidate-16" \
  --data "$HOME/tmp-dictfix/fresh" --queries check/queries/aggregate \
  --force-aggregate-spill
cargo test -p nata-exec --test aggregate_spill \
  dictionary_partials_fresh_fixture_shape_fits_forced_spill_budget -- --exact
python3 crates/nata-exec/mutate.py --work "$HOME/tmp-dictfix/mutants" \
  dictionary-scratch-admission-skipped
LOOM_MAX_PREEMPTIONS=2 crates/nata-sched/run_memory_loom.sh
```

For the negative control, archive `e198d08` into a scratch directory with
`git archive e198d08 Cargo.toml Cargo.lock rust-toolchain.toml crates | tar -x -C DIR`,
build it with `cargo build --locked --manifest-path DIR/Cargo.toml -p nata -p nata-check`,
and point `DICTFIX_CANDIDATE` at its `DIR/target/debug/nata`. The identical gate
then reports 64/65 with only `dictionary_partials` failing. For the complete final
gates, repeat the checker on each fixture directory with `check/queries/scan`,
`sql`, `sql-differences`, `aggregate`, then forced-spill `aggregate`.
