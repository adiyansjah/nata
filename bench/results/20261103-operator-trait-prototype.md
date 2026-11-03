# Operator protocol prototype (ADR 0001, open check 1)

Code: `spikes/operator-trait/` (throwaway; std only, about 2,000 lines with tests; delete once ADR 0001 is accepted). It tests control flow, not kernels: the batch is plain `i64` columns, with no Arrow and no selection vector. Run on the 8-vCPU x86-64 VM used elsewhere; nothing was run on ARM64.

## What was built

- **Protocol** (`src/api.rs`): `Source`, `Operator`, `LocalSink`/`GlobalSink`, result enums, a `Waker`, pipelines with dependencies. Everything is synchronous. A stage that cannot proceed stores the waker it was given and returns `Blocked`.
- **Scheduler** (`src/sched.rs`): a thread pool; tasks with the life cycle `Queued -> Running -> Parked | Queued | Done`; a wake that arrives while a task is still running is remembered, so no wake is lost; pipeline dependencies as events (a pipeline starts when its dependencies' `finalize` is done); cancellation; first error cancels the rest.
- **Operators** (`src/ops.rs`): generator source (morsels), filter, `LIMIT`, hash-join build and probe, a bounded exchange channel (producer sink, consumer source), a `SUM ... GROUP BY` that spills to disk over a per-task budget and merges one partition at a time in `finalize`, a collecting sink.

## The four hard cases, and what each needed

| case | test | result | what it forced into the protocol |
|---|---|---|---|
| `LIMIT` early termination | exact count across 8 tasks (also under heavy contention), scan stops early | pass: 777 rows exactly; the 10,000-morsel scan stopped after 8 morsels (1 thread) and 10 (4 threads) | `OpResult::Finished`; the driver drops pending upstream input and sets a pipeline-wide stop flag; stopped pipelines wake their parked tasks |
| hash join | build then probe, against a reference join; one case with 400 matches per probe row | pass, equal to the reference | `OpResult::HaveMoreOutput` (one input batch becomes about 40 output batches, input kept and re-presented); pipeline dependency so the probe starts only after the build's `finalize` |
| blocking exchange | consumer scheduled first on **one** worker thread; capacity-1 exchange with a slow consumer; 864-query stress over threads, tasks and capacities | pass: no deadlock, every row arrives | `SourceResult::Blocked` / `SinkResult::Blocked` with stored wakers; a blocked sink leaves its input untouched so the task can push it again |
| finalize with spill | 20,000 groups, budget 200 groups per task, 256 partitions, against a reference | pass: result equal; 50,000 entries spilled over 768 files; peak 200 groups per task (the budget) and 80 groups in the largest merged partition; spill files deleted as read; directory gone with the query | `GlobalSink::finalize` as a separate step after all `combine` calls; resource clean-up by `Drop`, not by the normal path |

Also tested: cancelling mid-query (work stops, spill removed), cancelling tasks parked on an empty exchange (woken, finish within 900 ms), an error in one task (others stop, error reported), a `LIMIT` above an exchange (producers stop), a dependency cycle (rejected).

## Protocol findings (what ADR 0001 should absorb)

1. **`Blocked` belongs to sources and sinks, not to operators.** No intermediate operator needed it. Waiting happens at the edges (exchange, I/O) or between pipelines (dependencies). Operators return `NeedMoreInput`, `HaveMoreOutput` or `Finished`.
2. **`HaveMoreOutput` is necessary.** Joins with fan-out cannot emit one input's output in one call. The driver keeps one pending-input slot per stage and always runs the deepest stage with pending input first.
3. **Early finish must travel three ways:** up the pipeline (drop pending upstream input), sideways (other tasks of the pipeline stop at their next loop and parked ones are woken), and across an exchange (the consumer tells its producers nobody is reading).
4. **`Source::finish()` is needed.** Without a hook run when a task ends, the consumer side of an exchange cannot close the channel, and a `LIMIT` above an exchange leaves producers blocked forever.
5. **Wake before park is normal** and the scheduler needs a "notified while running" flag.
6. **Cancellation has three duties:** wake parked tasks so they can notice; skip `combine` and `finalize`; free resources through `Drop`.

## A scheduler bug that the first test suite missed

- The first 13 tests passed, including 10 repeated runs on an idle machine. Run while 8 busy-loop processes competed for the CPUs, the exchange stress test hung in 2 of 5 runs.
- Cause: `RunningQuery::wait` set the `shutdown` flag and called `notify_all` without holding the queue lock the workers wait on. A worker that had just seen the flag false could miss the notification and sleep forever, so `wait()` never returned.
- Evidence: with a 5 s watchdog and 8 busy loops, the unfixed scheduler failed the stress test in 9 of 12 runs; with the flag set under the lock, 0 of 12, and the full suite passed 15 of 15 under the same load.
- A load test in the same process (spinning threads, 400 small queries) did **not** catch the bug in 3 trials. What did catch it, 3 of 3, is a deterministic test (`tests/shutdown.rs`) that sleeps in the worker's critical section to widen the race window. That test needs its own binary because the delay is process-wide.
- Lesson recorded in `way-of-working.md`: concurrent code is tested under CPU contention and with injected delays, because passing on an idle machine proves little.

## The tests can fail (mutation checks)

Each mutation was applied to a scratch copy and the named test had to fail or time out.

| mutation | caught by |
|---|---|
| wake ignored while the task is still running | exchange stress |
| `HaveMoreOutput` treated as `NeedMoreInput` | join fan-out |
| `LIMIT` without compare-and-swap | `limit_is_exact_under_heavy_contention` (the first version of the suite missed this one) |
| exchange consumer never closes the channel | `LIMIT` above an exchange |
| `Source::finish()` not called | `LIMIT` above an exchange |
| pipeline dependencies ignored | hash join |
| spill directory not removed | cancel and clean-up |
| `cancel` does not wake parked tasks | cancel of parked tasks (the first version of the test had no watchdog around `wait()`, so a hang looked like "missed") |
| shutdown flag set without the queue lock | `tests/shutdown.rs` only |

## Cost of the protocol (`examples/overhead.rs`, release, min of 7 runs)

| path | ns per batch, 1 thread | 2 threads |
|---|---:|---:|
| source -> filter -> sink, no blocking (includes building a 1-row batch) | 186 | 184 |
| through a capacity-1 exchange, where nearly every batch parks and wakes a task | 628 | 797 |

The batches have one row, so these are protocol costs with almost no work in them. If a real batch is 8K rows and a kernel costs about 1 ns per row, 0.2-0.8 microseconds per batch would be 2-10% of the batch's work; that is an estimate from these numbers, not a measurement of real batches.

## Limits (not covered)

- **Not the other half of the check.** The corrected pull-versus-push performance comparison (same morsel queue, right thread count, interleaved, optimized speed, both architectures) is still to do.
- Fusion and selection vectors (the driver runs each operator separately), real Arrow batches, work stealing (a single FIFO queue under one lock, not `crossbeam-deque`), fairness, and priorities.
- **`finalize` runs on one worker thread and blocks it** on file reads; other workers idle meanwhile. Parallel finalize is not implemented, and blocking disk I/O inside the CPU pool is not addressed by ADR 0002.
- **Memory is per task, not per query:** a query holds up to `tasks x budget` groups, and `finalize` is bounded by the partition size, not the budget (skew would need recursive repartitioning). The emitted result is not bounded. A query-level memory manager (tier T0 in `feature-map.md`) is still needed.
- The exchange is an in-process channel. A network or object-storage exchange fed by async tasks calling the same `push` and `wake` is plausible but was not built.
- ARM64: not run. The scheduler bug above was a lock-and-notify race, not a memory-ordering one, but ARM64's weaker ordering could expose others.
