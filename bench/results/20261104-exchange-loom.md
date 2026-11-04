# Loom model check of the exchange (2026-11-04)

Question: does the exchange of the operator-trait prototype (the bounded channel between two pipeline fragments, with `Blocked` + waker at the sink and the source, and early finish across it) keep its promises under **every** thread interleaving (within stated bounds), together with the real scheduler? This is the follow-up the scheduler model left open (`20261104-loom-scheduler.md`, "What is NOT covered": the exchange's own waker storage and check-then-store protocol; ADR 0001 open check 1b).

Code: `spikes/exchange-loom/` (throwaway). Nothing outside it was edited except this file. `spikes/operator-trait/` and `spikes/async-bridge/` were not touched.

## Method

- **Snapshot of the committed code**, not the working tree: `git show HEAD:spikes/operator-trait/src/...` at commit `ce5f7153c992a2a11f7e541ec0d5c9683461ab00`. Details, hashes and the diff are in `spikes/exchange-loom/SNAPSHOT.md`. Summary:
  - `api.rs`, `batch.rs`: byte-identical (sha256 `b84b2c71...`, `328f8490...`).
  - `sched.rs`, `sync.rs`: only the cfg name changed, `loom` -> `nata_loom` (4 + 7 lines, plus comments). Same convention as `spikes/async-bridge` (`--cfg loom` would switch on other crates' loom code). The scheduler's loom-only assertions (a task starts only from `Queued`; `wait()` finds the queue empty and every task `Done`) and its five `bug-*` features (off) are kept.
  - `src/exchange.rs`: the **real** exchange, `ops.rs` lines 237-390. Differences: the imports, and (added on the second pass, after finding F1) one marked fix in `close_consumers` (see F1). The imports: `Mutex`, `AtomicUsize`, `SeqCst` come from `crate::sync` (loom under `nata_loom`). `Arc` stays `std::sync::Arc` because the code holds `Arc<dyn GlobalSink>` / `Arc<dyn SourceFactory>`, and loom's `Arc` has no unsizing coercion (the scheduler does the same for its trait objects). Before the fix, the body was checked with `diff` against the original: no difference; since the fix, the only difference is the marked block in `close_consumers`.
  - `src/limit.rs`: the real `Limit` operator (`ops.rs` 141-184), imports only.
  - Added at the end of `exchange.rs`, marked: `queue_len()`, `capacity()` (read-only probes for the bound check), and `stored_wakers()` (a probe used by the cancel tests to assert that no waker is left). No change to any code path of the original. (`drop_stored_wakers()`, a test-harness cycle breaker of the first pass, was removed once the fix made it unnecessary.)
  - sha256 of `src/` at the time of writing: `api.rs b84b2c71...`, `batch.rs 328f8490...`, `exchange.rs 3c7f07a7a79f065be636847514bd5f046d792031237dd93ba13a1d026b04cf73` (with the F1 fix; the pre-fix copy was `52afc229...`), `limit.rs 07168199b2abaaaecb4d273f0a1b6d90b43819c020b90c62aeab13afae3e8d7a`, `sched.rs 1587970fcc91bdc5e7c577cc588685a8e11ec54a921f6cdf703cd6d5188aa236`, `sync.rs b9da43e6b0bc0257af7a2c6cb0a6995711062e8908a065b1db8b052c080e46c4`, `lib.rs befb270e...` (full values: `sha256sum spikes/exchange-loom/src/*.rs`). Only the header comment of `exchange.rs` changed (a line number in a comment) while the mutation run was in progress; the code is the same in all runs.
- **Test shape** (`tests/loom_exchange.rs`, gated so that the same file also runs as a std-thread stress loop without `--cfg nata_loom`): one producer pipeline (`P` tasks: generator source -> real exchange sink) and one consumer pipeline (`C` tasks: real exchange source -> optional real `LIMIT` -> collecting sink), the real `Query::start` / `cancel` / `wait`, capacity 1 (except e4: capacity 2), 1 or 2 worker threads, 0-3 items per producer. `consumer_first` swaps the pipeline order, which decides which task the scheduler queues first. Items are `producer_id*100 + seq`, so order within one producer is checkable. A deadlock (all loom threads blocked) is a failure; it is how a lost wake-up shows up.
- **Coverage counters** (std atomics outside the model, summed over all executions) show that the scenario a test is named after really happened, and the test fails if it did not: producer returned `Blocked` (full), consumer returned `Blocked` (empty), a producer that was `Blocked` later got `Finished` (woken by the consumer's close). Observed e.g.: a1 (producer first) 86,008 producer-`Blocked` events over 89,263 executions; a2 (consumer first) 67,071 consumer-`Blocked` events.
- Each loom test runs in its own process, one at a time.

## Properties and models (fixed code)

Bound = loom preemption bound (`LOOM_MAX_PREEMPTIONS` overrides). Executions = loom executions. Wall = `run_loom.sh`, one test per process, on a SHARED 8-core VM with load average about 8-10 from other agents' jobs (Linux x86-64, local, not a quiet machine; noisy).

| test | property | configuration | bound | executions | wall |
|---|---|---|---|---|---|
| a1_producer_blocked_on_full_producer_queued_first | (a) producer blocked on a full exchange is woken by the consumer's pop | 2 workers, P1 C1, 2 items | 3 | 89,263 | 12.1 s |
| a1_..._consumer_queued_first | same, other queue order | same | 3 | 115,770 | 18.3 s |
| a2_consumer_blocked_on_empty_consumer_queued_first | (a) consumer blocked on an empty exchange is woken by the producer's push | 2 workers, 1 item | 3 | 57,797 | 7.2 s |
| a2_..._producer_queued_first | same, other order | same | 3 | 44,012 | 4.1 s |
| a3_one_worker_consumer_queued_first | (a) both handshakes on ONE worker thread: a blocked task yields the thread | 1 worker, 3 items | 3 | 242 | 0.1 s |
| a3_one_worker_producer_queued_first | same | same | 3 | 233 | 0.1 s |
| b1_producer_finishes_while_consumer_blocked_consumer_queued_first | (b) `producer_done` wakes a blocked consumer, which ends | 2 workers, 0 items | 3 | 51,944 | 4.9 s |
| b1_..._producer_queued_first | same | same | 3 | 39,899 | 4.4 s |
| b2_consumer_limit_closes_blocked_producer_consumer_queued_first | (b) `LIMIT 1` consumer ends early -> `finish()` -> `close_consumers` wakes the producer blocked on a full exchange; producer ends; exactly `[0]` received | 2 workers, 3 items | 3 | 87,406 | 12.5 s |
| b2_..._producer_queued_first | same | same | 3 | 65,475 | 8.9 s |
| b2_..._one_worker | same on one worker | 1 worker | 3 | 215 | 0.2 s |
| b3_two_producers_blocked_when_consumer_closes | (b) two producer tasks both woken by the close | 2 workers, P2 C1, 2 items each, LIMIT 1 | 2 | 9,811 | 1.8 s |
| c1_cancel_during_full_empty_handshakes | (c) `cancel()` after `start` racing with the full/empty handshakes: ends with "cancelled", every task `Done`, scheduler queue empty | 2 workers, 2 items | 3 | 462,173 | 95.0 s |
| c2_cancel_during_early_close | (c) cancel racing with the LIMIT early close | 2 workers, 3 items, LIMIT 1 | 3 | 342,923 | 70.8 s |
| c3_cancel_while_producer_finishes_and_consumer_blocked | (c) cancel racing with `producer_done` and a blocked consumer | 2 workers, 0 items | 3 | 161,314 | 27.9 s |
| c4_cancel_producer_blocked_forever_on_full_exchange | (c) consumer never reads (stuck), producer blocked forever on the full exchange; only cancel can end it | 2 workers, 2 items | 3 | 165,256 | 26.4 s |
| c5_cancel_one_worker | (c) cancel on one worker | 1 worker, 3 items | 3 | 8,615 | 1.6 s |
| d1_bound_one_producer | (d) `queue_len() <= cap` after every push; and (items handed out - items collected) <= cap + P + C | 2 workers, 3 items | 3 | 229,531 | 56.8 s |
| d2_bound_two_producers | (d) same with two producer tasks | 2 workers, P2, 1 item each | 3 | 260,145 | 74.3 s |
| e1_order_one_producer | (e) the consumer sees exactly `[0,1,2]` (no loss, duplicate or reorder) | 2 workers, 3 items | 3 | 163,149 | 46.1 s |
| e2_order_per_producer_with_two_producers | (e) per producer: complete and in order | 2 workers, P2, 2 items each | 2 | 13,720 | 5.9 s |
| e3_two_consumers_each_item_once | (e) every item exactly once with two consumer tasks | 2 workers, C2, 2 items | 2 | 7,182 | 2.6 s |
| e4_items_queued_at_producer_done_are_delivered | (e) items still queued when the last producer is done are delivered | capacity 2, 2 items | 3 | 48,356 | 12.1 s |

Result, first pass: **all 23 tests pass** on the exchange as copied (sum of wall times about 520 s; the table is from one run, `run_loom.sh`; the `f1` of that pass is now the regression test described under F1, and c3/c5 later changed to 161,602 / 8,623 executions with the fix).

**After the F1 fix (second pass, local):** 20 of the 23 first-pass models were re-run on the fixed exchange, one at a time, and pass with the same execution counts (a1-a3, b1-b3, c3-c5, d1, e1-e4; sum of wall times 614 s on a machine with other agents' load, so slower than the first pass). **Not re-run locally, left for the GCP run: c1_cancel_during_full_empty_handshakes, c2_cancel_during_early_close, d2_bound_two_producers** (95, 71 and 74 s in the first pass). Also run and passing: loom `f1_cancel_leaves_no_waker_in_channel` (3,879 executions) and the 7 new models x1-x7 (table below), and the std tests `leak` f0-f2 (20 of 20) and `loom_channel` x1-x7. Those local runs were made before the rule "heavy local commands one at a time through `scripts/local-heavy.sh`" reached me; they were sequential (one loom run at a time) but did not use the wrapper. From that rule on I ran nothing heavy. The "slack" in (d): the channel queue itself is strictly bounded by the capacity; a blocked producer holds one batch outside the queue and a consumer holds one batch between `pop` and its sink, so the end-to-end bound used is `cap + producer tasks + consumer tasks`.

The plain tests are also leak-checked: loom fails an execution if a loom `Arc` is still alive at its end, and none of the plain (non-cancel) tests tripped it.

## Finding F1: a cancelled query left a waker in the channel, which kept the whole query alive (reference cycle) - FIXED in the copy

Found by the first loom run of the cancel tests: `Arc leaked` (loom-0.7.2 `rt/arc.rs:179`). Turned into tests before anything else (first pass, on the unfixed exchange):

- `tests/leak.rs` (std threads, no loom) holds only a `Weak<Channel>` and checks it is dead after the query ends: `f0` (plain query) passed; `f2_cancel_with_blocked_consumer_releases_the_channel` **failed 20 of 20** (orchestrator: 5 of 5 on a re-run); `f1_limit_with_two_consumer_tasks_releases_the_channel` (sibling-stop path, suspected, never leaked in 20 runs: no leak is claimed for it).
- `tests/loom_exchange.rs::f1_...` (then `#[ignore]`, expected to fail): `Arc leaked` in loom. The cancel models c1-c5 needed a harness-only waker cleanup to run at all; over c3's executions 43,232 stale wakers were left behind.

Mechanism (reasoned from the code, consistent with the tests): `Channel::pop` stores the consumer's waker in `consumer_wakers` when it returns `Blocked`; the list is drained only by a push or by `producer_done`. `producer_done` runs from `combine`, which the scheduler does not call for a cancelled query. So a consumer woken by `cancel` ends, but its waker stays. The waker owns the `TaskCell`, which owns `Shared`, which owns the pipelines, which own the exchange, which owns the `Channel`: a cycle through `Arc`s, i.e. in production a memory leak of the query's state and of any batches left in the queue per cancelled or failed query; not a correctness error.

**The fix** (`src/exchange.rs`, `close_consumers`, 5 lines and a comment): when the channel is closed, take the stored consumer wakers and drop them outside the lock.
```
let stale = std::mem::take(&mut s.consumer_wakers);   // new
drop(s);
wake_all(to_wake);
drop(stale);                                          // new
```
Why this is enough and safe (reasoned, then tested): the scheduler calls `Source::finish` for every task that ends, whatever the reason; the last one calls `close_consumers`; at that moment no consumer task is alive, so a stored consumer waker can only be stale. The producer wakers are already taken (and fired) by the same function, and a producer cannot register a new one afterwards (`push` checks `consumers_closed` first). On the normal path both lists are empty anyway (`push`, `producer_done`, `pop` drain them).

**Weak instead?** A `Weak<TaskCell>` inside the scheduler's waker would break the cycle for every resource that ever stores a waker (I/O, join gates, the tokio bridge), not only for this channel, and a stale waker would then cost only a dead `Weak`. Against it: it changes the scheduler's `Waker` (the code under the earlier scheduler model), adds an `upgrade` to every wake, silently turns a wake after `all_tasks.clear()` into a no-op, and still lets stale entries pile up in the channel's lists until drained. I chose the smaller, local change and tested it; the `Weak` variant is a reasoned alternative, not tried here. A future resource that stores wakers needs the same discipline (drop them when its owner ends), or the `Weak` change.

Evidence after the fix:

| check | before | after |
|---|---|---|
| `leak.rs::f2` (std, cancel with a blocked consumer) | fails 20/20 | passes 20/20 |
| `leak.rs::f0`, `f1` | pass | pass 20/20 |
| loom `f1_cancel_leaves_no_waker_in_channel` (1 worker, 0 items, cancel; bound 3; asserts `stored_wakers() == (0,0)` after `wait` and loom's own `Arc leaked` check) | `Arc leaked` (old ignored test) | passes, 3,879 executions, 0 stale wakers, 1.4 s |
| c3 (2 workers) / c5 (1 worker), now without any cleanup and with the same assertion | needed the cleanup | pass; executions 161,602 (was 161,314) and 8,623 (was 8,615): one more lock in the cancel path |
| mutant 17 `waker_kept_after_close` (the fix removed again) | - | caught by `leak:f2`, loom `f1` and `c5` (message: `waker(s) still stored in the channel after the query ended` / `LEAK: the channel is still alive after cancel`) |

## Mutation validation (a model that cannot fail is not evidence)

17 deliberate mutants of the exchange (16 of the original exchange, plus mutant 17 for the F1 fix; one mutant, 16, is in the scheduler's early-finish hook), each applied to a **private copy** by `mutants/mutate.py` (the anchor must match exactly once, so a mutation cannot silently be a no-op); the whole suite runs on each, one test per process, `mutants/run_mutants.sh`. Names:

| # | mutant | what it does |
|---|---|---|
| 1 | `pop_store_waker_without_recheck` | consumer: emptiness check and waker store no longer in one critical section |
| 2 | `push_store_waker_without_recheck` | producer: same for fullness |
| 3 | `pop_blocked_without_storing_waker` | consumer returns `Blocked` without storing the waker |
| 4 | `push_blocked_without_storing_waker` | same for the producer |
| 5 | `push_does_not_wake_consumer` | send without waking |
| 6 | `pop_does_not_wake_producer` | receive without waking blocked producers |
| 7 | `producer_done_does_not_wake_consumer` | last producer finishing does not wake the consumers |
| 8 | `close_consumers_does_not_wake_producers` | consumer's early close does not wake blocked producers |
| 9 | `wake_all_wakes_only_the_first` | only one of several waiters is woken |
| 10 | `consumer_finish_never_closes` | the last consumer never closes the channel |
| 11 | `push_ignores_closed_channel` | producers do not notice the close |
| 12 | `capacity_off_by_one` | queue may hold `cap + 1` |
| 13 | `pop_checks_finished_before_queue` | end of stream checked before the queue is drained (items lost) |
| 14 | `producer_done_drops_queued_items` | queued items dropped when the last producer is done |
| 15 | `push_to_front_reorders` | newest first (reorder) |
| 16 | `sched_task_end_skips_source_finish` | scheduler never calls `Source::finish`, so a consumer never closes the exchange |
| 17 | `waker_kept_after_close` | the F1 fix removed: stale consumer wakers stay in the channel after the close |

**Results of my own local run (completed mutants 1-6)**; each full suite of 23 tests per mutant, failures listed by test prefix (every failure was `deadlock; threads = [...]`: all threads blocked, main in `join`/condvar):

| mutant | caught by (tests) | not caught by |
|---|---|---|
| 1 `pop_store_waker_without_recheck` | 15: a1 x2, a2 x2, b1 x2, b2 x2 (2 workers), b3, d1, d2, e1, e2, e3, e4 (0.1-2.6 s each) | the 1-worker tests (a3 x2, b2 one worker), c1-c5 |
| 2 `push_store_waker_without_recheck` | 10: a1 x2, b2 x2, b3, d1, d2, e1, e2, e3 | a2, a3, b1, b2 one worker, c1-c5, e4 |
| 3 `pop_blocked_without_storing_waker` | 18: all a, b, d, e tests | c1-c5 |
| 4 `push_blocked_without_storing_waker` | 13: a1 x2, a3 x2, b2 x3, b3, d1, d2, e1, e2, e3 | a2, b1, c1-c5, e4 |
| 5 `push_does_not_wake_consumer` | 16: a1, a2, a3 (each x2), b2 x3, b3, d1, d2, e1, e2, e3, e4 | b1, c1-c5 |
| 6 `pop_does_not_wake_producer` | 13: a1 x2, a3 x2, b2 x3, b3, d1, d2, e1, e2, e3 | a2, b1, c1-c5, e4 |

**GCP run of mutants 1-16 (read by the orchestrator, not by me)**: all 16 mutants plus the unmutated control run in parallel on a clean 32-vCPU ARM64 VM with my `mutants/run_mutants.sh` (one mutant per worker, private copies, pinned toolchain), over the 23 loom models of the first pass: **16 of 16 caught, control `caught_by=0`**. Number of the 23 models that caught each (reported by the orchestrator; the suite had 23 tests, so these counts are out of 23):

| mutant | caught_by | | mutant | caught_by |
|---|---|---|---|---|
| 1 pop_store_waker_without_recheck | 15 | | 9 wake_all_wakes_only_the_first | 3 |
| 2 push_store_waker_without_recheck | 10 | | 10 consumer_finish_never_closes | 4 |
| 3 pop_blocked_without_storing_waker | 18 | | 11 push_ignores_closed_channel | 4 |
| 4 push_blocked_without_storing_waker | 13 | | 12 capacity_off_by_one | 4 |
| 5 push_does_not_wake_consumer | 16 | | 13 pop_checks_finished_before_queue | 12 |
| 6 pop_does_not_wake_producer | 13 | | 14 producer_done_drops_queued_items | 12 |
| 7 producer_done_does_not_wake_consumer | 12 | | 15 push_to_front_reorders | 1 |
| 8 close_consumers_does_not_wake_producers | 3 | | 16 sched_task_end_skips_source_finish | 4 |

My local numbers for mutants 1-6 above agree with these counts (15, 10, 18, 13, 16, 13). I stopped my own local run during mutant 7 and have no local per-test result for 7-16. Mutants 15, 12, 9 and 8 were the weakest (1, 4, 3, 3 models), which is what the second pass addressed.

**Second pass: new models aimed at the weak mutants** (`tests/loom_channel.rs`, scheduler-free, driving the real sink and source with counting wakers; each model passes on the fixed code, 0.3-0.7 s):

| new model | aimed at | executions |
|---|---|---|
| x1_fifo_order_with_capacity_3 | order (mutant 15) | 1 (sequential; 200 repetitions in the std build) |
| x2_capacity_is_exact | capacity: `cap` pushes fit, the next blocks with the batch untouched, a pop frees exactly one slot, for cap 1 and 2 (mutant 12) | 1 |
| x3_close_wakes_every_blocked_producer | two producers blocked on a full exchange are both woken exactly once by the close; later pushes see `Finished` (mutants 8, 9) | 1 |
| x4_push_and_producer_done_wake_every_blocked_consumer | two blocked consumers are both woken by a push, and by the last producer finishing (mutants 5, 7, 9) | 1 |
| x7_pop_wakes_every_blocked_producer | a pop wakes both blocked producers (mutants 6, 9) | 1 |
| x5_race_blocked_producer_vs_consumer_close | loom race: producer push on a full exchange vs the consumer's `finish`: if the push returned `Blocked` its waker has been fired (mutants 2, 4, 8) | 7 (bound 4) |
| x6_race_blocked_consumer_vs_push | loom race: consumer pop on an empty exchange vs a push: if `Blocked`, the waker has been fired (mutants 1, 3, 5) | 7 (bound 4) |

x5/x6 have so few executions because the channel has one lock; they are exhaustive for that shape. Also new: `f1` (above), and `c1-c5` now assert `stored_wakers() == (0,0)`.

**Local re-run of the five affected mutants + the control** (first pass of the second round; subset of tests `x1-x7, f1 (loom), leak f0-f2, e1, e4, a3 x2, b2 one worker, b3, c5` = 18 tests; each in its own process; this subset ran before the "one heavy command at a time" rule reached me, directly through `mutants/run_mutants.sh`):

| mutant | newly caught by (loom/std test, message) | caught_by of the 18 |
|---|---|---|
| 15 push_to_front_reorders | `x1` (`items must come out in the order they went in`), `x2`, `e4` (`items lost, duplicated or reordered`) | 3 (was 1 on GCP; x1 is independent of the scheduler and of e4's configuration) |
| 12 capacity_off_by_one | `x2` (`cap 1: push 1 must block`), `x3`, `x7` | 3 of this subset (d1/d2 not in the subset; GCP had 4 of 23) |
| 8 close_consumers_does_not_wake_producers | `b3` (deadlock), `x3` (`producer A was not woken by the close`), `x5` (`producer returned Blocked but the close never woke it`) | 3 |
| 9 wake_all_wakes_only_the_first | `b3` (deadlock), `x3` (`producer B was not woken`), `x4` (`a push must wake both blocked consumers`), `x7` (`a pop must wake both blocked producers`) | 4 |
| 17 waker_kept_after_close | `c5`, `f1` (loom), `leak:f2` | 3 |
| none (control) | nothing | 0 of 18 |

The `none` line is the control for this subset only. The full suite on all 17 mutants (and the control) over the now 31 loom models is for the GCP run (below).

Cross-check of the model design: the 1-worker models do not catch the check-then-store mutants (1, 2) because a lost wake-up needs a second thread; they are there for the "blocked task hands over the thread" property and catch mutants 3, 4, 5, 6 only.

## Limits of the models (weak spots I know of)

- **The cancel models c1-c5 catch none of the wake-protocol mutants 1-6** (local run): cancel wakes every task, so a lost wake-up in the exchange cannot hang a cancelled query. They check termination, "every task `Done`", and (since the fix) that no waker is left in the channel; the last is what catches mutant 17. So property (c) is a check of the cancel path and of the leak, not of the wake protocol; the wake protocol is carried by a, b and x5/x6.
- **Weakest mutants before the second pass** (GCP counts): 15 reorder (1 model: e4), 8 close-without-waking-producers (3), 9 wake-only-the-first (3), 12 capacity+1 (4). The second pass adds scheduler-free models aimed at each (x1, x2, x3, x4, x5, x7); their catch counts are in the table above, from a subset run, **not yet on the full suite**.
- **x1-x4 and x7 are sequential**: they show the property in the single-threaded case (which is where a missing wake, an off-by-one or a wrong order shows), not under races; the races are x5/x6 (one producer, one consumer, one item) and the scheduler-level models. A race that needs two waiters AND a concurrent close is only in b3.
- **Ordering with capacity 1** cannot show a reorder, since the queue holds at most one item; reorder is checked with capacity 2 (e4) and capacity 3 (x1).
- **Size.** 1-2 workers; at most 3 tasks (P+C <= 3); at most 3 items per producer; preemption bound 3 (2 for the 3-task tests b3, e2, e3). Bugs needing more workers, tasks, items or preemptions are not excluded. This is a bounded check, not a proof.
- **Memory ordering.** Everything is `SeqCst`; loom does not model the SeqCst total order. Weaker orderings and real ARM64 behaviour are not covered by this model (the ARM64 VM run is a mutation run of the same loom model, not a hardware memory-model test).
- **Spurious condvar wake-ups** are not modelled by loom.
- **Not modelled:** a blocked exchange sink under a dependency chain (join build), a producer pipeline with an upstream `LIMIT`, sibling-stop (`wake_pipeline`) between consumer tasks with a waiting task (probed only by `leak.rs` f1, std; no leak seen), an error from `combine`/`finalize`, `cancel` from a thread other than the one that waits, cancel racing with `start`, exchange with more than one consumer pipeline, the real `ops.rs` file as a whole (only the exchange and `Limit` are compiled in), the tokio-to-pool bridge (separate spike).
- **Wall times are noisy**: the VM was shared (load average 8-10 on 8 cores).

## Reproduce

```
cd spikes/exchange-loom
# default build (std threads, same fixtures and assertions, 200 repetitions of each scenario; EXL_STRESS=<n> changes that)
scripts/local-heavy.sh env RUSTFLAGS= CARGO_BUILD_JOBS=2 cargo test --release --no-fail-fast   # std build: loom_exchange, loom_channel, leak all pass (leak f2 failed before the F1 fix)
# the whole loom suite, one test per process, prints a table row per test (520 s sum measured here on a loaded machine)
CARGO_BUILD_JOBS=2 ./run_loom.sh        # 31 models; EXL_ONLY / EXL_SKIP (egrep on names) select; use scripts/local-heavy.sh on the shared machine
# one test, other bound
LOOM_MAX_PREEMPTIONS=4 RUSTFLAGS="--cfg nata_loom" CARGO_TARGET_DIR=/tmp/nata-exl-target cargo test --release --test loom_exchange a2_consumer_blocked_on_empty_producer_queued_first -- --exact --nocapture
# the loom regression test for finding F1 (before the fix: "Arc leaked"; now passes)
RUSTFLAGS="--cfg nata_loom" CARGO_TARGET_DIR=/tmp/nata-exl-target cargo test --release --test loom_exchange f1_cancel_leaves_no_waker_in_channel -- --exact --nocapture
# the new scheduler-free models
RUSTFLAGS="--cfg nata_loom" CARGO_TARGET_DIR=/tmp/nata-exl-target cargo test --release --test loom_channel -- --nocapture --test-threads=1
# mutation validation: all 17 mutants + control (private copies in /tmp/exl-mut/<name>, outputs in /tmp/exl-mut-out/<name>/)
CARGO_BUILD_JOBS=2 mutants/run_mutants.sh                         # sequential; local pace was 3-4 min per mutant on a loaded machine
EXL_ONLY='^(x[0-9]|f[0-9])' mutants/run_mutants.sh capacity_off_by_one none   # a subset of mutants and of tests; `python3 -I mutants/mutate.py --list` lists the names
```

## For the GCP run (NOT run locally; numbers to be taken on GCP)

Per the machine rule, the full suite and the mutant set are not run on the shared machine. Everything below is already scripted; run it on a quiet VM, one command at a time, from `spikes/exchange-loom/` with `CARGO_BUILD_JOBS` set to the vCPU count for the build only (each loom exploration is single-threaded; the mutant runner can use one process per mutant, as before).

```
# 1. whole suite on the fixed exchange, bound 3 (31 loom models: the 23 first-pass ones, f1, x1-x7 -> one row per test)
CARGO_BUILD_JOBS=4 ./run_loom.sh | tee exl-bound3.txt
# 2. std build: stress loop + leak tests (fast)
RUSTFLAGS= cargo test --release --no-fail-fast
# 3. mutation set: 17 mutants + control, full 31-model suite + 3 leak tests each (one mutant per worker is fine)
mutants/run_mutants.sh <mutant>        # for each name in: python3 -I mutants/mutate.py --list ; then: mutants/run_mutants.sh none
```

Expected, to be confirmed on GCP: (1) every row `rc=0`; execution counts equal to the tables above (loom is deterministic; c3 161,602, c5 8,623, c1 and c2 and d2 are the three not re-run locally since the fix, first-pass counts 462,173 / 342,923 / 260,145 expected unless the extra lock in `close_consumers` changes them, as it did by +288 for c3); (3) 17 of 17 mutants caught, control `caught_by=0`, and for the second-pass mutants at least: 15 by x1, e4; 12 by x2; 8 by x3, x5, b3; 9 by x3, x4, x7, b3; 17 by f1, c3, c5 and `leak:f2`. Wall time: the first-pass suite summed to about 520 s on the shared 8-core machine (about 615 s for 20 of the models with the fix, under heavier load); the quiet-VM time is **to be measured on GCP**. The mutant set cost about 3-4 minutes per mutant locally on the loaded machine, so about 1 h sequentially for 17 mutants (again: GCP number to be taken there).

## Long exploration (higher bounds; for the GCP VM, NOT run here)

I was told to run nothing long, so **none of the commands below was run at bound 4 or higher; the durations are not measured.** What is measured is the bound-3 time above (about 520 s for the whole suite on a loaded 8-core machine). For scale only: in the scheduler model (`20261104-loom-scheduler.md`) going from bound 3 to bound 4 multiplied executions by 10-18x for tests of similar size (b1 25,914 -> 314,329; c1 22,998 -> 231,349; c4 74,468 -> 1,371,620); if the same factor holds here (a guess, not measured), the bound-4 suite is of the order of 1-2 hours, c1/c2/d1/d2/e1 being the longest (tens of minutes each) and bound 5 several times that.

```
cd spikes/exchange-loom
# bound 4, whole suite, one test at a time, 2 h per-test cap
LOOM_MAX_PREEMPTIONS=4 EXL_TIMEOUT=7200 CARGO_BUILD_JOBS=2 ./run_loom.sh | tee exl-bound4.txt
# bound 5 for the small tests where it is plausible to finish
for t in a1_producer_blocked_on_full_producer_queued_first a2_consumer_blocked_on_empty_producer_queued_first b1_producer_finishes_while_consumer_blocked_consumer_queued_first b2_consumer_limit_closes_blocked_producer_producer_queued_first b3_two_producers_blocked_when_consumer_closes c3_cancel_while_producer_finishes_and_consumer_blocked e4_items_queued_at_producer_done_are_delivered; do
  /usr/bin/time -f "$t %e s" env LOOM_MAX_PREEMPTIONS=5 RUSTFLAGS="--cfg nata_loom" CARGO_TARGET_DIR=/tmp/nata-exl-target cargo test --release --test loom_exchange $t -- --exact --nocapture
done
# exhaustive (no preemption bound) for the one-worker models: they are small
for t in a3_one_worker_consumer_queued_first a3_one_worker_producer_queued_first b2_consumer_limit_closes_blocked_producer_one_worker c5_cancel_one_worker; do
  LOOM_MAX_PREEMPTIONS=100 RUSTFLAGS="--cfg nata_loom" CARGO_TARGET_DIR=/tmp/nata-exl-target cargo test --release --test loom_exchange $t -- --exact --nocapture
done
```

The tests' default bound (3, or 2) applies when `LOOM_MAX_PREEMPTIONS` is unset; the variable overrides it for every test. `LOOM_MAX_PREEMPTIONS=100` is "effectively unbounded" for these sizes; I have not run it, and if it does not finish, loom's own `LOOM_MAX_BRANCHES` (set to 200,000 in the test driver) may stop a model first.

## Findings

- **F1 (real, found by loom, confirmed by a std test, FIXED in the copy):** a cancelled query left a stale consumer waker in the channel, and the waker -> task -> scheduler -> pipeline -> channel cycle kept the query's state alive (memory leak, not a lost wake-up). The fix is 5 lines in `close_consumers` (take and drop the stored consumer wakers when the channel closes); `leak.rs::f2` went from 0/20 to 20/20, loom `f1` passes, and the mutant that removes the fix is caught by three tests. The fix has NOT been applied to `spikes/operator-trait/src/ops.rs` (out of scope); that is a decision for whoever owns the prototype.
- No lost wake-up, deadlock, bound violation, loss, duplication or reordering found in the exchange within the bounds above. The exchange's wake protocol (store the waker and re-check under one lock; wake all waiters after a push, pop, `producer_done` and close) is what the models say it is, and removing any piece of it makes a model fail (mutants 1-6 locally; 1-16 per the GCP report; 8, 9, 12, 15, 17 again locally with the new models).


## GCP run of the whole suite on the fixed exchange (2026-11-04, run `nata-1104-165412-3ddd`, `n4a-highcpu-16` Spot, Neoverse-N3)

All 31 loom models, bound 3 (smaller where noted), one test per process, inside the `armcheck` bundle (so concurrent with other tasks; loom is deterministic, execution counts do not depend on load). **31 of 31 `rc=0`; 2,429,545 loom executions in total.** The three models that had not been re-run since the F1 fix (`c1`, `c2`, `d2`) are in this list and pass. Raw output:

```
  Executable tests/loom_exchange.rs (/tmp/nata-exl-target/release/deps/loom_exchange-24373f6a8f7a436f)
a1_producer_blocked_on_full_consumer_queued_first                      rc=0  115770 executions, preemption bound Some(3)  wall=13.5s
a1_producer_blocked_on_full_producer_queued_first                      rc=0  89263 executions, preemption bound Some(3)  wall=8.4s
a2_consumer_blocked_on_empty_consumer_queued_first                     rc=0  57797 executions, preemption bound Some(3)  wall=5.2s
a2_consumer_blocked_on_empty_producer_queued_first                     rc=0  44012 executions, preemption bound Some(3)  wall=3.9s
a3_one_worker_consumer_queued_first                                    rc=0  242 executions, preemption bound Some(3)  wall=0.1s
a3_one_worker_producer_queued_first                                    rc=0  233 executions, preemption bound Some(3)  wall=0.1s
b1_producer_finishes_while_consumer_blocked_consumer_queued_first      rc=0  51944 executions, preemption bound Some(3)  wall=4.6s
b1_producer_finishes_while_consumer_blocked_producer_queued_first      rc=0  39899 executions, preemption bound Some(3)  wall=3.1s
b2_consumer_limit_closes_blocked_producer_consumer_queued_first        rc=0  87406 executions, preemption bound Some(3)  wall=9.7s
b2_consumer_limit_closes_blocked_producer_one_worker                   rc=0  215 executions, preemption bound Some(3)  wall=0.1s
b2_consumer_limit_closes_blocked_producer_producer_queued_first        rc=0  65475 executions, preemption bound Some(3)  wall=5.0s
b3_two_producers_blocked_when_consumer_closes                          rc=0  9811 executions, preemption bound Some(2)  wall=1.6s
c1_cancel_during_full_empty_handshakes                                 rc=0  462737 executions, preemption bound Some(3)  wall=45.0s
c2_cancel_during_early_close                                           rc=0  343279 executions, preemption bound Some(3)  wall=28.0s
c3_cancel_while_producer_finishes_and_consumer_blocked                 rc=0  161602 executions, preemption bound Some(3)  wall=10.7s
c4_cancel_producer_blocked_forever_on_full_exchange                    rc=0  165256 executions, preemption bound Some(3)  wall=18.3s
c5_cancel_one_worker                                                   rc=0  8623 executions, preemption bound Some(3)  wall=0.7s
d1_bound_one_producer                                                  rc=0  229531 executions, preemption bound Some(3)  wall=19.9s
d2_bound_two_producers                                                 rc=0  260145 executions, preemption bound Some(3)  wall=19.4s
e1_order_one_producer                                                  rc=0  163149 executions, preemption bound Some(3)  wall=15.3s
e2_order_per_producer_with_two_producers                               rc=0  13720 executions, preemption bound Some(2)  wall=1.9s
e3_two_consumers_each_item_once                                        rc=0  7182 executions, preemption bound Some(2)  wall=1.2s
e4_items_queued_at_producer_done_are_delivered                         rc=0  48356 executions, preemption bound Some(3)  wall=4.1s
f1_cancel_leaves_no_waker_in_channel                                   rc=0  3879 executions, preemption bound Some(3)  wall=0.7s
x1_fifo_order_with_capacity_3                                          rc=0  1 executions, preemption bound None  wall=0.2s
x2_capacity_is_exact                                                   rc=0  1 executions, preemption bound None  wall=0.2s
x3_close_wakes_every_blocked_producer                                  rc=0  1 executions, preemption bound None  wall=0.2s
x4_push_and_producer_done_wake_every_blocked_consumer                  rc=0  1 executions, preemption bound None  wall=0.2s
x5_race_blocked_producer_vs_consumer_close                             rc=0  7 executions, preemption bound Some(4)  wall=0.2s
x6_race_blocked_consumer_vs_push                                       rc=0  7 executions, preemption bound Some(4)  wall=0.1s
x7_pop_wakes_every_blocked_producer                                    rc=0  1 executions, preemption bound None  wall=0.1s
```
