# Iceberg interop and scan planning on clean VMs (verification-plan items 25 and 26)

Date 2026-11-04 (runs 23:18-23:54 UTC). Spike `spikes/iceberg-plan/`; findings, design and limits are in its `README.md`. This file holds the GCP evidence.
Four runs on Spot VMs of the orchestrator (nothing left afterwards, all cleaned up):

| Run | Job | Machine | VM time | Cost estimate | Verdict |
|---|---|---|---|---|---|
| `icebergplan-arm64` | `icebergplan` | `n4a-highcpu-8` (Neoverse-N3) | 1,278 s | $0.035 | 4 of 5 gating phases PASS; the mutation phase FAILED at 0 s (see below) |
| `icebergplan-x86` | `icebergplan` | `c3d-highcpu-8` (AMD EPYC, 4 cores x 2 threads) | 1,548 s | $0.030 | same |
| `icebergplan-mut-arm64` | `icebergplan-mut` | `n4a-highcpu-8` | 346 s | $0.009 | mutation phase PASS: **22 of 22 caught, 0 survived** (284 s) |
| `icebergplan-mut-x86` | `icebergplan-mut` | `c3d-highcpu-8` | 481 s | $0.009 | mutation phase PASS: **22 of 22 caught, 0 survived** (400 s) |

**The one failure was in the harness, not in a test.** The first runs reported `BASELINE FAILED: rustup could not choose a version of cargo` in the mutation phase: the mutation
copies of the crate live outside the repository, where a clean VM has no toolchain override. The fix (pin the active toolchain, commit `ae4b644`) was committed 5 minutes after the
VMs had received the script, so they ran the old one. The mutation phase was rerun alone (`IP_ONLY=mutants`, job `icebergplan-mut`) and passed on both architectures. The
fault in that phase is not a mutant result and was not counted as a catch or a miss.

## What passed on both architectures

- **Build and unit tests:** the transform vectors from the Iceberg specification (appendix B hashes, truncate and time-transform examples), the inclusive-projection property and the
  randomized planner soundness test (pruning never drops a file that holds a matching row: planning then filtering equals a full read).
- **Interop, tables we write, read by others** (80 random tables with all eight partition transforms, spec and schema evolution, position and equality deletes, removed files, data plus
  deletes in one commit): DuckDB's Iceberg reader returns the same rows as ours for **80 of 80** tables; pyiceberg matches on every table it can read (22 of 80); it cannot read the
  other 58 because pyiceberg 0.12.0 does not support equality deletes. **0 disagreements.**
- **Interop, tables pyiceberg writes, read by us** (71 tables): ours = pyiceberg = DuckDB on **71 of 71** tables, and our filter counts equal DuckDB's on **1,065 of 1,065** filters.
  **0 disagreements.**
- **The hazard report** (a `truncate[10]` table with values at both extremes of `int`): DuckDB and pyiceberg lose rows near the minimum (for example `k < 0`: truth 5 rows, both return 3;
  pyiceberg also loses the row for `k = -2147483648`); our planner returns all rows. Identical on both architectures. A real writer must refuse or reroute rows in that zone: a decision for
  the table-format ADR.
- **Planning at scale** (below): metadata-only tables of 10k, 100k and 1M data files, two layouts, 7 queries, 1 and 8 threads, minimum of 3.

Not measured by these runs: format version 3, the REST catalog (the spike reads table directories), time travel and branches, nested types, and a comparison against `iceberg-rust` or the Java library.

## Planning at scale (the figures that matter)

One process per query, minimum of 3 runs, on a Spot VM with 8 vCPUs that was otherwise idle (load about 1); the machine types differ (the x86-64 vCPUs are hardware threads), so compare each column
to itself. Every range below is the minimum and maximum over the queries of that kind in the raw tables further down (7 queries: `point`, `week` (partition filters), `id`, `name_prefix`, `k_range`
(filters on columns the partitions cannot prune), `nonselective` and `all` (return every file)).

