# Verification plan for the proposed ADRs

Written 2026-11-04. Every item is a test, proof or measurement (way-of-working.md, section 6); none is a review. **Gate** = wanted before the ADR is accepted. **Later** = goes under "revisit when". A check counts only if it was shown to fail on a known-bad version.

## ADR 0001: pipeline model

| # | What to verify | How | Cost | Priority | Status |
|---|---|---|---|---|---|
| 1 | The scheduler never loses a shutdown or wake, never runs a task twice, never hangs on cancel | loom model, checked to fail on the old bug and on wake-protocol mutants | local | Gate | **done within bounds** (`bench/results/20261104-loom-scheduler.md`); exchange and early finish not modelled |
| 2 | The protocol scheduler runs Arrow batches with selection vectors and returns DuckDB's results | differential test | local | Gate | **done on x86-64** (`bench/results/20261104-arrow-pipeline.md`); found and fixed the stop-depth bug; loom re-run and speed pending |
| 3 | `LIMIT` and early finish stay correct for random plans | property test against a simple reference | local | Gate | **done** (1,500 random cases, 33 of 33 mutants caught) |
| 4 | Exchange backpressure stays bounded | loom or property test | local | Later | **done within bounds** (`bench/results/20261104-exchange-loom.md`), found a cancel leak (fix pending) |
| 5 | Push cost with much cheaper kernels, static dispatch, branching pipelines | paired benchmark with a control arm | local, ARM64 about $0.01 | Later | not started |

## ADR 0002: async runtime and threading

| # | What to verify | How | Cost | Priority | Status |
|---|---|---|---|---|---|
| 6 | The tokio-to-CPU-pool waker bridge loses no wakeup and does not deadlock when the pool is saturated | loom or shuttle, plus a contention stress test | local | Gate | **done within bounds** (`bench/results/20261104-async-bridge.md`); latency and stress to re-run on a quiet machine |
| 7 | A busy CPU pool does not stall I/O, and a slow object store does not flood memory | measurement with a mock slow `object_store` | local | Gate | **done within bounds** (`bench/results/20261104-async-bridge.md`) |
| 8 | `object_store`, `crossbeam-deque` and the compression crates build and test on x86-64 and ARM64 | CI on GitHub's ARM runner | free (public repo) | Gate | needs a commit and push to run |
| 9 | Pool sizing respects cgroup CPU limits | test in a CPU-limited container | local | Later | not started |
| 22 | Fairness: a CPU-heavy query must not starve a concurrent small query or a consumer task (time slice or one morsel per run) | measurement with a starvation control | local | **Gate** (new, from item 6) | **done within bounds** (`bench/results/20261104-fairness.md`); overhead on a quiet machine still to measure |
| 10 | Split design against all-on-tokio | slice 11 bake-off | later | Later | not started |

## ADR 0003: batch format and kernels

| # | What to verify | How | Cost | Priority | Status |
|---|---|---|---|---|---|
| 11 | Kernels never overflow silently, mask errors by selection and validity, and sum exactly | Kani bounded proofs, mutation-checked | local | Gate | **done**: 62 of 62 harnesses, and the proofs are mutation-checked 15 of 15 (`bench/results/20261104-kani-kernels.md`) |
| 12 | The lints catch every banned lenient arrow call; `arithmetic_side_effects` noise on a real crate | negative-test file that must fail clippy; noise count | local | Gate | **done** (`bench/results/20261104-kernel-lints-and-guards.md`) |
| 13 | `WHERE x <> 0 AND 1/x > 1` and `CASE`-style guards work, including through wrapped arrow kernels | differential test against DuckDB | local | Gate | **done** (`bench/results/20261104-kernel-lints-and-guards.md`) |
| 14 | The decimal rules are right in type and value | exhaustive small-precision enumeration against an independent big-integer reference, plus DuckDB | local | Gate | **done, full run on GCP** (`bench/results/20261104-decimal-verification.md`: 56 of 56 mutants, 0 failures) |
| 15 | Joins and a general hash aggregate (many distinct keys, plain strings) cost about what the experiment showed | build both, differential test, benchmark | local, then ARM64 | Biggest evidence gap | **correctness done**: 30 of 30 mutants caught on a clean ARM64 VM, baseline green in every worker (`bench/results/20261104-hash-ops-correctness.md`); **still open: the timings and memory** (job `hashops`, with `overflow-checks` on and off), and that the `nullcheck` and DuckDB layers can fail on a bad engine |
| 16 | The NULL path can drop below 1.3x | word-wise bitmap experiment | local | Later | not started |
| 17 | Real clients accept `Decimal64` (Arrow IPC / C data interface; Postgres `NUMERIC` later) | test with pyarrow, DuckDB, polars | local | Later | **done** (`bench/results/20261104-decimal-clients.md`): Arrow-native clients accept it, JDBC rejects it so Flight SQL widens to Decimal128; Postgres `numeric` encoder byte-identical to PostgreSQL 16.2 |
| 18 | Compaction before arrow wrappers: what the copy costs | micro-benchmark | local | Later | not started |
| 21 | Re-measure generic-ops and hash-ops with `overflow-checks` on (workspace-wide) | rebuild with the profile, paired benchmark | local, then GCP x86-64 and ARM64 | **Gate** (new, from item 12) | generic-ops done on ARM64 (small effect, `bench/results/20261104-arm64-verify-run.md`); hash-ops in the `hashops` GCP job; x86-64 not done |
| 19 | Why Q1 is further from the fused loop on ARM64; does the V2 core differ | `perf` profile, C4A run | GCP about $0.01, needs a yes | Later | not started |
| 20 | Results hold on other data: skewed keys, string-heavy, many NULLs, TPC-DS | extra data sets through the same checks | local | Later | not started |

## How the work is grouped

Agents run at the same time only when their edit scopes are disjoint and none depends on another's unfinished code.

| Agent | Items | Edit scope |
|---|---|---|
| K1 (running) | 11 | `spikes/generic-ops/kani/` |
| L1 (done) | 1 | `spikes/operator-trait/` |
| A (done) | 12, 13 | new `crates/nata-kernels/` (copy of the spike kernels, lints on) |
| B (done) | 6, 7 | new `spikes/async-bridge/` |
| C (done, full run on GCP) | 14, 17 | new `spikes/decimal-verify/` |
| D | 15 | new `spikes/hash-ops/` |

Deferred until K1 and L1 finish, because they would edit the same files: 2, 3, 4, 16, 18. Item 8 needs a push. Items 5, 19 need a cost approval if they use GCP.

Benchmarks on the shared 8-vCPU VM are disturbed by the other agents. Correctness first; timings use interleaved arms with a control and are run when `uptime` shows a low load, and the report states the load.
