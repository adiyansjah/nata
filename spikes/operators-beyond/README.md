# operators-beyond (throwaway spike, verification-plan item 31)

Question: **does the operator protocol of ADR 0001 (unchanged, `src/proto/`, the snapshot used by `arrow-pipeline`) carry sort and top-N with
spill, outer/semi/anti joins, window functions, TPC-H Q18 and Q21?** If not, the protocol changes now, not after the API is public.

| Path | What |
|---|---|
| `src/proto/` | the protocol and scheduler, UNCHANGED (snapshot of `arrow-pipeline/src/proto`) |
| `src/sort.rs` | ORDER BY (sink with spill + k-way merge source, merge passes bound the open files) and top-N |
| `src/join.rs` | hash join: inner, left, right, full, semi, anti (+ a `<>` residual); in memory, and partitioned (grace) with spill |
| `src/agg.rs`, `src/window.rs` | hash aggregation (not spillable); window (row_number, rank, dense_rank, sum over partition, lag) |
| `src/queries.rs`, `src/bin/opsbeyond.rs` | pipelines for Q18, Q21, a Q13 variant (no LIKE), join-kind counts, lineitem sort, top-N, two window queries |
| `tests/ops.rs` | differential tests against naive references: empty inputs, one row, all-NULL keys, duplicates, NULL residuals, output larger than a batch, spill with a tiny budget, no leftover files |
| `check.sh`, `sql/` | DuckDB differential check of every query (multiset equality, row-by-row order check on the order-by columns, spill modes, no leftover spill files) |
| `mutate.py` | 21 mutants of the operators; each must be CAUGHT by `tests/ops.rs` |
| `gcp-check.sh` | everything on a clean VM (job `opsbeyond`) |

## Status (2026-11-04)

Run on the dev box (8 vCPU, shared) within the owner's 10-minute limit for local heavy work:

| What | Result |
|---|---|
| `cargo test --release` (8 tests, thousands of join cases) | all pass |
| `check.sh` on SF0.01 and on SF1, `--quick` (13 queries in memory; 6 join kinds + Q13 partitioned with 32 partitions and a 64 MB budget; sort with spill, fan-in 4 and 2, one task; windows with spill) | all comparisons pass; Q18 57 rows, Q21 100 rows, sort 6,001,215 rows identical to DuckDB; sort spills about 1.2 GB (merge passes included) with at most 5 files open |
| `mutate.py` | **21 of 21 CAUGHT** (4 minutes) |

NOT yet run: the non-quick part of `check.sh` (1, 2 and 8 tasks for Q18 and Q21), a clean ARM64 or x86-64 VM (`gcp-check.sh`, job
`opsbeyond`), and any timing claim. The DuckDB times that `gcp-check.sh` records are informational, one warm run, not a benchmark.
Scale (SF10 and up, larger than memory with many partitions) is item 32.

## What the protocol could and could not express (the answer to item 31)

**Fits without change**
- Sort, top-N, join build, aggregation: a SINK that gathers state, and a SOURCE in a dependent pipeline that streams the result. The
  scheduler's `finalize`-then-start-dependents order is exactly what is needed.
- Spill: sinks and sources may do synchronous file I/O (the protocol says `finalize` may be long). A shared per-query `Budget` (outside
  the protocol) did the accounting. `Source::finish()` is the right place to delete spill files when a task ends early.
- A probe with more output than one batch: `HaveMoreOutput` works (tested with 20,000 output rows from a few input batches).
- Semi/anti joins with a residual predicate (Q21's EXISTS and NOT EXISTS) as ordinary probe operators.

**Does not fit, and the workaround used (each one is a candidate change to ADR 0001)**
1. **Operators have no "input finished" call.** Anything that must emit after its last input cannot be an operator: the window function
   (it must flush the last partition) became a SOURCE fused with the sort that feeds it; Right and Full joins emit the unmatched build
   rows from a source of a later pipeline. Proposal: `Operator::finish(&mut self, out) -> OpResult`, called once after the last input,
   with the same `HaveMoreOutput` convention. It touches every operator, the scheduler and the loom model.
2. **A sink shared by two pipelines is finalized twice** (once per pipeline), so a UNION, or the outer join's second stream, cannot feed
   the downstream sink directly. Workaround: both feed a queue (`QueueSink`) that a third pipeline reads, which costs a full
   materialization. Proposal: a sink that declares how many pipelines produce into it, finalized once.
3. **Ordered streams are serial.** The merge and the window source run as ONE task; the window is not parallel across partitions. Parallel
   windows need hash partitioning (an exchange) first.
4. **Output order is not preserved** across tasks (so ORDER BY must be the last blocking step; a stable sort is stable only per input order).
5. **Memory accounting is not part of the protocol.** The spike passes an `Arc<Budget>` into every sink. A real design needs a query memory
   manager with fair shares and a spill request mechanism, so that one operator can be asked to spill for another.

**Gaps (not protocol limits; known and loud where possible)**
- The aggregate is not spillable (its memory is the number of groups). A global aggregate over an empty input errors instead of returning one row.
- One equi-key (cast to Int64) plus at most one `<>` residual; no multi-column or string keys; `NOT IN` (different NULL rules) not implemented.
- A build side or a partition that does not fit the budget is an ERROR (tested), not a recursive repartition. A partition's join output is
  held in memory before it is emitted.
- A partitioned join opens one file per partition (32 here, most open 60 across both sides): the 1,024-descriptor limit is item 32's question.
- Q13 is the variant without `NOT LIKE '%special%requests%'`; Q18 and Q21 are complete.
- TLC-style confidence only for small random shapes; the TPC-H comparisons are SF1.
