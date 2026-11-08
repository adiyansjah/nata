# 0016: Pipeline step protocol

Status: proposed
Date: 2026-11-08

## Context

[ADR 0001](0001-pipeline-model.md) chooses morsel-driven push and dependent breaker sources;
[0002](0002-async-runtime-and-threading.md) separates CPU and I/O workers.
[Card 0008](../slices/0008-engine-performance.md) needs storage and spill seams before sort.
Write the contract now. Build the small physical pipeline graph with feature-map slice 6.

The consulted prototypes are `spikes/operator-trait/src/{api,sched}.rs`,
`spikes/operators-beyond/src/proto/{api,sched}.rs` and its README, and
`spikes/pipeline-model/src/main.rs`. They demonstrate lifecycle and composition, not a production implementation.
The old API permits long `GlobalSink::finalize`; operators-beyond exposes missing end-of-input and producer-count rules.
ADR 0001 A1:153–172 already decides those rules. `crates/nata-sched/src/cooperative.rs:26–29,100–150`
has finite continuation steps but dispatches waves outside the memory-service loom model.
Its ordered scan, unordered claims, bridge and parked core have different obligations. Unification must preserve them.

## Decision (proposed)

Use one finite step protocol for runnable work in a small physical pipeline graph. Keep the generic driver and permit local fusion.
The graph records source → transforms → sink, breaker sink/source dependencies and required ordering.
A join build dependency is a graph edge, not a blocked intermediate operator. A serial ordered merge is valid.

| Boundary | Contract |
|---|---|
| Step | Bounded CPU work returns Yield (runnable continuation), Blocked (registered wake), or Done (result/error). No CPU worker waits on disk, network, a grant or another pipeline. Long loops poll cancel/deadline. State, retained input/output and their grants belong to the continuation |
| Source | Produces a batch, Blocked or Finished. Source finish/close runs once on every exit and informs upstream producers when consumers disappear |
| Transform | NeedMoreInput consumes its input; HaveMoreOutput retains it for another call; Finished may carry final output and stops upstream work. Transforms do not block. End-of-input finish is one lifecycle phase, resumed while it returns HaveMoreOutput, then completed once |
| Sink | NeedMoreInput accepts input; Blocked leaves it untouched for retry; Finished rejects further upstream work. Combine and finalization are bounded continuation work, not an unbounded callback on the coordinator |
| Breaker | Declares every producer pipeline. Finalization completes once after every producer/combine succeeds, before dependents become runnable. Publishes a bounded lazy task stream, not Vec<Task> |
| Wake | Sticky readiness survives wake-before-park and wake-during-run. At most one runner owns a task. Late wakes after completion cannot resurrect it; repeated wakes coalesce. No callback under a manager lock |
| Stop/error | Preserve stop_depth: output already admitted downstream of LIMIT survives sideways stop. Wake parked peers, close exchanges and release grants/files on every exit. Preserve ordered scan and lowest-error behavior where promised; grouped output stays unordered without ORDER BY |

ADR 0001 A1's finish and multi-producer hooks enter production with their first consumers (slices 7–9), with checks then.
The contract includes them now so sort and spill do not lock in incompatible APIs. Memory policy belongs to ADR 0009;
query cancellation/deadline/stats ownership belongs to proposed [0017](0017-query-owner.md).
This is an implementation contract for 0001/0002, not a runtime replacement or a change to exchange fencing/credit/EOS.
Acceptance needs dated clarifications to those ADRs if their implementation contract changes.

## Why

A common lifecycle lets aggregate and sort reuse scheduling, bounded publication and cleanup.

## Alternatives considered

Reusing the spike unchanged keeps
its long finalizer and missing A1 hooks. Keeping bespoke runners avoids migration now but repeats every continuation repair.
A full executor framework, pull rewrite or new runtime is not justified by these results. Keep the existing CPU/Tokio services.

## Evidence

- **Measured:** [operator prototype](../../bench/results/20261103-operator-trait-prototype.md),
  [ARM protocol](../../bench/results/20261104-arm64-run1-operator-protocol.md) and
  [stop-depth model](../../bench/results/20261104-stop-depth-loom.md) cover their stated prototype bounds, not this new driver.
  [Settled pull/push](../../bench/results/20261104-pull-vs-push-corrected.md) does not measure spill or windows.
- **Reasoned:** the lifecycle above. Architecture debates `/home/dev/perf-debate/arch/OPUS-r3.md` and `SOL-r3.md`,
  items 7 and final open points, cite `cooperative.rs:122–150` for the wave barrier. Reverse spill scaling alone does not
  attribute latency to it. No independent model result certifies the combined driver.
- **Not covered:** graph implementation, continuation representation/refill policy, real changed-source loom, useful overlap,
  combined memory/bridge cancellation and either CPU's production speed.

## Confidence

Medium in the small lifecycle and explicit dependencies, and in the cited measured prototype checks within their recorded bounds. Low in a particular refill policy or its performance until measured.
A lost wake, premature finalize, dropped admitted output or wait cycle rejects the implementation.

## Consequences

Continuations need explicit batch/grant ownership and task publication bounds. The ordered scan can retain its adapter.

## Revisit when

The first sort sink/source walk, a lifecycle counterexample or measured ready work waiting behind waves exposes an incompatible assumption.

**Open point R28:** continuation layout and completion-driven refill versus the existing wave barrier.
The cheapest first experiment is a paper walk of one aggregate and one sort sink/source through this table. Name any wave
assumption leaking into W-A/W-B. Then capture step completions, ready-queue and dispatcher-wait intervals beside disk busy/queue
and admission on the immutable baseline/fanout SF1 HC, eight-thread, 64-MiB full-CSV run on both CPUs.
Distinguish a wave wait from empty queues, I/O and output backpressure. Use a tiny asymmetric-step fixture (one delayed step,
one ready sibling) to demonstrate overlap and cancel under a proposed refill policy.
Only then accept a bounded W-B adapter brief naming paths, invariants, workload, time box and rerun limit.
No scheduler port follows from ranking alone. No experiment is run by this ADR.

Checks must use the real changed sources under the sync facade: wake-before-park/during-run, cancel at each boundary,
HaveMoreOutput from execute/finish, finalize-once after two producers, one-worker blocked consumer, stop_depth and leak gauges.
Mutants for dropped wake/cancel, double run/finalize, early finish and lost admitted output must fail. Model bounds are reported.

## Amendments

None. Proposed; no accepted ADR is changed by this document.
