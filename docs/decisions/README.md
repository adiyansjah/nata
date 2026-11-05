# Decisions

One file per architecture decision, `NNNN-title.md`, copied from `0000-template.md`.

| ADR | Title | Status |
|---|---|---|
| [0001](0001-pipeline-model.md) | Pipeline model | proposed |
| [0002](0002-async-runtime-and-threading.md) | Async runtime and threading | proposed |
| [0003](0003-batch-format-and-expression-kernels.md) | Batch format and expression kernels | proposed |
| [0004](0004-distributed-execution-and-failure-model.md) | Distributed execution and its failure model (draft) | proposed |
| [0005](0005-table-format-and-catalog.md) | Table format and catalog, Iceberg first (draft) | proposed |
| [0006](0006-testing-strategy.md) | Testing strategy: differential, mutation-checked, deterministic simulation (draft) | proposed |

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
