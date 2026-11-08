# 0017: Query owner

Status: proposed
Date: 2026-11-08

## Context

[ADR 0009](0009-query-memory-manager.md) owns execution grants, fair pressure, progress reserves and spill cleanup.
[ADR 0011](0011-object-store-io.md) owns bounded requests, byte credits and remote cancellation on Tokio.
A remote aggregate crosses both. `aggregate/pipeline.rs:446` opens a cursor;
`crates/nata-parquet/src/reader.rs:1243–1245,1454–1467` reaches synchronous remote fetch.
`crates/nata-objstore/src/node.rs:426–440` calls `node.call`; `:202–216` waits in `recv_timeout` on the caller.
A CPU continuation can therefore wait for a remote read despite ADR 0002's no-wait rule.

The architecture debates (`/home/dev/perf-debate/arch/OPUS-r3.md`, final correction 1, and `SOL-r3.md`, C11)
correct the earlier diagnosis: **CLI and pgwire already subtract the object-store allowance**.
See `crates/nata/src/sql.rs:234–247` and `crates/nata-pgwire/src/server.rs:162–176`.
ObjectNode also reserves internal allowances (`node.rs:148–178`) and exposes them (`:511–515`).
Separate query managers (`aggregate.rs:756–761`, `node.rs:264–276`) show a composition gap;
they do not establish double charging, a missing node envelope, a cap breach, deadlock or measured slowdown.

## Decision (proposed)

One owner per query coordinates memory pressure, cancellation, deadline and stats across execution and remote reads.
It gives operators and I/O requests bounded child handles. It uses the same-run stats record from card 0008 package 0.
It owns identity and lifecycle, not a new allocator, transport or node admission policy.

| Concern | Contract |
|---|---|
| Memory | Compose ADR 0009 execution grants and ADR 0011 request/cache allowances under explicit query and node limits. Reserve before allocation; transfer retained-byte ownership with batches/requests. Charge shared parents once. Keep spill/decode/output progress reserves; no waiter keeps a resource needed by its victim |
| Admission | Preserve conservative node-pool subtraction and fixed admitted query caps. No silent borrowing or doubled allowance. First reconcile CLI, server and ObjectNode equations and expose which limits are execution-only versus total query limits |
| Remote wait | Use the existing bounded owned async requests (`crates/nata-parquet/src/remote.rs:100–119`). Submit on Tokio, register a sticky completion, return Blocked and release the CPU slot. Completion requeues a finite step through proposed ADR 0016. Do not call synchronous recv_timeout from the CPU step |
| Cancel/deadline | One terminal reason propagates to grant waiters, remote requests, spill jobs, output and parked continuations. Cancel wakes every wait, aborts/drains owned work and prevents late completion publication. Teardown joins/drains before releasing the owner; all grants/handles/files return once |
| Stats | Stable query/task/request IDs tie admission, service intervals, process wall/CPU, retained bytes and waits to one execution. Worker elapsed sums remain labelled service wall, never reported as a critical path |

Keep different local disk and remote network policies. ADR 0009 still specifies pressure/fair shares and the private spill registry;
ADR 0011 still specifies transport bounds, retry/identity validation, request limits and cache ownership.
Node caches may outlive a query only under their existing node allowance and identity/auth rules; the query owns its references.
This proposal does not silently alter ADRs 0012–0015, distributed retries or write durability.
Write the seam during card 0008 with Track B, before W-A/W-B APIs harden. Scope integration separately;
certify combined remote execution in feature-map slices 11/12. Acceptance requires dated ADR 0009/0011 amendments,
not replacement of their independently valid checks by a general claim of shared ownership.

## Why

A query has one cancellation/deadline and needs fair pressure across remote reads and local spill. One owner makes the composition
explicit while retaining bounded services.

## Alternatives considered

Keeping separate managers unchanged cannot supply that shared pressure/stats lifecycle.
Subtracting the same allowance again would reduce usable memory without fixing the wait. Putting operators on Tokio or adding
blocking CPU workers hides the integration gap. A new transport or node-wide dynamic admission is outside this proposal.

## Evidence

- **Measured:** [object-store skeleton](../../bench/results/20261107-object-store-skeleton.md) and
  [debt A](../../bench/results/20261107-debt-a.md) provide bounded-transport and RSS lessons within their workloads.
  They do not certify remote aggregate plus spill or show a benefit from this owner.
- **Reasoned:** the blocking call chain and existing subtraction are source facts at the coordinates above.
  One lifecycle with parked reads should permit sibling progress without changing node limits. This is a design expectation.
- **Not covered:** composed grant equations, cross-service fairness, total-query limit semantics, combined bridge/manager/
  continuation model, both-CPU RSS/progress and remote aggregate speed. Adding independent model confidence is not certification.

## Confidence

Medium in the need for shared lifecycle and parked completions, and in the cited resource findings within their recorded workloads. Low in combined resource/progress behavior until exercised.
A grant overlap, lost completion, cancellation leak or wait cycle rejects the implementation.

## Consequences

Allocation sites and remote cursor state need explicit charge owners and completion handles. Existing conservative subtraction
remains useful. 
## Revisit when

The composed equation changes usable limits, remote pressure cannot reclaim enough memory, or
slices 11/12 expose a cache/request/continuation wait cycle.

**Open point R30:** reconcile the allowance equations and effective exposed limits first. Then use one CPU worker and a delayed
remote fetch: an independent ready sibling must progress while the fetch is parked. Extend to remote aggregate plus forced spill,
two queries, a suspended client, deadline and cancel at every wait. Log independent node/query live grants, retained capacities
and RSS. Assert bounded progress or a clear resource error, no late output, and zero live task/grant/file gauges after teardown.
Model the combined real bridge, manager and continuation with wake/cancel/release mutants, not just independent services.
The cheapest equation check precedes any measured combined fixture. This ADR runs neither.

## Amendments

None. Proposed; the source correction above is included from the first draft.
