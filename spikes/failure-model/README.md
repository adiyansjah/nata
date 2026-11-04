# failure-model (throwaway spike, verification-plan item 29)

Question: **what does a failed task, a failed worker, a failed coordinator and a cancel do in a distributed query, and do those rules
hold under duplicated, late and lost messages?** The model is `model/QueryLifecycle.tla` (TLC); the proposed rules are in
`docs/decisions/0004-distributed-execution-and-failure-model.md`.

| Path | What |
|---|---|
| `model/QueryLifecycle.tla`, `MC.tla`, `model/cfg/*.cfg` | the model, its instances, safety and liveness configurations, six coverage canaries |
| `model/check.py` | `model` (real configs must PASS, canaries must be VIOLATED) and `mutants` (12 edits of the rules, each must be CAUGHT) |
| `model/tlc_runner.py` | runs TLC and classifies the output (copied from the iceberg spike) |
| `gcp-check.sh <dir>` | everything on a clean VM (job `failmodel`); exit 0 only if all gating phases pass |

## Status (2026-11-04)

Run on the dev box with small bounds only (the owner's limit for local heavy work is about 10 minutes):

| What | Result |
|---|---|
| `smoke`, `safe-1x2` (1 task, 2 workers, one crash, one wrong suspicion, one lost message, one coordinator crash): safety invariants and the frozen-after-end property | PASS, 294,429 distinct states, 5 s |
| `live-safe-1x2`: liveness (query ends, no orphaned files, client learns the outcome) under weak fairness | PASS, 40 s |
| `safe-2x2` (2 tasks, 2 workers, same budgets), safety only, earlier version of the spec | PASS, 8.1 M distinct states, 101 s |
| the 6 canaries (each state is reachable) | all violated as required |
| the 12 mutants | **all 12 CAUGHT** (the model sees every rule it is meant to protect) |

GCP run `failmodel-arm64` (2026-11-04, `n4a-highcpu-16`, 16 vCPU, 32 GiB, TLC 2.19, JDK 17; results in `bench/gcp-runs/failmodel-arm64/`,
which is not committed): the run was **cut short**, see below.

| Configuration | Result | Distinct states | Time |
|---|---|---|---|
| `smoke`, `live-smoke` | PASS | 2,527 | 1 s |
| `safe-1x2`, `live-safe-1x2` | PASS | 294,429 | 2.5 s, 13 s |
| `safe-1x3`, `live-safe-1x3` | PASS | 6,299,337 | 18 s, 413 s |
| `safe-2x2`, `live-safe-2x2` (2 tasks, 2 workers, one crash, one wrong suspicion, one lost message, one coordinator crash) | PASS | 11,882,257 | 35 s, 727 s |
| `safe-2x3` (2 tasks, 3 workers) | PASS | 61,509,559 | 201 s |
| `live-safe-2x3` | **NOT FINISHED**: killed after about 30 minutes, still enumerating (30.3 M states found at the last log line) | | |
| the 6 canaries and the 12 mutants on GCP | **NOT RUN** (they come after the configurations; the mutants and canaries did pass on the dev box) | | |

So the final spec holds its safety properties for 2 tasks with 2 and 3 workers and its liveness properties for 2 tasks with 2 workers;
liveness for 3 workers is unchecked. The run was cut short because the orchestrator was a child of the interactive session, which was
interrupted; it died during its own cleanup, after the VM was deleted and before the data disk was (the disk was removed by hand, nothing
is left). Cost: about $0.17 of VM time plus a few cents of disk.

## What TLC found while the model was written (each one is now a rule, and a mutant reproduces it)

1. **Every worker suspected: the query hangs.** Tasks go back to pending, no worker is eligible, and the attempt limit is not reached.
   Rule: fail the query when no unsuspected worker is left (`no-worker-left-rule`).
2. **A lost `start` or `done` message: the query hangs.** Rule: the coordinator re-sends `start` while an attempt is outstanding, and a
   worker re-sends `done` while it holds the file (`no-resend`).
3. **A late `start` re-runs an attempt the worker already ran, forever** (a suspected zombie loops run, finish, discard). Rule: a worker
   runs each attempt id at most once (`no-start-dedupe`).

Separately, building the checks found a defect in the iceberg spike's `tlc_runner.py`: it looked for the text "No error has occurred."
but TLC 2.19 (the pinned v1.7.4) prints "No error has been found.", so every PASS of that spike would have been recorded as ERROR on
GCP. Fixed there too, together with its unit test, and the classifier now also recognises "Action property ... is violated".

## Limits of the model

Consumption of a task's output is atomic (no half-read stream: the real hazard is a consumer that has read part of an output when the
worker dies); tasks are deterministic; a crashed worker never returns and its disk is lost; a crashed coordinator is not replaced;
one query at a time; no data, only identities; the failure detector is a nondeterministic choice, not a timer.
