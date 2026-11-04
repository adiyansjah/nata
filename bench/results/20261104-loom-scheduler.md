# Loom model check of the operator-trait scheduler (2026-11-04)

Question: does the real scheduler (`spikes/operator-trait/src/sched.rs`) keep its three concurrency promises under **every** thread interleaving (within stated bounds), instead of the interleavings a stress run happens to hit? Context: the lost-shutdown-notification bug (`20261103-operator-trait-prototype.md`) was found by a stress run, not by a model.

## Method

- `src/sync.rs` re-exports `Arc`, `Mutex`, `MutexGuard`, `Condvar`, `AtomicBool`, `AtomicUsize` and `thread` from std normally and from `loom` under `--cfg loom`. `sched.rs` uses only these names. `loom` is a `[target.'cfg(loom)'.dependencies]` entry, so a normal build neither downloads nor compiles it (`cargo tree` prints the crate alone).
- `tests/loom_sched.rs` (gated `#![cfg(loom)]`) runs the **real** `Query::start` / `wait` / `cancel`, workers and `TaskCell` code. Sources, sinks and events in the test are tiny fixtures built on loom primitives; `ops.rs` is not compiled in under loom.
- Changes to `sched.rs` besides the indirection (default behaviour unchanged):
  - `self: &Arc<Self>` receivers became associated functions (`Shared::start_pipeline(this, ..)`, `TaskCell::wake(&t)`), because loom's `Arc` is not a stable receiver type.
  - The `Arc<dyn ...>` fields from `api.rs` stay on std's `Arc`.
  - The test-delay sleep is `#[cfg(not(loom))]`.
  - Two assertions exist only under `cfg(loom)`: (1) `run_task` asserts the task is `Queued` when a worker starts it (no start while another worker has it, none after `Done`); (2) `wait()` asserts, after joining the workers, that the queue is empty and every task is `Done`. They are checks, not behaviour.
  - Five deliberate-bug features (`bug-*`, all off by default) for validating the model.
- A deadlock (all loom threads blocked) is a loom failure; it is how a lost wake-up shows up.

## Reproduce

```
cd spikes/operator-trait
# whole suite (nine tests; dominated by a2: ~1.5 min alone on a quiet 8-vCPU VM, 14.9 min at load average 20-40)
RUSTFLAGS="--cfg loom" CARGO_TARGET_DIR=/tmp/nata-loom-target cargo test --release --test loom_sched -- --nocapture
# one test, other bound
LOOM_MAX_PREEMPTIONS=4 RUSTFLAGS="--cfg loom" CARGO_TARGET_DIR=/tmp/nata-loom-target cargo test --release --test loom_sched c1 -- --nocapture
# a mutant (must fail)
RUSTFLAGS="--cfg loom" CARGO_TARGET_DIR=/tmp/nata-loom-target cargo test --release --test loom_sched --features bug-lost-shutdown
# default build and existing tests (unchanged, std only)
RUSTFLAGS= cargo test --release
```

## Models, bounds, results (fixed code)

All tests: 2 worker threads. "Preemption bound N": loom explores every interleaving with at most N forced context switches; "none" is exhaustive for that configuration.

| test | property | configuration | bound | executions | result | wall |
|---|---|---|---|---|---|---|
| a1 | workers idle on an empty queue always terminate when `wait` shuts down | no pipeline; shutdown races worker start-up | none | 819 | pass | 0.07 s |
| a2 | same, right after a task finishes | 1 task that finishes at once | none | 1,400,886 | pass | 88 s |
| b1 | a wake while Running/Queued/Parked is never lost; task ends once; never started from a non-`Queued` state | 1 task blocking on 2 events in turn; main thread opens both | 3 | 25,914 | pass | 3.9 s |
| b2 | same | 2 tasks, 1 event each | 3 | 89,301 | pass | 10.4 s |
| b3 | same, waker called from a third thread | 1 task, 1 event, opener thread | 3 | 239,372 | pass | 22.6 s |
| c1 | cancel vs a task finishing: Ok (finalized once) or "cancelled"; task ends once; nothing left | 1 task | 3 | 22,998 | pass | 3.0 s |
| c2 | cancel wakes a task blocked forever; result "cancelled", no `combine`/`finalize` | 1 blocked task | 3 | 25,939 | pass | 3.8 s |
| c3 | cancel vs a task error: one error reported (the first), task ends once | 1 failing task | 3 | 41,094 | pass | 4.7 s |
| c4 | cancel vs the start of a dependent pipeline | 2 pipelines (1 task each), dependency | 3 | 74,468 | pass | 10.6 s |

Higher bound (bound 4, run earlier in separate processes, same code except for formatting): b1 314,329 executions, 48 s; c1 231,349, 33 s; c2 265,427, 44 s; c3 488,738, 65 s; c4 1,371,620, 160 s: all pass. b2 and b3 at bound 4 did **not** finish in the 280 s limit (no result).

Wall times are from a shared VM that other jobs were using (load average 10-19 on 8 cores), so they are noisy. Final run of the whole suite in one `cargo test` on the formatted code: 9 passed, 0 failed, `finished in 880.47s` (real 14m52s) at load average 20-40; the per-test numbers above come from separate-process runs at lower load. No timing here is a clean benchmark.