| 1,000,000 data files | ARM64, 1 thread | ARM64, 8 threads | x86-64, 1 thread | x86-64, 8 threads |
|---|---|---|---|---|
| clustered layout, partition filter (reads 2 of 200 manifests) | 0.15-0.19 s | 0.20 s | 0.18-0.33 s | 0.38 s |
| clustered layout, filters the partitions cannot prune (reads 200 of 200) | 14.2-21.3 s | 2.1-3.1 s | 18.5-22.2 s | 3.7-6.0 s |
| scattered layout, every query (reads 200 of 200) | 14.1-21.1 s | 1.9-3.9 s | 18.1-21.6 s | 3.5-6.5 s |
| peak memory, up to 1,000,000 files in the plan | 3.2 GB | 4.1 GB | 3.2 GB | 4.1 GB |

| Smaller tables | ARM64, 1 thread | ARM64, 8 threads | x86-64, 1 thread | x86-64, 8 threads |
|---|---|---|---|---|
| 100,000 files, partition filter (clustered) | 0.15-0.19 s | 0.18-0.19 s | 0.19-0.32 s | 0.33-0.37 s |
| 100,000 files, not prunable | 1.4-1.5 s | 0.3-1.0 s | 1.8-2.4 s | 0.6-1.7 s |
| 10,000 files, any query | 0.14-0.19 s | 0.17-0.20 s | 0.18-0.36 s | 0.35-0.40 s |

Readings.
1. **Partition pruning needs the manifests CLUSTERED by partition.** With files scattered over all partitions, the manifest-level summaries skip nothing, and the same one-partition filter reads all 200
   manifests: 14-21 s on one thread instead of 0.2 s. A table that is not maintained (manifests rewritten, files clustered) loses the benefit. The table-format ADR needs a rule and a
   maintenance job for this.
2. **The cost is Avro decoding and its memory, not the predicate evaluators:** 14-21 microseconds per entry on one thread on ARM64 (1,010,000 entries) and 18-22 on x86-64. Locally the spike measured about
   15 KB of heap per entry while a manifest is being decoded (the generic Avro value tree), which is why 8 manifests in flight cost about 1.3 GB and 1,000,000 returned tasks 3-4 GB.
3. **Eight threads help when there are many manifests:** 3.9-9.9x faster on ARM64 and 3.1-6.0x on x86-64 at 1,000,000 files (the x86-64 vCPUs are 4 cores with 2 hardware threads each). At 10,000 files
   (two manifests) threads do not help: there is nothing to run in parallel.
4. A hand-written decoder straight into compact entries, and streaming tasks out instead of collecting them, are the obvious next steps and were not done here.

Raw output follows.


## Raw output, ARM64 (`n4a-highcpu-8`)

### summary.txt

```
host: aarch64, 8 vCPU, 15978 MiB; rustc 1.99.1 (58cf8f9c3 2026-10-13)
tools: pyiceberg 0.12.0 pyarrow 26.0.1 duckdb 2.0.0
PASS build (100 s)
PASS cargo-tests (17 s)
FAIL mutants (0 s), see mutants.txt
PASS interop-ours-to-others (44 s)
PASS interop-others-to-ours (41 s)
INFO truncate-overflow: exit 0 (1 s), see truncate-overflow.txt
INFO scale: exit 0 (897 s), see scale.txt
VERDICT: FAILED
```

### interop-ours-to-others.txt (last 3 lines)

```
seed 80: pyiceberg: PyIceberg does not yet support equality deletes: https://github.com/apache/iceberg/issues/6568
{"py_zero_semantics": 0, "py_bugs": 11, "py_null_semantics": 24, "tables": 80, "duck_rows_ok": 80, "py_rows_ok": 22, "py_unsupported": 58, "duck_unsupported": 0, "filters": 1200, "duck_count_ok": 1200, "py_count_ok": 295, "py_count_unsupported": 0, "ours_planned": 54082, "py_planned": 12871, "live": 107985}
0 disagreements
```

### interop-others-to-ours.txt (last 3 lines)

```
{"tables": 71, "ours_eq_py": 71, "duck_eq_py": 71, "filters": 1065, "ours_eq_duck_count": 1065, "ours_files": 31371, "py_files": 0, "py_null_semantics": 0, "py_zero_semantics": 0, "py_bugs": 0, "partition_checks": 8125, "skipped_builds": 9}
0 disagreements
```

### truncate-overflow.txt

