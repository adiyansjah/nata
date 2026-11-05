# Lessons from the spikes

What the throwaway spikes (`spikes/`) taught us about building a better product. This is a working note, not a decision: the decisions
are in `docs/decisions/`. Each lesson points at the result file that backs it. Written 2026-11-05 from the spike results; where a
result is marked SANITY ONLY (measured on a loaded machine) it is not a final number.

Read with `docs/way-of-working.md` (sections 5 and 6) and `docs/decisions/verification-plan.md`.

## 1. Design notes for the engine

1. **Semantics cost more than scheduling.** The push protocol and time slicing cost 0-5% (`20261104-cpack-arm64.md`,
   `20261104-cpack-x86.md`; below about 3-5% is not resolved on the x86 machine, whose vCPUs are hardware threads). The large
   multipliers were NULL handling (1.8-2.5x of engine time at 10% NULLs; the word-wise bitmap version reached 1.08-1.22x, SANITY ONLY,
   `20261104-null-path.md`), `overflow-checks` (up to 2x on `sum`; `20261104-kernel-lints-and-guards.md`) and spilling (2.1x on one
   spill-heavy query; `20261104-fairness.md`, section 10). Put optimisation effort there, not in the scheduler.
2. **Never trust the values under NULL slots.** Arrow leaves them unspecified. An arrow-data 60.1.0 kernel panicked under
   `overflow-checks` on a garbage dictionary key under a NULL (`20261104-hash-ops-correctness.md`, the overflow section). Every kernel
   must ignore those slots, and every test must plant non-zero junk there. Keep `overflow-checks` on for our crates and off for
   dependencies (ADR 0003, decision 6).
3. **Resource limits are design inputs.** A fresh VM allows 1,024 open files; the dev machine allowed 524,288 and hid a defect where
   time slicing multiplied the open spill files (`20261104-fairness.md`, section 10). Operators must hold a bounded number of files and
   buffers, and the tests must run under the default limits.
4. **Cancellation and early finish are first-class.** Bugs found: the exchange cancel leak (a stale waker owns its task, which owns the
   scheduler, which owns the channel, so a cancelled query leaked itself; `20261104-exchange-loom.md`), a pipeline-wide stop flag that
   lost rows a `LIMIT` had already admitted (`20261104-arrow-pipeline.md`), and starvation when every worker is busy
   (`20261104-fairness.md`). Settle ownership early, give early finish a depth, and keep a leak check in the tests.
5. **Keep memory bounds explicit and tested.** The bounded channels held in every configuration; the unbounded control reached 879
   chunks (`20261104-cpack-arm64.md`).
6. **Write distributed code against a small environment trait** (clock, randomness, network, storage, task spawning) from the first
   line, so a failing seed replays exactly (`20261104-sim-harness.md`). Rules for developers: no wall clock, no global counters, no
   `HashMap` iteration order, no unseeded randomness in node code.
7. **Treat both CPU types as targets from the start.** ARM64 and x86-64 differ in memory ordering, in noise and in the ratios we measured.
   Run the same checks on both.

## 2. Notes for tables, Iceberg and I/O

8. **Real I/O will probably dominate.** Design the scan around request coalescing, prefetch and prompt cancellation, and measure it on a
   real store (`20261104-object-store-scan.md` covers a simulated store only).
9. **A wrong statistic is a bug; a missing one is safe.** parquet-rs 60.1.0 writes NaN as min and max of an all-NaN float chunk, which
   violates the Iceberg spec. The writer must verify statistics and turn off any it cannot make sound (`20261104-iceberg-plan.md`).
10. **Expect hazards in other engines.** On a `truncate` partition, rows near the minimum of an int are lost in DuckDB and pyiceberg
    today; pyiceberg cannot read equality deletes. Keep a list of known differences and test against two independent readers.
11. **Manifest layout and decoding matter.** Planning a million files took 0.15-0.38 s when partitions prune and 14-22 s on one thread
    when they cannot; the generic Avro value tree costs about 15 KB of heap per entry. Keep manifests clustered by partition and decode
    into compact entries.
12. **Fault tolerance for version 1: fail clean.** Never produce a wrong or duplicated result. Keep the exchange behind an interface so a
    spooling version can be added later, and decide a rule for non-deterministic functions before allowing task retry (ADR 0004).

## 3. Process notes

13. **Test the tooling as hard as the product.** Most failures of the first clean-VM runs were in our scripts: a mutation script that
    reused the previous mutant's build (cargo decides from file times), a copy that lost the executable bit, a shared run id, a results
    directory too large to pull, an exported variable that broke every command. Each fix has a test that fails on the old behaviour.
14. **Build testability into the binary:** `check` and `nullcheck` commands, seeds, trace hashes, and a DuckDB differential harness from
    the first slice.
15. **A check counts only if it has failed on a deliberately broken version** (a mutant), with an unmutated control run next to it. A
    run with no verdicts, or with a compile error, is not a catch.
16. **Run everything on a clean VM with default limits, on both architectures.** Label timings from a loaded machine SANITY ONLY. Do not
    compare a number without a control arm and its noise floor.

## 4. What the spikes did not tell us

- Sort, outer/semi/anti joins and window functions on the protocol (as far as the plan showed when this was written).
- Real object stores and real network behaviour.
- Scale beyond SF1 on 8 cores; spill under larger budgets; a spilling hash join.
- Iceberg format version 3, nested types, and the commit protocol against a real catalog.
- Proofs: 5 Kani harnesses of the real kernel crate (the exact 64-bit division and the 32-bit `checked_div`) are not proved
  (`20261104-kani-nata-kernels.md`).