## Mutation validation (a model that cannot fail is not evidence)

Each mutant is a cargo feature that reintroduces one bug into the real `sched.rs`. Full suite run per mutant, each test in its own process.

| mutant | what it does | caught by | not caught by | failure |
|---|---|---|---|---|
| `bug-lost-shutdown` | the original bug: `shutdown` set outside the queue lock, then `notify_all` | **all nine** (a1, a2, b*, c*: every test ends in `wait`) in 5-11 ms | none | loom: `deadlock; threads = [(Id(0), Blocked..), (Id(1), Blocked..), (Id(2), Blocked..)]` (main blocked in `join`, workers in the condvar) |
| `bug-wake-ignored-while-running` | `wake` on a Running task does not set `notified` | b1, b2, b3, c2 (0.6-8 s) | a1, a2, c1, c3, c4 (they never block) | deadlock: the task parks forever and `wait` never returns |
| `bug-park-ignores-notified` | after `Blocked`, `Running` is cleared to `Parked` without looking at `notified` | b1, b2, b3, c2 (1-7 s) | a1, a2, c1, c3, c4 | deadlock |
| `bug-wake-enqueues-running` | `wake` on a Running task also queues it (a second worker can pick it up) | b1, b2, b3, c1, c2, c3, c4 (0.5-4 s) | a1, a2 | `sched.rs:286: task started from a state other than Queued` (the loom-only assertion) |
| `bug-cancel-does-not-wake` | `cancel` does not wake the tasks | c2 only (23 ms) | all others | deadlock (a parked task never ends) |

The failing outputs are in `/tmp/nata-loom-out/<mutant>/` of the run (not kept in the repo). The deadlock mutants end with `rc=134` because loom then panics a second time while unwinding; the first panic is the deadlock report.

Cross-check with the old method: the delay-hook test (`tests/shutdown.rs`) also fails against `bug-lost-shutdown` (`round 0: wait() never returned: a worker missed the shutdown notification`, after the 10 s watchdog), so both methods agree on that mutant. Loom needed 5-11 ms, no hook and no luck.

## Existing suite, before and after (default build, `RUSTFLAGS=`)

- Before: protocol 15 passed (7.21 s), shutdown 1 passed (0.15 s).
- After: protocol 15 passed (7.47 s), shutdown 1 passed (0.16 s). The only difference in the output is the new `loom_sched` test binary reporting 0 tests (it is gated on `cfg(loom)`).
- Shutdown test under CPU load (8 busy-loop processes on top of other jobs' load average ~18): passed 3 of 3 (0.21-0.25 s each).

## What is NOT covered

- **Size.** 2 workers only; 1 or 2 tasks; c4 has 2 pipelines. Bound 3 preemptions for everything except a1 and a2 (exhaustive for their configuration). Bugs needing 3 workers, more tasks, or more than 3 (b2, b3: more than 4 unverified) preemptions are not excluded. This is a bounded check, not a proof.
- **Memory ordering.** The code uses SeqCst everywhere; loom does not model the SeqCst total order (it treats SeqCst as acquire/release at least). Weaker orderings were not tried, so nothing is shown about them. Real ARM64 behaviour is covered only by the earlier empirical run, not by this model.
- **Condvar spurious wake-ups** are not modelled by loom. The worker loop rechecks its condition, but this was not exercised.
- **Operators.** `ops.rs` (exchange, hash join, spill, `LIMIT`) is not compiled under loom. The exchange's own waker storage and its check-then-store protocol are the other half of the wake protocol and are **not** checked; tests b* use my `Gate` fixture, which does the check and the waker store under one lock, as a correct exchange would. A buggy source that loses a wake before the scheduler sees it is outside this model.
- **Scheduler paths not modelled:** early finish (`p.stop`, `wake_pipeline`), a blocked sink, an error from `combine` or `finalize`, a dependent pipeline with several tasks outside the cancel race, `cancel` racing with `start` itself (`cancel` can only be called after `start` returns), a cancel from a thread other than the one that waits.
- **Mutual exclusion of a task** is checked at the `TaskState` level (assertion in `run_task`, only under `cfg(loom)`). `TaskBody` is behind its own mutex, so two workers running one task at once would be serialized there and not observed as overlap; the state assertion is the check that catches the double start (mutant `bug-wake-enqueues-running`).
- The loom-only assertions and the `bug-*` features live in `sched.rs`. The default build compiles the `bug-*` branches out (they are `cfg!` on features that are off) and has no loom code; the loom build is not byte-for-byte the default code, though the differences are the primitives, those assertions and the missing sleep hook.
- `Cargo.lock` of the spike grew by about 250 lines (loom's dependencies); they are not built or downloaded in a normal build.

## Findings

- No bug found in the fixed scheduler within the bounds above.
- The original lost-wakeup bug is found by loom in milliseconds, without a delay hook.
- Both wake-protocol bugs that the prototype's doc only argues about (`notified` remembered; re-run after `Running`) are detected when removed, so the `Running { notified }` state is load-bearing, as claimed.