```
k < 0                  truth 5  DuckDB 3  pyiceberg 3  ROWS LOST
k <= -5                truth 5  DuckDB 3  pyiceberg 3  ROWS LOST
k < -2000000000        truth 4  DuckDB 2  pyiceberg 2  ROWS LOST
k = -2147483648        truth 1  DuckDB 1  pyiceberg 0  ROWS LOST
k > 50                 truth 3  DuckDB 3  pyiceberg 3  OK
k >= 2147483638        truth 2  DuckDB 2  pyiceberg 2  OK
{"k < 0": {"truth": 5, "duckdb": 3, "pyiceberg": 3}, "k <= -5": {"truth": 5, "duckdb": 3, "pyiceberg": 3}, "k < -2000000000": {"truth": 4, "duckdb": 2, "pyiceberg": 2}, "k = -2147483648": {"truth": 1, "duckdb": 1, "pyiceberg": 0}, "k > 50": {"truth": 3, "duckdb": 3, "pyiceberg": 3}, "k >= 2147483638": {"truth": 2, "duckdb": 2, "pyiceberg": 2}}
```

### scale-table.txt

```
/mnt/work/iceberg-plan-work/scale/s-clustered-10000/metadata/v2.metadata.json (10000 files, clustered layout, 2 MB of metadata, built in 0.2 s)
/mnt/work/iceberg-plan-work/scale/s-clustered-100000/metadata/v2.metadata.json (100000 files, clustered layout, 26 MB of metadata, built in 1.6 s)
/mnt/work/iceberg-plan-work/scale/s-clustered-1000000/metadata/v2.metadata.json (1000000 files, clustered layout, 263 MB of metadata, built in 15.7 s)
/mnt/work/iceberg-plan-work/scale/s-scattered-100000/metadata/v2.metadata.json (100000 files, scattered layout, 26 MB of metadata, built in 1.6 s)
/mnt/work/iceberg-plan-work/scale/s-scattered-1000000/metadata/v2.metadata.json (1000000 files, scattered layout, 263 MB of metadata, built in 15.7 s)
layout        files query         thr  planned  ms (min of 3)  manifests read/total   entries peak RSS MB
clustered     10000 point           1        0            193          2/2              10100         165
clustered     10000 week            1       70            142          2/2              10100         163
clustered     10000 id              1        0            193          2/2              10100         165
clustered     10000 name_prefix     1      368            142          2/2              10100         163
clustered     10000 k_range         1      611            142          2/2              10100         163
clustered     10000 nonselective    1    10000            144          2/2              10100         176
clustered     10000 all             1    10000            143          2/2              10100         175
clustered     10000 point           8        0            172          2/2              10100         186
clustered     10000 week            8       70            183          2/2              10100         463
clustered     10000 id              8        0            196          2/2              10100         167
clustered     10000 name_prefix     8      368            198          2/2              10100         323
clustered     10000 k_range         8      611            191          2/2              10100         323
clustered     10000 nonselective    8    10000            189          2/2              10100         330
clustered     10000 all             8    10000            189          2/2              10100         329
clustered    100000 point           1        0            188          2/20             10100         163
clustered    100000 week            1      700            146          2/20             10100         163
clustered    100000 id              1        0           1503         20/20            101000         173
clustered    100000 name_prefix     1     3822           1484         20/20            101000         179
clustered    100000 k_range         1     6312           1528         20/20            101000         184
clustered    100000 nonselective    1   100000           1449         20/20            101000         462
clustered    100000 all             1   100000           1443         20/20            101000         461
clustered    100000 point           8        0            179          2/20             10100         183
clustered    100000 week            8      700            186          2/20             10100         303
clustered    100000 id              8        0            994         20/20            101000        1272
clustered    100000 name_prefix     8     3822            320         20/20            101000        1289
clustered    100000 k_range         8     6312            534         20/20            101000        1292
clustered    100000 nonselective    8   100000            385         20/20            101000        1433
clustered    100000 all             8   100000            365         20/20            101000        1443
clustered   1000000 point           1        0            188          2/200            10100         166
clustered   1000000 week            1     7000            148          2/200            10100         170
clustered   1000000 id              1        1          14169        200/200          1010000         325
clustered   1000000 name_prefix     1    38451          19674        200/200          1010000         317
clustered   1000000 k_range         1    62161          21261        200/200          1010000         403
clustered   1000000 nonselective    1  1000000          14777        200/200          1010000        3159
clustered   1000000 all             1  1000000          14659        200/200          1010000        3139
clustered   1000000 point           8        0            202          2/200            10100         170
clustered   1000000 week            8     7000            202          2/200            10100         336
clustered   1000000 id              8        1           3085        200/200          1010000        1281
clustered   1000000 name_prefix     8    38451           2116        200/200          1010000        1460
clustered   1000000 k_range         8    62161           2157        200/200          1010000        1500
clustered   1000000 nonselective    8  1000000           3015        200/200          1010000        4135
clustered   1000000 all             8  1000000           2976        200/200          1010000        4134
scattered    100000 point           1        0           1489         20/20            101000         165
scattered    100000 week            1      706           1424         20/20            101000         171
scattered    100000 id              1        0           1517         20/20            101000         173
scattered    100000 name_prefix     1     3822           1492         20/20            101000         177
scattered    100000 k_range         1     6312           1529         20/20            101000         185
scattered    100000 nonselective    1   100000           1483         20/20            101000         462
scattered    100000 all             1   100000           1492         20/20            101000         461
scattered    100000 point           8        0           1072         20/20            101000        1269
scattered    100000 week            8      706            301         20/20            101000        1283
scattered    100000 id              8        0            930         20/20            101000        1274
scattered    100000 name_prefix     8     3822            294         20/20            101000        1292
scattered    100000 k_range         8     6312            322         20/20            101000        1295
scattered    100000 nonselective    8   100000            422         20/20            101000        1464
scattered    100000 all             8   100000            411         20/20            101000        1408
scattered   1000000 point           1        0          14662        200/200          1010000         166
scattered   1000000 week            1     6825          15598        200/200          1010000         186
scattered   1000000 id              1        1          14136        200/200          1010000         325
scattered   1000000 name_prefix     1    38451          19631        200/200          1010000         315
scattered   1000000 k_range         1    62161          21120        200/200          1010000         422
scattered   1000000 nonselective    1  1000000          15350        200/200          1010000        3145
scattered   1000000 all             1  1000000          15355        200/200          1010000        3153
scattered   1000000 point           8        0           3105        200/200          1010000        1279
scattered   1000000 week            8     6825           1942        200/200          1010000        1327
scattered   1000000 id              8        1           2649        200/200          1010000        1280
scattered   1000000 name_prefix     8    38451           2127        200/200          1010000        1461
scattered   1000000 k_range         8    62161           2230        200/200          1010000        1499
scattered   1000000 nonselective    8  1000000           3929        200/200          1010000        4106
scattered   1000000 all             8  1000000           3896        200/200          1010000        4107
```


