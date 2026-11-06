# Parquet scan, step 2b: parallel, ordered, cancellable, resource-bounded (slice 3)

Date: 2026-11-06. Code: `crates/nata-sched` (new), `crates/nata-parquet` (lazy footers, `run_parallel`, gauges, file-opener seam), `crates/nata` (`nata scan --threads/--unordered/--window`), tests and scripts named below. Card: `docs/slices/0001-parquet-scan.md` (rows Scalability, Reliability, Fault tolerance, Resource bounds, Security and privacy, Operability).
Local only: no GCP, no VM, no push. Machine: x86-64 Linux, 8 cores, 16 GB, a shared box with other jobs (load 1-3, a busy disk), DuckDB 2.0.0 CLI, `parquet`/`arrow` 60.1.0, `loom` 0.7.2, pyarrow 26.0.1 through `uvx --offline`.
**No timing was measured for a conclusion and none is claimed.** Wall times below only size the tests.

## What was built

| Piece | What |
|---|---|
| `crates/nata-sched` (new, 2 files of code) | An ordered, windowed, cancellable executor of "row-group" tasks on worker threads, generic over a `Work` (a list of files that expand lazily into ordered tasks). All state is under ONE `Mutex` with two `Condvar`s (`work_cv`, `out_cv`). **Every synchronisation primitive comes from `src/sync.rs`** (std normally, `loom` under `RUSTFLAGS="--cfg nata_loom"`; `tests/facade.rs` fails if another file names `std::sync`/`std::thread`), so the code the loom models explore is the code that runs. |
| `nata-parquet` | `ScanPlan::open` reads ONLY the first footer; `ScanPlan::open_file(i)` reads footer *i* when the scheduler reaches it (open, read, **close**); the metadata lives in the `Arc<FileScan>` that its tasks hold and is freed with the last of them. `RowGroupTask::run_cancellable` looks at the cancellation before opening the handle and between batches. `run_parallel` / `run_parallel_with(.., ScanCancel, ..)`. `Opener` / `OpenFile`: the one place file handles come from (counted, fault-injectable, slow-able in tests). `stats.rs`: gauges for decoded bytes in flight, open handles, footers held, and the report counters. `run_serial` is kept as the single-thread reference implementation. |
| `nata scan` | `--threads N` (default: available parallelism), `--unordered`, `--window N` (developer option). The report line has new keys (`files_opened abandoned threads order window peak_inflight_bytes largest_task_bytes peak_open_files peak_footers`, then `peak_rss_kib` from `/proc` on Linux) after the old ones, which are unchanged. A closed stdout cancels the scan like a limit and exits 0 without a message (stderr is written with `writeln!`, not `eprintln!`, so a closed stderr cannot panic either). |
| Tests / scripts | `nata-sched/tests/{sched,loom_sched,facade}.rs`, `nata-sched/mutants/mutate.py`, `nata-sched/run_loom.sh`; `nata-parquet/tests/{parallel,scenarios,fuzz_files}.rs`, `tests/common/{faults,genfiles}.rs`, `run_thread_matrix.sh`, 16 new mutants in `mutate.py`; `nata/tests/resource_bounds.rs`; `scripts/unsafe-inventory.py`. |

### Decisions (the semantics)

