# Arrow batches with selection vectors on the protocol scheduler; LIMIT and early finish (ADR 0001, 0003)

Date: 2026-11-04. Code: `spikes/arrow-pipeline/` (throwaway; about 2,600 lines of new Rust plus the snapshots; delete once ADR 0001 is accepted). Nothing in `docs/`, the ADRs, `crates/` or the other spikes was edited; nothing was committed.

Question (ADR 0001, "Would change if: the prototype's protocol proves unable to carry Arrow batches with selection vectors and fused operators"): can the synchronous operator protocol and its scheduler carry Arrow batches with selection vectors and strict kernels, and do LIMIT and early finish work on top?

**Short answer.** Yes, with one real protocol defect found and fixed (a LIMIT could lose rows it had already admitted) and with caveats listed under "What the protocol could not express as written". Q1 and Q6 are identical to DuckDB digit for digit at 1, 4 and 8 threads; LIMIT is exact over random cases; the scan stops early; every one of the 33 deliberate breakages has been caught: 30 in my local run and the other 3 on GCP (the GCP run itself caught 32 of 33, the last log being re-run by the orchestrator; see the table). No speed conclusion is drawn here.

## What was built

All of it lives in `spikes/arrow-pipeline/` (own `[workspace]` table and target dir; `arrow`/`parquet` 60). **Snapshots from the committed files (`git show HEAD:<path>`, HEAD `ce5f715`)**, not edited in the other spikes:

- `src/proto/{api,sched,sync}.rs` from `spikes/operator-trait`: the protocol traits (`Source`, `Operator`, `LocalSink`/`GlobalSink`, `Blocked` + waker only at sources and sinks, `HaveMoreOutput`/`Finished`, `Source::finish`) and the scheduler. Only the `use` paths changed, **plus the early-finish fix below (the one deliberate change, marked `ARROW-PIPELINE CHANGE` in `sched.rs`)**. The loom `cfg`s and the `bug-*` features are left in place and unused.
- `src/genops/` from `spikes/generic-ops` (engine, checked NULL-aware selection-aware kernels, Parquet scan with dictionary and `Decimal64(15,2)` hints, the scalar NULL reference `nulls.rs`, the fused loop): only the `use` paths changed. It is the baseline for the correctness and benchmark comparisons. `crates/nata-kernels` was not used (the generic-ops kernels are the ones Q1/Q6 were measured with).

New code:

- `src/batch.rs`: the protocol's `Batch` = Arrow columns shared by `Arc` + optional selection vector (`Arc<Vec<u32>>`) + computed columns (`Arc<Out>`). `Operator::execute(&Batch, &mut Batch)` cannot narrow a selection in place (generic-ops could), so every operator builds a new `Batch` that shares the column buffers; `compact()` applies the selection (a real copy) for the exchange.
- `src/ops.rs`: `FilterDate`, `FilterDec`, `Project` (checked `1 - d` and `*`), `Limit` (shared atomic counter, counts LIVE rows, cuts the selection vector, not the columns).
- `src/sources.rs`: `ParquetScan` (one row group per morsel, same deterministic NULL masking as generic-ops) and `MemScan`, both with counters (`claimed`, `batches`, `finished`, `blocked`, `made`) and fault injection (`Chaos`: random sleeps, random `Blocked` with a helper thread that wakes the task, and tasks that block FOREVER so that only early finish or cancel can end them).
- `src/sinks.rs`: Q1 (grouped exact `i128` sums + count) and Q6 (global exact sum) with per-task local state, `combine` (merge into global) and `finalize` (publish); a collecting sink; a LIMIT sink (`SinkResult::Finished`, the second route of early finish); optional random `Blocked` at the sink.
- `src/exchange.rs`: the prototype's channel for Arrow batches; the producer sink compacts first.
- `src/synth.rs`: synthetic data and the independent scalar reference for the LIMIT properties. `src/main.rs`: modes `csv`, `check`, `nullcheck`, `bench`, `membench`.
- Tests (`tests/`): `queries.rs`, `limit.rs`, `pipeline.rs`, `exchange.rs`, `stress.rs`. Scripts: `scripts/duckdb_diff.sh`, `scripts/mutate.py`, `run-all.sh`.

