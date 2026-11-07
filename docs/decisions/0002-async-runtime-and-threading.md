# 0002: Async runtime and threading

Status: accepted (by the owner, 2026-11-05)
Date: 2026-11-03

## Context

Nata does two kinds of work: waiting on I/O (object storage, network) and CPU-bound batch processing. 0001 puts CPU work on a morsel scheduler and makes operators synchronous, with a `blocked` state and a waker. `object_store`, the planned storage crate, is built on tokio.

## Decision

- **tokio for I/O** (object storage, network, client connections), on a small runtime.
- **A dedicated CPU worker pool for operators**, sized to the available cores (cgroup aware). It runs the morsel scheduler from 0001, built on `crossbeam-deque` (an existing work-stealing deque) rather than hand-rolled atomics (`portability.md`, section 3). **Parquet decoding, including decompression, counts as CPU work** and runs on this pool.
- **Operators are synchronous and never run on the I/O runtime.** A source or sink that must wait (empty or full exchange, pending I/O) returns `Blocked` after storing a waker, and the scheduler re-queues the task when the waker fires; intermediate operators do not block (0001). The CPU pool never calls `block_on`.
- Sources prefetch bytes on tokio and hand them to the CPU pool through bounded channels, so a slow object store cannot flood memory and a busy CPU pool cannot stall I/O.

## Why

- A CPU-bound operator that does not yield blocks every I/O task on the same runtime thread. Keeping operators off the I/O runtime avoids latency spikes without making every operator yield carefully.
- Decoding and decompression are a large share of CPU time (see Evidence), so they must not run inside I/O tasks.
- Bounded channels give backpressure in both directions. The `blocked` and waker protocol is what connects 0001's async exchange to synchronous operators.

## Alternatives considered

- **A. Everything on tokio.** One runtime, least code. CPU-bound operators starve I/O tasks and cause latency spikes unless every operator yields carefully.
- **B. A second tokio runtime for CPU work, operators as async tasks** (the "dedicated executor" pattern). Reuses tokio's scheduler and wakers, and fits an async exchange naturally. This is the closest alternative. Lost on control, by reasoning only: operators would become poll-based state machines with yield points, and tokio's scheduler gives less control over per-worker state, memory accounting and NUMA placement. It must be included in the slice 11 bake-off.
- **C. rayon (or a similar fork-join pool) for operators.** The existing crate for data-parallel CPU work. Lost by reasoning: fork-join tasks model short parallel loops, not long-lived pipelines that park and are re-queued by wakers, and spill or memory accounting do not fit its model. Not tried.
- **D. Thread-per-core runtime (glommio, monoio).** Best locality and lowest overhead. `object_store` and most of the ecosystem expect tokio, glommio is Linux-only, and it is too early to pay for it.

## Evidence