## Raw output, x86-64 (`c3d-highcpu-8`)

### summary.txt

```
host: x86_64, 8 vCPU, 16001 MiB; rustc 1.99.1 (58cf8f9c3 2026-10-13)
tools: pyiceberg 0.12.0 pyarrow 26.0.1 duckdb 2.0.0
PASS build (146 s)
PASS cargo-tests (32 s)
FAIL mutants (0 s), see mutants.txt
PASS interop-ours-to-others (46 s)
PASS interop-others-to-ours (41 s)
INFO truncate-overflow: exit 0 (1 s), see truncate-overflow.txt
INFO scale: exit 0 (1177 s), see scale.txt
VERDICT: FAILED
```

### interop-ours-to-others.txt (last 3 lines)

```
seed 80: pyiceberg: PyIceberg does not yet support equality deletes: https://github.com/apache/iceberg/issues/6568
{"py_zero_semantics": 0, "py_bugs": 11, "py_null_semantics": 24, "tables": 80, "duck_rows_ok": 80, "py_rows_ok": 22, "py_unsupported": 58, "duck_unsupported": 0, "filters": 1200, "duck_count_ok": 1200, "py_count_ok": 295, "py_count_unsupported": 0, "ours_planned": 54082, "py_planned": 12871, "live": 107985}
0 disagreements
```

### interop-others-to-ours.txt (last 3 lines)

