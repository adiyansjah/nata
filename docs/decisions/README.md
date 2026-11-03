# Decisions

One file per architecture decision, `NNNN-title.md`, copied from `0000-template.md`.

| ADR | Title | Status |
|---|---|---|
| [0001](0001-pipeline-model.md) | Pipeline model | proposed |
| [0002](0002-async-runtime-and-threading.md) | Async runtime and threading | proposed |
| [0003](0003-batch-format-and-expression-kernels.md) | Batch format and expression kernels | proposed |

Before a proposed ADR is accepted (see `way-of-working.md`, section 6):

| ADR | Open check |
|---|---|
| 0001 | Operator-protocol prototype: **done** (`bench/results/20261103-operator-trait-prototype.md`). ARM64 tests: **done** (`bench/results/20261104-arm64-run1-operator-protocol.md`). Remaining: rebuild it on Arrow batches with selection vectors and a fused operator; pull versus push, corrected: **done on x86-64** (`bench/results/20261104-pull-vs-push-corrected.md`), ARM64 remaining; re-review |
| 0002 | Check x86-64 and ARM64 builds and fast paths for `object_store`, `crossbeam-deque` and the compression crates; re-review |
| 0003 | Kernel benchmark on ARM64; lints on a real kernel crate; guarded-expression and selection-aware error tests; generic-operator experiment (Q1 + Q6); re-review. Decimal rules: done (`docs/decimal-rules.md`) |

Still needed, from the open questions in `feature-map.md`:

| # | Question | Needed before |
|---|---|---|
| 5 | Strictness spec. **Decimal rules: decided** (`docs/decimal-rules.md`; tests in `check/queries/decimal*`). Still open: coercion between other types, NULL, ordering, `TRY()` | Slice 4 |
| 2, 4, 6, 7 | Cluster state, shuffle, statistics, decorrelation | Phase 3 |
