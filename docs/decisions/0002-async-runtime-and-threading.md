# 0002: Async runtime and threading

Status: proposed
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
- **Reasoned:** the whole Decision and Why, including the choice of `crossbeam-deque` and the judgements on B and C.
- **Not covered:** any tokio behaviour (waking a parked CPU-pool task from a tokio task is plausible, since `wake()` only enqueues, but was not built); blocking disk I/O on the CPU pool (the prototype's spill read blocks a worker during `finalize`); any async behaviour; a remote scan; contention between I/O and CPU work; how a woken pipeline is re-queued under load; ARM64. The spike also suggests, but did not show, that per-page buffer allocation contends across threads (8 threads reached 4x, 8 processes 6x), which a buffer pool at the hand-off might fix.

## Confidence

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
- **NUMA.** The scheduler must not assume uniform memory access (`portability.md`, section 4). NUMA-aware placement is tier T3 in `feature-map.md`, but the scheduler interface must not rule it out.
- Local-file scans (slices 3 to 10) can run on the CPU pool directly; async only appears with S3.
- Before acceptance, check that `object_store`, `crossbeam-deque` and the Parquet and compression crates build and have fast paths on both x86-64 and ARM64 (`portability.md`, section 6) and note the result here.

## Revisit when

Slice 11 produces the bake-off result, or the hand-off proves to be a bottleneck in profiles.

## Revision notes

- 2026-11-03, operator-protocol prototype: the `Blocked`/waker protocol is now shown to work without tokio (one worker thread, no deadlock, 864-query stress); confidence in that part raised to medium-high; blocking is reported by sources and sinks only; added the spill I/O question and the contention-testing rule, after the prototype's own scheduler had a lost-notification bug.

- 2026-11-03, after independent review: fixed the Snappy figure (the earlier "26-33%" was not in the results; now 33% of the v2 profile); added the missing alternatives B (second tokio runtime) and C (rayon) and relabelled; narrowed the glommio/monoio claim (only glommio is Linux-only); specified `crossbeam-deque` and the synchronous-operator and `blocked` protocol that 0001 needs; split the confidence per claim.

## Amendments

None yet.
