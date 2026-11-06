# nata-exec

Execution of the logical plans of `nata-sql` (slice 4, card `docs/slices/0002-select-where-limit.md`, step 3). `nata sql` is the thin command around it.

| module | what |
|---|---|
| `eval` | the expression evaluator: a typed expression over the rows of one Arrow batch, for a SELECTION of rows. Checked integers (the range of the SQL type), exact `i128` decimals checked against the digits of the result type (`docs/decimal-rules.md` section 3), three-valued logic, `NaN`/`-0.0` ordering, `AND`/`OR`/`CASE`/`BETWEEN` evaluating an operand only for the rows that need it, a `WHERE` that only follows TRUE rows |
| `cast` | `CAST` and the binder's widenings: rounding half away from zero, the exact binary value of a double, plain text forms only, errors without the value |
| `num` | exact number helpers (powers of ten, rounded division, decimal/double/text conversions) with their own tests |
| `value` | columns: one typed vector and one validity vector, converted from Arrow on first use and back for the output |
| `exec` | `execute(plan, config, cancel, sink)`: the slice 3 scan with the pushed conjuncts, `Filter` and `Project` inside each scan task (`ScanPlan::open_with_map`), `Offset` and `Limit` on the consuming thread; deep expressions run on a thread with a stack sized for them |
| `explain` | the text of `EXPLAIN ANALYZE` |
| `output` | the CSV and aligned-table writers (streaming), the error rendering with a caret, the exit codes |

Why a new crate and not `nata-kernels`: the kernels crate holds the checked `i64` spike kernels and `strict_cast` of ADR 0003; the executor needs every integer
width, decimals up to 38 digits, text, dates and floats. It follows the same rules (errors only for selected valid rows, checked arithmetic, no lenient arrow
entry point: `clippy.toml` is a copy of the kernels', `strict_cast` is the one conversion of arrow it uses), written as scalar loops over the selection.
Replacing them with SIMD kernels is the later optimisation (improvement register P5); the scalar loops are the reference for it.

## Checks (`cargo test -p nata-exec`; the commands for the long runs are in `bench/results/20261106-sql-execution.md`)

- `tests/exec.rs`: the rules of the dialect as running code on generated Parquet files (three-valued logic, errors only for selected rows, CASE, exact decimals,
  floats, text, limit and offset, the limit stopping the scan, zero-column queries, a chain of 10,000 terms, errors without cell values, casts, globs).
- `tests/scenarios.rs`: 600 random scenarios by default (`NATA_SCENARIOS`, `NATA_SCENARIO_SEED`, `NATA_SCENARIO_ONLY=<n>` replays one, `NATA_COMPETITORS=1` adds a
  busy loop on every core): random query, threads, window and order, with a closed stdout, a cancel at a random moment, an I/O error or a panic injected into the
  opener, a data error in a random row, or delays; after each one no handle, footer, buffer or thread is left and the output relates to the single-thread run.
- `mutate.py`: each mutant of the executor, the CSV writer and the command must be caught by a test.
