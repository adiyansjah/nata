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

Loom model check of the scheduler (`bench/results/20261104-loom-scheduler.md`; the real `sched.rs` with swappable sync primitives; 2 workers, 1-2 tasks; written by a delegated agent, and the orchestrator re-ran the default suite, the fixed code under loom, and two of the mutants):
- Shutdown with idle workers always terminates (exhaustive, no preemption bound, 819 and 1.4M executions); a wake that arrives while a task is running is never lost, a task starts only from `Queued`, and cancel racing with completion, an error and a dependent-pipeline start always ends with every task `Done` and an empty queue (preemption bound 3, up to 239k executions each).
- The model can fail: reintroducing the original lost-shutdown bug makes loom report a deadlock (the child reports all 9 tests; the orchestrator re-ran a1 and saw it); four wake-protocol mutants are each caught by at least one test (the cancel-does-not-wake mutant only by one). The default build is unchanged: 15 protocol tests and the shutdown test pass.
- Arrow batches with selection vectors through the protocol scheduler, with `LIMIT` and early finish (`bench/results/20261104-arrow-pipeline.md`; delegated agent; x86-64; the orchestrator re-ran the DuckDB diff and the failing-first test, and the mutation set was run on a clean 32-vCPU ARM64 VM):
- Q1 and Q6 as pipelines on the real protocol (a Parquet source, filter and project operators passing Arrow batches with an `Arc`-shared selection vector, sinks with combine and finalize, an Arrow exchange): identical to DuckDB digit for digit in 12 of 12 configurations (1/4/8 threads, 2-16 tasks), identical to the generic-ops engine, and equal to the independent scalar reference at 10/50/100% NULLs. Built with `overflow-checks` on.
- `LIMIT` and early finish: 1,500 random cases (k=0, exact, larger than the input, empty input, NULLs, 1-8 threads) all give exactly min(k, n) rows of the filtered result; a Parquet scan with `LIMIT 100` reads 1-4 of 49 row groups; 432 configurations stay within `ceil(k/m) + 2 x tasks` of 2,000 morsels; 800 stuck-task cases and 300 mid-flight cancels terminate; 800 queries under busy threads and delay hooks, no timeouts.
- **A real protocol bug found and fixed in the copy:** the pipeline-wide `stop: bool` let a task parked on a blocked sink drop rows its `LIMIT` had already admitted (LIMIT 1 returned 0 rows in 418 of 600 runs). The fix records the stage where each task finished early and keeps a pipeline `stop_depth` (the deepest stage that has finished); a task only drops work upstream of that depth. The failing-first test `a_batch_already_admitted_by_the_limit_survives_a_sideways_stop_while_the_sink_is_blocked` passes on the fix, and the mutant that reintroduces the bug is caught by it (also on the clean ARM64 VM).
- All 33 mutants are caught (32 on the ARM64 VM, the remaining one by 8 tests in the agent's own run). Gaps reported by the agent: computed columns are not Arrow arrays; `LIMIT` keeps whole source batches alive; no fused operator was built; no join, spill or speed evidence; the loom model was NOT re-run on the changed scheduler.

Loom model of the exchange (`bench/results/20261104-exchange-loom.md`; delegated agent; the REAL exchange and scheduler from commit ce5f715 under loom; the orchestrator re-ran the leak test and read the clean-machine logs):
- 23 loom tests with 1-2 workers, capacity 1 (2 in one test), 1-3 items per producer, preemption bound 3 (2 for the three-task tests), 215 to 462,173 executions each: no lost wakeup in the full and empty handshakes (both queue orders), a producer finishing while the consumer is blocked, a LIMIT consumer closing early while producers are blocked, cancel racing with each of these (every task Done, queue empty), items in flight bounded by capacity plus producer and consumer tasks, and no item lost, duplicated or reordered within a producer.
- The model can fail: all 16 mutants of the exchange and the early-finish path are caught (on a clean 32-vCPU ARM64 VM, one mutant per worker; the unmutated control passes all 23 tests). Weak spots: `push_to_front_reorders` is caught by one test, and several mutants by only 3-4 (the agent was asked to add models for them).
- **A real bug found:** a cancelled query leaks. After a cancel, the channel keeps the blocked consumer's waker, and that waker keeps the whole query alive (a reference cycle). `leak.rs::f2` fails every time in plain std runs (5 of 5 re-run by the orchestrator, 20 of 20 by the agent), and the ignored loom test reports `Arc leaked`. The sibling-stop path (`wake_pipeline`) did not leak. **Fixed in the exchange copy and verified:** `close_consumers` takes and drops the stored consumer wakers; `leak.rs::f2` now passes 8 of 8 re-runs by the orchestrator (20 of 20 by the agent), and the loom test `f1` passes in 3,879 executions at bound 3. 20 of the 23 earlier models pass on the fixed code (3 are left for the GCP run), 7 new scheduler-free channel models pass, and a new mutant (`waker_kept_after_close`) is caught by 3 tests. The fix is in the spike copy only; porting it to the prototype exchange in `spikes/operator-trait` is pending.
- Not covered: more than three tasks, preemption bounds above 3, weaker memory orderings than SeqCst, condvar spurious wakeups, errors from `combine`/`finalize`, the rest of `ops.rs`.
Reproduced from a clean tarball on a Spot VM (ARM64, `bench/results/20261104-arm64-verify-run.md`): 9 of 9 models pass in 100 s, and each of the 5 deliberate bugs is caught (4 as a deadlock, one by the never-run-twice assertion). The model checks SeqCst semantics, so this is a reproduction, not new ARM64 evidence.
- Bounds: 2 workers and 1-2 tasks, preemption bound 3 (at bound 4, 5 of the 7 bounded tests finished and passed; two did not finish in 280 s); loom does not model the SeqCst total order or spurious wake-ups; `ops.rs` (the exchange waker protocol), early finish, blocked sinks and finalize errors are not under loom.

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
- **The scheduler's shutdown, wake and cancel protocol is free of lost wakeups and deadlocks: medium-high within the loom bounds** (2 workers, 1-2 tasks); exhaustive for shutdown, bounded for wake and cancel, and not covering the exchange.
- **The protocol can express join, `LIMIT`, blocking exchange and spill: medium-high.** Shown in a prototype with deliberate-breakage checks, on x86-64 and (tests only) ARM64, and without Arrow.
- **"Push costs little" (dispatch overhead on x86-64 and ARM64, linear chain, Arrow kernels): high.** On ARM64 the quiet machine resolves differences of about 1%, and finds none. The corrected comparison finds no difference beyond about ±3-5% at any batch size or thread count, down to 256-row batches. Not shown for ARM64, cheaper kernels or branching pipelines.
- Would change if: the prototype's protocol proves unable to carry Arrow batches with selection vectors and fused operators; or the same comparison with much cheaper kernels, static dispatch or branching pipelines shows a clear cost for push.

## Consequences
- **Early finish carries a depth.** A single `stop` flag for the whole pipeline is wrong: a task that finishes at stage `d` (a `LIMIT`, or the sink) must stop only work UPSTREAM of `d`; work already admitted downstream of `d` must still be delivered. Measured: with a plain flag, `LIMIT 1` returned no rows in 418 of 600 runs when the sink blocked (`bench/results/20261104-arrow-pipeline.md`).

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
  1. ~~Prototype the operator trait against the hard cases~~ done: `bench/results/20261103-operator-trait-prototype.md`. ARM64 tests: done (`bench/results/20261104-arm64-run1-operator-protocol.md`). Remaining from it: rebuild the scheduler prototype itself on Arrow batches with selection vectors (the generic-operator experiment, `bench/results/20261104-generic-ops-x86.md`, ran Arrow batches with selection vectors through separate filter, project and aggregate stages on a simple morsel loop, not on the protocol prototype); a fused operator.
  1a. ~~Run Arrow batches with selection vectors through the protocol scheduler, with `LIMIT` and early finish~~ done on x86-64 (`bench/results/20261104-arrow-pipeline.md`); it found the pipeline-wide stop bug, fixed with a `stop_depth`. Remaining: re-run the scheduler loom suite on the `stop_depth` change; speed comparison with the generic-ops engine on a quiet machine; a fused operator.
  1b. ~~Model-check the scheduler~~ done within bounds: `bench/results/20261104-loom-scheduler.md`. ~~Model the exchange waker protocol~~ done within bounds: `bench/results/20261104-exchange-loom.md`, which found a cancel leak (fix pending). Remaining: more than three tasks and higher bounds on a quiet machine; errors from `combine`/`finalize`.
  2. ~~Re-run pull versus push correctly~~ done for x86-64: `bench/results/20261104-pull-vs-push-corrected.md`. Also done for ARM64: `bench/results/20261104-arm64-run2-benchmarks.md`. Open check 2 is closed.
- At the window function slice (slice 8), the first real test of partitioned parallel windows with spill.

## Revision notes

- 2026-11-04, Arrow batches with selection vectors through the protocol scheduler: Q1 and Q6 identical to DuckDB, `LIMIT` and early finish exact (1,500 random cases), all 33 mutants caught. A real protocol bug found: the pipeline-wide stop flag dropped rows `LIMIT` had admitted; fixed with a per-stage `stop_depth` (now a rule in Consequences). Not yet re-checked under loom; no speed evidence. The main integration risk named in the plan (items 2 and 3) is closed on x86-64.

- 2026-11-04, exchange loom model: 23 models of the real exchange and scheduler (handshakes, early close by a LIMIT consumer, cancel, bounded in-flight items, order) pass, and all 16 mutants are caught. It found a real bug in the prototype: a cancelled query leaks through a waker reference cycle; the fix is pending. Confidence in the exchange protocol is medium-high within the bounds, with the leak as a known defect.

- 2026-11-04, loom model of the scheduler (2 workers, 1-2 tasks): no lost wakeup or deadlock in shutdown, wake-while-running and cancel races; reintroducing the original bug makes the model fail; four more mutants caught. Confidence in the scheduler protocol raised to medium-high within the stated bounds; exchange and early finish still unmodelled.

- 2026-11-04, generic-operator experiment (x86-64): operators over Arrow batches with a selection vector, as separate stages with a morsel-per-row-group loop, cost 1.06-1.12x a hand-fused loop end to end on Q1 and are faster on Q6. This does not test the operator protocol or the scheduler, which stay as in the earlier prototype; it supports the unfused operator-per-stage shape of the pipeline model. x86-64 only here; the ARM64 run (`bench/results/20261104-generic-ops-arm64.md`) showed the same direction with a larger unselective-query gap (1.20-1.29x).

- 2026-11-04, pull versus push on ARM64 (quiet Neoverse-V2 Spot VM): no difference beyond about 1% in any of 12 cells; "push costs little" raised to high for dispatch overhead on a linear chain with Arrow kernels on both architectures; open check 2 closed. Limits that remain: cheaper kernels, static dispatch, branching pipelines.

- 2026-11-04, ARM64 run of the operator-protocol suite: 6 of 6 repetitions passed on a Neoverse-N3 Spot VM; the "run its tests on ARM64" part of open check 1 is done. Pull versus push on ARM64 remains.

- 2026-11-04, corrected pull-versus-push comparison (x86-64): no difference detectable beyond about ±3-5% at any batch size or thread count, so the earlier 10-20% gap is withdrawn as unexplained and probably run-order drift; "push costs little" raised from low to medium-high for x86-64; open check 2 reduced to the ARM64 run.

- 2026-11-03, operator-protocol prototype: all four hard cases expressed and tested. Changes to the text: blocking is reported by sources and sinks, not operators; `HaveMoreOutput` and `Finished` are the operator results; sources have a `finish()` hook; early finish travels up, sideways and across an exchange; added consequences for parallel finalize, spill I/O, query-level memory and load testing; open check 1 marked done with two follow-ups. The prototype found and fixed a lost-shutdown-notification bug in its own scheduler.

- 2026-11-03, after independent review: corrected the evidence (the 8-thread pull row used 7 threads; the 2-thread row was left out; the two-variables-at-once design; the mixed-session speedup, now 2.9x; "exact sums also faster" was never isolated); lowered the confidence in "push costs little" and withdrew "no measurable cost"; dropped "little differentiation" as a reason against pull; added synchronous operators with `blocked` and a waker, compaction before an exchange, float determinism, and a replacement open check (the operator-trait prototype, plus a corrected comparison); added precedent, then checked it on the web (DuckDB and Velox only through secondary sources; Presto's `Operator` interface confirmed).

## Amendments

None yet.