- **Measured, indirectly** (`bench/results/20261103-pipeline-model-spike.md`; local files, plain threads, x86-64 only): in the optimized spike, Snappy decompression alone was 33% of a single-thread profile, and Parquet decoding as a whole (decompression, dictionary-index and bit-unpacking) was about 60-70%; the query logic was about a quarter. This supports putting decode on the CPU pool. It says nothing about tokio versus a separate pool, because the spike used no async.
- **Measured, for the blocked/waker protocol** (`bench/results/20261103-operator-trait-prototype.md`; std threads, no tokio; x86-64, and the suite also passed 6 of 6 times on an ARM64 Spot VM, `bench/results/20261104-arm64-run1-operator-protocol.md`): a blocked consumer frees the only worker thread so the producer can run; an 864-query stress over thread, task and capacity combinations found no deadlock; about 630-800 ns per batch through a capacity-1 exchange on one-row batches. The same prototype found a lost-notification bug in its own scheduler that only appeared under CPU contention, which is the kind of code this ADR says we will write and own.
- **Measured, for the tokio-to-CPU-pool bridge** (`bench/results/20261104-async-bridge.md`; delegated agent; x86-64 only; the orchestrator re-ran the default tests, two loom models on the fixed code and three mutants, and the memory measurement; the latency and stress results were NOT re-run because the shared machine was loaded and are as the agent recorded them, at load 2-6.5):
  - Loom runs the real bridge channel (`chan.rs`) and the real snapshotted scheduler (HEAD 7f2e3be): 9 models with 1-2 workers, 1 consumer task and 1-2 senders (send/block races, close races, cancel), 20k-368k interleavings each, preemption bounds 2-5. All pass; 7 mutants are each caught by a deadlock or the never-run-twice assertion (store waker without re-check, close or send without waking the consumer, close-receiver without waking senders, wake ignored while running, double enqueue, shutdown without the lock). The tokio side is a stand-in (a loom thread polls the real send future).
  - Stress: 4,200 runs on 1- and 2-thread pools (plain, 8 busy threads, delay hooks, both together), 0 wrong, 0 hung; the same stress hangs against four mutants.
  - Memory bound: peak chunks equal capacity N + producers P + 1 per consumer task in all 8 bounded cases (exactly the bound, re-run and confirmed), against 645-660 of 960 chunks with an unbounded channel; a slow producer with a fast consumer uses 0.000 cores on the pool against 1.47 for a self-waking control.
  - I/O latency: with the CPU pool saturated, the median send latency was not worse than with an idle pool (36-39 us idle, 25-27 us saturated). p99 and beyond are noise-dominated and unresolved (an idle pool's own p99 moved from 94 us to 1.3 ms between runs, and a control of plain spinning OS threads shows the same tail). The control that runs the heavy work ON tokio raised the median to about 500 us (1 ms morsels) and 5.3 ms (10 ms morsels), so the measurement can see starvation.
- **Measured, for fairness** (`bench/results/20261104-fairness.md`; delegated agent; x86-64 only; the orchestrator re-ran the starvation and fairness tests and the old suites, and the control with slicing disabled; the mutation check was also reproduced on a clean ARM64 VM):
  - The control: on the scheduler without slicing, 5 of 5 cases in which every worker is busy (1-2 workers, 2-8 CPU-heavy tasks of about 60 s) never completed a 3 ms task within 2 s; the free-worker control passed in 3 ms. Re-run by the orchestrator with the slice effectively disabled: the same 5 cases fail.
  - The fix: cooperative time slicing at morsel boundaries. A task yields once its slice (default 32 morsels or 1 ms) is used up and the run queue is non-empty; it is re-queued at the BACK as `Queued`. The task state machine and the wake protocol are unchanged; a lone task renews its slice in place.
  - Latency: a task runnable behind K heavy tasks starts within `ceil(K/W) x slice`; measured 0.95-1.09x of that bound on a quiet-ish machine, 1.6-3x at load 14 (a slack factor of 3 passes there). All 6 starvation and 14 fairness tests pass, and the old protocol and shutdown suites (15 + 1) pass on the sliced scheduler, also with a one-morsel slice.
  - Loom: 7 new models (1-2 workers, 1-2 tasks, preemption bounds 3-5; one exhaustive) plus the 9 copied ones pass (316 s for all 16). All 8 mutants of the new code are caught (6 by loom and the plain tests, 2 only by the plain tests); the 8 of 8 and the clean control were reproduced on a clean ARM64 VM.
  - **A defect found by a clean-VM run (2026-11-04):** with a 1,024-file descriptor limit (the default of a fresh Debian VM; this machine's is 524,288, which hid it) the spill test `finalize_spills_when_over_budget_and_stays_within_it` FAILS on the sliced scheduler (`spill failed: Too many open files`) and PASSES on the original scheduler (reproduced locally with `ulimit -n 1024`). Cause (confirmed by a counter, 2026-11-04): the aggregate kept one spill writer open per (task, partition) until `combine`; peak open files with 4 tasks and 256 partitions were 768 unsliced (3 workers x 256), 1,020 with the default slice and 1,024 with 1-morsel slices, and the unsliced scheduler fails the same way with 4 or more workers, so slicing worsened a latent defect. **Fixed:** `spill()` opens, appends to and closes each partition's file within one call; peak open files are 3 (one per running worker) for every slice, and the whole suite passes under `ulimit -n 1024` (re-run by the orchestrator: fairness 14, protocol 15, shutdown 1, starvation 6, two new spill-fd test files 4; the mutant that keeps the writers open is caught by all 5 checks). **Price, measured on one spill-heavy query (200,000 rows, 20,000 keys, 256 partitions, budget 200 — near the worst case, about one record per file per spill):** opens rose from 1,024 to 180,912 and the wall time from about 135 ms to about 285 ms (2.1x; the agent measured 125 vs 267 ms). Larger budgets and operators other than the aggregate (a hash join that spills) were not measured. Not yet run on a fresh VM with the fix (the cpack rerun is pending).
  - **Overhead of slicing, ARM64** (`bench/results/20261104-cpack-arm64.md`, quiet `n4a-highcpu-8` Spot VM, Neoverse-N3, 8 vCPU, 1-minute load 3.2-3.7 caused by the benchmark's own workers; paired and interleaved against an unlimited-slice control and a control twin): the default slice (32 morsels / 1 ms) costs a median paired wall ratio of 1.002 (range 0.985-1.024) and a CPU ratio of 1.00-1.02 at morsel sizes 65,536 and 1,048,576 rows; a 1-morsel slice costs 1.010 at worst; the control twin differs from the control by up to 1.2% on CPU, so differences below about 2% are not resolved. This replaces the earlier load 13-18 numbers for ARM64; x86-64 on a quiet machine is still to be measured. NOTE: that VM ran the code as of 13:48 (an earlier state of the spill-file fix). **Rerun with the final fix (open-append-close per spill) on a fresh VM, 2026-11-04, run `nata-1104-165120`, `n4a-highcpu-8`: the protocol and shutdown suites pass at `ulimit -n 1024` (15 + 1), including `finalize_spills_when_over_budget_and_stays_within_it`, which failed before the fix; fairness 14 and starvation 6 pass; the default slice costs a median paired wall ratio of 0.997-1.012 (paired range 0.979-1.038) and a CPU ratio of 0.99-1.01 against the unlimited control; bridge stress 1,500 runs per mode, 0 wrong, 0 hung** (`bench/results/20261104-cpack-arm64.md`, second section). The two new descriptor test files (`spill_fds`, `spill_fd_limit`) were not in that run's suite and are now added to it.
  - **Overhead of slicing, x86-64** (`bench/results/20261104-cpack-x86.md`, run `nata-1104-165420-b597`, `c3d-highcpu-8` Spot VM, AMD EPYC 9B14, 8 vCPU = 4 cores x 2 threads; first x86 run through the orchestrator): the default slice costs a median paired wall ratio of 0.990-1.019 and a CPU ratio of 0.96-1.04 across six cells; the control twin itself spans 0.937-1.099 (paired) and 0.971-1.021 (CPU), so on this machine differences below about 3-5% are not resolved: the data are consistent with no cost, and they exclude one above about 5%. The same suites also pass on x86-64 at `ulimit -n` 1024 (protocol and shutdown 15 + 1, fairness 14, starvation 6, bridge stress 1,500 runs per mode with 0 wrong and 0 hung, crates codecs/deque/store 1 + 2 + 6).
  - Not resolved: the default slice is chosen by reasoning.
- **Measured on ARM64** (quiet `n4a-highcpu-8` Spot VM, Neoverse-N3; the same code as the x86-64 results above, except where noted):
  - Bridge stress with the weak memory ordering of ARM64: 600 runs per configuration of Normal, Cancel, EarlyClose and Abort, with 8 busy threads and delay hooks: `wrong=0`, `hung=0` in every case (150 runs per mode; about 90-120 of the Abort/Cancel runs were really cut off mid-flight, so the interesting paths ran).
  - I/O latency with the CPU pool saturated: lag from ready to polled (microseconds, p50/p99): idle pool 10/16, CPU pool saturated 10/16, spinning OS threads 10/15; with the heavy work ON tokio: 548/1,701 (1 ms morsels) and 5,138/16,659 (10 ms morsels). The saturated CPU pool does not delay the I/O tasks; heavy work on tokio does, by roughly the morsel time. With backpressure and a saturated pool the producers wait (about 495 ms), by design.
  - Memory bound: in all 8 bounded configurations the peak queued chunks equal the bound (3, 6, 18, 6, 9, 21, 18, 50); the unbounded control reached 879 chunks.
  - Idle CPU: the real bridge (blocked task, waker stored, re-queued by the producer's send) uses 0.000 CPU-seconds per second on the pool and on tokio; the known-bad control that re-wakes itself while empty burns 1.25 cores, so the measurement can see a spinning pool.
  - `crossbeam-deque`, the codecs and `object_store`: all tests pass on ARM64 (codecs 1, deque 2, store 6 tests).
  - **Real object store, measured 2026-11-05** (`bench/results/20261105-object-store-real.md`; GCS Standard in the VM's region, native API, read-only service account; ARM64 and x86-64; 320 runs per machine, 0 not ok): the read path of the split design works against a real bucket. First byte p50 25-36 ms, p99 42-188 ms; the simulated store of item 27 was conservative (real wall time 0.49-0.95 of simulated, median 0.72-0.74); 16 requests in flight are 2.3-13.5x faster than one; merging ranges and reading 8 row groups per unit wins on layouts with many row groups at the price of up to 4-5x more bytes. Not covered: DuckDB as a reference on the real store, other regions and storage classes, concurrent queries sharing connections.
  - Still not covered: real network wakeups at scale; tail latency beyond p99 (noise-dominated); x86-64 on a quiet machine.
- **Reasoned:** the whole Decision and Why, including the choice of `crossbeam-deque` and the judgements on B and C.
- **Not covered, after the bridge spike (ARM64 has since been measured, see above):** a real object store and real network wakeups; the tail latency (p99 and beyond); and fairness. The snapshotted scheduler has no time slicing, so a pool full of CPU-heavy tasks starves the consumer completely (shown by the spike). Earlier text, before it: any tokio behaviour (waking a parked CPU-pool task from a tokio task is plausible, since `wake()` only enqueues, but was not built); blocking disk I/O on the CPU pool (the prototype's spill read blocks a worker during `finalize`); any async behaviour; a remote scan; contention between I/O and CPU work; how a woken pipeline is re-queued under load; ARM64. The spike also suggests, but did not show, that per-page buffer allocation contends across threads (8 threads reached 4x, 8 processes 6x), which a buffer pool at the hand-off might fix.

## Confidence

- **The tokio-to-pool wake handshake is free of lost wakeups and deadlocks, and the channel bound holds: medium-high within the loom bounds** (1-2 workers, 1-2 senders); x86-64 only.
- **Cooperative time slicing prevents starvation by CPU-heavy tasks: medium** (it works in the tested cases; it also made the spill test exceed a 1,024-file descriptor limit, a defect that is now fixed at a measured cost of 2.1x on one spill-heavy query, see above; not yet re-run on a fresh VM) (1-2 workers, up to 8 heavy tasks, x86-64, synthetic CPU-bound morsels); the overhead of slicing and the default slice are unmeasured, long morsels are not interruptible, and fairness is per task, not per query.
- **A saturated pool does not delay I/O at the median: medium.** Median only; the tail is unresolved.
- **Operators synchronous and never on the I/O runtime, with the `Blocked`/waker protocol: medium-high.** Shown to work, without tokio, in the 0001 prototype; the tokio half is untested.
- **Dedicated pool on `crossbeam-deque` rather than B or C: low.** Reasoned only; the slice 11 bake-off decides it.
- **Decode on the CPU pool: medium-high.** Supported by the profile.
- Would change if: the slice 11 bake-off (split design, a second tokio runtime, and everything-on-tokio, on a remote scan with a concurrent CPU-heavy query) shows the split costs more than it saves, or the hand-off is a bottleneck in profiles.

## Consequences

- **Local spill I/O is blocking disk I/O.** It must be decided whether the CPU pool may do it (simple, blocks a worker) or whether spill reads and writes become `Blocked` sources and sinks serviced elsewhere; the 0001 prototype blocks a worker.
- Every lock, condition variable and waker we write needs tests under CPU contention and with injected delays (`way-of-working.md`, section 2).
- We write and own the hand-off between the two pools: bounded channels, wakeups, cancellation propagating both ways. Consider a buffer pool here.
- The scheduler needs a parking and re-queue mechanism for `blocked` pipelines; its design is part of the operator-trait prototype in 0001.
- Thread counts for the tokio pool and the CPU pool need defaults that work with no tuning.
- **Fairness is the scheduler's job, and a first design is measured:** each task runs for a bounded slice (32 morsels or 1 ms by default) and yields at a morsel boundary when other tasks are queued (see Evidence). A morsel that runs for a long time cannot be interrupted, so morsels must stay short; fairness is per task, not per query, so a query that spawns many tasks gets more turns than one that spawns few; both need a decision before the multi-query scheduler.
- **NUMA.** The scheduler must not assume uniform memory access (`portability.md`, section 4). NUMA-aware placement is tier T3 in `feature-map.md`, but the scheduler interface must not rule it out.
- Local-file scans (slices 3 to 10) can run on the CPU pool directly; async only appears with S3.
- Before acceptance, check that `object_store`, `crossbeam-deque` and the Parquet and compression crates build and have fast paths on both x86-64 and ARM64 (`portability.md`, section 6) and note the result here.

## Revisit when

Before acceptance: (1) ~~Test the tokio-to-pool bridge~~ done within bounds: `bench/results/20261104-async-bridge.md`; re-run the latency and stress parts on a quiet machine, and on ARM64. (2) ~~Test fairness~~ done within bounds: `bench/results/20261104-fairness.md`. Remaining: the overhead of slicing and the default slice on a quiet machine (x86-64 and ARM64); fairness per query; per-worker queues. (3) The crate checks on ARM64 (`object_store`, `crossbeam-deque`, the compression crates). Later: slice 11 produces the bake-off result, or the hand-off proves to be a bottleneck in profiles.

## Revision notes

- 2026-11-06, the pool sizing is checked (plan item 9, `bench/results/20261106-cgroup-pool-size.md`): the scan scheduler's default size follows
  cgroup v2 CPU quotas (rounded down) and affinity masks, the smaller limit winning.

- 2026-11-06, **the first product scheduler does not use `crossbeam-deque`** (slice 3, `crates/nata-sched`, decided by the assistant under the
  owner's delegation): the Parquet scan hands out row groups from ONE FIFO under one lock, because its ordered output needs tasks to start in
  sequence order inside a bounded reorder window (a stealing deque does not keep that order) and because loom can check the lock but cannot
  see inside `crossbeam-deque` (16 loom models, 23 of 23 scheduler mutants caught, `bench/results/20261106-scan-parallel.md`). The decision
  above stands for work whose order does not matter (hash build, aggregation); the slice that first needs that brings the deque in, behind
  the same `claim()` seam, with its own check. The cost of the single lock on millisecond tasks is measured by the slice 3 benchmark.

- 2026-11-05, **accepted by the owner, with these checks still open** (verification-plan items in brackets):
  - `object_store`, `crossbeam-deque` and the compression crates have not yet built and tested on x86-64 and ARM64 in CI [8]; this was a gate and must pass before the first slice that uses them merges.
  - Bridge latency and stress, and the overhead of cooperative time slicing, still need a re-run on a quiet machine [6, 22].
  - The spill-file fix for time slicing (1,024-descriptor limit) is verified locally only; the fresh-VM re-run is pending.
  - Pool sizing under cgroup CPU limits [9] and the split-design bake-off [10] are not started; the dedicated pool on `crossbeam-deque` stays at low confidence until slice 11.

- 2026-11-04, fairness defect found by the clean-VM run: time slicing makes interleaved tasks hold many spill files open at once and exceeds a 1,024-file limit. Confidence in cooperative slicing is reduced to medium until the fix is verified (the fix is verified locally, 2026-11-04, later; the fresh-VM rerun is pending); operators must not hold unbounded numbers of open files across a yield.

- 2026-11-04, fairness: starvation by CPU-heavy tasks reproduced (5 of 5 cases) and removed by cooperative time slicing at morsel boundaries; loom-checked (16 models) and mutation-checked (8 of 8, also on a clean ARM64 VM). Overhead, the default slice and per-query fairness remain open.

- 2026-11-04, tokio-to-CPU-pool bridge spike: loom-checked wake handshake and bounded channel (7 mutants caught), 4,200 stress runs without a hang, memory bound exact, median I/O latency unaffected by a saturated pool; found that the scheduler has no fairness (a pool of CPU-heavy tasks starves a consumer), now a consequence and an open check. The tokio half is no longer untested; the dedicated-pool-versus-alternatives confidence stays low until slice 11.

- 2026-11-03, operator-protocol prototype: the `Blocked`/waker protocol is now shown to work without tokio (one worker thread, no deadlock, 864-query stress); confidence in that part raised to medium-high; blocking is reported by sources and sinks only; added the spill I/O question and the contention-testing rule, after the prototype's own scheduler had a lost-notification bug.

- 2026-11-03, after independent review: fixed the Snappy figure (the earlier "26-33%" was not in the results; now 33% of the v2 profile); added the missing alternatives B (second tokio runtime) and C (rayon) and relabelled; narrowed the glommio/monoio claim (only glommio is Linux-only); specified `crossbeam-deque` and the synchronous-operator and `blocked` protocol that 0001 needs; split the confidence per claim.

## Amendments

None yet.
- 2026-11-07, the open comparison of the split runtime against a separate tokio CPU runtime (card 0006 rule 3, `bench/results/20261107-object-store-baseline.md`): ratios 0.90-1.08 in 10 cells on both CPU types, idle and saturated, none near the 1.2x needed to change, so **the split runtime stays**.
