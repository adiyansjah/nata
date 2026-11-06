# Decisions

One file per architecture decision, `NNNN-title.md`, copied from `0000-template.md`.

| ADR | Title | Status |
|---|---|---|
| [0001](0001-pipeline-model.md) | Pipeline model | accepted 2026-11-05 |
| [0002](0002-async-runtime-and-threading.md) | Async runtime and threading | accepted 2026-11-05 |
| [0003](0003-batch-format-and-expression-kernels.md) | Batch format and expression kernels | accepted 2026-11-05 |
| [0004](0004-distributed-execution-and-failure-model.md) | Distributed execution and its failure model (draft, amended 2026-11-05) | accepted 2026-11-06 (by the assistant under the owner's delegation; condition met: stage B passed) |
| [0005](0005-table-format-and-catalog.md) | Table format and catalog, Iceberg first | accepted in part 2026-11-05 (rules 1-7; rule 8 open) |
| [0006](0006-testing-strategy.md) | Testing strategy: differential, mutation-checked, deterministic simulation | accepted 2026-11-05 |
| [0007](0007-sql-dialect-and-strictness.md) | SQL dialect and strictness (the subset of slice 4; `docs/dialect.md`) | accepted 2026-11-06 (by the assistant under the owner's delegation) |
| [0008](0008-client-protocol.md) | Client protocol (Postgres wire 3.0; checked hand codec by R1) | accepted 2026-11-07 (by the assistant under the owner's delegation) |

0001-0003 and 0006 were accepted on 2026-11-05 with checks still open, and 0005 in part (rules 1-7); each ADR's revision note of that date lists them. 0004 was accepted on 2026-11-06 by the assistant under the owner's delegation, when stage B of plan item 28 passed (its acceptance condition). The table below is kept as it stood at acceptance.

Before a proposed ADR is accepted (see `way-of-working.md`, section 6):

| ADR | Open check |
|---|---|
| 0001 | Operator-protocol prototype: **done** (`bench/results/20261103-operator-trait-prototype.md`). ARM64 tests: **done** (`bench/results/20261104-arm64-run1-operator-protocol.md`). Remaining: rebuild the scheduler prototype on Arrow batches with selection vectors (the batch and kernel side was exercised by the generic-operator experiment); a fused operator; pull versus push, corrected: **done on x86-64 and ARM64** (`bench/results/20261104-pull-vs-push-corrected.md`, `bench/results/20261104-arm64-run2-benchmarks.md`); re-run the verification |
| 0002 | Check x86-64 and ARM64 builds and fast paths for `object_store`, `crossbeam-deque` and the compression crates; re-run the verification |
| 0003 | Kernel benchmark on ARM64: **done** (`bench/results/20261104-arm64-run2-benchmarks.md`); lints on a real kernel crate; guarded-expression and selection-aware error tests; generic-operator experiment (Q1 + Q6): **done on x86-64 and ARM64** (`bench/results/20261104-generic-ops-x86.md`, `bench/results/20261104-generic-ops-arm64.md`); re-run the verification (Kani proofs and loom model are in progress). Decimal rules: done (`docs/decimal-rules.md`) |

Still needed, from the open questions in `feature-map.md`:

| # | Question | Needed before |
|---|---|---|
| 5 | Strictness spec. **Decimal rules: decided** (`docs/decimal-rules.md`; tests in `check/queries/decimal*`). Still open: coercion between other types, NULL, ordering, `TRY()` | Slice 4 |
| 2, 4, 6, 7 | Cluster state, shuffle, statistics, decorrelation | Phase 3 |
