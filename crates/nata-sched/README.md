# nata-sched

The scheduling core of the parallel scan (slice 3, step 2b; card `docs/slices/0001-parquet-scan.md`, record `bench/results/20261106-scan-parallel.md`):
an ordered, windowed, cancellable executor of "row-group" tasks on worker threads. Generic over a [`Work`] (a list of files that expand lazily into
ordered tasks); knows nothing about Parquet. `nata-parquet`'s `run_parallel` is its one user.

* **One lock, two condition variables.** All state (the claim cursor, the window, the result slots, the stop flags) is in one `Mutex`; tasks run outside it.
  Claims are made in key order (file, then row group) and get dense sequence numbers, so the ordered output is the single-thread scan's.
* **Window.** `claimed - released < window` before every claim: at most `window` results are in flight, waiting, or in the consumer's hands. The reorder
  buffer cannot grow, whatever the speed of the slowest task.
* **Cancellation.** One atomic (`abandon_from`): a task with `seq >= abandon_from` is cancelled. The consumer cancels everything (limit, sink error), a
  failed task cancels the tasks above it, `CancelHandle::cancel` cancels from outside. `Scheduler::run` returns only after every worker is joined and
  everything undelivered is dropped, also when the sink panics.
* **Errors.** The error of the lowest sequence number is returned, always (ordered or not); an error after a satisfied limit is never reported.
* **`src/sync.rs` is the only place that names `std::sync`/`std::thread`**; under `RUSTFLAGS="--cfg nata_loom"` it hands out `loom`'s types, and
  `tests/loom_sched.rs` explores the real scheduler. `tests/facade.rs` fails if another file bypasses the facade.
* `crossbeam-deque` (ADR 0002) is not used: the window needs tasks to start in sequence order, which one FIFO under one lock gives by construction.

```sh
cargo test -p nata-sched                              # std tests: order, window, limit, errors, cancel, panics, facade
scripts/local-heavy.sh crates/nata-sched/run_loom.sh  # 16 loom models, one per process, bound 2 (4 for the one-worker model), about 1 minute
LOOM_MAX_PREEMPTIONS=3 SCHED_ONLY='m0[1-5]' crates/nata-sched/run_loom.sh   # a higher bound for chosen models (bound 3 is about 10x the executions)
scripts/local-heavy.sh python3 crates/nata-sched/mutants/mutate.py --jobs 3   # 24 mutants of src/lib.rs against the std tests and the loom models
```