```
{"tables": 71, "ours_eq_py": 71, "duck_eq_py": 71, "filters": 1065, "ours_eq_duck_count": 1065, "ours_files": 31320, "py_files": 0, "py_null_semantics": 0, "py_zero_semantics": 0, "py_bugs": 0, "partition_checks": 8125, "skipped_builds": 9}
0 disagreements
```

### truncate-overflow.txt

```
k < 0                  truth 5  DuckDB 3  pyiceberg 3  ROWS LOST
k <= -5                truth 5  DuckDB 3  pyiceberg 3  ROWS LOST
k < -2000000000        truth 4  DuckDB 2  pyiceberg 2  ROWS LOST
k = -2147483648        truth 1  DuckDB 1  pyiceberg 0  ROWS LOST
k > 50                 truth 3  DuckDB 3  pyiceberg 3  OK
k >= 2147483638        truth 2  DuckDB 2  pyiceberg 2  OK
{"k < 0": {"truth": 5, "duckdb": 3, "pyiceberg": 3}, "k <= -5": {"truth": 5, "duckdb": 3, "pyiceberg": 3}, "k < -2000000000": {"truth": 4, "duckdb": 2, "pyiceberg": 2}, "k = -2147483648": {"truth": 1, "duckdb": 1, "pyiceberg": 0}, "k > 50": {"truth": 3, "duckdb": 3, "pyiceberg": 3}, "k >= 2147483638": {"truth": 2, "duckdb": 2, "pyiceberg": 2}}
```

### scale-table.txt

