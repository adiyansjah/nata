# 0006: Testing strategy (differential, mutation-checked, deterministic simulation)

Status: accepted (by the owner, 2026-11-05)
Date: 2026-11-05

## Context

The engine reads and writes tables, runs single-node and distributed, and its answers must equal other engines' answers (`docs/way-of-working.md` sections 5 and 6, `docs/decisions/verification-plan.md`).
The spikes tried five kinds of check; this ADR says which ones become rules. Everything here was run on clean ARM64 and x86-64 Spot VMs of the orchestrator.

## Decision (proposed rules)

1. **A check counts only if it is shown to fail on a known-bad version.** Every property test, differential test, model and proof comes with mutants (deliberate breakages) that it must catch; a mutant it misses, or a run that
   ends in an error (it does not compile, its test binary crashes, or its edit matched nothing and it never ran), is not a catch: such a mutant is reported INVALID. The mutation script itself is tested: two runs in a row must both pass their baseline, and a copy that keeps old file times must be shown to fail.
2. **Differential testing against DuckDB is the default correctness oracle,** with a grammar-based fuzzer (NULL, NaN, -0.0, dates and time zones, Unicode, ordering) and a shrinker. Known differences from DuckDB are listed, not hidden.
   Where DuckDB cannot be the oracle (Iceberg semantics), two independent readers are used and their known differences are recorded.
3. **Concurrency is model-checked, with the bounds stated:** loom for the scheduler, exchange, bridge and the early finish; the bound, the number of executions and the mutants are part of the result.
4. **Protocols are specified formally before they are built:** TLA+ models with TLC for the query lifecycle and the Iceberg commit (invariants, liveness, canaries that must be reachable, mutants of the model).
5. **Kernels are proved, bounded:** Kani proofs of the checked arithmetic and the guards, mutation-checked, with the cover (non-vacuity) checks recorded per harness and the unproved harnesses listed.
6. **All distributed code is written against a small environment trait and tested under a deterministic simulation:** injected clock, randomness, network, storage and task spawning; a failing seed replays identically; the trace hash of a seed
   is identical on ARM64 and x86-64, in debug and release, and across processes. Rules for developers: no wall clock, no global counters, no `HashMap` iteration order, no unseeded randomness in node code.
7. **Nothing counts until it has run on a clean VM, on both architectures.** Timings count only from a settled machine (1-minute load below 0.5, printed next to the result); otherwise they are labelled SANITY ONLY. A job that
   puts large files in the results directory, or starts a timed step right after a heavy one, is a defect of the job. The settle applies to every timed BLOCK, not only to every step: a block that starts inside a longer step
   (a later phase of one script, the second benchmark of one step, each network profile of one check) settles before it starts, and the line that says whether it did is part of the result. A result from a random-loss network profile is quoted with
   its spread over repeated runs, never as one number.
8. **The tooling is tested like the product:** job scripts have stub tests (which steps each job runs, that a failure is not hidden, that a stale file from a previous incarnation is not reused), because most failures of the first clean-VM runs
   were in the scripts, not in the code under test. Every place a job is registered (the job list of the orchestrator and of `job.sh`, the VM start-up script, the lists of the tests themselves) is covered by a test, so that
   a job added in one place and forgotten in another fails a test instead of silently skipping a gating step.

## Why

