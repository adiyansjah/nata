# 0004: Distributed execution and its failure model

Status: proposed (draft written 2026-11-04 with the model; amended 2026-11-05 with the transport findings; not accepted, see the revision notes for the acceptance condition)
Date: 2026-11-04

## Context

The owner decided the engine runs single-node and distributed (verification-plan items 28, 29). A distributed query must have a stated
rule for every failure, or each node will invent its own. Model: `spikes/failure-model/`.

## Decision (proposed rules)

1. **Failed task or worker.** The coordinator re-runs the task under a new attempt number on another worker, at most `MaxAttempts` times,
   then fails the query. It also fails the query when no unsuspected worker is left.
2. **Fencing.** The coordinator commits one attempt per task and accepts a result only if its attempt number and worker match the current
   assignment; any other result is rejected and its file is discarded. A worker runs each attempt id at most once.
3. **Consumed output stays valid.** A task whose output was already consumed is never re-run, even if its worker later dies or is suspected.
4. **Lost messages.** The coordinator re-sends `start` while an attempt is outstanding; a worker re-sends `done` while it holds the file.
5. **Failed coordinator.** The query fails (no recovery in v1). Each worker releases its tasks and files when its lease expires.
6. **Cancel and end.** The coordinator sends `release` to every worker, suspected ones included, until each acknowledges; a worker that
   released refuses new starts of that query.
7. **Client.** Is told "succeeded" only if the query succeeded; if the coordinator dies the client sees failure.
8. **A stream ends with a handshake (added 2026-11-05 from the transport spike).** The sender sends an end-of-stream frame carrying the batch count, half-closes, and keeps the connection until the receiver's consumer has taken the whole stream and closed its side; a connection that closes without that frame is a lost peer, never a finished stream. (A process that exits right after writing its last frame can otherwise lose that frame: Linux resets a socket closed with unread data and drops what it has not sent. On a 10 ms path 10 of 25 short correct runs failed.)
9. **Flow control is credit, sized from the round trip (added 2026-11-05).** The receiver grants a window and returns bytes when the CONSUMER takes a batch; the sender never has more than the window in flight; a batch larger than the window is refused, not deadlocked. The window must cover bandwidth times round-trip time: at a 20 ms round trip 64 KiB gave 31 MB/s, 1 MiB 490 MB/s and 4 MiB 968 MB/s.
10. **Liveness is a timer on every connection (added 2026-11-05).** Any received byte is a sign of life; an idle side pings; a side that has heard nothing for `dead_after` shuts the connection and reports the peer lost. A killed process is noticed within milliseconds (the kernel closes its sockets); a silent partition only by this timer (3.0 s for a 3 s setting, never earlier).

## Why

Each rule is the smallest one that makes the model's invariants and liveness properties hold; removing any of them is caught by a mutant
(`spikes/failure-model/README.md`). Three of the rules (no worker left, re-sending, run-once) were found by the model checker, not planned.

## Alternatives considered

- Coordinator recovery from a log: more availability, much more state; deferred, revisit after the first distributed slice.
- Lineage-based re-execution of upstream stages on worker loss (Spark style): needs deterministic, side-effect-free tasks; our tasks are
  deterministic by assumption only (see Not covered).
- Exactly-once through two-phase commit of task outputs: heavier than first-commit-wins plus fencing, and not needed for a query that has no
  side effects. Writes (item 24, 30) are a different commit protocol.

## Evidence

- **Measured:** nothing on real machines. Model checking results are in `spikes/failure-model/README.md`: the final spec passes safety and liveness
  for 2 tasks with 2 and 3 workers (up to 61.5 M states), all canaries trigger and all 12 mutants are caught, on GCP (ARM64).
- **Reasoned:** the rules for a write query (Iceberg commit) are separate; the failure of a coordinator in the middle of a commit is item 24.
- **Measured, stage A, simulated network** (`bench/results/20261105-net-exchange.md`; 4 processes in network namespaces on one ARM64 Spot VM, `tc netem`, 2026-11-05, 24 of 24 checks, 8 of 8 mutants of the transport caught): a failed peer becomes an error on every
  survivor within 0.02-0.09 s of a kill -9 and within the liveness timer (3.01-3.03 s for a 3 s timer) of a silent partition, never a wrong or partial answer; a cancel reached all four nodes within about 1 ms on a clean network (not measured with delay); buffered bytes stayed within the credit with slow consumers; and a transport bug that loopback hides was found and fixed
  (a process exiting right after its last frame can make Linux drop the end-of-stream frame; the sender must wait for the receiver to close). This is the stream layer under rules 1-7, not the rules themselves: no task re-run, fencing, coordinator or `release` was run on it. Re-run on x86-64 and ARM64 gave the same 24 of 24; a third pair of runs confirmed segmentation offload off and measured the spread of the 10 ms + 0.05% loss profile at about 9x over 8 runs (45 to 413 MB/s on ARM64, 42 to 384 on x86-64), so only profiles without random loss give reproducible throughput. Stage B (separate VMs) is not done.
- **Not covered:** half-read streams; non-deterministic tasks (a re-run that returns different rows); coordinator replacement; network
  partitions that heal (a partitioned worker that returns is modelled as a zombie); several queries sharing workers; memory and spill
  pressure; the real failure detector and its timeouts.

## Confidence

Low to medium for the rules as stated, until the GCP run and the network exchange spike (item 28). Would change it: a counterexample in a
larger configuration, or a half-read stream that cannot be made safe by the run-once rule.

## Consequences

Every node must carry an attempt number and a lease; shuffle and spill files must be named by (query, task, attempt, worker); the
simulation harness (item 23) must be able to inject message loss, duplication and delay, and worker and coordinator death.

## Revisit when

The first distributed slice runs; streaming consumers are added; the coordinator must survive its own failure.

## Revision notes

- 2026-11-05, **amended, still proposed.** Rules 8 to 10 (stream handshake, credit sized from the round trip, liveness timer) were added from the transport spike (`bench/results/20261105-net-exchange.md`: 24 to 26 of 26 checks on ARM64 and x86-64 in network namespaces, 8 of 8 mutants of the transport caught). **Acceptance condition:** the owner accepts this ADR when stage B has run (the exchange between separate VMs, plan item 28, which needs a multi-VM mode in the orchestrator) or when the first distributed slice is designed, whichever comes first. **Owner decision, 2026-11-05: no multi-VM mode in the orchestrator is built now; stage B waits for the design of the first distributed slice.** Until then no code may depend on rules 1 to 7: they are checked as a model and, for the transport, in a simulated network on one VM; none of them has run on real code, and the query-level rules (task re-run, fencing, coordinator, `release`) have not run on any transport.
