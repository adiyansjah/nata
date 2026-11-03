# 0003: Batch format and expression kernels

Status: proposed
Date: 2026-11-03

## Context

Slice 2 builds the batch format and the first expression kernels, and every operator uses them. Requirements:

- No silent coercion; overflow is an error (`vision.md`). Decimals must be exact, and results must not depend on thread count (0001).
- Arrow layout is the interop format and is never replaced (`vision.md`). Dictionary-encoded columns are common in Parquet.
- Reuse a commodity component unless a benchmark shows it is the bottleneck (`vision.md`, `way-of-working.md` section 6).
- Scalar-first, correct and measured on x86-64 and ARM64, runtime dispatch rather than build-time tuning (`portability.md`).

This is open question 3 of `feature-map.md` ("where kernels come from").

## Decision

1. **Batch = Arrow columns plus an optional selection vector.** Columns are Arrow arrays (`arrow-array` types). The selection vector is our own type (arrow-rs has none) and travels with the batch. Compaction (applying the selection) is an explicit step, and happens before a batch enters an exchange (0001) and before any arrow-rs kernel that cannot take a selection.
2. **Dictionary columns stay dictionary arrays in the batch type**, not expanded to strings. Dictionary-aware operator paths (filter, group-by) are the "compressed execution" feature, tier T2 in `feature-map.md`; slice 2 does not require them, but the batch type and kernel interface must not rule them out.
3. **Decimals use the narrowest Arrow type that fits:** `Decimal64` for precision up to 18, `Decimal128` up to 38; above 38 is `not supported yet`. Result precision and scale for arithmetic follow **`docs/decimal-rules.md`**, not arrow's kernels. For multiplication that means DuckDB's width-class cap (two operands of precision up to 18 give a result of at most 18 digits when the scale is below 18), which is also what arrow's `Decimal64` multiply happens to do, so the common case stays 64-bit and every result is checked. Results are widened at the client edge where a protocol needs it.
4. **Errors are raised only for rows that are selected and valid.** `WHERE x <> 0 AND 1 / x > 1` and `CASE` guards must work, so an overflow, invalid cast or division by zero in a row the selection removed, or in a NULL slot, is not an error. Own kernels mask their error flag with selection and validity. arrow-rs wrappers compact their input first (or fold the selection into the validity mask).
5. **Kernels are staged behind our own expression-evaluation interface**, which takes a batch and its selection vector:
   - **Own from the start:** integer and decimal arithmetic (checked, NULL-aware without per-row branches, selection-aware errors), `SUM`/`AVG` and the other numeric aggregates (exact accumulators, `i128` for decimal sums), the hash aggregate and the joins.
   - **arrow-rs behind the interface, with strict wrappers:** comparisons, boolean logic, filter and take, casts (always `safe: false`), string functions and `LIKE`, date and time functions, dictionary operations. Replace one at a time only when a benchmark shows both that the kernel matters (a top self-time entry in a benchmark query, at least 5% of its time) and that a replacement is at least 1.5x faster.
   - **Fused kernels** (for example `a * (1 - b)` in one pass) are an optimization on top of the interface, each with its own bake-off. Expression compilation is T3 (`feature-map.md`).
6. **Strictness is enforced mechanically** (tried; see Evidence): a `clippy.toml` `disallowed-methods` list bans the lenient arrow functions (`add_wrapping`, `sub_wrapping`, `mul_wrapping`, plain `sum`, `cast` and `cast_with_options`); casts go through one `strict_cast` wrapper module that is explicitly allowed to call them with `safe: false`. The kernel crates build with `overflow-checks = true` in release too, and deny `clippy::arithmetic_side_effects`, `cast_possible_truncation` and `cast_possible_wrap`. `nata-check` carries overflow, invalid-cast and guarded-expression cases.
7. **Every optimized kernel has a scalar reference** and tests against it; fast paths are chosen at runtime and logged (`portability.md`).

## Why

- **Semantics.** Arrow's kernels compute every slot and know nothing about selection, so a strict arrow kernel would raise errors for rows the user filtered out. Owning arithmetic and the aggregates is the way to get selection-aware errors without copying first. Arrow's strict behaviour also exists only behind non-default entry points (plain `sum` wraps, `cast` defaults to NULL), hence the lint.
- **Cost.** Measured on a two-operation expression, arrow's two checked kernels run at about 1.9-2.2 ns/row; two own primitive passes at 1.2-1.3 (1.6-1.8x faster, above the 1.5x rule); one fused pass at 0.77. Arrow's `Decimal128` is the outlier (4.5-9.6 ns/row against 1.3-2.7 for own loops).
- For comparisons, filter, strings and dictionaries there is no measurement showing a problem, so the reuse rule says to reuse.
- Keeping dictionary columns saved most of the Parquet decode cost for the string columns in the spike. A batch type that expands them would throw that away.
- The interface in front of the kernels is what makes "replace one at a time" and later fusion cheap.