```
/mnt/work/iceberg-plan-work/scale/s-clustered-10000/metadata/v2.metadata.json (10000 files, clustered layout, 2 MB of metadata, built in 0.3 s)
/mnt/work/iceberg-plan-work/scale/s-clustered-100000/metadata/v2.metadata.json (100000 files, clustered layout, 26 MB of metadata, built in 2.0 s)
/mnt/work/iceberg-plan-work/scale/s-clustered-1000000/metadata/v2.metadata.json (1000000 files, clustered layout, 263 MB of metadata, built in 19.2 s)
/mnt/work/iceberg-plan-work/scale/s-scattered-100000/metadata/v2.metadata.json (100000 files, scattered layout, 26 MB of metadata, built in 2.1 s)
/mnt/work/iceberg-plan-work/scale/s-scattered-1000000/metadata/v2.metadata.json (1000000 files, scattered layout, 263 MB of metadata, built in 20.1 s)
layout        files query         thr  planned  ms (min of 3)  manifests read/total   entries peak RSS MB
clustered     10000 point           1        0            348          2/2              10100         166
clustered     10000 week            1       70            189          2/2              10100         164
clustered     10000 id              1        0            357          2/2              10100         166
clustered     10000 name_prefix     1      368            183          2/2              10100         165
clustered     10000 k_range         1      611            191          2/2              10100         164
clustered     10000 nonselective    1    10000            338          2/2              10100         170
clustered     10000 all             1    10000            200          2/2              10100         172
clustered     10000 point           8        0            348          2/2              10100         194
clustered     10000 week            8       70            382          2/2              10100         464
clustered     10000 id              8        0            403          2/2              10100         168
clustered     10000 name_prefix     8      368            400          2/2              10100         323
clustered     10000 k_range         8      611            354          2/2              10100         323
clustered     10000 nonselective    8    10000            379          2/2              10100         481
clustered     10000 all             8    10000            363          2/2              10100         329
clustered    100000 point           1        0            322          2/20             10100         162
clustered    100000 week            1      700            193          2/20             10100         164
clustered    100000 id              1        0           2375         20/20            101000         173
clustered    100000 name_prefix     1     3822           1805         20/20            101000         179
clustered    100000 k_range         1     6312           1802         20/20            101000         187
clustered    100000 nonselective    1   100000           1788         20/20            101000         462
clustered    100000 all             1   100000           1826         20/20            101000         462
clustered    100000 point           8        0            334          2/20             10100         194
clustered    100000 week            8      700            374          2/20             10100         304
clustered    100000 id              8        0           1662         20/20            101000        1277
clustered    100000 name_prefix     8     3822            601         20/20            101000        1290
clustered    100000 k_range         8     6312            559         20/20            101000        1293
clustered    100000 nonselective    8   100000            571         20/20            101000        1426
clustered    100000 all             8   100000            569         20/20            101000        1440
clustered   1000000 point           1        0            328          2/200            10100         166
clustered   1000000 week            1     7000            180          2/200            10100         171
clustered   1000000 id              1        1          18517        200/200          1010000         180
clustered   1000000 name_prefix     1    38451          20838        200/200          1010000         317
clustered   1000000 k_range         1    62161          22154        200/200          1010000         405
clustered   1000000 nonselective    1  1000000          18999        200/200          1010000        3162
clustered   1000000 all             1  1000000          19474        200/200          1010000        3159
clustered   1000000 point           8        0            381          2/200            10100         170
clustered   1000000 week            8     7000            376          2/200            10100         337
clustered   1000000 id              8        1           6042        200/200          1010000        1289
clustered   1000000 name_prefix     8    38451           3703        200/200          1010000        1457
clustered   1000000 k_range         8    62161           3709        200/200          1010000        1511
clustered   1000000 nonselective    8  1000000           4998        200/200          1010000        4134
clustered   1000000 all             8  1000000           4880        200/200          1010000        4134
scattered    100000 point           1        0           2234         20/20            101000         166
scattered    100000 week            1      706           1814         20/20            101000         171
scattered    100000 id              1        0           2243         20/20            101000         173
scattered    100000 name_prefix     1     3822           1893         20/20            101000         180
scattered    100000 k_range         1     6312           1910         20/20            101000         186
scattered    100000 nonselective    1   100000           1912         20/20            101000         462
scattered    100000 all             1   100000           1850         20/20            101000         463
scattered    100000 point           8        0           1977         20/20            101000        1271
scattered    100000 week            8      706            468         20/20            101000        1285
scattered    100000 id              8        0           1880         20/20            101000        1271
scattered    100000 name_prefix     8     3822            488         20/20            101000        1291
scattered    100000 k_range         8     6312            527         20/20            101000        1292
scattered    100000 nonselective    8   100000            917         20/20            101000        1415
scattered    100000 all             8   100000            682         20/20            101000        1433
scattered   1000000 point           1        0          19721        200/200          1010000         167
scattered   1000000 week            1     6825          18643        200/200          1010000         186
scattered   1000000 id              1        1          18121        200/200          1010000         180
scattered   1000000 name_prefix     1    38451          21293        200/200          1010000         317
scattered   1000000 k_range         1    62161          21606        200/200          1010000         423
scattered   1000000 nonselective    1  1000000          18177        200/200          1010000        3148
scattered   1000000 all             1  1000000          19481        200/200          1010000        3136
scattered   1000000 point           8        0           6460        200/200          1010000        1277
scattered   1000000 week            8     6825           3497        200/200          1010000        1324
scattered   1000000 id              8        1           5368        200/200          1010000        1289
scattered   1000000 name_prefix     8    38451           3689        200/200          1010000        1470
scattered   1000000 k_range         8    62161           3680        200/200          1010000        1501
scattered   1000000 nonselective    8  1000000           5614        200/200          1010000        4108
scattered   1000000 all             8  1000000           5306        200/200          1010000        4108
```


## Raw output, the mutation phase rerun, ARM64