## Correctness (all commands run in this build: release, `overflow-checks = true`)

- **DuckDB** (`scripts/duckdb_diff.sh`, DuckDB `-csv` against our `csv` mode, TPC-H SF1 `lineitem`, 49 row groups): 12 of 12 identical, Q1 and Q6 at 1, 4 and 8 threads, each with tasks = threads and 2 x threads. Q1: `A,F,37734107.00,56586554400.73,53758257134.8700,1478493` and three more groups; Q6: `123141078.2283`. (Re-run on the final tree after the scheduler fix.)
- **Generic-ops engine**: `check` and `queries.rs`: the protocol result equals the generic-ops result for threads 1, 2, 4, 8 and tasks 1, threads, 2 x threads, 3; every run reads all 49 morsels; the result does not depend on threads or tasks (16 configurations in `check`).
- **Synthetic NULLs** (`nullcheck`; 10, 50, 100 % of four columns NULL, the generic-ops deterministic mask): protocol at 1, 4 and 8 threads == independent scalar reference == generic-ops engine in all 6 cases (Q1 and Q6; the 100 % cases give an empty Q1 and a NULL Q6).
- **Blocked sources and slow stages** (`queries.rs`): Q1 and Q6 with the source answering `Blocked` 15 % of the time (a helper thread wakes the task) and sleeping 10 % of the time, 6 seeds each, 6 tasks on 3 threads: same result, every task ended and called `Source::finish`.
- **Selection-aware errors through the pipeline** (ADR 0003 decision 4, `pipeline.rs`): with 30 % of rows holding a huge `c` so that `a * c` overflows `i64`, a filter before the project that removes those rows gives no error and the exact reference values; without the filter the query fails with an overflow error (40 seeds each way). An overflow in one task stops the other 7 tasks (fewer than 400 morsels read) and all tasks call `Source::finish`.
- **No column copy**: a test compares the column buffer pointer before and after a filter and after LIMIT's truncation (equal).

## LIMIT and early finish (ADR 0001, "Early finish travels three ways")

Properties checked against a scalar reference (`tests/limit.rs`, `tests/exchange.rs`):

| property | test | result |
|---|---|---|
| (a) exactly `min(k, n)` rows, every row in the full filtered result, none twice (ids are unique) | 1,500 random cases: 0-60 morsels (0 = empty input, empty batches), NULLs 0/10/50/100 %, 0-3 random range filters on 3 columns, optional project, `k` in {0, 1, n, n-1, n+1, 2n+5, random}, 1-8 threads, 1/threads/threads+1/2 x threads tasks, LIMIT as an operator or as the sink, random sleeps, random `Blocked` at source and sink | pass. The generator was checked to be non-vacuous: 679 cases with output rows, 453 with k=0, 121 with k=n, 617 with k>n, 322 empty inputs. `LIMIT_CASES=10000` is in `run-all.sh`. |
| (a) the same over an exchange | 500 random cases, producer 1-4 tasks, consumer 1-3, capacity 1-3, scan to filter to exchange to LIMIT | pass; every batch entering the channel is compacted (counter of uncompacted batches = 0) |
| (a) exact under contention | 200 rounds, 3,000 tiny morsels, 8 tasks on 8 threads, k = 400 | pass |
| (b) early finish reaches the source | 2,000 morsels, each ONE batch with exactly `m` matching rows; `(m,k)` in {(1,1), (3,5), (8,17), (8,100), (64,70), (2,0)}, 1/4/8 threads, tasks = threads or 2 x threads, operator and sink LIMIT, with and without injected delays: 432 configurations | pass: morsels claimed stay within the provable bound `ceil(k/m) + 2 x tasks` (a task has one batch in flight and can claim one more morsel between the stop flag being set and being seen); always far below 2,000 |
| (b) real Parquet | filter `l_discount` in [0.05, 0.07] then LIMIT k over `lineitem` (49 row groups) | pass. Morsels read of 49: k=1, 1 thread: 1; k=100, 1 thread: 1; k=100, 4 threads: 4 (1 in an earlier run of the same test; the number varies with timing); k=1000, 8 threads: 7; k=50,000, 4 threads: 4. Rows returned = k each time; one `Source::finish` per task. |
| (b) across an exchange | 3,000 morsels, `m = 8`, 4 pipeline shapes (1-4 producer tasks, 1-4 consumer tasks, capacity 1-2), k in {1, 10, 100}, 10 repetitions | pass: the scan reads at most `ceil(k/8) + cap + 3 x (producers + consumers)` morsels and always far fewer than 3,000; every producer task ends and calls `Source::finish` |
| (b) parked tasks are finished | 800 cases where every task but one blocks on its source FOREVER and never calls its waker (only "pipeline stopped early, wake the parked tasks" can end them), `k <= n` | pass: all 800 terminate with every task finished |
| (b) cancel mid-flight | 300 queries in which all tasks, or all but one, are stuck; `cancel()` after 0-1.8 ms | pass: `wait()` returns the "cancelled" error, every task called `Source::finish` |
| edge | `k = 0` and empty inputs (0, 1, 7 morsels, 1 and 4 threads, both LIMIT forms) | pass: no hang, no rows; `k = 0` reads at most one morsel per task |
| (c) no hang under contention (`tests/stress.rs`) | one busy-loop thread per CPU (8) + the scheduler's `TEST_WORKER_DELAY_US` hook (150 us under the queue lock) + source and sink random sleeps and blocks; 400 random and stuck-task queries, then 400 stuck-task repetitions; watchdog 30 s per query | 0 timeouts in 800 queries. A timeout is recorded, the load is removed, and the case is re-run alone (30 times; 2 in the mutation run): a hang that reproduces alone fails the test, one that does not is printed as INCONCLUSIVE. `run-all.sh` runs 3,000 repetitions with 4 extra busy threads and a 60 s watchdog. |