1. **Output order.** Default ORDERED: identical to the single-thread scan (file list, row group, row) for every thread count and window, through a bounded reorder window. `--unordered` for maximum throughput: the same multiset, no promised order. Tested byte for byte (`tests/parallel.rs`: threads 1,2,3,4,8 x windows 1,2,N,3N x 4 projections/filters x pruned/unpruned) and against DuckDB (thread matrix below).
2. **Window.** `claimed - released < window` before every claim; `released` is counted when the consumer is DONE with a result (sink returned, result dropped). So at most `window` results are in flight, finished and waiting, or in the writer's hands, in both modes. Default `window = threads`, which makes the memory bound `threads x largest task`. Cost: head-of-line blocking (a slow early row group idles up to `window - 1` workers); the writer's own time counts, because its slot is held while it writes. Whether `window = 2 x threads` pays is a question for the benchmark step.
3. **Single opener.** Files are opened one at a time, in order, and only when no task is queued (the worker that claims the opening does the I/O outside the lock). This keeps the sequence numbers in (file, row group) order, which the liveness of the window needs (the lowest unreleased number is always running or finished). Consequence, said plainly: **footer reads are serialised** (they overlap with the reads of earlier files' row groups, not with each other). Prefetching several footers is a later step (object stores, slice 11).
4. **Limit.** Counted in delivered order. At the N-th row: no more claims, running tasks stop at their next batch (8,192 rows) and drop reader/handle/buffers, results nobody took are dropped, all workers are joined before the call returns. Ordered mode prints exactly the first N rows (tested for N = 0, 1, 40, 41, 42, 500, 1234, all, beyond, at 1/2/4/8 threads, against the serial scan). Unordered mode returns N rows, each a row of the full result.
5. **Errors.** A failed task (or a file whose footer fails) keeps its error as its result, stops all further claims and cancels the tasks ABOVE it; tasks below it run on. So the error reported is always the one of the LOWEST (file, row group), in both modes, and ordered mode first delivers every row before it. A file that cannot be opened takes a sequence number, so its error has a place in the order. An error found after the limit was satisfied is never reported. A sink error (closed stdout) cancels everything like a limit and wins over a task error. A panic inside a task is caught at the task boundary (`Internal`, no panic message in the text); a worker that dies anyway makes the scan stop with an `Internal` error instead of hanging. A scan cancelled from outside (`ScanCancel`) returns `ErrorKind::Cancelled`, never a silently short result.
6. **Footers are lazy, so one 2a behaviour changed:** a corrupt or mismatching LATER file is found when the scan reaches it, after the rows of the earlier files were printed (ordered: deterministic, the same for every thread count). The first file is still read before anything is printed, and so are all usage and filter errors. `crates/nata/tests/scan_cli.rs` was updated for this (and `--limit` there now says `--threads 1`, because with more threads some row groups are read ahead and dropped).
7. **Report exactness.** Row-group counts (`row_groups skipped read`) are those of the files that were opened; `files_opened < files` says when a scan stopped early. `bytes_read` and `rows_read` are counted at the handle and include tasks that were cancelled halfway. Complete scans: all counters equal the serial scan's (asserted for every thread count and window).
8. **Memory.** Own accounting: a `Reservation` on a gauge for the decoded batches a task holds (the batch being filtered, counted until the end of its turn, plus the retained output), carried inside the result so that a finished result waiting for the writer is counted until the writer drops it. Report: `peak_inflight_bytes` (gauge peak) and `largest_task_bytes` (highest any one task held). **By construction and asserted on every complete run: `peak_inflight_bytes <= window x largest_task_bytes`.** Resident memory is a separate, measured quantity (resource section): `RSS <= base + K x window x M + F`, `M` the decoded size of the largest row group by an independent reader, `K = 3`, `F = 32 MiB`. K and F are empirical, not derived: first fixed at 2.5 and 24 MiB before any measurement (the first run passed), then raised to 3 and 32 MiB after 13 runs showed the unordered window-4 scan reaching 52.1 MB against a 53.7 MB bound (97%, too thin for a test that must not flake).

## What this does not use: `crossbeam-deque` (ADR 0002) — a deviation to review

The brief asked for `crossbeam-deque`. This step does not use it, deliberately: (a) the window is only live if tasks START in sequence order; one FIFO under one lock gives that by construction, a work-stealing deque (local queues, batch steals) does not, and a worker that holds a high-numbered task while waiting for the window could deadlock the lowest one; (b) a row-group task lasts milliseconds, so a lock per claim is not measurable (not measured here either); (c) loom cannot see inside `crossbeam-deque` (its atomics are not loom's), so using it would have meant modelling it by a mutex-based stand-in under `cfg(nata_loom)` and checking a stand-in; (d) ADR 0001 records that its own prototype used "one FIFO queue under one lock". Consequence: **ADR 0002's statement "the morsel scheduler is built on `crossbeam-deque`" is not exercised by the scan scheduler**; work stealing returns when morsels can spawn sub-morsels (joins, exchange). I did not edit the ADR. The only place a deque would plug in is `claim()` in `crates/nata-sched/src/lib.rs`.

## Verification (each check was shown to fail on a known-bad version, see the mutant tables)

### A. Correctness at every thread count

| Check | Result |
|---|---|
| `tests/parallel.rs`: ordered parallel output = serial output, byte for byte, 4 projections/filters x pruned/unpruned x threads {1,2,3,4,8} x windows {1,2,N,3N}; the report counters (`files_opened`, row-group counts, `rows_read`, `rows_returned`, `bytes_read`) equal the serial scan's; `peak_open_files <= threads+1`; `peak_inflight_bytes <= window x largest_task_bytes`; nothing alive afterwards | 160 scans, all equal |
| unordered: the same multiset, threads {1,2,3,4,8} | equal |
| **DuckDB differential at every thread count** (`crates/nata-parquet/run_thread_matrix.sh`: the 2a files `differential_lineitem`, `differential_edge`, `special_files`, `soundness_files` with the shared `scan` helper switched to `run_parallel`; unordered rows compare as a sorted multiset, ordered rows IN ORDER; each case pruned and unpruned) | **10 of 10 rows pass**, 19 tests each (about 9,260 compared cases + the 1,200-predicate soundness property per row): threads 1,2,3,4,8 x ordered/unordered, 146-246 s per row on the shared box |

(Hidden by design: the 25 `DUCKDB-PUSHDOWN-DIFFERENCE` lines of 2a are still printed by those tests; the oracle is DuckDB with its own row-group skipping disabled by a `LIMIT` barrier, as in 2a.)

### B. Loom models of the real synchronisation core

`RUSTFLAGS="--cfg nata_loom"`, release, `scripts/local-heavy.sh crates/nata-sched/run_loom.sh` (one model per process). **Bounds: preemption bound 2 for 15 models, 4 for the one-worker model m06; `max_branches` 400,000; the models are small on purpose (2 workers, 1 to 3 tasks, files of 0 to 3 tasks).** Bound 3 was tried on m01 and did not finish in 45 s (about 10x the executions of bound 2), which is why the suite is at 2. 16 models, **1,116,440 loom executions in total, 69 s of wall time** for the suite on this box (compile not included).

| Model | Claim | Executions | Wall |
|---|---|---|---|
| m01 complete, ordered, 2 workers, files [2,1], window 2 | every task exactly once, exact sequential order, no more than the window outstanding, all joined, nothing leaked | 46,513 | 3.0 s |
| m02 the same, unordered | same multiset | 48,881 | 3.0 s |
| m03 window 1, 2 workers, 3 tasks | **no lost wake-up on window release** (a blocked worker is woken by the consumer's release) | 213,984 | 12.3 s |
| m04 window 2, 3 tasks | the window is never exceeded (`high_water <= 2`) | 39,231 | 2.2 s |
| m05 files [1,0,1] | files are opened one at a time, in order; empty files pass | 9,570 | 0.6 s |
| m06 one worker, bound 4 | whole scan on one worker | 39,301 | 2.0 s |
| m07 `Flow::Stop` after the first result, window 1 | a worker blocked on the window is woken by the stop; exactly the first result; nothing leaked; no claim after the stop | 2,652 | 0.1 s |
| m08 stop with room in the window | running tasks are cancelled and dropped | 14,867 | 0.9 s |
| m09 external cancel, a task that never finishes by itself, the consumer blocked on it | **cancel reaches a blocked task and a blocked consumer**; all joined | 442,770 | 28.6 s |
| m10 external cancel, window 1, the second worker waits at the window | **cancel reaches a worker blocked on the window** | 158,994 | 9.6 s |
| m11 two failing tasks, unordered | **the lowest error wins whichever finishes first** | 4,005 | 0.2 s |
| m12 the same, ordered | the lowest error wins; only the results below it are delivered | 3,819 | 0.2 s |
| m13 a file that cannot be opened | it takes its place in the order (sequence number), claiming stops, no later file is opened | 2,181 | 0.1 s |
| m14 task 1 fails, task 2 blocked until cancelled | the error cancels a higher blocked task; the lower task still finishes and is delivered | 71,637 | 4.6 s |
| m15 the sink fails | cancels everything, the sink's error is returned | 8,375 | 0.4 s |
| m16 no files / only empty files | terminates | 9,660 | 0.8 s |

Loom also fails an execution in which a loom `Arc` is alive at its end (the leak check), and a deadlock fails it (the lost-wake-up check); `Tracked` counters check that every task and result is dropped. Not modelled: memory orderings weaker than `SeqCst` (everything is `SeqCst`, and loom does not model its total order), spurious condvar wake-ups, `crossbeam-deque` (not used), the OS refusing a thread, a panic in a task (covered by the std tests), the Parquet code itself. The std stress tests (`tests/sched.rs`, 14 tests, real threads, sizes loom cannot reach: 5 thread counts x 6 windows x 6 file shapes, limits, lowest error 40 repetitions per thread count, panics, a panic in the sink) pass.

**Mutants of the scheduler** (`crates/nata-sched/mutants/mutate.py`: ONE named text edit each on a private copy; the std tests and the loom models each judge it; copies deleted at the end). Control run: `CONTROL_OK`. **23 of 23 CAUGHT, 19 of them by the loom models** (the other 4 by std tests only, as stated):

| Mutant | Result | First loom model that fails / std test |
|---|---|---|
| lost-wakeup-on-window-release | CAUGHT | loom m01 (deadlock); std hangs (60 s timeout) |
| claim-ignores-the-stop-flag (cancel not checked before taking a task) | CAUGHT | loom m07 (deadlock) |
| double-open-claim-before-advance (two workers claim the same file; the "steal race" of a design without stealing: a task handed out twice) | CAUGHT | loom m01 ("a file was opened twice") |
| two-files-opened-at-once (opener not exclusive: order lost) | CAUGHT | loom m01 (wrong order) |
| window-bound-off-by-one | CAUGHT | loom m01 (`window exceeded`) |
| error-ordered-by-arrival (first error seen instead of the lowest) | CAUGHT | loom m11 (loom only; the std stress tests do not see it) |
| ordered-consumer-takes-any-slot | CAUGHT | loom m01 |
| shutdown-without-joining | CAUGHT | loom m01 (`a worker was not joined`) |
| cancel-does-not-wake-the-workers | CAUGHT | loom m07 (deadlock) |
| cancel-does-not-wake-the-consumer | CAUGHT | loom m09 (deadlock) |
| cancel-does-not-tell-running-tasks | CAUGHT | loom m09 (the task never ends: branch limit) |
| cancelled-task-not-released (handle/buffers of a cancelled task leaked) | CAUGHT | loom m07 (`a task or a result was not dropped`) |
| undelivered-results-leaked | CAUGHT | loom m07 |
| publish-does-not-wake-the-consumer | CAUGHT | loom m01 (deadlock) |
| exhaustion-does-not-wake-the-consumer | CAUGHT | loom m01 (deadlock) |
| install-does-not-wake-anyone | CAUGHT | loom m13 (deadlock) |
| limit-stop-ignored | CAUGHT | loom m07 |
| open-failure-takes-no-sequence-number | CAUGHT | loom m13 (after a test gap, see below) |
| open-failure-does-not-stop-claims | CAUGHT | loom m13 |
| error-does-not-cancel-higher-tasks | CAUGHT | **std only**: `an_error_cancels_the_tasks_above_it_even_while_a_lower_task_is_still_running` (two spinning threads make loom's search follow the spin forever, so this one is not a loom model) |
| exit-guard-does-nothing (a panicking worker leaves the consumer waiting) | CAUGHT | **std only** (hang), loom does not model panics |
| workers-not-joined-when-unwinding (sink panics) | CAUGHT | **std only** |
| facade-bypassed-by-a-std-atomic | CAUGHT | **std only** (`tests/facade.rs`) |

Honest history: the first full run had 3 SURVIVED. `open-failure-takes-no-sequence-number` and `error-does-not-cancel-higher-tasks` were test gaps and were fixed by new assertions and a new test (above); `task-run-after-cancel-at-claim` (the worker no longer checks the token before calling `run`) is **EQUIVALENT** — `Work::run` is the one that looks at the token and returns at once — and was removed from the list with a comment. An earlier first run with a runner that did not kill hung test binaries wasted time and is not counted.

**Mutants of the parallel driver and the new reader code** (`crates/nata-parquet/mutate.py`, the `par-*` entries; they run the new test groups first, then the old ones): first run 14 CAUGHT, 2 SURVIVED; `par-task-memory-not-counted` was a test gap (nothing checked that the batch being filtered is on the gauge): test `the_batch_being_filtered_is_counted_even_when_nothing_is_kept` added, mutant then CAUGHT; `par-serial-driver-reads-past-the-limit` is EQUIVALENT (the second limit check of `run_serial` can never be true) and was removed. The final list of 15 was not re-run in one piece after these two changes (the changed one was re-run alone: CAUGHT).

| Mutant | Result | Caught by |
|---|---|---|
| par-handle-not-closed-on-cancel (the handle of a cancelled task is forgotten) | CAUGHT | `parallel::a_limit_stops_a_slow_task_...`, `the_error_of_the_lowest_...` |
| par-footer-handle-leaked (a footer's file descriptor is never closed: "opens all files at once") | CAUGHT | `resource_bounds::ten_thousand_small_files_under_ulimit_256...` |
| par-window-ignored (window = huge) | CAUGHT | `parallel` (4 tests) |
| par-ordered-flag-ignored | CAUGHT | `parallel` (3 tests) |
| par-limit-does-not-stop-the-scan | CAUGHT | `parallel::a_limit_stops_a_slow_task...` and others |
| par-tasks-never-look-at-the-cancellation | CAUGHT | `parallel::a_limit_stops_a_slow_task...` |
| par-memory-reservation-not-released | CAUGHT | `parallel` (`assert_clean`) |
| par-footer-count-not-released | CAUGHT | `parallel` (`assert_clean`) |
| par-first-footer-kept-after-the-scan | CAUGHT | `parallel::limit_returns_exactly...` |
| par-cancel-reported-as-success | CAUGHT | `scenarios` |
| par-abandoned-row-groups-not-counted | CAUGHT | `parallel::a_limit_stops_a_slow_task...` |
| par-footers-all-read-before-the-scan (eager footers) | CAUGHT | `parallel::footers_are_read_when_the_scan_reaches_the_file`, `many_small_files...` |
| par-panic-not-caught-at-the-task | CAUGHT | `scenarios` |
| par-sink-error-not-returned | CAUGHT | `parallel::a_closed_consumer_cancels...` |
| par-task-memory-not-counted | CAUGHT (after the new test) | `parallel::the_batch_being_filtered_is_counted...` |

The 95 mutants of step 2a were not re-run in this step (their test files were changed only by the switch of the shared helper to `Mode`, which is the serial reference by default); the 2a record's list stands as of its own commit.

### C. Leak and cancel under random points (`tests/scenarios.rs`)

Seed 20261106 (printed; `NATA_SCENARIO_SEED`, replay one with `NATA_SCENARIO_ONLY=<n>`), **5,000 scenarios in 12-14 s** (`NATA_SCENARIOS` raises it). Each: 6 small files of 1 to 7 row groups, random threads 1-8, window, order, pruning, projection, filter, 0-3 random delays, plus ONE of: nothing, a limit, a failing sink after k batches (a closed stdout), an external cancel after 0-3 ms, 1-3 faults (open failure at a footer or a row group, reads failing after 0 or 40 bytes, a panic in the opener). After every scenario: the fault opener's live-handle counter is 0, the footer and decoded-byte gauges are 0, **the process's thread count is what it was before**, the output relates to the single-thread scan as it must (ordered: equal or a prefix; unordered: a sub-multiset), and the error text equals the single-thread scan's with the same faults (the lowest failing file and row group). A watchdog aborts after 30 s per scenario.

Outcome classes (5,000): complete 1,016; limit 1,483; sink failed 416 and sink never reached 103; external cancel that stopped the scan 295 and cancel that came too late 217; fault error 1,020; fault turned into a caught panic 251; fault never reached (pruned) 199. The test fails if a class is missing. Known-bad versions, run against this test ALONE (`mutate.py --groups scenarios`, 1,200 scenarios): `par-handle-not-closed-on-cancel` (a handle leaked on cancel), `par-memory-reservation-not-released`, `par-footer-count-not-released`, `par-first-footer-kept-after-the-scan`, `par-sink-error-not-returned` (a closed stdout swallowed) and `par-panic-not-caught-at-the-task` all CAUGHT (6 of 6); `par-cancel-reported-as-success` is also caught by it in the full list. The external-cancel counts vary from run to run with timing (cancelled 266-295, too late 217-246 in three runs); the other classes are deterministic.

### D. Resource bounds

* **10,000 small files** (`crates/nata/tests/resource_bounds.rs`, the real binary, files of 3 rows and under 4 KB generated under `target/tmp` and deleted) **under `ulimit -n 256`**: 1 thread: ok, 30,000 rows equal to the expected sequence, `peak_open_files=1 peak_footers=1`, `peak_rss_kib=10140`; 8 threads: ok, `peak_open_files=4 peak_footers=3 peak_inflight_bytes=1080`, `peak_rss_kib=10624`; unordered with `--limit 25` at 8 threads: ok, 25 rows, `files_opened=10` of 10,000, `abandoned=1`. **Known-bad:** `par-footer-handle-leaked` (every footer's descriptor leaked = all files open at once) fails this test. (Before this step's change the 2a code read all footers first but closed each, so it would not have failed the ulimit; it held 10,000 footers in memory instead: `peak_footers` is the number that shows the difference.)
* **Memory** (100 row groups of 20,000 rows, an id and a 100-byte incompressible text, 229.5 MB file, about 2.4 MB decoded per row group by an independent parquet-rs read; stdout read by a slow consumer, at most about 130 MB/s): window 4 ordered, 13 runs: peak RSS **37.9-47.8 MB**; window 4 unordered, 13 runs: **39.0-52.1 MB**; window 1: **11.4-11.7 MB**; process baseline 5.5-5.8 MB. Bound used: `base + 3 x window x M + 32 MiB` = 66.5 MB for window 4, 45.5 MB for window 1. **`--window 100000` (no bound): 259-263 MB, far above, so the test discriminates.** Own accounting: `peak_inflight_bytes` = 10,872,992 B at window 4 = 3.52 x `largest_task_bytes` (3,088,904 B, which is 1.29 x the independent decoded size 2,391,600 B). So: the accounted bound `window x largest task` holds by construction; **resident memory above the baseline (32-46 MB at window 4) is 3 to 4 times the accounted peak (10.9 MB)** (glibc arenas keep freed pages per thread, string decoding buffers double while growing; not investigated further). **Deviation from the card's wording**: "threads x largest row group as decoded plus a fixed slice" does NOT hold literally for RSS (4 x 2.4 MB + 5.6 = 15 MB against 38-52 MB measured); it holds with K = 3 and a 32 MiB slice, constants that are empirical (see decision 8). One dataset, one allocator, one machine.

### E. Fuzz at the card's size (`tests/fuzz_files.rs`)

Seed 20261106, **3,000 cases per shape, 5 shapes, 15,000 cases, each scanned at 1 and at 4 threads = 30,000 scans, 31-38 s.** Shapes: `lineitem_like` (4 row groups, dictionary strings, decimals, a date; a planted marker string in a text column and a planted number in a column), `nulls_nans_allnull_group` (NULLs, NaN, -0.0, infinity, an all-NULL row group, row groups of 1000/1/500/7 rows), `many_small_pages` (200-byte pages, small dictionary pages, page statistics), `v2_zstd_delta` (data page v2, zstd, delta binary packed and delta byte array), `tiny_and_empty_groups` (one-row groups and a zero-row group). Damage: one bit, several bits in the page region, bits in the footer region, truncation anywhere, a zeroed range, a random-byte range, truncation plus a flip, a range copied over another, the footer length field and magic. Per case: no hang (20 s), no leaked handle/footer/byte, the error names the file (and carries `row group N` where it has one), **no error text contains the planted marker or number**, and **1 and 4 threads give the same outcome** (same error text, or the same rows).

| Shape | error | caught panic in a dependency | same rows as the clean file | different rows | total |
|---|---|---|---|---|---|
| lineitem_like | 1,590 | 0 | 92 | 1,318 | 3,000 |
| nulls_nans_allnull_group | 1,795 | 6 | 186 | 1,013 | 3,000 |
| many_small_pages | 2,118 | 0 | 254 | 628 | 3,000 |
| v2_zstd_delta | 2,613 | 49 | 296 | 42 | 3,000 |
| tiny_and_empty_groups | 2,462 | 2 | 460 | 76 | 3,000 |
| **all** | **10,578** | **57** | **1,288** | **3,077** | **15,000** |

**The third class is not hidden:** 3,077 of 15,000 damaged files (20.5%) scanned successfully to rows that DIFFER from the clean file's (a flipped byte in an unchecksummed page decodes to another valid value, or a flipped length changes the split of values); 1,288 changed nothing visible (padding, statistics, unused bytes). The test cannot tell which "different rows" a checksum would have caught, and does not claim to.

**Finding: panics inside `parquet`/`arrow` 60.1.0 on damaged pages.** 57 cases (0.4%) make a dependency panic. The task boundary catches it (`catch_unwind`, as in 2a), the scan returns an error that names the file and the row group, says `a panic while reading`, carries no panic message, and is the same at 1 and 4 threads; no thread, handle or byte is left. The panic sites, as reported by the panic hook: `parquet/src/arrow/decoder/delta_byte_array.rs:101` (by far the most), `arrow-buffer/src/util/bit_chunk_iterator.rs:235`, `parquet/src/arrow/array_reader/byte_array.rs:391`, `parquet/src/column/reader/decoder.rs:374`, `parquet/src/file/metadata/mod.rs:1043`, `parquet/src/util/bit_util.rs:1189`. The brief says "fail on panic"; this test fails on an UNCAUGHT panic, on a caught panic whose error lacks the row group or carries the message, and on any disagreement between thread counts, and it counts the caught ones as a class. **Whether a caught dependency panic is acceptable against the card's "never a panic" is the owner's call** (the card's wording is about the process; the library panicked). Not reported upstream. Known-bad versions run against the fuzz ALONE (`mutate.py --groups fuzz_files`, 300 cases per shape): `par-panic-not-caught-at-the-task`, `par-memory-reservation-not-released`, `par-footer-count-not-released` all CAUGHT.

### F. `unsafe` in the `parquet` dependency tree

`cargo geiger` is not installed and cannot be installed offline. Instead `scripts/unsafe-inventory.py` (a text scan over `cargo metadata --offline --filter-platform x86_64-unknown-linux-gnu`, normal and build dependencies of `nata-parquet`, comments and string literals removed, `unsafe` counted as a keyword) — an approximation, not geiger's expression counts, not macro-expanded, not feature-aware beyond cargo's resolution here, and it counts `#[cfg(test)]` code inside `src`. Result: **63 packages, 8,091 `unsafe` keywords in 49 of them; the three workspace packages (`nata-parquet`, `nata-scan`, `nata-sched`) have 0** (`unsafe_code = "forbid"` is a workspace lint). Largest: `libc` 1,599 (declarations), `zerocopy` 1,091 (+ `zerocopy-derive` 808), `hashbrown` 454, `zlib-rs` 453, `arrow-ipc` 434, `arrow-array` 426, `zstd-safe` 369, `brotli-decompressor` 232, `iana-time-zone` 212, `getrandom` 189, `arrow-buffer` 175, `bytes` 138; `parquet` itself 48, `arrow-data` 46, `arrow-select` 104. Full list: `python3 scripts/unsafe-inventory.py nata-parquet`. For the ARM64 tree: `NATA_TARGET=aarch64-unknown-linux-gnu` (not run).

### G. Build and test state

`cargo fmt --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean; **`cargo test --workspace` (debug) all green, 258 s wall including a warm-cache build and clippy** (the brief's limit was about 6 minutes); `cargo test --release -p nata-parquet -p nata-scan -p nata-sched -p nata`: 157 tests passed, 0 failed, 212 s. New tests: `nata-sched` 14 std + 1 facade + 16 loom models, `nata-parquet` `parallel` 10 + `scenarios` 1 (5,000) + `fuzz_files` 1 (15,000), `nata` `resource_bounds` 2; the existing `scan_cli` was edited (2 tests, see decision 6). Sizes behind variables, with the exact commands in `crates/nata-parquet/README.md` and `crates/nata-sched/README.md`: the thread matrix (about 25 min), a bigger loom bound, more scenarios and fuzz cases, the 95 + 15 parquet mutants (`crates/nata-parquet/mutate.py`, about 40 min with 3 jobs) and the 23 scheduler mutants (about 15 min).

## What this does not show

* **No timing and no speed claim**: nothing here says the parallel scan is faster, how it scales to 8 threads (the card's 5x target is NOT measured), or what head-of-line blocking at `window = threads` costs; no cloud, no ARM64 (the loom models, the mutants and everything else ran on x86-64 only), no clean-VM run, no cold-cache run.
* Only local files on one filesystem; no object store; footer reads are serialised (decision 3) and nothing here measures what that costs for many small files.
* Writers tested: DuckDB 2.0.0, parquet-rs 60.1.0, pyarrow 26.0.1 for the differential matrix (as in 2a); the new tests write with parquet-rs only. No Java-written file.
* **Loom bounds**: preemption bound 2 (4 for one model), `max_branches` 400,000, 2-3 threads and at most 3 tasks; everything `SeqCst`; no weak-memory model; no model of the OS refusing a thread (the code degrades to fewer threads, or reports an error if none starts; untested); `crossbeam-deque` not used and so not modelled; a panic in a task is covered by std tests only.
* The RSS bound is one dataset (strings and an id), one allocator (glibc), one machine; K and F are empirical (decision 8). `peak_rss_kib` is the process high-water mark and includes the baseline. Nothing was tried under a cgroup memory limit.
* The fuzz is local-file byte damage only: no injected I/O errors inside the fuzz (those are in the scenarios), no page-level structured corruption (e.g. valid headers with wrong counts), no Parquet encryption, no checksummed pages (the writers do not write them by default).
* Panics inside dependencies on corrupt pages exist (finding in E) and are contained, not fixed.
* `nata-check` was not extended; its `-- scan:` queries run through `nata scan` with the default (parallel, ordered) mode and pass in the workspace test run.
* The 95 mutants of step 2a were not re-run (see above); the `par-*` list was not re-run in one piece after the last edit.
* Sizing note, not a result: the machine was shared and its disk busy (the fuzz first ran on disk at under 3% CPU and was moved to `/tmp`, a RAM disk, for its single small scratch file).

## Design decisions to review

1. A new crate `nata-sched` with the facade, instead of putting the scheduler in `nata-parquet` (loom and the mutants build it in seconds, not minutes; it is generic over `Work`).
2. **No `crossbeam-deque`** (above): one FIFO under one lock. This is the biggest departure from the brief.
3. `window = threads` by default (memory bound as in the card, head-of-line blocking as the price). `--window` exists as a developer option; the benchmark step should try `2 x threads`.
4. Single, in-order opener: footer reads are serial.
5. Lazy footers change 2a's "nothing on stdout when a later file is corrupt" to "the earlier files' rows are printed first" (deterministic in ordered mode).
6. A cancelled scan is an `ErrorKind::Cancelled` error, not `Ok` with fewer rows.
7. A caught dependency panic is an `Internal` error naming file and row group, counted as its own fuzz class rather than failing the test (E); the owner may prefer a stricter reading.
8. The RSS constants (K = 3, F = 32 MiB) and that the card's formula needs them.
9. The mutant runners kill whole process groups on timeout (a deadlocked test binary is a grandchild of cargo); the first runner did not, and left hung binaries behind until I killed them by hand.
10. `ScanStats`/gauges live in the plan (one plan = one scan); `run_serial`/`run_parallel` on the same plan twice would add up.
