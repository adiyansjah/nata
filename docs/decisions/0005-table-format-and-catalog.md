# 0005: Table format and catalog (Apache Iceberg first)

Status: accepted in part (by the owner, 2026-11-05): rules 1 to 7 and the evidence behind them; rule 8 (catalog and commit protocol) stays open, see the revision notes
Date: 2026-11-05

## Context

The owner decided the engine reads AND writes tables, single-node and distributed, and targets Apache Iceberg first (`docs/decisions/verification-plan.md`, items 24, 25, 26, 30). The
format is an external specification that other engines implement, so the cheapest safe rule is: conform to the spec, and prove it against independent implementations. Spikes:
`spikes/iceberg-plan/` (planning and interop), `spikes/parquet-write/` (writing data files), `spikes/iceberg-commit/` (the commit protocol; GCP runs 2026-11-05: the first left 15 of 24 model runs without a result, the targeted re-run `icebergmodel-arm64` the same day gave all of them a result, see `bench/results/20261104-iceberg-commit.md` sections 8 and 10).

## Decision (proposed rules)

1. **Format version 2 first.** Version 3 (deletion vectors replace position delete files, row lineage, new types) is a later slice; the spike did not cover it.
2. **Conformance is tested, not asserted.** Transforms are checked against the specification's own vectors (appendix B hashes, truncate and time examples); reads and writes are
   checked against two independent readers. DuckDB (SQL semantics) is the gate for filter counts; pyiceberg is a second reader whose known differences are listed, not counted as failures.
3. **Pruning is sound by construction and by test.** The planner may skip a file only when no row of it can match; "plan, then filter" must equal a full read, on random tables and
   filters, with mutants. Where Iceberg's bound order and SQL semantics differ, the planner keeps the file: a zero literal is widened to the more inclusive zero (-0.0 sorts below +0.0
   in bounds, SQL treats them as equal); NaN rows are kept for negated ordering comparisons; a truncate partition value that is not a multiple of its width is never pruned on.