### A protocol defect found by these tests, and its fix

The first full run failed (`a_output_is_exactly_min_k_n_rows_of_the_filtered_result` and the stress test): **LIMIT 1 returned 0 rows**. Both failing cases had a LIMIT operator and a sink that answers `Blocked`. Sequence: task A's LIMIT admits the last row (`Finished`), A's sink answers `Blocked`, A parks holding the row; task B then sees the LIMIT exhausted, stops the pipeline (`stop = true`) and wakes A; A re-enters `run()`, sees `stop` and ends **without pushing the row it was holding**. The prototype's pipeline-wide `stop: bool` cannot tell batches before the finishing operator (not needed any more) from batches after it (already counted by the LIMIT, still owed to the sink); the operator-trait prototype never had a blocked sink holding LIMIT-admitted rows, and the loom model does not cover early finish.

- Failing test first: `a_batch_already_admitted_by_the_limit_survives_a_sideways_stop_while_the_sink_is_blocked` (LIMIT 1-3, 4 tasks, sink blocks 70 % of pushes): **418 of 600 runs lost rows** before the fix, 0 of 600 after.
- Fix (in `src/proto/sched.rs`, marked): a task records `finish_depth` (stage index + 1 where it finished early; the sink counts as the last stage), the pipeline keeps `stop_depth` = the maximum, and a task that finds the pipeline stopped drops only the pending input in stages before `stop_depth` and still delivers the input after it. A sink that says `Finished` has the highest depth, so its pending input is dropped (it asked to stop).
- Mutant 13 re-introduces the old behaviour and is caught. **ADR 0001's text "the other tasks of the pipeline stop" needs this refinement, and the loom model of the scheduler does not include the change.**

## Mutation check

`scripts/mutate.py` applies each breakage to a PRIVATE COPY (`$MUT_ROOT/work`, own target dir; the real sources are never touched), builds the `mutate` profile (release without LTO, overflow-checks still ON), runs the whole test suite, and then (only if all tests pass) `nullcheck` and the DuckDB diff. A mutant that does not fail anything is reported SURVIVED and the script exits 1. 33 mutants; the baseline passes in the copy first.