## Alternatives considered

- **A. arrow-rs kernels everywhere.** Least work and the best reuse. Its `Decimal64` kernels are about as fast as its integer kernels. Lost on selection-aware errors (it would copy to compact before every strict kernel), on the 1.6-1.8x cost, and on `Decimal128`; we would also own a strictness layer anyway.
- **B. Our own kernels for everything from day one.** Maximum control. Lost: no evidence that comparisons, strings or filters need it, and it delays every slice.
- **C. A different batch layout (DuckDB-style flat, constant, dictionary vectors).** More encodings are possible. Lost: the vision fixes Arrow as the interop format and the Parquet reader produces Arrow, so a second layout means a conversion at every edge.
- **D. Floating point for decimals.** Fast and simple. Lost: results change with thread count and are not exact.

## Evidence

**Measured** (`bench/results/20261103-kernels-arrow-rs.md`, `bench/results/20261103-pipeline-model-spike.md`; x86-64 only, shared VM; kernels single thread, 8192-row batches, synthetic data, no selection vector):

- `p * (100 - d)`, ns/row: arrow checked `i64` 1.9-2.5; arrow `Decimal64` 1.9-2.2 (result type `Decimal64(18, 4)`); arrow `Decimal128` 4.5-9.6 (varies across builds of the same code, cause unknown); own two passes 1.2-1.3; own one fused pass 0.74-0.78; own narrowed to `i64` and building a `Decimal128(31,4)` array 1.3-1.4 (the output width an exact `p1 + p2` multiplication rule would have needed; the adopted rule keeps 64-bit operands at 64 bits, which is the 0.74-0.78 path); own `i128` loop 2.7-3.4. Allocating the output per batch cost nothing extra (0.74 against 0.75-0.78).
- With 10% NULLs: arrow 2.4-2.7, an own loop that branches on validity 3.1-3.5 (worse than arrow), a branch-free own loop 1.6-1.7.
- Semantics: plain `sum` wraps silently, default `cast` returns NULL, `*_wrapping` wraps; the checked variants error.
- In the Parquet spike: decoding the two string columns took 80-85 ms as plain strings and 13-14 ms as dictionary arrays; converting `i128` decimals to `f64` cost about 70 ms of a 100 ms project step; exact integer sums matched DuckDB digit for digit; filtering by copying cost about 40 ms, and about 21 ms once the strings were dictionaries.
- Strictness lints (`bench/results/20261103-lint-check.md`; arrow 60.1, clippy 1.99.1): function-level bans work, including through re-exports; `arithmetic_side_effects`, `cast_possible_truncation` and `cast_possible_wrap` flag plain `+`, `*` and truncating `as`; `overflow-checks = true` makes release `i32::MAX + 1` panic, while the Cargo default wraps silently. `CastOptions::default()` and a literal `safe: true` cannot be flagged, which is why both cast functions are banned instead.

**Reasoned:** selection-aware error semantics (a SQL requirement, but the interaction with arrow kernels is argued from how they compute, not measured); the selection-vector design and its compaction cost; `Decimal64` for precision up to 18 (the kernel micro-benchmark shows `i64` storage is faster than `i128`, but end to end the reader returning `Decimal64` made no difference beyond noise, because Parquet decode dominated); client-edge widening (Postgres wire `NUMERIC` and Arrow Flight SQL client support for `Decimal64` was not checked); the 5% and 1.5x thresholds; and that `clippy` `disallowed-methods` can express every ban in decision 6.

**Not covered:**

- ARM64: the kernel benchmark ran once, on one quiet Neoverse-V2 VM (`bench/results/20261104-arm64-run2-benchmarks.md`); nothing else (selection-aware kernels, real data) did. By source inspection arrow-rs kernels have no architecture-specific code and no NEON path (they rely on LLVM autovectorization), so there is no fast path to lose, and no measurement of what LLVM produces there. The `parquet` crate has none either.
- Real column data, multiple threads, strings, comparisons, filters, casts, joins, expressions wider than two operations, and kernels that take a selection vector. The own loops are lower bounds.

## Confidence

- **Arrow's strict and lenient behaviour: high.** Measured on the version in use.
- **Selection-aware errors as a requirement: medium-high.** Standard SQL, argued from how arrow computes.
- **Own arithmetic and aggregate kernels: medium.** The reason is semantics first and a measured speed gain second: owning the primitive kernels is 1.6-1.8x faster than arrow's two checked kernels on x86-64 and 1.4-1.5x on ARM64 (right at the 1.5x rule), and fusing adds 1.5-1.7x (x86-64) or 1.8-1.9x (ARM64). The speed figures are micro-benchmarks without selection vectors or real data.
- **Reuse arrow-rs for everything else: medium.** No evidence either way; it follows the reuse rule.
- **The lint and `overflow-checks`: medium-high.** Tried on a scratch crate and confirmed for every function ban and for the arithmetic and cast lints. Open: how noisy `arithmetic_side_effects` is on real kernel code, and lenient arrow entry points not on the list.
- **Selection vector design, `Decimal64`, client-edge widening: low.** Reasoned only.
- Would change if: own kernels with selection and NULL handling lose their 1.5x on real data or on ARM64; the generic-operator experiment shows the cost of generality outweighs the gains; or a newer arrow-rs ships selection-aware or faster checked kernels.

