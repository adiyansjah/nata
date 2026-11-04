# iceberg-plan (throwaway spike, verification-plan items 25 and 26)

Items: **25** what we write, others read, and the reverse (interop with DuckDB and pyiceberg; partition transforms against the spec's own vectors; schema and
partition evolution; deletes); **26** planning an Iceberg scan is correct (pruning never loses a row) and fast (10k, 100k, 1M data files). Built on the Iceberg
spec text read on 2026-11-04 (`format/spec.md`, main): Appendix B (hashes), the transform tables, Appendix D (bound serialization), Scan Planning, Delete Formats.
The earlier `iceberg-commit` spike (item 24, the commit protocol) is a different question and is still unrun on GCP.

| Path | What |
|---|---|
| `src/types.rs`, `src/transform.rs` | primitive types, values, the bounds order (floats in totalOrder), Appendix D serialization, Murmur3, all eight transforms and their **inclusive projection** |
| `src/meta.rs`, `src/manifest.rs` | table metadata (v2) JSON; manifests and manifest lists in Avro with the spec's field ids (and the Java header keys), reading files written by anyone (any codec) |
| `src/table.rs` | a table writer: real Parquet data files (field ids), metrics (counts, NaN counts, bounds truncated to 16 with an incremented upper bound), position and equality deletes, a row delta (data and deletes in one commit), schema and partition-spec evolution, removed files; and opening an existing table |
| `src/expr.rs`, `src/plan.rs`, `src/read.rs` | filters with three evaluators (row-level oracle, inclusive metrics, inclusive partition projection); the planner (manifest summaries, partition tuples, column metrics, delete association by sequence number and partition; parallel over manifests); the reader (columns by field id, deletes applied) |
| `src/synth.rs`, `scale.sh` | metadata-only tables of any size and the planning benchmark |
| `tests/` | `transforms` (spec vectors, projection soundness over 400k random cases), `soundness` (randomized planner soundness against a naive oracle), `common/` (random tables and filters, the oracle) |
| `interop/` | `ours_to_others.py`, `others_to_ours.py`, `truncate_overflow.py` (pyiceberg 0.12.0, pyarrow 26.0.1, pyiceberg-core 0.11.0, DuckDB 2.0.0 with its `iceberg` extension) |
| `mutate.py` | 22 mutants of the transforms and the planner |
| `gcp-check.sh` | everything on a clean VM (job `icebergplan`) |

## What "sound" means here, and how it is checked

For random tables (8 columns, ten partition-spec shapes over every transform, a second spec added mid-way, a column added and one renamed, position deletes, equality
deletes, files removed, and **row deltas** where data and deletes share one commit) and random filters (comparisons, IN, NOT IN, IS [NOT] NULL, IS [NOT] NaN,
starts_with, AND / OR / NOT, literals taken from the data and perturbed by one, by a width, by a prefix), the rows that the planner's tasks produce after the filter
must equal the rows of ALL live files after the filter, computed by an oracle that reads every file, associates deletes with a plain nested loop over the spec's rules
and evaluates the filter row by row in Kleene logic. A row matches if it matches under ANY of three float semantics: the Iceberg bound order (`Total`), SQL
(`Sql`: NaN largest, -0.0 = +0.0) or IEEE (`Ieee`). The planner's delete association must equal the nested loop, and a no-filter plan must list every live file.

## Status (2026-11-04)

Dev box first (all pass), then **clean VMs on GCP, ARM64 (`n4a-highcpu-8`) and x86-64 (`c3d-highcpu-8`), job `icebergplan`, 2026-11-04**: build, the cargo tests, interop in
both directions, the truncate-overflow report and the benchmark ran identically on both; the **mutation phase FAILED to start on both for a harness reason** (rustup had no
toolchain for the copies outside the repo; fixed in `gcp-check.sh`) and is being rerun alone (job `icebergplan-mut`); the 22 of 22 below were measured on the dev box.

| Check | Result |
|---|---|
| Spec Appendix B hash vectors (int, long, decimal, date, time, timestamp, timestamptz, string, uuid, binary, boolean, float, double incl. -0.0), truncate and time-transform examples, dates before 1970 | all pass (dev box, ARM64, x86-64) |
| Inclusive projection is sound for every transform (400k random value/literal/operator cases, both sides of the epoch) | pass (all three) |
| Planner soundness: 60 random tables, 2,400 random filters; about half of all file plans pruned; 1,959 of 2,400 filters pruned at least one file | pass (all three) |
| Mutants of the transforms and planner | **22 of 22 caught on the dev box** (two were caught only after a gap was closed, below); on GCP: pending |
| Ours -> others, 80 random tables per machine: rows read by DuckDB's `iceberg_scan` equal ours, DuckDB's counts equal ours on every filter; pyiceberg equal where it can read (it cannot read equality deletes) | **PASS on ARM64 and x86-64** (dev box earlier: 40 of 40 tables, 480 of 480 filters) |
| Others -> ours, tables built by pyiceberg (every transform, appends, spec and schema evolution, deletes by rewrite): our rows equal pyiceberg's and DuckDB's; our planner and reader's filter counts equal DuckDB's; our transforms recompute every partition value pyiceberg (iceberg-rust) wrote | **PASS on ARM64 and x86-64** (dev box earlier: 35 of 35 tables, 420 of 420 filters, 4,192 partition checks, 0 mismatches) |
| `truncate_overflow.py` on `truncate[10]` with extreme values | identical on both machines: `k < 0` returns 3 of 5 rows in DuckDB and pyiceberg; pyiceberg also loses the `k = MIN` row |

### Planning cost on clean VMs (8 vCPU, minimum of 3 runs, one process per query)

1,000,000 files in 200 manifests of 10,000 entries (plus a delete manifest each), 8 threads unless stated.

