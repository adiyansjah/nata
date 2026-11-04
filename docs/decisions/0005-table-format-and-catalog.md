# 0005: Table format and catalog (Apache Iceberg first)

Status: proposed (draft written 2026-11-05 from the read-path, planning and write evidence; the commit and catalog part has NO evidence yet; not accepted)
Date: 2026-11-05

## Context

The owner decided the engine reads AND writes tables, single-node and distributed, and targets Apache Iceberg first (`docs/decisions/verification-plan.md`, items 24, 25, 26, 30). The
format is an external specification that other engines implement, so the cheapest safe rule is: conform to the spec, and prove it against independent implementations. Spikes:
`spikes/iceberg-plan/` (planning and interop), `spikes/parquet-write/` (writing data files), `spikes/iceberg-commit/` (the commit protocol; written, NOT yet run).

## Decision (proposed rules)

1. **Format version 2 first.** Version 3 (deletion vectors replace position delete files, row lineage, new types) is a later slice; the spike did not cover it.
2. **Conformance is tested, not asserted.** Transforms are checked against the specification's own vectors (appendix B hashes, truncate and time examples); reads and writes are
   checked against two independent readers. DuckDB (SQL semantics) is the gate for filter counts; pyiceberg is a second reader whose known differences are listed, not counted as failures.
3. **Pruning is sound by construction and by test.** The planner may skip a file only when no row of it can match; "plan, then filter" must equal a full read, on random tables and
   filters, with mutants. Where Iceberg's bound order and SQL semantics differ, the planner keeps the file: a zero literal is widened to the more inclusive zero (-0.0 sorts below +0.0
   in bounds, SQL treats them as equal); NaN rows are kept for negated ordering comparisons; a truncate partition value that is not a multiple of its width is never pruned on.
4. **The writer refuses, or routes elsewhere, rows in the truncate overflow zone** (values within one width of the type's minimum). OPEN DECISION for the owner: refuse the write, or write
   them to a separate partition. Other engines lose rows there today (see Evidence).
5. **Statistics written must be sound; a missing statistic is allowed, a wrong one is not.** The writer checks every column chunk (no NaN as a bound, correct truncation with increment,
   bounds that do not cut a UTF-8 character, correct null counts) and turns a statistic off when it cannot be sound.
6. **Manifests are kept clustered by partition, and rewriting them is a required maintenance operation.** A scattered layout makes every partition filter read every manifest.
7. **The read path decodes manifests straight into compact entries and streams tasks out.** The generic Avro value tree used by the spike costs about 15 KB of heap per entry; this is a
   planned improvement, not done. Whether to reuse `iceberg-rust` for this is undecided (no bake-off yet).
8. **Catalog and commit protocol: NOT DECIDED.** The intended interface is the REST catalog with its atomic commit; the formal model and the concurrent-commit spike exist (item 24) but have
   not run, so there is no evidence for or against any rule here. Questions the spike raises: operation token versus the REST `Idempotency-Key` for retried commits; which catalog to certify
   against (the Apache fixture is a test fixture); whether catalog-less commit by conditional put is allowed at all.

## Why

A table format is a contract with other engines. Their readers decide what our files mean, so independent readers are the only honest test, and the bugs found so far were found by them
(a wrong `content` value in a manifest-list entry, seen only by pyiceberg; the truncate overflow, seen only by running three engines on the same table).

## Alternatives considered

- **Delta Lake or Hudi first:** not evaluated; the owner chose Iceberg.
- **Reuse `iceberg-rust` for planning and writing:** not compared; no bake-off. pyiceberg (which uses iceberg-rust for parts) was used as a reader only.
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
- **Reasoned:** the maintenance rule for manifests, and that a missing statistic is safe.
- **Not covered:** format version 3; the REST catalog and the commit protocol (spike written, not run); time travel, branches and tags; nested types; name mapping for files without field ids;
  strict projection and residuals; pruning of delete files by their own metrics; a comparison with `iceberg-rust` or the Java library; real object stores.

## Confidence

- **Read path, semantics and pruning soundness: medium-high.** Two independent readers, random tests with mutants, two architectures. It would drop on a disagreement with the Java library
  (not available here) or on a nested-type or v3 table.
- **Writing data files with sound statistics: medium.** Checked by an independent verifier and DuckDB; other readers not tested.
- **Planning speed: medium for the numbers, low for the design.** The numbers are from idle VMs but come from one decoder; a hand-written one may change them by a large factor.
- **Commit and catalog: low (no evidence).**

## Consequences

Easier: any result we produce can be checked against other engines. Harder: the writer carries extra rules (the overflow zone, statistics checks) that other engines do not; manifest maintenance becomes a
product feature, not an afterthought.

## Revisit when

The `iceberg` commit run has results; a nested-type or v3 table is needed; the Java library or `iceberg-rust` can be compared; a plan over 100,000 files is on the hot path of a real query.
