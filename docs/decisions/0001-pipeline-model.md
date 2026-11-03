# 0001: Pipeline model

Status: proposed
Date: 2026-11-03

## Context

Every operator is written against this model, so it is the most expensive decision to reverse. What it must support, from `vision.md` and `feature-map.md`:

- morsel-driven parallelism with work stealing;
- an exchange abstraction from day one, so single-node and distributed execution share one operator interface;
- window functions over whole tables or partitions, with partitioned parallelism and spill;
- per-query memory budgets, spill, and cancellation that frees memory and tasks.

## Decision

Hybrid: push-based inside a node, stream-based between fragments.

- **Inside a fragment:** the plan is cut at pipeline breakers (hash build, sort, aggregate, window). Each pipeline is source -> push transforms -> sink. Worker threads take morsels from a shared queue and push them through the pipeline into thread-local sink state. Breakers have a combine/finalize step that runs once all morsels are done.
- **Everything is synchronous, and blocking lives at the edges.** Sources and sinks (an empty or full exchange, pending I/O) return `Blocked` after storing the waker they were given; the scheduler parks that task and re-queues it when the waker fires. Intermediate operators never block: they return `NeedMoreInput`, `HaveMoreOutput` (one input needs several calls, as in a join with fan-out) or `Finished` (no more output ever, for example `LIMIT`). Waiting for another pipeline (a join build) is a dependency between pipelines, not a blocked operator. A worker thread never waits inside any of them. A source has a `finish()` hook called when its task ends, so an exchange consumer can tell its producers nobody is reading. See 0002.
- **Early finish travels three ways:** up the pipeline (pending upstream input is dropped), sideways (the other tasks of the pipeline stop and parked ones are woken) and across an exchange (the consumer closes the channel).
- **Between fragments:** an `Exchange` interface, an async stream of batches with backpressure. On one node it is an in-memory channel; later the same interface is backed by network shuffle and object-storage spill. Batches are compacted (selection applied) before they enter an exchange.
- **Operators may be fused** inside a pipeline, and pass selection vectors rather than filtered copies (see Consequences, and 0003).
- Morsel size (a row group or a smaller slice) is decided in the Parquet scan slice.

## Why