| Layout and filter | ARM64 | x86-64 | Peak RSS |
|---|---|---|---|
| clustered by day, one day and one bucket (partition filter) | 0.20 s (2 of 200 manifests read) | 0.38 s | 170 MB |
| clustered, one week | 0.20 s | 0.38 s | 340 MB |
| clustered, `id = x` (no partition column; every manifest read), 1 thread / 8 threads | 14.2 s / 3.1 s | 18.5 s / 6.0 s | 325 MB / 1.28 GB |
| **scattered** (every manifest holds every day), one day and one bucket, 1 thread / 8 threads | 14.7 s / 3.1 s | 19.7 s / 6.5 s | 166 MB / 1.28 GB |
| a filter that prunes nothing, returns 1,000,000 file tasks, 1 thread / 8 threads | 14.8 s / 3.0 s | 19.0 s / 5.0 s | 3.2 GB / 4.1 GB |
| 100,000 files, clustered, partition filter / `id = x` (1 / 8 threads) | 0.19 s / 1.5 s / 1.0 s | 0.32 s / 2.4 s / 1.7 s | 163 MB / 173 MB / 1.27 GB |

About 14 microseconds per manifest entry on one ARM64 thread (18 on x86-64), 3 on eight threads. Parallelism over manifests scales 4.6x on ARM64 and 3.1x on x86-64
(`c3d` has 4 cores, 8 threads); with only 20 manifests (100,000 files) it hardly helps. These are minimums on otherwise idle VMs, not a comparison with another planner.

## Findings

1. **The spec's truncate formula overflows near the minimum of the type, and engines then lose rows.** `v - (((v % W) + W) % W)` for `v` within `W` of `i32::MIN`
   is below the type's minimum; Java wraps to a large positive number, which is neither a lower bound of `v` nor a multiple of `W`. We write the wrapped value (to agree with
   Java), and a reader pruning with the standard inclusive projection (`k < 0` -> `k_trunc <= trunc(0)`) then drops those rows. Measured with `interop/truncate_overflow.py`
   (a `truncate[10]` table with values at both extremes): `k < 0` returns **3 of 5** rows in both DuckDB and pyiceberg, `k < -2000000000` returns 2 of 4,
   and pyiceberg also loses the row for `k = -2147483648`. Our planner never prunes on a truncate partition value that is not a multiple of its width, and never projects a
   literal that truncates to a wrapped value; it returns all rows. A real writer should refuse (or route elsewhere) rows in that zone: a decision for the table-format ADR.
2. **-0.0 and +0.0.** Iceberg bounds order -0.0 below +0.0 (Java's `Double.compare`), SQL engines and Arrow treat them as equal. A planner that compares a literal `0.0` to
   bounds in the bound order prunes files that SQL says match. DuckDB and pyiceberg both returned 121 where a bound-order evaluation returned 126 for `score NOT IN (-0.0)`.
   The planner widens a zero literal to whichever zero is the more inclusive for the bound being tested.
3. **NaN.** Under IEEE (pyiceberg), `NOT (x >= c)` is TRUE for a NaN row; under the total order it is FALSE. Pushing NOT down to the leaves is valid for the total order only; the planner
   rewrites a negated ordering comparison as `x < c OR x IS NaN` (and keeps everything for a NaN literal). The cost: negated comparisons on float columns prune less.
4. **pyiceberg 0.12.0:** does not support equality deletes (it raises); its residual evaluator crashes with a `TypeError` on a NULL identity-partition value (`planFiles` of a
   filter such as `k >= 12` on a table with a NULL partition); `is_in` treats -0.0 and +0.0 as different; NOT IN and `<>` follow Iceberg's two-valued logic (NULL <> 5 is true), SQL's is
   unknown. We count these as known differences, not as disagreements; DuckDB (SQL semantics) is the gate for filter counts.
5. A defect found by pyiceberg: a manifest-list entry for a delete manifest must have `content = 1`, not the file content (2 for equality deletes). Our reader never looks at that field,
   so only the other engine could see it.
6. **Planning cost is dominated by Avro decoding and its memory**, not by the evaluators: 14 to 18 microseconds per entry on one thread (table above), and roughly 15 KB of
   heap per entry while a manifest is being decoded (apache-avro builds a generic value tree), so 8 manifests in flight cost 1.3 GB and 1,000,000 returned tasks 3 to 4 GB. A hand-written decoder straight into compact
   entries, and streaming tasks out, are the obvious next steps; not done here.
7. Partition pruning needs the manifests CLUSTERED by partition: the same one-day filter takes 0.2 s on a clustered 1M-file table and 14.7 s (3.1 s on 8 threads) when the files are scattered over all days, because the manifest summaries then skip nothing. A writer must keep manifests clustered (and compaction must rewrite them so).

## Gaps the tests found in the tests (closed)

Two mutants survived the first round: "equality delete applies to a data file of the same commit" and "position delete misses a data file of the same commit". The random tables never
committed data and deletes together; `Table::row_delta` and a row-delta step in the generator now do (the upsert pattern), and both mutants are caught.

## Not covered

Format version 3 (deletion vectors, row lineage, new types), nested types (struct, list, map columns are kept as opaque), `time`, `timestamp_ns`, `uuid` and `fixed` as partition
sources, decimal precision above 38; the REST catalog (this spike uses table directories); name mapping for files without field ids; multiple snapshots / time travel / branches;
strict projection and residuals; pruning of delete files by their own metrics; manifests with unknown transforms; the `iceberg-rust` crate as a planner to compare against (a bake-off is
the missing build-versus-reuse evidence); Spark and the Java library as references (not available here). pyiceberg and DuckDB are the only independent readers, and
pyiceberg cannot read equality deletes. 
