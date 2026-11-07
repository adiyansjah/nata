# 0009: Query memory budgets, fair shares and spill requests

Status: accepted (by the owner, 2026-11-07, with card 0005); its default sizes are frozen only after the walking skeleton measures them (card 0005 part 3, decision 4)
Date: 2026-11-07

## Context

[ADR 0001 A1.5](0001-pipeline-model.md#amendments) leaves a query memory manager to slice 5; this is register R1, an on-path blocker. Sort, join and windows (slices 6–8) must reuse its grants and spill lifecycle. The current product scheduler is the ordered row-group executor in `crates/nata-sched/src/lib.rs`, not the spike's general push scheduler. `nata-exec/src/exec.rs` maps filter/project inside scan tasks; the CLI and pgwire server both call it. An aggregate must not accumulate all groups on the consuming thread or receive B independently in every task.

## Decision

Propose a query-owned reservation service, shared by the scan, expression work, blocking sinks, partition merge, spill I/O and output. Operator logic remains on ADR 0002's CPU workers. This supplements ADR 0001, not a distributed admission controller.

- **Budgets (reasoned proposed defaults, not measured):** node execution pool N is half the smaller of physical and cgroup memory limits. Query budget B is N divided by the configured maximum admitted queries (CLI: one), with a test/user override downward; never promise the whole N to every session. Sum of admitted query caps cannot exceed N. Keep existing CPU admission; no cross-query borrowing in this slice. On machines without a usable memory limit, require an explicit cap rather than assume unlimited memory. Expose effective B and its source in diagnostics/ANALYZE. Reject a cap below the checked minimum working set before starting the query.
- **Reserve before allocation:** RAII grants charge capacity, not logical length: hash table old+new capacity during rehash, key arenas/offsets, wide states, selections/expressions, decoded batches, partial/merge tables and bounded I/O/output buffers. Charge shared Arrow buffers once and transfer ownership with the grant. Shrink/drop/cancel returns bytes once. B is a live-execution bound, not a claim about process RSS: record fixed stacks/runtime and allocator retention separately, with the card's independent RSS gate. Retained protocol plans remain under ADR 0008's separate caps.
- **Fair shares:** first reserve bounded non-revocable decode/output and spill-progress workspace from B; the remainder is divided equally among active spillable operators, not multiplied by task count. Each operator subdivides its grant among local states. Unused shares may be borrowed within the query. A newly runnable operator can reclaim a borrowed share; the fair share is a reclaim target, not a reason to fail while other memory is idle. Limit merge concurrency to the number of minimum partition grants that fit.
- **Pressure protocol:** a failed `try_reserve` queues one request and selects the largest borrower above its current share (stable operator-id tie break), or the requester's own spillable state if none is above share. Request revocation, never call the operator while holding the manager lock. It spills at a batch boundary, releases grants only when the corresponding memory is freed, and wakes queued requests in FIFO order. Coalesce requests. A grant is retried after wake; a wake itself grants no bytes. Neither locks nor CPU workers are held waiting for grants or disk: park/requeue with the ADR 0001 blocked/waker seam; bounded disk jobs return completions. Final merge is schedulable work too. Do not add an unbounded I/O pool.
- **Progress:** reserve the spill worker's bounded write/read buffers up front, so reclaim needs no additional grant. A waiter cannot retain a resource needed by its chosen victim; release its own revocable state or spill it first. If no reclaimable bytes can satisfy the minimum operation, terminate with a positioned memory-limit error, not a wait cycle or self-wake loop. Cancellation wakes every waiter, discards pending requests and joins I/O/tasks before query teardown.
- **Disk/files:** a per-query spill registry owns private exclusive-created files and all cleanup. Proposed targets: at most 32 simultaneously open spill descriptors per query, further reduced against the process soft limit after reserving scan/socket headroom; live spill-byte quota D defaults to the smaller of 4×B and half the spill volume's available bytes at admission, checked again before writes. These are reasoned initial limits to verify in the baseline, not measured optima. An explicit bounded disk override supports large-scale tests. Concurrent admissions reserve disk allowance too; ENOSPC still fails cleanly. Success/error/cancel removes the registry's files. Process death leaves a query-owned directory reclaimed on restart only after proving no live owner; no arbitrary path deletion.

## Why

**Reasoned:** grants shared across phases prevent a nominal budget from growing with workers. Revocable borrowing permits a lone aggregate to use idle space while leaving an interface for a later sort/join/window. An up-front progress reserve and no waiting CPU workers avoid the obvious spill-to-free-memory deadlock. Separate RSS checks catch memory missing from the accounting.

## Alternatives considered

- **Fixed per-operator/per-worker budgets, or reuse the spike's `Budget` unchanged:** simple byte counter, but no fair shares/revocation; separately assigned caps do not enforce a query total or let another operator progress.
- **Full node-wide dynamic admission/borrowing and query priorities now:** better utilization, but introduces cross-query fairness and victim selection beyond this single-node operator slice. Fixed admitted caps compose with the accepted server admission model.
- **Reuse a general allocator quota alone:** can reject allocation, but does not know which operator can spill, transfer a batch charge or reserve progress. No candidate allocator has been measured; custom allocators are not proposed.

## Evidence

- **Measured:** [bridge](../../bench/results/20261104-async-bridge.md) checks bounded queued chunks and cancellation; it is not a memory manager. [Spill benchmark](../../bench/results/20261106-spill-bench.md) shows SF10 grace join failure at a 64 MB budget on both CPUs and RSS beyond its operator budget because decode buffers also exist. [SF100](../../bench/results/20261106-scale-sf100.md) shows 512 join files open on ARM64. [Debt A R16](../../bench/results/20261107-debt-a.md) repairs a real RSS breach that unchanged inflight counters could not explain. These are distinct workloads, not aggregate budget measurements.
- **Reasoned:** all grant policy/defaults/progress rules above. `spikes/operators-beyond/src/spill.rs` supplies a counter/registry starting point, not evidence for revocation.
- **Not covered:** production reservations, fair-share convergence, cgroup/RSS overhead, spill latency, disk quotas and crash cleanup on either architecture. Part 4 of [card 0005](../slices/0005-hash-aggregate.md) must settle the new handshake before implementation.

**Acceptance checks (planned):** real service behind the scheduler sync facade under loom: grant sum never above B, no double release, revoke/cancel/drop races, no lost wake, eventual progress when a victim releases (1–2 workers, two operators, preemption bound 3; state bounds reported). Reachable pressure/cancel canaries and mutants that grant twice, omit a wake or reserve spill buffers late must fail. Real-code stress adds three competing operators standing in for aggregate/sort/join, skew, delayed I/O, CPU contention, out-of-memory/disk-full and a slow default client; exact grant/file/thread gauges return to zero. The low-budget fixture must spill and complete on both CPUs; an intentionally unaccounted allocation must fail the independent RSS gate. These are requirements, not executed results.

## Confidence

**Medium** in the need for shared grants (reasoned and supported by resource failures); **low** in these unbuilt policies/defaults. **Medium** in each cited empirical resource finding within its recorded workload; none proves this manager. A deadlock counterexample, unacceptable reservation cost or unbounded library allocation changes the proposal before acceptance.

## Consequences

Scan windows may shrink to fit B; an oversize row group/record may fail clearly. Every future spilling operator uses the same grants/registry and tests cross-operator pressure. The scheduling adapter needs bounded park/wake and disk-completion handling; the current ordered scan is preserved, not assumed to implement that seam already. Full node workload management stays later.

## Revisit when

The two-operator experiment fails progress, measured RSS exceeds the declared envelope, fixed caps waste memory in a real multi-query workload, or slices 6–8 need unequal fair shares. Freeze default sizes only with the baseline and owner/parent decision; changes after acceptance require a dated amendment.

## Amendments

- 2026-11-07, after the card 0005 baseline spike (`bench/results/20261107-hash-aggregate-baseline.md`): the scheduler adapter uses a **sticky notification** handshake. A wake that arrives before the waiter parks is kept and consumed, so a release can never be lost between checking the budget and parking. The spike's counterexamples (cap removed, wake dropped, release twice) become models of the real service. This clarifies the design and changes no budget default.