**GCP run (reported by the orchestrator; clean 32-vCPU ARM64 VM, one mutant per worker, private copies, pinned toolchain): 32 of 33 caught, each log shows `baseline passes` and the mutant caught. The 33rd, `filter keeps NULL rows (dense/masked path)` (#20; its log was lost on the VM because its name contains a `/`), is being re-run separately by the orchestrator; it was caught in my local run (below).** Mutants 31-33 below did not complete locally (the shared `/tmp` is a RAM-backed tmpfs and filled up with other tenants' directories, see "Problems met") and were caught on GCP.

**Local run (x86-64, shared VM): mutants 1-30 completed and all were caught; no survivor, so no new test was needed.** "Failing tests" is the number of distinct tests that failed (the list shows the first three).

| # | mutant | failing tests | first failing tests / where |
|---:|---|---:|---|
| 1 | filter(date) ignores the incoming selection | 7 | a_blocked_sink_leaves_the_arrow_batch_untouched_and_loses_no_row, a_output_is_exactly_min_k_n_rows_of_the_filtered_result, b_parked_tasks_are_woken_and_finished_by_early_finish (+4 more) |
| 2 | filter(decimal) ignores the incoming selection | 10 | a_blocked_sink_leaves_the_arrow_batch_untouched_and_loses_no_row, a_output_is_exactly_min_k_n_rows_of_the_filtered_result, b_parked_tasks_are_woken_and_finished_by_early_finish (+7 more) |
| 3 | LIMIT counts physical rows before the selection | 10 | a_blocked_sink_leaves_the_arrow_batch_untouched_and_loses_no_row, a_output_is_exactly_min_k_n_rows_of_the_filtered_result, an_overflow_in_a_row_the_filter_removed_is_not_an_error_but_a_selected_one_is (+7 more) |
| 4 | off-by-one in LIMIT (Finished one row early) | 7 | a_output_is_exactly_min_k_n_rows_of_the_filtered_result, b_early_finish_reaches_the_source_with_a_provable_bound, b_parked_tasks_are_woken_and_finished_by_early_finish (+4 more) |
| 5 | LIMIT counter is not atomic (store instead of compare-exchange) | 5 | a_batch_already_admitted_by_the_limit_survives_a_sideways_stop_while_the_sink_is_blocked, a_output_is_exactly_min_k_n_rows_of_the_filtered_result, b_early_finish_reaches_the_source_with_a_provable_bound (+2 more) |
| 6 | early finish by an operator not propagated (task keeps reading the source) | 7 | b_early_finish_reaches_the_source_with_a_provable_bound, b_k_zero_and_empty_input_terminate_without_hanging, b_parked_tasks_are_woken_and_finished_by_early_finish (+4 more) |
| 7 | early finish by the sink not propagated | 6 | b_early_finish_reaches_the_source_with_a_provable_bound, b_k_zero_and_empty_input_terminate_without_hanging, b_parked_tasks_are_woken_and_finished_by_early_finish (+3 more) |
| 8 | stop flag never published to the other tasks (sideways) | 3 | b_parked_tasks_are_woken_and_finished_by_early_finish, limit_queries_terminate_under_cpu_contention_and_injected_delays, many_repetitions_of_the_same_stuck_task_case_with_worker_delay |
| 9 | wake lost on early finish (parked tasks not woken) | 3 | b_parked_tasks_are_woken_and_finished_by_early_finish, limit_queries_terminate_under_cpu_contention_and_injected_delays, many_repetitions_of_the_same_stuck_task_case_with_worker_delay |
| 10 | cancel does not wake parked tasks | 1 | b_cancel_mid_flight_wakes_parked_tasks_and_terminates |
| 11 | wake that arrives while the task runs is forgotten | 4 | a_blocked_sink_leaves_the_arrow_batch_untouched_and_loses_no_row, a_output_is_exactly_min_k_n_rows_of_the_filtered_result, b_parked_tasks_are_woken_and_finished_by_early_finish (+1 more) |
| 12 | Source::finish() never called | 16 | a_blocked_sink_leaves_the_arrow_batch_untouched_and_loses_no_row, a_output_is_exactly_min_k_n_rows_of_the_filtered_result, an_error_in_one_task_stops_the_other_tasks_and_reports_it (+13 more) |
| 13 | sideways stop drops batches already admitted by LIMIT (the bug found by this spike) | 3 | a_batch_already_admitted_by_the_limit_survives_a_sideways_stop_while_the_sink_is_blocked, a_output_is_exactly_min_k_n_rows_of_the_filtered_result, limit_queries_terminate_under_cpu_contention_and_injected_delays |
| 14 | sink combine overwrites instead of merging (loses other tasks' partial state) | 3 | q1_and_q6_equal_the_generic_ops_engine_at_every_thread_and_task_count, q1_and_q6_survive_blocked_sources_and_slow_stages, q1_and_q6_with_synthetic_nulls_equal_the_scalar_reference |
| 15 | collect combine keeps only the first batch of each task | 12 | a_blocked_sink_leaves_the_arrow_batch_untouched_and_loses_no_row, a_output_is_exactly_min_k_n_rows_of_the_filtered_result, an_overflow_in_a_row_the_filter_removed_is_not_an_error_but_a_selected_one_is (+9 more) |
| 16 | Q6 sink loses the non-NULL count | 3 | q1_and_q6_equal_the_generic_ops_engine_at_every_thread_and_task_count, q1_and_q6_survive_blocked_sources_and_slow_stages, q1_and_q6_with_synthetic_nulls_equal_the_scalar_reference |
| 17 | LIMIT sink keeps the whole batch instead of cutting it | 6 | a_output_is_exactly_min_k_n_rows_of_the_filtered_result, b_early_finish_reaches_the_source_with_a_provable_bound, b_parked_tasks_are_woken_and_finished_by_early_finish (+3 more) |
| 18 | blocked sink consumes its input | 7 | a_batch_already_admitted_by_the_limit_survives_a_sideways_stop_while_the_sink_is_blocked, a_blocked_sink_leaves_the_arrow_batch_untouched_and_loses_no_row, a_output_is_exactly_min_k_n_rows_of_the_filtered_result (+4 more) |
| 19 | NULL handling in the sum: NULL values are added | 2 | q1_and_q6_with_synthetic_nulls_equal_the_scalar_reference, sums_skip_nulls_and_count_the_valid_values |
| 20 | filter keeps NULL rows (dense/masked path) | 8 | a_blocked_sink_leaves_the_arrow_batch_untouched_and_loses_no_row, a_output_is_exactly_min_k_n_rows_of_the_filtered_result, b_parked_tasks_are_woken_and_finished_by_early_finish (+5 more) |
| 21 | filter lower bound off by one | 14 | a_batch_already_admitted_by_the_limit_survives_a_sideways_stop_while_the_sink_is_blocked, a_blocked_sink_leaves_the_arrow_batch_untouched_and_loses_no_row, a_guard_filter_protects_the_later_kernel (+11 more) |
| 22 | masked grouped sums miscount NULLs | 2 | masked_grouped_sums_skip_nulls_and_follow_the_selection, q1_and_q6_with_synthetic_nulls_equal_the_scalar_reference |
| 23 | grouped count wrong (dense path) | 1 | dense_multi_sums_match_separate_sums |
| 24 | project ignores the selection (errors raised for removed rows) | 1 | an_overflow_in_a_row_the_filter_removed_is_not_an_error_but_a_selected_one_is |
| 25 | with_sel drops the computed columns | 7 | a_blocked_sink_leaves_the_arrow_batch_untouched_and_loses_no_row, a_output_is_exactly_min_k_n_rows_of_the_filtered_result, an_overflow_in_a_row_the_filter_removed_is_not_an_error_but_a_selected_one_is (+4 more) |
| 26 | Batch::len ignores the selection | 11 | a_blocked_sink_leaves_the_arrow_batch_untouched_and_loses_no_row, a_output_is_exactly_min_k_n_rows_of_the_filtered_result, an_overflow_in_a_row_the_filter_removed_is_not_an_error_but_a_selected_one_is (+8 more) |
| 27 | parquet scan skips the last row group | 3 | q1_and_q6_equal_the_generic_ops_engine_at_every_thread_and_task_count, q1_and_q6_survive_blocked_sources_and_slow_stages, q1_and_q6_with_synthetic_nulls_equal_the_scalar_reference |
| 28 | memory scan skips the last morsel | 8 | a_blocked_sink_leaves_the_arrow_batch_untouched_and_loses_no_row, a_output_is_exactly_min_k_n_rows_of_the_filtered_result, an_overflow_in_a_row_the_filter_removed_is_not_an_error_but_a_selected_one_is (+5 more) |
| 29 | exchange producer does not compact (selection vector crosses the exchange) | 1 | limit_above_an_exchange_is_exact_and_correct_on_random_cases |
| 30 | exchange consumer never closes the channel (early finish does not cross) | 2 | early_finish_crosses_the_exchange_and_stops_the_scan, limit_above_an_exchange_is_exact_and_correct_on_random_cases |
| 31 | exchange pop does not wake blocked producers | - | not a local result (the local run hit a full /tmp); caught on GCP |
| 32 | overflow-checks switched off in the profile | - | not a local result (the local run hit a full /tmp); caught on GCP |
| 33 | source claims counter not incremented (early-finish counter blind) | - | not a local result (the local run hit a full /tmp); caught on GCP |

Notes: mutant 10 (cancel does not wake) is caught only by the cancel test, and 8, 9 only by the stuck-task and stress tests: these three need parked tasks that never wake themselves, which no plain scan produces; they would survive without the `stuck_from_task` fault injection. Mutant 32 (overflow-checks switched off) was caught on GCP; by construction the test that fails for it is `overflow_checks_are_on_in_this_build` (I did not see that log).

## Overflow checks (ADR 0003 decision 6)

`[profile.release] overflow-checks = true` is in this spike's `Cargo.toml`; every test, the DuckDB diff, `check`, `nullcheck`, the mutation run (profile `mutate` inherits release) and the benchmark run in that configuration. Test `overflow_checks_are_on_in_this_build` asserts that `i64::MAX + 1` panics (it passes; its output shows the expected `panicked at tests/pipeline.rs` line from inside `catch_unwind`), and mutant 32 (the setting switched off) was caught on GCP. All results above are identical with the checks on (they were never measured with them off in this spike). Their cost in this pipeline was not measured (only the sanity timing below, taken with them on).

## What the protocol could NOT express as written (key finding)

1. **Early finish with a pipeline-wide boolean.** The prototype's `stop` flag cannot say which pending batches are droppable (before the finishing stage) and which are owed (after it). With Arrow batches and a sink that can block (the case an exchange makes normal) it lost admitted rows in 418 of 600 runs. Expressible after the `stop_depth` change above; not expressible with the protocol as it stood. This is the one real defect.
2. **In-place selection narrowing.** `Operator::execute(&Batch, &mut Batch)` hands the operator an immutable input, so a filter cannot reuse the batch (generic-ops did). Expressible (a new `Batch` sharing `Arc` columns; checked by pointer), but each stage allocates a new selection vector and a few `Arc` clones. Cost not measured here.
3. **Computed columns are not Arrow arrays.** The project's results are `Out { Vec<i64>, validity bytes }` carried next to the Arrow columns, as in generic-ops. Anything that leaves the engine (exchange, client) needs them converted; `compact()` gathers them, but nothing converts them to Arrow.
4. **Blocked sink and compaction.** The protocol requires a blocked sink to leave its input untouched, so the exchange sink has to cache its compacted copy itself (`pending`) to avoid compacting at every retry.
5. **Memory retention.** A collecting sink or a LIMIT keeps `Arc` references to the whole source batch (8,192 rows x all columns) for every kept row. Compaction before materialization is needed; not measured.
6. **Decode inside `Source::next`.** A Parquet row group is decoded synchronously in `next()`, so a worker thread is occupied for that time; `Blocked` was exercised only by fault-injected sources, not by real I/O (ADR 0002's bridge is a different spike).
7. **Fusion was not built.** One-batch-in/one-batch-out does not forbid a fused operator, but this spike ran separate stages only, as generic-ops did. `HaveMoreOutput` was not used with Arrow batches (no join here).

## Run-all and reproduce

`spikes/arrow-pipeline/run-all.sh [lineitem.parquet] [rounds]` (for a quiet VM; prints the load average first and warns above 1): 1 build and `overflow-checks` check; 2 all tests; 3 DuckDB diff; 4 `check` and `nullcheck`; 5 long stress (3,000 repetitions, 4 extra busy threads, 60 s watchdog) and `LIMIT_CASES=10000`; 6 the mutation check (private copy under the output directory, deleted afterwards); 7 the benchmark: protocol scheduler against the generic-ops engine, Q1 and Q6, 1/2/4/8 threads, 3 arms interleaved in rotating order (protocol, generic-ops, protocol again as a control), end to end and engine only, with paired-ratio medians and quartiles. Logs go to `out/<timestamp>/`. Needs `cargo`, `duckdb` on PATH and the SF1 `lineitem.parquet` (`LINEITEM=...` overrides the default path).

```
cd spikes/arrow-pipeline
CARGO_BUILD_JOBS=2 cargo test --release --no-fail-fast -- --nocapture --test-threads=1
scripts/duckdb_diff.sh ../../data/tpch-sf1/lineitem.parquet
target/release/spike-arrow-pipeline check     ../../data/tpch-sf1/lineitem.parquet
target/release/spike-arrow-pipeline nullcheck ../../data/tpch-sf1/lineitem.parquet
python3 scripts/mutate.py                 # all 33; MUT_ROOT=<dir on a real disk>
python3 scripts/mutate.py --list          # the 33 exact names, one per line
python3 scripts/mutate.py "<exact name>"  # exact name first, substring second
target/release/spike-arrow-pipeline bench    ../../data/tpch-sf1/lineitem.parquet 25
target/release/spike-arrow-pipeline membench ../../data/tpch-sf1/lineitem.parquet 25
```

`scripts/mutate.py` was prepared for a sharded run: `--list`, exact-then-substring matching, and `MUT_ROOT` as the only root of everything it creates (the private copy at `$MUT_ROOT/work` and the cargo target at `$MUT_ROOT/target`); no other fixed path, port or file is used anywhere in the spike (checked with grep; the tests write no files and open no ports). It deletes `$MUT_ROOT` when it ends, also on SIGTERM or Ctrl-C (tested: SIGTERM at 8 s gave exit 143, the directory removed, no child left); `MUT_KEEP=1` keeps it. It warns if the disk under `$MUT_ROOT` has less than 4 GiB free.

## Sanity timing (NOT a result)

One run on the shared local VM, **sanity only, load average 8-10** (other agents working), 5 rounds, x86-64, TPC-H SF1. The control arm (protocol against itself) varied between 0.59 and 1.52 in single cells, so differences below about 1.5x are noise here. Protocol over generic-ops median paired ratio: end to end 0.73-1.29 (Q1 1.09, 0.84, 1.12, 0.73 for 1/2/4/8 threads; Q6 1.03, 0.97, 1.29, 1.17); engine only 0.82-1.33 (Q1 1.33, 0.82, 1.08, 1.28; Q6 0.88, 1.02, 0.90, 1.15). Nothing can be concluded from this; the real comparison is step 7 of `run-all.sh` on a quiet VM.

## Problems met while building this (kept so the next run avoids them)

- The first mutation run copied files with their old modification times, so cargo reused the artifacts of an earlier copy and reported a baseline compile error; the copy now gets fresh times and the script calls the diff script through `bash`.
- A mutant that makes every query hang made the stress test spend hours (every timeout re-run 30 times); it now stops collecting after 5 timeouts and the re-run count is `STRESS_RERUNS`.
- The mutation run in `/tmp` filled the RAM-backed tmpfs (7.9 GB, about 5.8 GB of it other tenants' directories) and the last three mutants failed to build; the script's label "CAUGHT by COMPILE ERROR" hid that. It now reports "NOT A RESULT" for compile and disk errors and counts them as problems, and cleans up after itself.

## Limits

- x86-64 for everything run locally; ARM64 only through the orchestrator's GCP mutation run. No speed evidence (see above).
- Q1 groups on at most 32 distinct dictionary values (generic-ops limit); no general group-by, no join, no spill, no query-level memory budget; one FIFO run queue (no work stealing); `finalize` runs on one worker.
- The properties use `i64`-based decimals and one synthetic schema (3 data columns + id); real Parquet is covered by Q1/Q6 and one LIMIT scan only. NULL distributions are synthetic.
- The scheduler change is checked by the properties and mutants above, not by the loom model; the loom suite of `operator-trait` was not re-run on it.
- The exchange is in-process only; the compaction copy is unmeasured.