```
host: aarch64, 8 vCPU; mode: mutants only
PASS mutants (284 s)
VERDICT: the mutation phase passed

baseline ok
CAUGHT   truncate-remainder-can-be-negative: by spec_truncate_and_time_transform_examples
CAUGHT   day-truncates-toward-zero: by spec_truncate_and_time_transform_examples
CAUGHT   hour-truncates-toward-zero: by spec_truncate_and_time_transform_examples
CAUGHT   bucket-keeps-the-sign: by spec_truncate_and_time_transform_examples
CAUGHT   bucket-hashes-int-as-int: by spec_truncate_and_time_transform_examples, spec_appendix_b_hash_vectors
CAUGHT   time-projection-lt-is-strict: by planner_is_sound_on_random_tables_and_filters, inclusive_projection_is_sound
CAUGHT   truncate-projection-gt-is-strict: by planner_is_sound_on_random_tables_and_filters, inclusive_projection_is_sound
CAUGHT   bucket-projects-ranges: by planner_is_sound_on_random_tables_and_filters, inclusive_projection_is_sound
CAUGHT   le-prunes-equal-lower-bound: by planner_is_sound_on_random_tables_and_filters
CAUGHT   ge-prunes-equal-upper-bound: by planner_is_sound_on_random_tables_and_filters
CAUGHT   eq-prunes-equal-upper-bound: by planner_is_sound_on_random_tables_and_filters
CAUGHT   nan-rows-are-ignored-by-gt: by planner_is_sound_on_random_tables_and_filters
CAUGHT   zero-literal-not-widened: by planner_is_sound_on_random_tables_and_filters
CAUGHT   negated-comparison-forgets-nan: by planner_is_sound_on_random_tables_and_filters
CAUGHT   truncate-overflow-guard-removed: by planner_is_sound_on_random_tables_and_filters
CAUGHT   string-upper-bound-not-incremented: by planner_is_sound_on_random_tables_and_filters
CAUGHT   binary-upper-bound-not-incremented: by planner_is_sound_on_random_tables_and_filters
CAUGHT   manifest-summary-lower-bound-is-the-max: by planner_is_sound_on_random_tables_and_filters
CAUGHT   equality-delete-applies-to-same-commit: by planner_is_sound_on_random_tables_and_filters
CAUGHT   position-delete-misses-same-commit: by planner_is_sound_on_random_tables_and_filters
CAUGHT   global-equality-delete-also-needs-partition: by planner_is_sound_on_random_tables_and_filters
CAUGHT   projection-uses-the-default-spec: by planner_is_sound_on_random_tables_and_filters
```


## Raw output, the mutation phase rerun, x86-64

```
host: x86_64, 8 vCPU; mode: mutants only
PASS mutants (400 s)
VERDICT: the mutation phase passed

baseline ok
CAUGHT   truncate-remainder-can-be-negative: by spec_truncate_and_time_transform_examples
CAUGHT   day-truncates-toward-zero: by spec_truncate_and_time_transform_examples
CAUGHT   hour-truncates-toward-zero: by spec_truncate_and_time_transform_examples
CAUGHT   bucket-keeps-the-sign: by spec_truncate_and_time_transform_examples
CAUGHT   bucket-hashes-int-as-int: by spec_appendix_b_hash_vectors, spec_truncate_and_time_transform_examples
CAUGHT   time-projection-lt-is-strict: by planner_is_sound_on_random_tables_and_filters, inclusive_projection_is_sound
CAUGHT   truncate-projection-gt-is-strict: by planner_is_sound_on_random_tables_and_filters, inclusive_projection_is_sound
CAUGHT   bucket-projects-ranges: by planner_is_sound_on_random_tables_and_filters, inclusive_projection_is_sound
CAUGHT   le-prunes-equal-lower-bound: by planner_is_sound_on_random_tables_and_filters
CAUGHT   ge-prunes-equal-upper-bound: by planner_is_sound_on_random_tables_and_filters
CAUGHT   eq-prunes-equal-upper-bound: by planner_is_sound_on_random_tables_and_filters
CAUGHT   nan-rows-are-ignored-by-gt: by planner_is_sound_on_random_tables_and_filters
CAUGHT   zero-literal-not-widened: by planner_is_sound_on_random_tables_and_filters
CAUGHT   negated-comparison-forgets-nan: by planner_is_sound_on_random_tables_and_filters
CAUGHT   truncate-overflow-guard-removed: by planner_is_sound_on_random_tables_and_filters
CAUGHT   string-upper-bound-not-incremented: by planner_is_sound_on_random_tables_and_filters
CAUGHT   binary-upper-bound-not-incremented: by planner_is_sound_on_random_tables_and_filters
CAUGHT   manifest-summary-lower-bound-is-the-max: by planner_is_sound_on_random_tables_and_filters
CAUGHT   equality-delete-applies-to-same-commit: by planner_is_sound_on_random_tables_and_filters
CAUGHT   position-delete-misses-same-commit: by planner_is_sound_on_random_tables_and_filters
CAUGHT   global-equality-delete-also-needs-partition: by planner_is_sound_on_random_tables_and_filters
CAUGHT   projection-uses-the-default-spec: by planner_is_sound_on_random_tables_and_filters
```