## Consequences

- We own a thin kernel layer, its strictness tests, and the lint configuration from slice 2.
- A branch-free NULL strategy is part of every own kernel's design. The naive version is a regression.
- Every arrow-rs wrapper that cannot take a selection compacts first; that copy is unmeasured and may be the first thing to replace.
- The decimal rules are written (`docs/decimal-rules.md`); the kernels implement them, and `check/queries/decimal*` are the tests. The rules keep `price * (1 - disc) * (1 + tax)` on `DECIMAL(15,2)` columns at `DECIMAL(18,6)`, so the checked `i64` path measured here applies; they make division exact decimal, which needs a wider accumulator for `AVG`.
- Parquet scans must be able to hand back dictionary arrays and the narrowest decimal type, which may need our own page decoding later (the `parquet` crate expands dictionaries for some types).
- When dictionary-aware operators arrive (T2) they need a cap on the product of key cardinalities, with a hash fallback above it.
- Sums stay exact: `i128` accumulators for decimals, overflow is an error.

## Revisit when

- **Before acceptance:**
  1. ~~Run the kernel benchmark on ARM64~~ done: `bench/results/20261104-arm64-run2-benchmarks.md`; conclusions hold with smaller margins.
  2. Run the lints on a real kernel crate (the scratch trial is done); judge the noise of `arithmetic_side_effects`.
  3. Write the guarded-expression tests (`WHERE x <> 0 AND 1 / x > 1`, `CASE`) and the selection-aware error behaviour of own and wrapped kernels.
  4. ~~Write the decimal rules~~ done: `docs/decimal-rules.md`, with the queries in `check/queries/decimal*` (they report `UNSUPPORTED` until the engine exists).
  5. Run the generic-operator experiment: TPC-H Q1 plus a selective query (Q6) through generic filter, project and aggregate operators with selection vectors, NULL handling and a general hash aggregate. The gap to the hand-fused spike is the cost of generality, which this ADR has not measured.
  6. Re-review the evidence.
- When arrow-rs releases a new major version, or when a profile of a benchmark query puts an arrow-rs kernel at the top of the self-time list.

## Revision notes

- 2026-11-04, kernel benchmark on ARM64 (Neoverse V2): semantics identical; the case for owning arithmetic kernels holds, but the speed margin over arrow's two checked kernels is 1.4-1.5x there (x86-64: 1.6-1.8x), at the 1.5x threshold, so the justification rests mainly on semantics (selection-aware errors); fusing helps more on ARM64; arrow's Decimal128 outlier is an x86-64 effect (1.1x slower than Decimal64 on ARM64, 2.0-2.4x on x86-64); the branch-free NULL requirement matters on x86-64 (a per-row branch was 1.1-1.4x worse than arrow) and not on ARM64. Open check 1 closed.

- 2026-11-03, decimal rules: written as `docs/decimal-rules.md` after an independent review; multiplication follows DuckDB's width-class cap instead of exact `p1 + p2`, because the exact rule would have made the whole Q1 expression chain `Decimal128`; the earlier remark that arrow's `Decimal64` cap is unusual was wrong (DuckDB caps the same way).
- 2026-11-03, lint trial: the `disallowed-methods` bans, the arithmetic and cast lints and `overflow-checks` were tried on a scratch crate and work as listed, except that `CastOptions::default()` cannot be banned, so both cast functions are banned instead and casts go through one wrapper. Lint confidence raised from low to medium-high.
- 2026-11-03, after independent review, with two new measurements:
  - Added decision 4 (errors only for selected, valid rows) and the compaction rule; the reviewer's main finding, checked against the kernel code (the branch-free loop masked overflow by validity only).
  - Lowered the lint confidence from high to low, narrowed the cast ban to `arrow::compute::cast`, and added `overflow-checks` and the arithmetic lint.
  - New measurements: arrow's `Decimal64` kernels are about as fast as its integer kernels, so the first draft's "decimals are special, 5-10 ns/row against 0.8" was mostly arrow's `Decimal128` and storage width; own two-pass kernels are 1.6-1.8x faster than arrow, fusing adds another 1.6x; output allocation does not matter. The "Why" now rests on semantics first and a 1.6-1.8x cost second.
  - Dictionary-aware operators moved to T2 and out of slice 2; the decimal rules moved before slice 2; `Decimal32` and precision above 38 stated; client-edge widening listed as unverified.

## Amendments

None yet.