- Work stealing, per-worker state, spill and cancellation are natural when workers push morsels through a pipeline and own their sink state. With pull iterators they have to be added around the iterators.
- The vision requires the exchange from day one. A defined boundary between fragments is where distribution, network shuffle and spill plug in later without touching operators.
- Whole-table window functions with spill are the first real workload (`vision.md`); partitioned parallel execution with a combine step fits the push design.
- Precedent: DuckDB executes push-based, morsel-driven pipelines (several explainer sites; not checked against DuckDB's own docs), and Presto/Trino's `Operator` interface (`needsInput`, `addInput`, `getOutput`, `isBlocked` as a future) has the same states listed above (confirmed in its Javadoc). A Velox issue quotes a matching `isBlocked`; Velox's source was not read. See `bench/results/20261103-lint-check.md`.

## Alternatives considered

- **A. Pull-based Volcano iterators (DataFusion style).** Simplest to write and reason about. Parallelism comes from static partitioning plus repartition operators; work stealing, backpressure and spill coordination must be added around the iterators. Lost on the requirements above, not on speed.
- **B. Push-based pipelines with no explicit exchange interface.** Same CPU behaviour as the decision, but the distributed phase would retrofit the boundary into every operator. Lost because the vision requires the exchange from day one.

## Evidence

**Measured** (`bench/results/20261103-pipeline-model-spike.md`; one query shape, TPC-H Q1 at SF1; x86-64 only; a shared VM that drifts 10-30% between sessions):

- *First comparison, superseded by the next bullet:* Pull/static and push/morsel both return DuckDB's result. They are equal within noise only at 4 threads (193 / 210 against 189 / 211 ms, min / median). At 1 thread push was 11% slower (min) and 17% (median); at 2 threads 20% (min) and 10% (median). Cause unknown. The 8-thread row is invalid: pull/static ran with 7 threads (see the review corrections in the results file).
- The comparison changed two things at once, scheduling (static partitions against a shared morsel counter) and dispatch style (pull iterators against push transforms). Pull with a morsel queue was never run. The runs were not interleaved.
- **Corrected comparison** (`bench/results/20261104-pull-vs-push-corrected.md`; x86-64 only, one shared VM): pull with the **same morsel queue** as push, exactly the requested thread count, interleaved with a pull-against-pull control, optimized operators, 25 rounds, batch sizes 8192, 1024 and 256, 1 to 8 threads. Push/pull median paired ratio 0.971-1.020 in all 12 cells; the control 0.989-1.008, with quartiles of about ±3-5%. At 256-row batches (about 11 microseconds per batch) any dispatch difference is under about 2%, so under roughly 0.2 microseconds per batch. The earlier "push is 10-20% slower at 1-2 threads" does not reproduce; its likely cause is run order, which that comparison could not exclude.
- Float sums changed in the last digits with thread count. The v2 path with exact integer sums returned DuckDB's result digit for digit.
- Fusing filter, project and aggregate in one loop with no filtered copy was part of a speedup of about 2.9x (371 to 129 ms single thread, same session); the other parts were dictionary-preserving strings, exact integer arithmetic, dense group accumulation and 64-bit decimals from the reader, and their individual shares were not isolated.
- Eight threads in one process reached about 4x one thread, eight independent processes about 6x. The gap is partly the tail (49 row groups over 8 threads is 7 rounds, at best 87% efficiency); the rest is not explained.

- **Protocol prototype** (`bench/results/20261103-operator-trait-prototype.md`; std only, no Arrow, x86-64 only): a scheduler with parking and wakers plus a source/operator/sink protocol expressed all four hard cases and passed tests for each: `LIMIT` exact across 8 tasks, with the scan stopping after 8-10 of 10,000 morsels; a hash join equal to a reference, including 400 matches per probe row; an exchange consumer scheduled first on **one** worker thread without deadlock, and an 864-query stress; an aggregate spilling 50,000 entries within a 200-group budget (80 groups in the largest merged partition), result equal to a reference. Cancellation and an error in one task stop the rest. Nine deliberate breakages of the scheduler and operators were each caught by a test.
- **Corrected comparison on ARM64** (`bench/results/20261104-arm64-run2-benchmarks.md`; one quiet `c4a-highcpu-8` Spot VM, Neoverse V2, 8 vCPU; same method, 25 rounds, three batch sizes, 1 to 8 threads): push/pull median paired ratio 0.978-1.012 in all 12 cells, the pull-against-pull control 0.997-1.010, quartiles about ±1%. At 256-row batches the ratio is 0.999-1.001. One cell shows push 2.2% faster and does not repeat elsewhere.
- **ARM64 test run** (`bench/results/20261104-arm64-run1-operator-protocol.md`; one `n4a-highcpu-8` Spot VM, Neoverse N3, 8 vCPU): the whole protocol suite, including the deterministic shutdown test, passed in 6 of 6 repetitions, with no failure, panic or hang. One machine and six repetitions is modest evidence for a weaker memory model.
- That prototype also found a real scheduler bug (a lost shutdown notification) that 10 of 10 idle-machine runs missed and that hung most runs under CPU contention; fixed, and covered by a deterministic test.
- Protocol cost on one-row batches: about 190 ns per batch without blocking, 630-800 ns per batch through a capacity-1 exchange where nearly every batch parks and wakes a task. About 2-10% of the work in an 8K-row batch if a kernel costs about 1 ns per row (an estimate, not measured).

**Reasoned:** skew handling, window functions and the exchange boundary. These are reasons for the choice and none was measured. The precedent in Why is confirmed only through secondary sources for DuckDB and Velox.

**Not covered:**

- Skew and window functions. The prototype covers join, `LIMIT`, a blocking exchange and spill with plain `i64` batches, not Arrow batches, selection vectors or fused operators.
- A parallel `finalize`: the prototype's runs on one worker and blocks it on file reads, while the others idle.
- A query-level memory budget: the prototype's is per task (a query holds up to tasks x budget), and its merge step is bounded by partition size, which skew would break.
- A network or object-storage exchange fed by async tasks.
- Work stealing: the prototype uses one FIFO queue under one lock.
- ARM64 performance of anything other than the pull-versus-push ratio (see `portability.md`, section 7). The prototype's tests and the pull-versus-push comparison did run on ARM64 (see Evidence).
- Pull versus push with **cheaper kernels** or a **fused single-loop pipeline** (where dispatch would be a larger share), with **static dispatch**, and on **branching pipelines**. The corrected comparison covers one linear chain with Arrow kernels that allocate per batch, on x86-64.

## Confidence

- **Decision: medium.** It rests on requirements and design reasoning that match `vision.md` and common practice, not on the spike.
- **The protocol can express join, `LIMIT`, blocking exchange and spill: medium-high.** Shown in a prototype with deliberate-breakage checks, on x86-64 and (tests only) ARM64, and without Arrow.
- **"Push costs little" (dispatch overhead on x86-64 and ARM64, linear chain, Arrow kernels): high.** On ARM64 the quiet machine resolves differences of about 1%, and finds none. The corrected comparison finds no difference beyond about ±3-5% at any batch size or thread count, down to 256-row batches. Not shown for ARM64, cheaper kernels or branching pipelines.
- Would change if: the prototype's protocol proves unable to carry Arrow batches with selection vectors and fused operators; or the same comparison with much cheaper kernels, static dispatch or branching pipelines shows a clear cost for push.

## Consequences

- Operators are state machines with local state per worker and a combine step. The operator trait is designed in slice 2 and must express: need more input, has output, finished, blocked (with a waker).
- **Fusion.** The interface must allow adjacent operators in a pipeline to be fused into one loop, and operators pass **selection vectors** instead of copying filtered data.
- **Morsel size** may need to be finer than a row group, to avoid the tail effect seen in the spike. To be tested in the scan slice, not assumed.
- **Determinism.** With dynamic morsel assignment the order in which partial results merge differs from run to run, so floating-point sums can differ between runs even at a fixed thread count (static partitioning did not have this). Decimal and integer sums are exact. For floats, `nata-check` uses a tolerance (`portability.md`, section 5), or merges partial results in morsel order.
- Cancellation and memory checks happen at morsel boundaries.
- **Concurrency tests include load.** The scheduler and every channel or waker are tested under CPU contention and with injected delays (`way-of-working.md`, section 2); the prototype's first suite missed a hang that appeared only under load.
- `finalize` should be able to run as several tasks (parallel finalize), and spill, which is blocking disk I/O, must be decided: allowed on the CPU pool, or made a `Blocked` source or sink (see 0002).
- Memory is a query-level budget with reservations, not a per-task one (`feature-map.md`, tier T0).
- Hand-offs between workers use channels or `Mutex`. Any atomic ordering is written down and tested on ARM64 (`portability.md`, section 3).
- Async I/O sits at the source edge; see 0002.

## Revisit when

- **Before acceptance:**
  1. ~~Prototype the operator trait against the hard cases~~ done: `bench/results/20261103-operator-trait-prototype.md`. ARM64 tests: done (`bench/results/20261104-arm64-run1-operator-protocol.md`). Remaining from it: rebuild it on Arrow batches with selection vectors and a fused operator.
  2. ~~Re-run pull versus push correctly~~ done for x86-64: `bench/results/20261104-pull-vs-push-corrected.md`. Also done for ARM64: `bench/results/20261104-arm64-run2-benchmarks.md`. Open check 2 is closed.
- At the window function slice (slice 8), the first real test of partitioned parallel windows with spill.

## Revision notes

- 2026-11-04, pull versus push on ARM64 (quiet Neoverse-V2 Spot VM): no difference beyond about 1% in any of 12 cells; "push costs little" raised to high for dispatch overhead on a linear chain with Arrow kernels on both architectures; open check 2 closed. Limits that remain: cheaper kernels, static dispatch, branching pipelines.

- 2026-11-04, ARM64 run of the operator-protocol suite: 6 of 6 repetitions passed on a Neoverse-N3 Spot VM; the "run its tests on ARM64" part of open check 1 is done. Pull versus push on ARM64 remains.

- 2026-11-04, corrected pull-versus-push comparison (x86-64): no difference detectable beyond about ±3-5% at any batch size or thread count, so the earlier 10-20% gap is withdrawn as unexplained and probably run-order drift; "push costs little" raised from low to medium-high for x86-64; open check 2 reduced to the ARM64 run.

- 2026-11-03, operator-protocol prototype: all four hard cases expressed and tested. Changes to the text: blocking is reported by sources and sinks, not operators; `HaveMoreOutput` and `Finished` are the operator results; sources have a `finish()` hook; early finish travels up, sideways and across an exchange; added consequences for parallel finalize, spill I/O, query-level memory and load testing; open check 1 marked done with two follow-ups. The prototype found and fixed a lost-shutdown-notification bug in its own scheduler.

- 2026-11-03, after independent review: corrected the evidence (the 8-thread pull row used 7 threads; the 2-thread row was left out; the two-variables-at-once design; the mixed-session speedup, now 2.9x; "exact sums also faster" was never isolated); lowered the confidence in "push costs little" and withdrew "no measurable cost"; dropped "little differentiation" as a reason against pull; added synchronous operators with `blocked` and a waker, compaction before an exchange, float determinism, and a replacement open check (the operator-trait prototype, plus a corrected comparison); added precedent, then checked it on the web (DuckDB and Velox only through secondary sources; Presto's `Operator` interface confirmed).

## Amendments

None yet.