Each rule answers a failure that actually happened in these spikes. A check that cannot fail gave false comfort several times (a Kani cover counter that always read zero; a "no result" counted as a catch; a mutation run that reused the previous
mutant's build). Distributed bugs are only worth finding if they can be replayed. Hand-written tests share the author's assumptions; DuckDB does not.

## Alternatives considered

- **Property tests and unit tests alone:** cheap, but they do not show they can fail (rule 1).
- **Jepsen-style testing of a deployed cluster:** realistic but not replayable; kept as a later layer, not a replacement for simulation.
- **Adopting a simulation framework (madsim, turmoil, shuttle):** tried on the same toy (`bench/results/20261104-sim-harness.md`): madsim refuses OS threads, so the real scheduler cannot run inside it; turmoil simulates only its own network types;
  shuttle is useful for thread interleavings and is used next to loom. The small hand-written executor and environment trait are the foundation.

## Evidence

- **Measured, clean ARM64 and x86-64 VMs:** differential SQL fuzzer: 28 of 28 known-bad engines caught on 3 seeds and 60,000 clean queries per architecture (`spikes/sqlfuzz/`); simulation harness: 20,000 of 20,000 control seeds, 13 of 13 mutants
  caught with identical replays, trace hashes identical across the two architectures, the environment trait itself costing +0.6% / +1.3% on one thread and its fault-point hooks 100-130 ns per very small batch (`bench/results/20261104-sim-harness.md`); loom: 31 exchange models, 16 `stop_depth` models with 13 of 13 mutants; Kani: 100 of 105 harnesses of the real crate and 18 of 18 mutants;
  TLA+: the failure model and the Iceberg commit model with their mutants; mutation checks of the hash-ops, NULL-path, Arrow-pipeline, exchange and planner code.
- **Reasoned:** that these rules scale to a large code base; that the environment trait costs little (the spike measured the abstraction cost, see its results).
- **Not covered:** the fuzzer and the simulation have only been run against spikes, not against Nata itself (it does not exist yet); sanitizers (ASan, TSan, Miri) on the real code; continuous integration; a deployed-cluster fault test.

## Confidence

Medium-high that the rules are the right ones for the spike-sized code tried; medium that they hold at product size. It would drop if the environment trait proved too costly in the real engine, or if the oracle (DuckDB) turned out to disagree with the
chosen semantics too often to be useful.

## Consequences

Easier: every change has a cheap, replayable check and a record of what it can catch. Harder: each feature carries its mutants and its clean-VM run; the environment trait constrains how node code is written from the first line.

## Revisit when

The first slice of the engine exists and the fuzzer runs against it; the abstraction-cost benchmark is available from a quiet machine; CI is set up.

## Revision notes

- 2026-11-05, **accepted by the owner, with these checks still open** (verification-plan items in brackets):
  - Nothing here has run against Nata itself: the differential fuzzer, the simulation harness and the mutation runners were built and run on spikes; the first slice is where they meet real code [33, 23].
  - Sanitizers, Miri and the dependency checks (`cargo-deny`, `cargo-audit`, `cargo-geiger`) are not set up. CI runs formatting, lints, tests and the operator-protocol tests on x86-64 and ARM64 runners (all green on 2026-11-05), but not the mutation runners or the script tests [34, 8].
  - Lessons from applying the rules in the days after the draft, to be folded into the text of rules 1, 7 and 8 as amendments (applied by the owner on 2026-11-05, see Amendments; each cites its evidence):
    - Rule 7: a timed block must settle before it starts, and a block that starts inside a longer step needs its own settle. The step-level settle missed three such blocks (the later phases of the NULL-path run, the push-cost benchmark, the Arrow engine-only block), and the exchange check's profiles ran back to back and started at loads of 0.9 to 2.2 (`bench/results/20261105-timing-settled-rerun.md`, `20261105-net-exchange.md`). A result from a random-loss network profile is quoted with its spread over repeated runs, never as one number: the same job gave 980, 82 and 698 MB/s, and about 9x between the best and worst of 8 runs.
    - Rule 1: a mutant run that does not compile, or whose test binary crashes, is INVALID and not a catch; and a mutant whose edit matched nothing must be reported as INVALID (it happened once this week and would have counted as "survived" or "caught" by accident).
    - Rule 8: every place a job is registered needs a test (the job list, the VM start-up script, the tests' own lists). The first `icebergspikes` runs would have skipped their gating interop because `vm-startup.sh` did not list the job; a test now fails if an Iceberg job that runs the interop is missing there.

## Amendments

- 2026-11-05, by the owner: rules 1, 7 and 8 amended with the three lessons listed in the revision note above (INVALID mutants; settle every timed block and quote random-loss results with their spread; a test for every place a job is registered). Each cites its evidence there. The decisions of the rules are unchanged.
