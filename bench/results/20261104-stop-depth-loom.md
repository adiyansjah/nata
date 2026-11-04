# Loom model of the `stop_depth` early-finish protocol (arrow-pipeline spike)

Date: 2026-11-04. Status: **model and mutants written; RUN ON GCP on 2026-11-04 (see the section at the end): 16 of 16 loom models pass, 13 of 13 mutants are caught.** The text below was written before the run; the columns marked 'to be filled' are in the raw output at the end. Every number below that says "to be filled" comes from the GCP run (rule: all runs happen on GCP; this box only compile-checks).

Question: does the early finish of ADR 0001 (LIMIT through a per-stage `stop_depth`, `Source::finish` closing the exchange, `wake_pipeline` on stop) keep its promises under **every** thread interleaving within stated bounds? Until now `spikes/arrow-pipeline` had differential tests against DuckDB and a property test (1,500 random LIMIT cases) for it, but no model checking (`20261104-arrow-pipeline.md`). This is the formal half of the evidence rule (`docs/way-of-working.md`, sections 5, 6 and 8); a check counts only because it is shown to fail on mutants.

## Choice made: a small model crate that includes the real files by `#[path]`

`spikes/arrow-pipeline/loom-model/` (own `[workspace]`, own target dir) compiles **the spike's own files**, not copies:

| included by `#[path]` | what |
|---|---|
| `src/proto/{mod,api,sched,sync}.rs` | the scheduler, `stop`, `stop_depth`, `finish_depth`, `wake_pipeline`, the protocol |
| `src/exchange.rs` | the exchange (channel, `close_consumers`, `Source::finish`) |
| `src/limit.rs` | the real LIMIT operator |

The only thing that is not the spike's is `loom-model/src/batch.rs`, a 100-line stand-in for the Arrow `Batch` (values + selection vector, the methods the included files call, and a counter of live batches).