4. **The writer refuses rows in the truncate overflow zone** (values within one width of the type's minimum), with an error that names the column, the width and the value. **Decided by the owner,
   2026-11-05: refuse.** Writing them to a separate partition was the alternative; other engines lose rows there today (see Evidence), and a layout they misread would still look healthy. The rule can be relaxed later;
   a wrong answer cannot be taken back.
5. **Statistics written must be sound; a missing statistic is allowed, a wrong one is not.** The writer checks every column chunk (no NaN as a bound, correct truncation with increment,
   bounds that do not cut a UTF-8 character, correct null counts) and turns a statistic off when it cannot be sound.
6. **Manifests are kept clustered by partition, and rewriting them is a required maintenance operation.** A scattered layout makes every partition filter read every manifest.
7. **The read path decodes manifests straight into compact entries and streams tasks out.** The generic Avro value tree used by the spike costs about 15 KB of heap per entry; this is a
   planned improvement, not done. **Bake-off done 2026-11-05** (`bench/results/20261105-manifest-decode.md`, development machine, not settled): a hand-written schema-compiled decoder is 18 times faster than the generic reader and 41 times faster than `iceberg-rust` 0.11 on one
   thread (1,010,000 entries: 0.67 s, 12.2 s, 27.7 s) and uses 2.1 and 4.0 times less memory (1.17 GiB, 2.4 GiB, 4.7 GiB), all three agree entry for entry, and it agrees with iceberg-rust on 147 manifests written by pyiceberg. **Decided by the owner, 2026-11-05: we write our own decoder** and keep `iceberg-rust` and
   the generic reader as test oracles (ADR 0006 rule 2). Still to do before it is product code: all partition types, resolution by field id, v1/v3 layouts, a run on Java-written manifests, a settled-machine timing.
8. **Catalog and commit protocol: model checked, catalog NOT DECIDED.** The intended interface is the REST catalog with its atomic commit. The formal model of the commit (bounded: at most 3 writers with one operation
   each, 2 attempts, one crash, one duplicate delivery, one lost request, one pinned reader) now has an expected result for all 24 of its runs on GCP (ARM64, 2026-11-05; from two runs, nine and fifteen, with the same model files by sha256): no invariant violated, liveness holds, all 11 coverage canaries are
   reachable and 9 of 9 mutants are caught, the mutants only on the small configurations (see Evidence). This is checking within bounds, not a proof. That does not decide any rule here about a product: the spike ran only against the Apache reference fixture and a local conditional create, and the model is not tied to the
   code by a refinement check. The commit suite and the pyiceberg interop also pass against **Apache Polaris 1.8.0**, a product catalog, on ARM64 and x86-64 (`bench/results/20261104-iceberg-commit.md` section 12), but only in a test configuration Polaris refuses by default: in-memory metastore, local-disk storage, loopback. Questions that stay open: operation token versus the REST `Idempotency-Key` for retried commits (probed by hand against Polaris 1.8.0, `bench/results/20261104-iceberg-commit.md` section 13: it dedupes a retry within its lifetime when switched on, but it is off by default, does not advertise its lifetime in `/v1/config`, does not compare the request when a key is reused, and is gone after the lifetime; the operation token stays necessary for correctness, **decided by the owner, 2026-11-05: the operation token alone for now**; `Idempotency-Key` may be added later only for a catalog that advertises its lifetime, with a fresh key for every commit); the owner chose Polaris as the first catalog to certify against, and a database metastore with a real object store is not tried; whether catalog-less commit by
   conditional put is allowed at all.

## Why

A table format is a contract with other engines. Their readers decide what our files mean, so independent readers are the only honest test, and the bugs found so far were found by them
(a wrong `content` value in a manifest-list entry, seen only by pyiceberg; the truncate overflow, seen only by running three engines on the same table).

## Alternatives considered

- **Delta Lake or Hudi first:** not evaluated; the owner chose Iceberg.
- **Reuse `iceberg-rust` for planning and writing:** only manifest DECODING was compared (rule 7: 41 times slower and 4 times more memory than a hand-written decoder on 1M entries, same results); its planning, writing and catalog parts are not compared. pyiceberg (which uses iceberg-rust for parts) was used as a reader only.
- **Catalog-less (file-system) commit:** the specification calls it deprecated and unsafe on object stores; a conditional-create variant is modelled but not run.

## Evidence

- **Measured** (`bench/results/20261104-iceberg-plan.md`; clean ARM64 `n4a-highcpu-8` and x86-64 `c3d-highcpu-8` Spot VMs, 2026-11-04): the transform vectors pass; 80 of 80 random tables we write
  are read identically by DuckDB and 71 of 71 tables pyiceberg writes are read identically by us, with 1,065 of 1,065 filter counts equal to DuckDB, on both architectures; planner soundness on
  random tables and filters; 22 of 22 mutants caught on each architecture. Planning 1,000,000 files: 0.15-0.38 s when partitions prune (clustered layout); 14-22 s on one thread and 2-6.5 s on eight
  when the filter cannot prune or the layout is scattered; 3-4 GB peak memory.
- **Measured, writing** (`spikes/parquet-write/`, `bench/results/`; ARM64 and x86-64, 500,000 rows, 163 rolled files): an independent statistics verifier flags all 11 known-bad statistic files;
  parquet-rs 60.1.0 writes NaN as min and max of an all-NaN float chunk (a spec violation; the writer turns those statistics off); the library cannot be made to write bounds narrower than the data.
- **Measured, hazards:** on a `truncate[10]` table with values at both extremes of `int`, `k < 0` returns 3 of 5 rows in both DuckDB and pyiceberg; pyiceberg also loses the row for
  `k = -2147483648`. pyiceberg 0.12.0 cannot read equality deletes (58 of 80 tables) and crashes on a NULL identity-partition value.
- **Measured** (`bench/results/20261104-iceberg-commit.md` sections 8 and 10; ARM64 Spot VMs, 2026-11-05; the first run `n4a-highcpu-8` verdict FAIL, the targeted re-run `icebergmodel-arm64` on `n4a-highcpu-16` verdict PASS for the model phases it covers): in the TLA+
  model of the commit (bounded as in rule 8) all 24 model runs ended as expected, nine in the first run and fifteen in the re-run (`IcebergCommit.tla` and `MC.tla` have identical sha256 in both; the `cfg/` files are not hashed, they are unchanged in git); TLC found no invariant violation in any unmutated configuration, the largest three at 116.8 million (`three-rest-ser`, `three-store-ser`, identical counts) and 129.2 million (`three-rest-snap`) distinct states; `three-rest-snap-i4` violates
  I4_NoPhantom as expected (I4 is a live check); liveness passes for two writers (`live-2`) and for three (`live-3`, 8.6 million distinct states); all 11 coverage canaries are reached, so the situations the model claims to cover are reachable in it; 9 of 9 mutants of the model are caught (run once, in the first run, on the small configurations only). The commit spike passes all its gating checks against the Apache REST reference
  fixture 1.12.0 and against conditional create on a local file system (18 forced interleavings with 5 of 5 known-bad writers caught, 12 crash points, 6 of 6 stress seeds, on each). The code side was re-run on ARM64 and x86-64 (`icebergspikes`, section 11 of that file): the same checks pass on both, and pyiceberg 0.12.0 reads our manifests identically on all 6 snapshots on each. **Not obtained:**
  the model on x86-64 (it does not depend on the VM); the spike against a catalog with a database metastore or an object-store warehouse (Polaris was tried only in a test configuration), or against a real object store for the commit itself; a check that ties the model to the code. TLC's fingerprint-collision estimates are small but not zero (3.6E-3 calculated, optimistic, 4.9E-4 from the actual fingerprints for the largest re-run configuration).
- **Reasoned:** the maintenance rule for manifests, and that a missing statistic is safe.
- **Not covered:** format version 3; the commit protocol beyond the bounded results above (the Apache fixture and Apache Polaris 1.8.0 in a test configuration; no database metastore, no real S3/GCS/Azure for the commit; larger bounds; equality deletes and deletion vectors are not in the model); time travel, branches and tags; nested types; name mapping for files without field ids;
  strict projection and residuals; pruning of delete files by their own metrics; a comparison with `iceberg-rust` or the Java library; real object stores.

## Confidence

- **Read path, semantics and pruning soundness: medium-high.** Two independent readers, random tests with mutants, two architectures. It would drop on a disagreement with the Java library
  (not available here) or on a nested-type or v3 table.
- **Writing data files with sound statistics: medium.** Checked by an independent verifier and DuckDB; other readers not tested.
- **Planning speed: medium for the numbers, low for the design.** The numbers are from idle VMs but come from one decoder; a hand-written one may change them by a large factor.
- **Commit protocol as modelled: medium.** Exhaustive within stated bounds, with liveness, reachable canaries and caught mutants; it would drop on a counterexample at larger bounds or on a disagreement between the model and a real catalog.
- **Commit against a product catalog, REST protocol (Apache Polaris 1.8.0, test configuration): medium-low.** The same suite passes as against the reference fixture, on two architectures, and its known-bad writers are caught; it would drop on a failure in a database-backed or object-store-backed deployment.
- **Commit against production deployments of a catalog, and over real object stores: low (no evidence).**

## Consequences

Easier: any result we produce can be checked against other engines. Harder: the writer carries extra rules (the overflow zone, statistics checks) that other engines do not; manifest maintenance becomes a
product feature, not an afterthought.

## Revisit when

The commit spike runs against a product catalog with a database metastore or an object-store warehouse, or a catalog that advertises its `Idempotency-Key` lifetime is to be used; the model is tied to the code or its bounds are enlarged; a nested-type or v3 table is needed; the Java library or `iceberg-rust` can be compared; a plan over 100,000 files is on the hot path of a real query.

## Revision notes

- 2026-11-06, the changed commit model (a compaction conflicts with any new delete file naming its inputs, matching the code and Iceberg's
  rewrite validation) is re-checked in full on GCP: all 25 runs as expected, the same three-writer state counts, 10 of 10 model mutants
  caught (`bench/results/20261104-iceberg-commit.md` section 17).

- 2026-11-06 (latest), **rule 8, finding 1 of the trace validation is resolved by changing the model, not the code.** Done by the assistant under the owner's delegation (section 16 of `bench/results/20261104-iceberg-commit.md`). A compaction now conflicts with any delete file committed after its base that names one of its input files, even a redundant one that deletes no new row; this is what the code already did (an unnecessary abort, never a lost or resurrected row) and what the owner's brief says Apache Iceberg's rewrite validation does (not re-read from the source here). The model's change makes it stricter (a retry it allowed is now an abort) and adds an invariant (I8) and a configuration with two deleters (`ddc-rest`), the only one that can tell the two rules apart; a mutant that returns to the old rule is caught by I8, and the former mismatch trace is now VALID. Local results: 24 of 24 corpus traces as expected, 10 of 10 model mutants caught, the eight smaller runs pass with the state counts unchanged. **The full re-run of the model on GCP (the three-writer runs, the liveness run and the canaries) is pending** and needs the owner's cost yes, so the model gate of rule 8 is not re-stated as met for the new text until then. ADR status unchanged.
- 2026-11-06 (later), **rule 8, the commit model is now tied to the code for the object-store backend: trace validation, one mismatch found.** Done by the assistant under the owner's delegation, from section 15 of `bench/results/20261104-iceberg-commit.md` (local, no GCP): each suite run records the model's actions the code performed and TLC checks the model allows that sequence. All 28 correct-writer traces of the standard run and 39 of 40 of an extended run (40 stress seeds with SIGKILLs) are allowed; the 5 known-bad writers the scenarios force are refused at the bad event; the checker is mutation-checked (21 corrupted traces, 23 mutants, all caught). **Finding:** the code aborts a compaction when a concurrent delete file touches its inputs even when that delete repeats rows already deleted; the model (whose delete snapshots count only newly deleted pairs) says it must retry. The code is the more conservative (an unnecessary abort, no lost or resurrected row); the model was not changed and its 24 runs were not repeated. The earlier open item "the model is not tied to the code by a refinement check" is closed for the object-store backend only: the REST backend, real GCS, the garbage collector and readers are not in the traces. The ADR status is unchanged.
- 2026-11-06, **rule 8, catalog-less commit by conditional put: allowed as an explicit opt-in on Google Cloud Storage only.** Decided by
  the assistant under the owner's delegation of 2026-11-06, from section 14 of `bench/results/20261104-iceberg-commit.md`: on a real GCS
  bucket the create-only write is refused the second time (HTTP 412) and the full commit suite passes (18 forced interleavings with 5 of 5
  known-bad writers caught, 12 crash points, 6 of 6 stress seeds), and the VM identity was shown unable to write anywhere else. The default
  stays the REST catalog. S3 and Azure are not allowed in this mode until the same suite has passed on each. Which REST catalog to certify
  stays open (the write slice's card decides).

- 2026-11-05, **accepted in part by the owner**: rules 1 to 7 (format version 2 first; conformance tested against independent readers; sound pruning; the writer refuses the truncate overflow zone; sound statistics; clustered manifests with maintenance; our own manifest decoder). **Rule 8 stays open** (catalog and commit protocol). Checks still open (verification-plan items in brackets):
  - Rule 8: retried commits: **decided by the owner on 2026-11-05: the operation token alone for now** (`Idempotency-Key` later and only where a catalog advertises its lifetime, one fresh key per commit); the probe of 2026-11-05 (by hand, development machine, section 13 of `bench/results/20261104-iceberg-commit.md`) found that Polaris 1.8.0 dedupes within a lifetime when switched on but is off by default, does not advertise the lifetime, does not compare a reused key's request, and forgets after the lifetime, so the token stays necessary [24]; a database metastore and an object-store warehouse behind the catalog (Polaris was run only as a loopback test fixture with an in-memory metastore and local-disk storage); the commit against a real object store; whether catalog-less commit by conditional put is allowed at all; equality deletes and deletion vectors are not modelled; the model is not tied to the code by a refinement check.
  - Rule 7: the decoder is a spike (`spikes/manifest-decode`). Before it is product code: all partition types (it keeps int, long, date and timestamp), resolution by field id, v1 and v3 manifests, a run on manifests written by the Java library, a timing on a settled machine, and the cross-check of each manifest's entry count against the manifest list (plain Avro has no checksum).
  - Format version 3, nested types, name mapping for files without field ids, time travel, branches and tags are not covered.
  - Read-path confidence stays medium-high until a Java-written table has been read.

- 2026-11-05 (later), **owner decisions:** rule 4 is confirmed as "refuse"; retried commits use the operation token alone for now (above). Rule 8 stays open for the other questions listed in the first note.

## Amendments

None yet.