**Why not put the model in the main crate.** Tried and measured: `RUSTFLAGS="--cfg nata_loom" cargo check --lib` on the main crate fails with 6 type errors (`src/sinks.rs` takes std mutexes through the scheduler's `lock()`, which becomes `loom::sync::Mutex`). Making the main crate loom-capable would mean changing the Arrow sinks for nothing, and every mutant rebuild would compile Arrow and parquet. The main crate therefore refuses the cfg with a `compile_error!` (`src/lib.rs`), and the model crate builds in seconds. Drift is impossible by construction: there is no second copy of the scheduler, the exchange or LIMIT.

### Edits to the real sources (all inside `spikes/arrow-pipeline/`, all compile-checked, none run)

1. `src/proto/sync.rs`, `src/proto/sched.rs`: cfg name `loom` -> `nata_loom` (3 `#[cfg]` guards + comments), the same change as `spikes/exchange-loom/SNAPSHOT.md`. This switches ON the scheduler's loom-only assertions (a task starts only from `Queued`: no task runs twice; when `wait()` returns the queue is empty and every task is `Done`). Mutant 12 below proves they are live.
2. `src/exchange.rs`: `Mutex` and `AtomicUsize` come from `crate::proto::sync` (they were std's, so the channel lock was invisible to loom). **Behaviour change, please read:** `close_consumers` now also takes and drops the consumer wakers that are still stored (finding F1 of `20261104-exchange-loom.md`: a stale waker owns its task, the task owns the scheduler, the scheduler owns the channel; a cancelled query leaked itself). The arrow-pipeline copy of the exchange did not have that fix. By reading the code, without running it, a `cancel()` with a consumer parked on an empty exchange leaks there too; mutant 10 reverts the fix so the GCP run shows whether the models see it. I ported the fix so that the unmutated control can pass; it is 3 lines and the same as in `spikes/exchange-loom/src/exchange.rs`.
3. `src/limit.rs` is new: `Limit`, `LimitOp` and `truncate` moved verbatim out of `src/ops.rs` (imports only; the counter now comes from `proto::sync`). `ops.rs` re-exports them (`pub use`), so no other path changed. The three LIMIT mutants of `scripts/mutate.py` now name `src/limit.rs`; all 33 anchors of that script were re-checked to occur exactly once.
4. `src/lib.rs`: `pub mod limit;` and the `compile_error!` under `nata_loom`. `Cargo.toml`: `cfg(nata_loom)` added to `check-cfg`, comment.

## Properties, and the test that checks each

Fixtures (all in `loom-model/tests/loom_stop.rs`): a morsel source with a shared claim counter (`quiet` mode: after its morsels it stays `Blocked` and never fires a waker); the **real** LIMIT wrapped in a spy; an expanding operator (one row per call, `HaveMoreOutput`) to leave input pending upstream of the LIMIT; a collecting sink with an optional **gate** (it returns `Blocked` until the first task of the pipeline ends and combines, so a task holds admitted rows exactly while a sibling stops the pipeline, and can only deliver them after the stop is published); spies around the **real** exchange sink and source.

Every test also asserts, after the query ends and all handles are dropped: the live-batch counter is 0 (every morsel created is released) and loom's own `Arc leaked` check passes. A deadlock (all threads blocked) is a failure: that is how a lost wake-up, or a stop that is not propagated, shows up.

| # | property | tests | what is asserted |
|---|---|---|---|
| 1 | after a stage reached its limit it produces nothing more, and every upstream stage is told to stop and terminates | `s1_one_task_limit_drops_upstream_pending_and_stops_source`, `x1_*` (2 queue orders), `x3_*` (two exchanges, three pipelines) | the LIMIT never produces output after it returned `Finished`; the task that saw it finish never runs it again nor any stage before it (the expanding operator, whose input was still pending); the source is read exactly `ceil(k / rows)` times; the producer's exchange sink was told `Finished` (x1: producer, x3: the middle stage); no hang |
| 2 | the per-stage rule: stopping stage i drops what is pending before i and nothing after; it stops nothing that still has consumers | `s2_*` (2 and 1 workers), `s3_*` (`[pass, LIMIT, pass]`), `s4_*` (2 queue orders), `x2_*` | s2/s3: rows the LIMIT admitted and a sibling's stop found parked on a blocked sink are still delivered (result has exactly 1 row; a gate makes the order deterministic: the delivery can only happen after the stop). s4: two pipelines of one query, A has LIMIT 1 and stops, B must deliver all 3 morsels. x2: an exchange with 2 consumer tasks is closed only by the last one: a producer is never told `Finished` while a consumer task is still live |
| 3 | no lost wake-up, no task run twice, no leak at shutdown | `s5_*` (2 and 1 workers), the scheduler's loom assertions in every test, `x4_*`, `x5_*` | s5: a task parked on a source that never wakes it is woken by the stop (else deadlock). x4/x5: `cancel()` racing with the early close: ends with "cancelled", queue empty, every task `Done`, no stored waker left in a channel (no leaked `Arc`), every morsel released |
| 4 | the result has exactly `min(limit, available)` rows in every interleaving | every non-cancel test | row count, every value from the source, no duplicate; with one task end to end also the exact sequence |

### Bounds (stated, not chosen after the fact)

Loom explores every interleaving up to a **preemption bound** per test (`LOOM_MAX_PREEMPTIONS` overrides it for all tests; unset, each test uses the bound below, which the harness prints). 1-2 worker threads, 1-3 tasks per pipeline, 1-3 pipelines, 1-3 morsels (2 rows per morsel in s1b, 3 in s1), exchange capacity 1, LIMIT 1 (3 in s1b, 2 in s1). `loom` allows 4 threads: main + at most 2 workers are used.

| test | shape | workers | tasks | morsels | LIMIT | bound | executions | wall |
|---|---|---:|---|---:|---:|---:|---|---|
| s1_one_task_limit_drops_upstream_pending_and_stops_source | 1 pipeline `[expand, LIMIT]` | 1 | 1 | 2x3 rows | 2 | 3 | to be filled | |
| s1b_two_tasks_shared_limit_count_exact | same | 2 | 2 | 2x2 rows | 3 | 2 | to be filled | |
| s2_sibling_stop_keeps_rows_the_limit_admitted | `[LIMIT]` + gate | 2 | 2 | 2 | 1 | 3 | to be filled | |
| s2_..._one_worker | same | 1 | 2 | 2 | 1 | 3 | to be filled | |
| s3_stop_depth_with_stages_on_both_sides_of_the_limit | `[pass, LIMIT, pass]` + gate | 2 | 2 | 2 | 1 | 3 | to be filled | |
| s4_stop_of_one_pipeline_does_not_stop_the_other (+ `_b_queued_first`) | 2 pipelines in one query | 2 | 1 + 1 | 2 and 3 | 1 on A | 3 | to be filled | |
| s5_task_parked_on_a_quiet_source_is_woken_by_the_stop (+ `_one_worker`) | `[LIMIT]`, quiet source | 2 / 1 | 2 | 1 | 1 | 3 | to be filled | |
| x1_consumer_limit_stops_the_producer_{producer,consumer}_queued_first | producer -> X -> consumer `[LIMIT]` | 2 | 1 + 1 | 3 | 1 | 3 | to be filled | |
| x2_shared_exchange_is_closed_only_by_the_last_consumer | producer -> X -> 2 consumer tasks | 2 | 1 + 2 | 3 | 1 | 2 | to be filled | |
| x3_stop_crosses_two_exchanges | producer -> X1 -> middle -> X2 -> consumer | 2 | 1 + 1 + 1 | 3 | 1 | 2 | to be filled | |
| x3_..._one_worker | same | 1 | 1 + 1 + 1 | 3 | 1 | 3 | to be filled | |
| x4_cancel_racing_with_the_early_close | producer -> X -> consumer, `cancel()` | 2 | 1 + 1 | 3 | 1 | 3 | to be filled | |
| x5_cancel_three_pipelines_one_worker | three pipelines, `cancel()` | 1 | 1 + 1 + 1 | 3 | 1 | 3 | to be filled | |

16 tests. The execution counts are unknown until the GCP run (the 2-pipeline models of `exchange-loom` took 40,000-460,000 executions at bound 3; the 3-pipeline models x3/x5 are the ones that may be large, which is why x3 uses bound 2 with 2 workers). The harness reports a timeout (default 3,000 s per test) as a failure, never as a catch.

## Mutants (13, applied to a private copy by `loom-model/mutants/mutate.py`)

The script copies the files the model compiles to `$MUT_ROOT/work`, applies one edit that must occur exactly once (else **INVALID**), builds it (a compile error is **INVALID**, never a catch), and runs the tests aimed at that mutant plus the cheap one-worker tests. **CAUGHT** needs a test that passed on the unmutated copy to fail (assertion, loom deadlock, `Arc leaked` or a scheduler assertion); a timeout is not a catch. The real sources are hashed before and after (`real sources unchanged`). Copies use `shutil.copy` (fresh mtimes). Each shard first runs its baseline: the unmutated copy must pass every test that judges its mutants.

| # | mutant | the edit | aimed at | expected to be caught by | result |
|--:|---|---|---|---|---|
| 1 | stop_depth_replaced_by_pipeline_wide_flag | `run()`: `depth = stop_depth.min(len)` -> `depth = len` (a stop drops the pending input of every stage, also behind the finishing one) | property 2 | s2, s3: the admitted row parked on the gate is dropped, result 0 rows | to be filled |
| 2 | stop_flag_shared_by_all_pipelines | `run()`: `p.stop` -> `any pipeline's stop` (one pipeline's stop stops the whole query) | property 2 | s4: B delivers fewer than 3 morsels | to be filled |
| 3 | stop_not_propagated_upstream_source_finish_never_called | `finish_task`: `source.finish()` removed (a stopped consumer never closes the exchange) | property 1 (hang) | x1, x3: producer blocked on the full exchange for ever = loom deadlock | to be filled |
| 4 | stop_not_propagated_upstream_last_consumer_never_closes | `finish()`: `== 1` -> `== 99` | property 1 (hang) | x1, x3 | to be filled |
| 5 | limit_off_by_one_finishes_one_row_early | `take == remaining` -> `take + 1 >= remaining` | property 4 | s1 (1 row instead of 2), s1b | to be filled |
| 6 | limit_admits_one_row_too_many | `Limit::new(n)` counts `n + 1` | property 4 | s1 (3 rows instead of 2), x1 | to be filled |
| 7 | first_consumer_closes_the_shared_exchange | `finish()`: `== 1` -> `>= 1` (a stopped consumer on a shared exchange closes it for the other consumers) | property 2 | x2: producer told `Finished` while a consumer task is live | to be filled |
| 8 | wake_dropped_when_stopping | `finish_task`: `if stopped_early { wake_pipeline }` -> `if false` | property 3 | s5: the sibling parked on the quiet source is never woken = deadlock | to be filled |
| 9 | pending_input_not_dropped_when_an_operator_finishes | `advance`: the `inputs[..=i] = None` drop of the queued input before the finishing stage removed ("queued morsels not drained at stop") | properties 1, 3 | s1: the expanding operator runs again after its task's LIMIT finished | to be filled |
| 10 | stale_consumer_wakers_kept_when_the_exchange_closes | `close_consumers`: the F1 fix reverted (this is the arrow-pipeline exchange as it was before my edit) | property 3 (leak at shutdown) | x4 (cancel with a consumer parked on the empty exchange): `Arc leaked`, morsels still alive | to be filled |
| 11 | shutdown_does_not_release_the_tasks | `wait()`: `all_tasks.clear()` removed (tasks and scheduler keep each other, and the queued morsels, alive) | property 3 | every loom test: `Arc leaked` | to be filled |
| 12 | stop_wakes_by_enqueueing_directly_task_runs_twice | `wake_pipeline`: `TaskCell::wake` -> `enqueue` regardless of state (a running or finished task would run twice) | property 3 (no task twice) | s1, s2: scheduler assertion "task started from a state other than Queued" | to be filled |
| 13 | cancel_does_not_wake_the_parked_tasks | `cancel()`: wake loop disabled | property 3 | x4, x5: deadlock | to be filled |

The six mutants asked for: stop_depth -> pipeline-wide flag = 1 (and 2, the query-wide variant); stop not propagated upstream (hang) = 3, 4; off-by-one in the limit = 5 (and 6); a stopped consumer on a shared exchange stops the others = 7; a wake dropped when stopping = 8; queued morsels not drained at stop = 9, with 10 and 11 for the leak at shutdown. The "expected to be caught by" column is my prediction from reading the code, not a result; a mutant that comes out SURVIVED is a finding about the model (or an equivalent mutant), reported as such.

**One thing the models cannot distinguish, said openly.** The line in `run()` that drops the pending input before `stop_depth` when a SIBLING stopped the pipeline is, in this protocol, an optimisation as far as the output goes: a task that keeps that input runs it into the already exhausted LIMIT, which returns `Finished` with no rows. The count of rows cannot tell, and the extra work of one call is within what a task may legitimately do after the stop. I therefore did not make a mutant of it (it would likely come out SURVIVED as an equivalent mutant); mutant 9 covers the same drop on the task's own finish, where the effect is observable. This is a *reasoned* statement.

## The unmutated control and the driver check

- **Control**: step 3 of the script runs all 16 tests on the real, unmutated sources (table with rc, executions, bound, wall); step 4 runs the same tests on std threads (200 repetitions each); each mutation shard repeats its baseline on its private copy and aborts with exit status 2 if the unmutated copy fails.
- **Driver check** (the defect found twice on 2026-11-04: a reused target directory plus restored files with old mtimes makes cargo reuse the previous mutant's binary): `loom-model/mutants/mutate-reuse-check.sh` runs `mutate.py` twice in a row on one root; both baselines must pass. `--demonstrate-failure` runs it with the known-bad copy (`MUT_COPY2=1`, i.e. `shutil.copy2`) and requires the second baseline to FAIL, which shows the check can fail.

## GCP command

Needs: a clean Debian VM, the repository checked out, rust installed with the toolchain pinned by `rust-toolchain.toml` (the script names it explicitly), `python3`, `build-essential` (loom's `generator` dependency uses a C compiler), no network after the crates are fetched. Not needed: DuckDB, TPC-H data.

```
cd <repo> && spikes/arrow-pipeline/gcp-check-loom.sh /tmp/stop-depth-loom-summary.txt
# optional: LS_TIMEOUT=900 (first run), LOOM_MAX_PREEMPTIONS=4 (one deeper bound for every test),
#           LS_ONLY / LS_SKIP / LS_MUTANTS (egrep on names), LS_WORKERS=<mutant shards>, LS_WORK=<scratch dir>
```

It writes the plain-text summary (counts, the model table, one line per mutant) to the path given, full logs to `<path>.d/`, and exits non-zero on any failed model, stress failure, SURVIVED / INVALID / ERROR mutant, failed baseline, failed driver check or changed source. Run time: unknown (no run yet); a guess, labelled as a guess, is 30-90 minutes on 8 vCPUs, dominated by the 2-worker models and the mutants that are caught by them; `LS_TIMEOUT` bounds the worst case per test. Machine type, region, date and cost go next to the result when it is run (way-of-working section 8).

GCP results: to be filled.

## What was checked locally (compile only, one command at a time through `scripts/local-heavy.sh`)

- `cargo check --tests` of `spikes/arrow-pipeline` (std): passes, no warnings; `scripts/mutate.py`'s 33 anchors each occur exactly once.
- `cargo check --tests` of `loom-model/` with and without `RUSTFLAGS="--cfg nata_loom"`: passes.
- `RUSTFLAGS="--cfg nata_loom" cargo check --lib` of the main crate: fails by design (the `compile_error!`); before that was added it failed with the 6 type errors described above.
- `mutants/mutate.py --check` (every mutant applied to a private copy and `cargo check --tests` under `--cfg nata_loom`): all 13 apply exactly once and compile, so none will be INVALID for that reason.
- Not run: any test, any loom exploration, the mutation runs, the driver check, `gcp-check-loom.sh`.

## Limits (what this evidence cannot show)

- **Nothing here is a result yet.** The assertions of the fixtures were reasoned through by hand for the unmutated code (for example "the producer of x1 can never complete by itself, so it must be told `Finished`"); a wrong assertion would show up as a failing control on GCP, not as a hidden pass.
- Bounded: 1-2 workers, at most 3 tasks, at most 3 morsels, preemption bound 2-3. A bug that needs more workers, tasks, morsels or preemptions is not excluded.
- Not modelled: two tasks of one pipeline finishing at *different* depths at the same time (`stop_depth` is the maximum; needs a LIMIT and a closing exchange sink in one pipeline); several LIMITs in one pipeline; one exchange with several consumer pipelines (the exchange is work-sharing, one consumer pipeline); `SinkResult::Finished` other than from a closed exchange; errors from `combine` / `finalize`; the Arrow batch itself (the stand-in has a selection vector but no columns).
- Everything is `SeqCst`; loom does not model weaker orderings or the hardware memory model of ARM64. Spurious condvar wake-ups are not modelled.
- The mutants predicted "caught by" are predictions. The F1 port changed the exchange the differential tests ran against; those tests were not re-run (compile-checked only).


## GCP results (2026-11-04, run `nata-1104-165412-3ddd`, `n4a-highcpu-16` Spot, Neoverse-N3, 16 vCPU)

The task ran inside the `armcheck` bundle, concurrently with other tasks (load average 15-32), which does not matter for a model checker. Raw output of `gcp-check-loom.sh`, unedited:

```
== loom model of the stop_depth early-finish protocol (spikes/arrow-pipeline/loom-model)
date (UTC): 2026-11-04T09:59:41Z
host: Linux 6.1.0-54-cloud-arm64 aarch64, 16 vCPUs, 31.3 GiB
rustc: rustc 1.99.1 (58cf8f9c3 2026-10-13)   cargo: cargo 1.99.1 (5f94df478 2026-08-27)   python3: Python 3.11.2   toolchain: 1.99.1
source revision: unknown (+ uncommitted changes)
LOOM_MAX_PREEMPTIONS: unset (per-test bound from tests/loom_stop.rs, printed per test below)   per-test timeout 3000s   mutant shards: 6
load average at start: 15.83 8.45 3.37

== 1 build (RUSTFLAGS=--cfg nata_loom)
  build OK

== 2 mutant edits apply exactly once (no cargo)
  all 13 edits apply exactly once

== 3 loom models on the UNMUTATED sources = the control (16 tests, one process each)
test                                                                     rc   executions     bound     wall
s1_one_task_limit_drops_upstream_pending_and_stops_source                0    81             Some(3)   0.1s 
s1b_two_tasks_shared_limit_count_exact                                   0    5228           Some(2)   0.8s 
s2_sibling_stop_keeps_rows_the_limit_admitted                            0    111238         Some(3)   13.2s 
s2_sibling_stop_keeps_rows_the_limit_admitted_one_worker                 0    206            Some(3)   0.1s 
s3_stop_depth_with_stages_on_both_sides_of_the_limit                     0    111238         Some(3)   12.2s 
s4_stop_of_one_pipeline_does_not_stop_the_other                          0    41940          Some(3)   4.5s 
s4_stop_of_one_pipeline_does_not_stop_the_other_b_queued_first           0    36049          Some(3)   3.2s 
s5_task_parked_on_a_quiet_source_is_woken_by_the_stop                    0    103134         Some(3)   9.9s 
s5_task_parked_on_a_quiet_source_is_woken_by_the_stop_one_worker         0    274            Some(3)   0.1s 
x1_consumer_limit_stops_the_producer_consumer_queued_first               0    87406          Some(3)   11.4s 
x1_consumer_limit_stops_the_producer_producer_queued_first               0    65475          Some(3)   5.8s 
x2_shared_exchange_is_closed_only_by_the_last_consumer                   0    16097          Some(2)   2.2s 
x3_stop_crosses_two_exchanges                                            0    7786           Some(2)   1.4s 
x3_stop_crosses_two_exchanges_one_worker                                 0    853            Some(3)   0.2s 
x4_cancel_racing_with_the_early_close                                    0    343279         Some(3)   34.0s 
x5_cancel_three_pipelines_one_worker                                     0    24793          Some(3)   1.7s 
  loom models: 16 passed, 0 failed, of 16

== 4 the same tests on std threads (stress loop, LS_STRESS=200 runs each)
  std stress: 16 passed, 0 failed

== 5 two driver invocations in a row both pass their baseline (and the check can fail)
  reuse check passed: the second baseline did not see the first mutant
  reuse check can fail: with copy2 the second baseline reused the first mutant's binary and FAILED, as expected

== 6 mutants (13, on private copies, 6 parallel shard(s); each shard's baseline = the unmutated copy passing the tests that judge its mutants)
  CAUGHT   stop_depth_replaced_by_pipeline_wide_flag  [s2_sibling_stop_keeps_rows_the_limit_admitted_one_worker: assertion `left == right` failed: wrong number of rows (want min(limit, available) = 1): []]  (4s)
  CAUGHT   stop_flag_shared_by_all_pipelines  [s4_stop_of_one_pipeline_does_not_stop_the_other: assertion `left == right` failed: pipeline B stopped by A's LIMIT, or lost rows: []]  (3s)
  CAUGHT   stop_not_propagated_upstream_source_finish_never_called  [x1_consumer_limit_stops_the_producer_producer_queued_first: deadlock; threads = [(Id(0), Blocked(Location(None))), (Id(1), Blocked(Location(None))), (Id(2), Blocked(Locat]  (3s)
  CAUGHT   stop_not_propagated_upstream_last_consumer_never_closes  [x1_consumer_limit_stops_the_producer_producer_queued_first: deadlock; threads = [(Id(0), Blocked(Location(None))), (Id(1), Blocked(Location(None))), (Id(2), Blocked(Locat]  (2s)
  CAUGHT   limit_off_by_one_finishes_one_row_early  [s1_one_task_limit_drops_upstream_pending_and_stops_source: assertion `left == right` failed: wrong number of rows (want min(limit, available) = 2): [0]]  (3s)
  CAUGHT   limit_admits_one_row_too_many  [s1_one_task_limit_drops_upstream_pending_and_stops_source: assertion `left == right` failed: wrong number of rows (want min(limit, available) = 2): [0, 1, 2]]  (3s)
  CAUGHT   first_consumer_closes_the_shared_exchange  [x2_shared_exchange_is_closed_only_by_the_last_consumer: assertion `left == right` failed: a producer was told Finished while 1 consumer task(s) of the exchange are st]  (4s)
  CAUGHT   wake_dropped_when_stopping  [s5_task_parked_on_a_quiet_source_is_woken_by_the_stop: deadlock; threads = [(Id(0), Blocked(Location(None))), (Id(1), Blocked(Location(None))), (Id(2), Blocked(Locat]  (3s)
  CAUGHT   pending_input_not_dropped_when_an_operator_finishes  [s1_one_task_limit_drops_upstream_pending_and_stops_source: an upstream stage ran again in the task whose LIMIT had already finished (pending input not dropped)]  (3s)
  CAUGHT   stale_consumer_wakers_kept_when_the_exchange_closes  [x4_cancel_racing_with_the_early_close: Arc leaked.]  (8s)
  CAUGHT   shutdown_does_not_release_the_tasks  [s1_one_task_limit_drops_upstream_pending_and_stops_source: Arc leaked.]  (4s)
  CAUGHT   stop_wakes_by_enqueueing_directly_task_runs_twice  [s1_one_task_limit_drops_upstream_pending_and_stops_source: task started from a state other than Queued]  (3s)
  CAUGHT   cancel_does_not_wake_the_parked_tasks  [x4_cancel_racing_with_the_early_close: deadlock; threads = [(Id(0), Blocked(Location(None))), (Id(1), Blocked(Location(None))), (Id(2), Blocked(Locat]  (9s)
  mutants: 13 CAUGHT, 0 SURVIVED, 0 INVALID, of 13; shard problems: 0

== 7 the real sources are unchanged
  byte-identical before and after (11 files hashed)

load average at end: 32.50 21.81 10.30
RESULT: PASS (models 16/16, std 16/16, mutants 13/13 CAUGHT, control OK, reuse checks OK, sources unchanged)
```

Reading it: the control (unmutated sources) passes all 16 loom models; the same 16 tests pass on std threads (200 runs each); all 13 mutants are caught, each by the test its property needs (a deadlock for the missing wake, a wrong row count for the off-by-one, an `Arc leaked` for the stale wakers and the shutdown leak); the reuse check passes and its demonstration with the old copy method FAILS as intended; the real sources are byte-identical before and after. Not modelled (as stated above): two tasks finishing at different depths in one pipeline, several LIMITs in one pipeline, and weaker memory orderings.
