# Card 0006 part 4, reference run: DuckDB and the spike's controls on a real GCS bucket (2026-11-07)

Decision rule 5 of [card 0006](../../docs/slices/0006-object-store-io.md): "first record DuckDB and spike control results, noise, achievable scaling and resource envelope in `bench/results/YYYYMMDD-object-store-reference.md`; owner/parent freezes final per-cell targets and defaults before production Nata numbers ... Missing real S3/GCS/DuckDB arm is NOT MEASURED, not PASS". This record is that. **It freezes nothing**: it PROPOSES a threshold and a reason for each provisional row of card part 2 (section "Proposed frozen thresholds"). The spike (wrapper policy, cache, runtime arm, faults) is in [20261107-object-store-baseline.md](20261107-object-store-baseline.md). Raw, redacted: `bench/results/raw/object-store-baseline/{arm64,x86-64}/` (CSV per step, logs, expected answers, machine files, orchestrator reports).

## What was measured

TPC-H lineitem SF1 (6,001,215 rows) and SF10 (59,986,052 rows), generated on each VM by `scripts/gen-tpch.sh`, written as the layouts of the earlier object-store spikes by DuckDB (`spikes/object-store-baseline/mk-layouts.sh`): SF1 `big1` (4 row groups, one file), `mid1` (49), `small1` (293), `files100` (100 files, one row group each); SF10 `mid10` (one file, 489 row groups, about 2.0 GiB) and `files10` (10 files, 6 row groups each, about 2.0 GiB). The layouts were uploaded to a dedicated prefix of the write-test bucket (`<write-test bucket>`, us-central1, storage class not confirmed by the metadata call (Standard assumed, as in the earlier real-store record), 114 objects, 5,120,090,626 bytes) by the write-test service account (`<write-test SA>`, read-write Cloud Storage scope; object access on that one bucket, no project role), read back from VMs in the same zone, and **deleted at the end of each run; the prefix was proved empty (0 objects after deleting 114)**. Nothing in `*-poc-data` buckets or the trino account was touched. The bucket also deletes objects older than a day.

Machines (Spot, us-central1-a, Debian 12, 8 vCPU, 16 GB, 1,024 open files default): `n4a-highcpu-8` (ARM64, Neoverse-N3, 8 cores) and `c3d-highcpu-8` (x86-64, AMD EPYC 9B14, 4 cores × 2 threads). Software: DuckDB 2.0.0 with `httpfs`, `object_store` 0.14.2, parquet/arrow 60.1.0, Rust 1.99.1.

Arms, per cell (layout × query), 5 rounds, every arm run twice per round (an arm and its identical twin), arm order rotated every round and cell, the machine settled to load under 0.5 before each step (all steps settled; no NOT SETTLED line), 2,296 timed runs per CPU, **0 wrong answers**: every run's answer was compared with DuckDB on the local source file, and a verification pass of every arm and cell precedes the timing.

| arm | what it is |
|---|---|
| `duck` | DuckDB CLI process per run, `httpfs`, reading `https://storage.googleapis.com/<bucket>/<prefix>/...` with a bearer token from the VM's metadata server in an `http` secret (DuckDB's S3/GCS secret types need an HMAC key the VM's account cannot make; `gs://` URLs were therefore not used), `SET threads=8`. Time = the CLI's own timer for the query statement: planning (HEAD and footers), execution and TLS setup are inside; process start, extension load, secret creation and the token call are outside (they cost 0.19 s on ARM64 and 0.25 s on x86-64 on average, from `process_wall` in the CSV) |
| `ctl` | the unmerged parallel control: one request per needed column chunk, at most 16 in flight, same retry/validation code |
| `wrap` | the wrapper with ADR 0011's merging: column chunks of a row group merged within 1 MiB gaps, 64 MiB responses and 25 % extra data |
| `wrapx8` | the optional cross-row-group merging of rule 2: 8 row groups planned and fetched together (decode still per row group) |
| `wrapc` | `wrap` plus the bounded metadata cache (repeated metadata cells only) |
| `wrapT` | `wrap` with the decode on a separate tokio runtime (baseline record) |

Our arms build a fresh store per run (cold connection pool and credentials, the token fetch is inside the time) and use 8 CPU workers + 2 I/O threads. Queries: `full` = `count(*)` and one `sum` per column, all 16 columns decoded (an earlier version used `count(col)`, which DuckDB answers from Parquet statistics in 0.06 to 0.1 s without reading data; it was replaced before the final runs and is NOT in these tables), `proj` = four BIGINT sums, `sel` = `count(*), sum(l_partkey)` where `l_orderkey` is in a 1 % band (row groups pruned by statistics; DuckDB and the wrapper both prune), `meta` = `count(*)` (footers only; five statements per run for the repeated-metadata cells).

Limits that apply to every number: one bucket, one region, one day, same-zone reads; 5 rounds per cell; GCS Standard; a DuckDB with an auth path (bearer over https) that differs from `gs://` with HMAC; DuckDB request counts were not captured (its HTTP log was not enabled), so DuckDB's requests and bytes are NOT MEASURED; the wrapper's engine is a spike, not Nata; one query at a time (concurrent admitted queries are **NOT MEASURED**).

## Runs, cost, cleanup

| run id | machine | purpose | VM time | cost (VM, orchestrator estimate) | result |
|---|---|---|---|---|---|
| nata-1107-172004-58bb | n4a-highcpu-8 | **final ARM64** | 3,580 s | $0.098 | exit 0 on all 17 steps; cleanup OK; leak scan clean; resources left: none |
| nata-1107-173032-b0c8 | c3d-highcpu-8 | **final x86-64** | 4,359 s | $0.083 | same |
| nata-1107-162014-677c | c3d-highcpu-8 | x86-64 shake-down (first run) | 2,935 s | $0.056 | ran every step; its `full` query was the statistics-answerable one, so its `full` cells are void; the other cells agreed with the final run; cleanup OK. Superseded by the final run (this was that run's one rerun) |
| nata-1107-161117-9152 | n4a-highcpu-8 | first ARM64 attempt | 377 s | $0.010 | stopped by me at the preflight: DuckDB's first answer was polluted by the `CREATE SECRET` result row (`true`) that no local trial had produced because the trial used no secret; fixed, a unit test added |
| nata-1107-171030-b885 | n4a-highcpu-8 | second ARM64 attempt | 369 s | $0.010 | stopped by me at the preflight: the tiny preflight object had DOUBLE columns, the new `full` query rejects types TPC-H does not use; fixed (decimals, dates, strings), the preflight now runs against a local fake in the trial |

Per-run worst case printed by `orchestrate.sh plan` and approved: $0.314 (ARM64, 80 GB data disk, 2.5 VM-hours) and $0.218 (x86-64); the caps were $0.35 each. Actual cost of a final run is the VM figure above plus about $0.01 for the 80 GB data disk, the 20 GB boot disk and the external address for one hour (disk and address not itemized; list prices). Total VM time of all five launches: about $0.257. **Deviation from the brief ("at most one rerun per run after a fix"): the ARM64 run was launched three times**, because two preflight defects (the `CREATE SECRET` row; the DOUBLE-typed preflight object against the new `full` query) were found one after the other by the preflight itself, each within 7 minutes of VM time and about $0.01. The x86-64 run used one rerun (the shake-down, then the final run). After each launch `orchestrate.sh status --run-id <id>` shows no instances and no disks. The orchestrator's `report.txt` shows `VM seconds counted: 0` and `$0.000` for the two aborted attempts (known defect R14, aborted-run accounting); their figures above are from the VM creation and deletion times in its log. Raw: `bench/gcp-runs/<id>/` (not in git); redacted copies of the final runs' results are under `raw/`.

Uploaded objects: each launch uploaded 1 preflight object (aborted attempts: deleted by the same step, "1 objects ... 0 after deleting") and the three complete runs 114 layout objects each; all deleted; each prefix proved empty. A debugging probe on the first ARM64 VM uploaded 1 object under its own prefix and deleted it.

Machine-readable steps (`run-reference.sh`): build, tests, preflight, data, upload, bench-sf1, bench-meta, bench-rt, bench-sf10, bench-rss, bench-scale, cleanup, leakscan, run as a `heavy` job (`heavy.steps` is git-ignored; `gcs.env` with the bucket and a unique prefix is git-ignored and shipped with the tree).

## Noise

Twin ratio = median of an arm / median of its identical twin, per cell (82 cells with twins per CPU, SF1 and SF10, 8 workers). Deviation from 1 (taking the larger of r and 1/r): **median 1.03 (ARM64) and 1.04 (x86-64); 90th percentile 1.10 and 1.17; worst 1.26 (`wrap`) and 1.32 (`wrapx8`)**. Cells that need a larger twin spread are marked in the tables by the twin column; a difference between two arms smaller than the cell's own twin ratio is not a result. Run-to-run spread (max/min of 5) is mostly 1.1 to 1.3, with occasional single-run outliers (for example `sf1/big1/full` `wrapx8` on x86-64: median 1.09 s, one run 6.3 s); medians are not affected. First-byte latency of the real store (all requests, per run, medians over runs): p50 24 to 60 ms, typically p99 36 to 140 ms; single runs reached p99 of 1.0 s (ARM64) and 1.8 s (x86-64). Over about 396,000 requests per CPU the wrapper made **0 retries on ARM64 and 6 on x86-64** (all ended in success); the class of those 6 was not logged. Authentication and connection: a fresh store's first HEAD (token fetch, TLS, request) takes 26 to 47 ms median, a warm HEAD 13 to 27 ms (probes at the start of each step).

## Results per cell

Medians over 5 rounds in seconds, min-max in brackets; `x / y` columns are ratios of medians; "twin ratios" are arm / its twin. `meta` rows are footers only. For each CPU: SF1 first, then SF10, then requests and bytes, repeated metadata reads, scaling with CPU workers and peak RSS.

### ARM64 (`n4a-highcpu-8`, Neoverse-N3)

#### SF1, 8 CPU workers (wall time of the whole query, cold store/process; the answer was checked against DuckDB on local files)

| cell | duck med (min-max) s | ctl med (min-max) s | wrap med (min-max) s | wrapx8 med (min-max) s | twin ratios (arm / twin) | wrap / duck | wrap / ctl | ctl / duck | wrapx8 / wrap |
|---|---|---|---|---|---|---|---|---|---|
| sf1/big1/full | 0.475 (0.425-0.820) | 0.533 (0.518-0.665) | 0.504 (0.479-0.631) | 0.894 (0.887-0.975) | duck 1.02; ctl 0.93; wrap 0.93; wrapx8 0.98 | 1.06 | 0.94 | 1.12 | 1.77 |
| sf1/big1/meta | 0.068 (0.062-0.085) | 0.071 (0.061-0.080) | 0.075 (0.064-0.112) | 0.072 (0.065-0.095) | duck 1.03; ctl 0.98; wrap 1.26; wrapx8 1.16 | 1.10 | 1.06 | 1.04 | 0.97 |
| sf1/big1/proj | 0.236 (0.219-0.740) | 0.200 (0.166-0.308) | 0.216 (0.185-0.220) | 0.288 (0.262-0.305) | duck 0.98; ctl 1.12; wrap 1.08; wrapx8 1.10 | 0.92 | 1.08 | 0.85 | 1.33 |
| sf1/big1/sel | 0.183 (0.181-0.196) | 0.145 (0.137-0.158) | 0.162 (0.140-0.178) | 0.166 (0.149-0.174) | duck 0.92; ctl 0.93; wrap 0.99; wrapx8 1.04 | 0.89 | 1.12 | 0.79 | 1.02 |
| sf1/files100/full | 0.473 (0.439-0.845) | 2.883 (2.869-3.011) | 0.586 (0.548-0.740) | 0.612 (0.567-0.679) | duck 1.00; ctl 1.02; wrap 1.02; wrapx8 0.98 | 1.24 | 0.20 | 6.09 | 1.04 |
| sf1/files100/meta | 0.309 (0.288-0.318) | 0.532 (0.517-0.557) | 0.547 (0.533-0.578) | 0.575 (0.534-0.622) | duck 0.99; ctl 0.97; wrap 1.00; wrapx8 1.04 | 1.77 | 1.03 | 1.72 | 1.05 |
| sf1/files100/proj | 0.422 (0.405-0.446) | 0.970 (0.936-0.984) | 0.596 (0.554-0.613) | 0.590 (0.562-0.598) | duck 1.04; ctl 1.01; wrap 1.04; wrapx8 1.03 | 1.41 | 0.61 | 2.30 | 0.99 |
| sf1/files100/sel | 0.307 (0.297-0.325) | 0.555 (0.531-0.586) | 0.547 (0.525-0.573) | 0.534 (0.524-0.558) | duck 1.00; ctl 1.02; wrap 1.00; wrapx8 0.97 | 1.78 | 0.99 | 1.81 | 0.98 |
| sf1/mid1/full | 0.374 (0.348-0.446) | 2.310 (2.280-2.380) | 0.379 (0.369-0.414) | 0.475 (0.435-0.509) | duck 0.96; ctl 1.02; wrap 0.98; wrapx8 0.95 | 1.01 | 0.16 | 6.18 | 1.25 |
| sf1/mid1/meta | 0.060 (0.058-0.063) | 0.089 (0.079-0.102) | 0.082 (0.069-0.087) | 0.083 (0.076-0.097) | duck 1.09; ctl 1.02; wrap 0.87; wrapx8 0.97 | 1.37 | 0.92 | 1.49 | 1.01 |
| sf1/mid1/proj | 0.265 (0.259-0.284) | 0.681 (0.678-0.713) | 0.300 (0.266-0.306) | 0.295 (0.287-0.301) | duck 0.90; ctl 1.01; wrap 1.07; wrapx8 1.01 | 1.13 | 0.44 | 2.57 | 0.98 |
| sf1/mid1/sel | 0.152 (0.133-0.172) | 0.161 (0.147-0.193) | 0.150 (0.145-0.191) | 0.170 (0.128-0.175) | duck 0.93; ctl 0.99; wrap 1.00; wrapx8 1.06 | 0.99 | 0.93 | 1.06 | 1.13 |
| sf1/small1/full | 0.728 (0.692-0.878) | 13.178 (12.730-13.545) | 1.046 (1.033-1.151) | 0.398 (0.382-0.409) | duck 1.02; ctl 0.99; wrap 0.96; wrapx8 1.03 | 1.44 | 0.08 | 18.10 | 0.38 |
| sf1/small1/meta | 0.111 (0.087-0.119) | 0.094 (0.090-0.102) | 0.095 (0.088-0.098) | 0.102 (0.093-0.126) | duck 1.12; ctl 0.84; wrap 1.03; wrapx8 1.06 | 0.86 | 1.01 | 0.85 | 1.08 |
| sf1/small1/proj | 0.655 (0.619-0.661) | 3.358 (3.329-3.399) | 0.972 (0.946-1.002) | 0.881 (0.866-0.899) | duck 1.04; ctl 1.01; wrap 1.00; wrapx8 0.98 | 1.48 | 0.29 | 5.13 | 0.91 |
| sf1/small1/sel | 0.178 (0.172-0.181) | 0.161 (0.148-0.175) | 0.165 (0.142-0.185) | 0.150 (0.143-0.163) | duck 0.95; ctl 1.04; wrap 1.06; wrapx8 0.95 | 0.93 | 1.03 | 0.90 | 0.91 |

#### SF10, 8 CPU workers

| cell | duck med (min-max) s | ctl med (min-max) s | wrap med (min-max) s | twin ratios (arm / twin) | wrap / duck | wrap / ctl | ctl / duck | wrapx8 / wrap |
|---|---|---|---|---|---|---|---|---|
| sf10/files10/full | 2.073 (1.997-2.092) | 3.698 (3.588-3.885) | 1.583 (1.513-1.636) | duck 1.02; ctl 1.02; wrap 1.01 | 0.76 | 0.43 | 1.78 | - |
| sf10/files10/proj | 0.763 (0.736-0.777) | 1.009 (0.996-1.042) | 0.629 (0.583-0.649) | duck 1.00; ctl 1.00; wrap 0.96 | 0.82 | 0.62 | 1.32 | - |
| sf10/files10/sel | 0.244 (0.236-0.268) | 0.147 (0.139-0.157) | 0.163 (0.143-0.196) | duck 0.97; ctl 1.02; wrap 1.08 | 0.67 | 1.11 | 0.60 | - |
| sf10/mid10/full | 2.709 (2.674-2.710) | 22.965 (22.682-23.813) | 2.852 (2.780-2.939) | duck 1.00; ctl 1.01; wrap 1.02 | 1.05 | 0.12 | 8.48 | - |
| sf10/mid10/proj | 1.101 (1.061-1.320) | 5.733 (5.684-5.924) | 1.864 (1.792-1.904) | duck 1.02; ctl 0.99; wrap 1.01 | 1.69 | 0.33 | 5.21 | - |
| sf10/mid10/sel | 0.192 (0.181-0.194) | 0.151 (0.147-0.186) | 0.156 (0.145-0.160) | duck 1.01; ctl 0.90; wrap 1.06 | 0.81 | 1.03 | 0.78 | - |

#### SF1 requests and bytes (medians over runs)

| cell | arm | data GETs | fetched MiB | needed MiB | extra % | retries | first-byte p50 / p99 ms (median over runs) | CPU s | first unit s |
|---|---|---|---|---|---|---|---|---|---|
| sf1/big1/full | ctl | 64 | 181.0 | 181.0 | 0 | 0 | 42 / 74 | 0.89 | 0.363 |
| sf1/big1/full | wrap | 4 | 181.0 | 181.0 | 0 | 0 | 48 / 66 | 0.84 | 0.401 |
| sf1/big1/full | wrapx8 | 3 | 181.0 | 181.0 | 0 | 0 | 41 / 56 | 0.83 | 0.887 |
| sf1/big1/proj | ctl | 16 | 37.0 | 37.0 | 0 | 0 | 50 / 105 | 0.16 | 0.149 |
| sf1/big1/proj | wrap | 4 | 37.0 | 37.0 | 0 | 0 | 46 / 66 | 0.16 | 0.162 |
| sf1/big1/proj | wrapx8 | 4 | 37.0 | 37.0 | 0 | 0 | 47 / 73 | 0.16 | 0.281 |
| sf1/big1/sel | ctl | 2 | 6.4 | 6.4 | 0 | 0 | 38 / 49 | 0.04 | 0.137 |
| sf1/big1/sel | wrap | 1 | 6.4 | 6.4 | 0 | 0 | 32 / 52 | 0.03 | 0.155 |
| sf1/big1/sel | wrapx8 | 1 | 6.4 | 6.4 | 0 | 0 | 35 / 48 | 0.04 | 0.158 |
| sf1/files100/full | ctl | 1600 | 198.1 | 198.1 | 0 | 0 | 24 / 40 | 1.26 | 0.096 |
| sf1/files100/full | wrap | 100 | 198.1 | 198.1 | 0 | 0 | 24 / 38 | 0.97 | 0.083 |
| sf1/files100/full | wrapx8 | 100 | 198.1 | 198.1 | 0 | 0 | 24 / 37 | 0.97 | 0.080 |
| sf1/files100/proj | ctl | 400 | 51.1 | 51.1 | 0 | 0 | 24 / 36 | 0.34 | 0.081 |
| sf1/files100/proj | wrap | 100 | 51.1 | 51.1 | 0 | 0 | 24 / 41 | 0.30 | 0.069 |
| sf1/files100/proj | wrapx8 | 100 | 51.1 | 51.1 | 0 | 0 | 24 / 36 | 0.30 | 0.071 |
| sf1/files100/sel | ctl | 4 | 0.7 | 0.7 | 0 | 0 | 24 / 36 | 0.06 | 0.264 |
| sf1/files100/sel | wrap | 2 | 0.7 | 0.7 | 0 | 0 | 24 / 39 | 0.07 | 0.263 |
| sf1/files100/sel | wrapx8 | 2 | 0.7 | 0.7 | 0 | 0 | 24 / 36 | 0.07 | 0.242 |
| sf1/mid1/full | ctl | 784 | 194.5 | 194.5 | 0 | 0 | 43 / 76 | 1.11 | 0.160 |
| sf1/mid1/full | wrap | 49 | 194.5 | 194.5 | 0 | 0 | 51 / 107 | 0.88 | 0.146 |
| sf1/mid1/full | wrapx8 | 7 | 194.5 | 194.5 | 0 | 0 | 61 / 86 | 0.85 | 0.173 |
| sf1/mid1/proj | ctl | 196 | 49.1 | 49.1 | 0 | 0 | 43 / 82 | 0.27 | 0.134 |
| sf1/mid1/proj | wrap | 49 | 49.1 | 49.1 | 0 | 0 | 46 / 90 | 0.24 | 0.127 |
| sf1/mid1/proj | wrapx8 | 49 | 49.1 | 49.1 | 0 | 0 | 46 / 85 | 0.24 | 0.177 |
| sf1/mid1/sel | ctl | 4 | 1.4 | 1.4 | 0 | 0 | 39 / 72 | 0.02 | 0.138 |
| sf1/mid1/sel | wrap | 2 | 1.4 | 1.4 | 0 | 0 | 28 / 66 | 0.02 | 0.123 |
| sf1/mid1/sel | wrapx8 | 2 | 1.4 | 1.4 | 0 | 0 | 32 / 76 | 0.02 | 0.163 |
| sf1/small1/full | ctl | 4704 | 211.4 | 211.4 | 0 | 0 | 43 / 72 | 1.67 | 0.166 |
| sf1/small1/full | wrap | 294 | 211.4 | 211.4 | 0 | 0 | 44 / 83 | 1.07 | 0.142 |
| sf1/small1/full | wrapx8 | 37 | 211.4 | 211.4 | 0 | 0 | 48 / 98 | 0.91 | 0.173 |
| sf1/small1/proj | ctl | 1176 | 58.6 | 58.6 | 0 | 0 | 43 / 71 | 0.45 | 0.143 |
| sf1/small1/proj | wrap | 294 | 58.6 | 58.6 | 0 | 0 | 43 / 80 | 0.35 | 0.129 |
| sf1/small1/proj | wrapx8 | 266 | 73.2 | 58.6 | 25 | 0 | 43 / 77 | 0.36 | 0.153 |
| sf1/small1/sel | ctl | 8 | 0.5 | 0.5 | 0 | 0 | 44 / 61 | 0.02 | 0.136 |
| sf1/small1/sel | wrap | 4 | 0.5 | 0.5 | 0 | 0 | 38 / 55 | 0.02 | 0.139 |
| sf1/small1/sel | wrapx8 | 4 | 0.5 | 0.5 | 0 | 0 | 40 / 57 | 0.02 | 0.143 |

#### SF10 requests and bytes

| cell | arm | data GETs | fetched MiB | needed MiB | extra % | retries | first-byte p50 / p99 ms (median over runs) | CPU s | first unit s |
|---|---|---|---|---|---|---|---|---|---|
| sf10/files10/full | ctl | 960 | 1975.5 | 1975.5 | 0 | 0 | 40 / 80 | 9.67 | 0.324 |
| sf10/files10/full | wrap | 60 | 1975.5 | 1975.5 | 0 | 0 | 45 / 342 | 8.09 | 0.349 |
| sf10/files10/proj | ctl | 240 | 534.9 | 534.9 | 0 | 0 | 42 / 77 | 2.04 | 0.146 |
| sf10/files10/proj | wrap | 60 | 534.9 | 534.9 | 0 | 0 | 42 / 137 | 1.88 | 0.161 |
| sf10/files10/sel | ctl | 2 | 6.2 | 6.2 | 0 | 0 | 36 / 47 | 0.05 | 0.139 |
| sf10/files10/sel | wrap | 1 | 6.2 | 6.2 | 0 | 0 | 37 / 48 | 0.05 | 0.156 |
| sf10/mid10/full | ctl | 7824 | 2097.3 | 2097.3 | 0 | 0 | 44 / 78 | 11.47 | 0.159 |
| sf10/mid10/full | wrap | 489 | 2097.3 | 2097.3 | 0 | 0 | 46 / 84 | 9.53 | 0.163 |
| sf10/mid10/proj | ctl | 1956 | 643.6 | 643.6 | 0 | 0 | 43 / 75 | 3.02 | 0.136 |
| sf10/mid10/proj | wrap | 489 | 643.6 | 643.6 | 0 | 0 | 45 / 90 | 2.66 | 0.133 |
| sf10/mid10/sel | ctl | 12 | 4.6 | 4.6 | 0 | 0 | 48 / 66 | 0.05 | 0.128 |
| sf10/mid10/sel | wrap | 6 | 4.6 | 4.6 | 0 | 0 | 40 / 56 | 0.04 | 0.119 |

#### Repeated metadata reads (the `meta` query: footers only, 5 statements per run; rep 0 is cold)

| cell | arm | rep 0 med s | reps 1-4 med s | HEADs / footer GETs rep 0 | HEADs / footer GETs rep 1 | twin ratio rep 0 |
|---|---|---|---|---|---|---|
| sf1/files100/meta | duck | 0.317 | 0.244 | (not counted: DuckDB) | (not counted) | 1.05 |
| sf1/files100/meta | wrap | 0.534 | 0.531 | 100 / 100 | 100 / 100 | 0.97 |
| sf1/files100/meta | wrapc | 0.526 | 0.205 | 100 / 100 | 100 / 0 | 0.98 |
| sf1/small1/meta | duck | 0.103 | 0.089 | (not counted: DuckDB) | (not counted) | 1.04 |
| sf1/small1/meta | wrap | 0.096 | 0.096 | 1 / 2 | 1 / 2 | 0.94 |
| sf1/small1/meta | wrapc | 0.097 | 0.026 | 1 / 2 | 1 / 0 | 1.01 |

#### Scaling with CPU workers, SF10 `mid10` (one file, 489 row groups); speedup = median(1 worker) / median(N workers)

| query | arm | 1 worker s | 4 workers s | 8 workers s | speedup at 4 | speedup at 8 |
|---|---|---|---|---|---|---|
| full | duck | 10.49 | 2.90 | 2.71 | 3.61 | 3.87 |
| full | wrap | 11.54 | 5.44 | 2.85 | 2.12 | 4.05 |
| proj | duck | 7.85 | 1.98 | 1.10 | 3.97 | 7.13 |
| proj | wrap | 7.24 | 3.51 | 1.86 | 2.06 | 3.88 |

#### Peak RSS, one cell and one arm per FRESH process (full scans)

| cell | duck MiB | ctl MiB | wrap MiB |
|---|---|---|---|
| sf1/files100/full | 389 | 106 | 114 |
| sf1/small1/full | 260 | 67 | 76 |
| sf10/files10/full | 2370 | 670 | 1020 |
| sf10/mid10/full | 681 | 166 | 224 |

### x86-64 (`c3d-highcpu-8`, AMD EPYC 9B14)

#### SF1, 8 CPU workers (wall time of the whole query, cold store/process; the answer was checked against DuckDB on local files)

| cell | duck med (min-max) s | ctl med (min-max) s | wrap med (min-max) s | wrapx8 med (min-max) s | twin ratios (arm / twin) | wrap / duck | wrap / ctl | ctl / duck | wrapx8 / wrap |
|---|---|---|---|---|---|---|---|---|---|
| sf1/big1/full | 0.585 (0.506-0.939) | 0.654 (0.581-0.746) | 0.507 (0.450-0.727) | 1.090 (1.056-6.320) | duck 1.08; ctl 1.04; wrap 0.83; wrapx8 0.99 | 0.87 | 0.78 | 1.12 | 2.15 |
| sf1/big1/meta | 0.084 (0.067-0.184) | 0.075 (0.067-0.097) | 0.074 (0.060-0.086) | 0.068 (0.063-0.112) | duck 1.02; ctl 1.05; wrap 1.08; wrapx8 0.92 | 0.88 | 0.98 | 0.89 | 0.93 |
| sf1/big1/proj | 0.289 (0.259-0.681) | 0.238 (0.208-0.381) | 0.220 (0.189-0.257) | 0.295 (0.260-0.306) | duck 1.02; ctl 1.22; wrap 1.07; wrapx8 0.89 | 0.76 | 0.92 | 0.82 | 1.34 |
| sf1/big1/sel | 0.212 (0.177-0.249) | 0.157 (0.150-0.169) | 0.184 (0.172-0.222) | 0.183 (0.173-0.255) | duck 1.10; ctl 0.86; wrap 1.12; wrapx8 1.03 | 0.87 | 1.17 | 0.74 | 1.00 |
| sf1/files100/full | 0.615 (0.597-0.799) | 3.348 (3.220-12.359) | 0.744 (0.694-1.113) | 0.847 (0.726-1.007) | duck 0.97; ctl 1.04; wrap 0.84; wrapx8 0.95 | 1.21 | 0.22 | 5.44 | 1.14 |
| sf1/files100/meta | 0.400 (0.386-0.434) | 0.674 (0.608-0.815) | 0.715 (0.682-0.834) | 0.696 (0.618-0.842) | duck 0.99; ctl 0.85; wrap 1.02; wrapx8 0.84 | 1.79 | 1.06 | 1.69 | 0.97 |
| sf1/files100/proj | 0.533 (0.469-0.603) | 1.178 (1.055-1.273) | 0.747 (0.670-0.828) | 0.731 (0.697-0.833) | duck 1.02; ctl 1.03; wrap 1.03; wrapx8 0.98 | 1.40 | 0.63 | 2.21 | 0.98 |
| sf1/files100/sel | 0.429 (0.367-0.481) | 0.735 (0.698-0.922) | 0.730 (0.639-1.102) | 0.711 (0.663-0.839) | duck 0.97; ctl 1.00; wrap 1.05; wrapx8 0.99 | 1.70 | 0.99 | 1.71 | 0.97 |
| sf1/mid1/full | 0.441 (0.394-0.658) | 2.519 (2.335-2.791) | 0.434 (0.404-0.461) | 0.583 (0.516-0.913) | duck 1.03; ctl 1.04; wrap 1.02; wrapx8 1.11 | 0.98 | 0.17 | 5.71 | 1.34 |
| sf1/mid1/meta | 0.068 (0.056-0.073) | 0.109 (0.099-0.127) | 0.095 (0.076-0.127) | 0.097 (0.086-0.104) | duck 1.08; ctl 1.09; wrap 0.90; wrapx8 1.03 | 1.40 | 0.87 | 1.60 | 1.02 |
| sf1/mid1/proj | 0.374 (0.294-0.413) | 0.774 (0.691-0.787) | 0.329 (0.304-0.421) | 0.339 (0.294-0.414) | duck 1.17; ctl 1.04; wrap 1.00; wrapx8 1.03 | 0.88 | 0.43 | 2.07 | 1.03 |
| sf1/mid1/sel | 0.174 (0.164-0.190) | 0.203 (0.150-0.364) | 0.177 (0.144-0.217) | 0.172 (0.151-0.203) | duck 0.91; ctl 1.16; wrap 1.14; wrapx8 0.76 | 1.02 | 0.87 | 1.16 | 0.97 |
| sf1/small1/full | 0.779 (0.764-0.845) | 14.030 (13.555-14.501) | 1.236 (1.094-1.324) | 0.458 (0.438-0.535) | duck 0.99; ctl 1.01; wrap 1.02; wrapx8 0.91 | 1.59 | 0.09 | 18.01 | 0.37 |
| sf1/small1/meta | 0.116 (0.107-0.129) | 0.117 (0.101-0.123) | 0.113 (0.110-0.120) | 0.115 (0.106-0.120) | duck 1.05; ctl 1.09; wrap 0.98; wrapx8 1.02 | 0.97 | 0.97 | 1.00 | 1.02 |
| sf1/small1/proj | 0.801 (0.718-0.808) | 3.602 (3.428-3.701) | 1.020 (1.010-1.167) | 0.970 (0.925-1.089) | duck 1.12; ctl 1.01; wrap 1.00; wrapx8 0.97 | 1.27 | 0.28 | 4.50 | 0.95 |
| sf1/small1/sel | 0.218 (0.195-0.243) | 0.169 (0.162-0.189) | 0.169 (0.155-0.241) | 0.174 (0.154-0.189) | duck 0.96; ctl 1.04; wrap 0.93; wrapx8 0.93 | 0.77 | 1.00 | 0.78 | 1.03 |

#### SF10, 8 CPU workers

| cell | duck med (min-max) s | ctl med (min-max) s | wrap med (min-max) s | twin ratios (arm / twin) | wrap / duck | wrap / ctl | ctl / duck | wrapx8 / wrap |
|---|---|---|---|---|---|---|---|---|
| sf10/files10/full | 2.367 (2.270-2.496) | 4.115 (3.980-4.176) | 2.271 (2.243-2.423) | duck 0.98; ctl 1.08; wrap 0.99 | 0.96 | 0.55 | 1.74 | - |
| sf10/files10/proj | 0.915 (0.821-1.019) | 1.121 (1.096-1.205) | 0.707 (0.626-0.751) | duck 1.04; ctl 0.96; wrap 0.97 | 0.77 | 0.63 | 1.22 | - |
| sf10/files10/sel | 0.295 (0.281-0.408) | 0.163 (0.150-0.213) | 0.187 (0.157-0.252) | duck 0.98; ctl 1.00; wrap 1.03 | 0.63 | 1.15 | 0.55 | - |
| sf10/mid10/full | 2.973 (2.893-3.088) | 24.613 (23.567-25.535) | 3.357 (3.166-3.492) | duck 1.02; ctl 1.01; wrap 1.04 | 1.13 | 0.14 | 8.28 | - |
| sf10/mid10/proj | 1.260 (1.183-1.334) | 6.380 (6.173-6.464) | 2.028 (1.937-2.200) | duck 1.00; ctl 0.98; wrap 0.90 | 1.61 | 0.32 | 5.06 | - |
| sf10/mid10/sel | 0.222 (0.206-0.368) | 0.230 (0.155-0.281) | 0.173 (0.150-0.306) | duck 0.91; ctl 1.22; wrap 0.93 | 0.78 | 0.75 | 1.04 | - |

#### SF1 requests and bytes (medians over runs)

| cell | arm | data GETs | fetched MiB | needed MiB | extra % | retries | first-byte p50 / p99 ms (median over runs) | CPU s | first unit s |
|---|---|---|---|---|---|---|---|---|---|
| sf1/big1/full | ctl | 64 | 181.0 | 181.0 | 0 | 0 | 46 / 107 | 1.19 | 0.445 |
| sf1/big1/full | wrap | 4 | 181.0 | 181.0 | 0 | 0 | 48 / 55 | 1.07 | 0.424 |
| sf1/big1/full | wrapx8 | 3 | 181.0 | 181.0 | 0 | 0 | 43 / 69 | 1.08 | 1.079 |
| sf1/big1/proj | ctl | 16 | 37.0 | 37.0 | 0 | 0 | 54 / 112 | 0.22 | 0.170 |
| sf1/big1/proj | wrap | 4 | 37.0 | 37.0 | 0 | 0 | 46 / 61 | 0.21 | 0.166 |
| sf1/big1/proj | wrapx8 | 4 | 37.0 | 37.0 | 0 | 0 | 47 / 59 | 0.20 | 0.284 |
| sf1/big1/sel | ctl | 2 | 6.4 | 6.4 | 0 | 0 | 45 / 49 | 0.05 | 0.146 |
| sf1/big1/sel | wrap | 1 | 6.4 | 6.4 | 0 | 0 | 36 / 50 | 0.05 | 0.173 |
| sf1/big1/sel | wrapx8 | 1 | 6.4 | 6.4 | 0 | 0 | 38 / 48 | 0.05 | 0.173 |
| sf1/files100/full | ctl | 1600 | 198.1 | 198.1 | 0 | 0 | 27 / 59 | 1.62 | 0.112 |
| sf1/files100/full | wrap | 100 | 198.1 | 198.1 | 0 | 0 | 26 / 56 | 1.45 | 0.085 |
| sf1/files100/full | wrapx8 | 100 | 198.1 | 198.1 | 0 | 0 | 26 / 54 | 1.48 | 0.103 |
| sf1/files100/proj | ctl | 400 | 51.1 | 51.1 | 0 | 0 | 26 / 59 | 0.45 | 0.088 |
| sf1/files100/proj | wrap | 100 | 51.1 | 51.1 | 0 | 0 | 26 / 51 | 0.40 | 0.082 |
| sf1/files100/proj | wrapx8 | 100 | 51.1 | 51.1 | 0 | 0 | 27 / 57 | 0.40 | 0.076 |
| sf1/files100/sel | ctl | 4 | 0.7 | 0.7 | 0 | 0 | 26 / 49 | 0.09 | 0.336 |
| sf1/files100/sel | wrap | 2 | 0.7 | 0.7 | 0 | 0 | 26 / 54 | 0.08 | 0.333 |
| sf1/files100/sel | wrapx8 | 2 | 0.7 | 0.7 | 0 | 0 | 26 / 49 | 0.08 | 0.337 |
| sf1/mid1/full | ctl | 784 | 194.5 | 194.5 | 0 | 0 | 44 / 87 | 1.42 | 0.203 |
| sf1/mid1/full | wrap | 49 | 194.5 | 194.5 | 0 | 0 | 54 / 119 | 1.42 | 0.160 |
| sf1/mid1/full | wrapx8 | 7 | 194.5 | 194.5 | 0 | 0 | 55 / 90 | 1.25 | 0.172 |
| sf1/mid1/proj | ctl | 196 | 49.1 | 49.1 | 0 | 0 | 45 / 91 | 0.34 | 0.144 |
| sf1/mid1/proj | wrap | 49 | 49.1 | 49.1 | 0 | 0 | 50 / 118 | 0.31 | 0.137 |
| sf1/mid1/proj | wrapx8 | 49 | 49.1 | 49.1 | 0 | 0 | 52 / 98 | 0.31 | 0.191 |
| sf1/mid1/sel | ctl | 4 | 1.4 | 1.4 | 0 | 0 | 46 / 72 | 0.03 | 0.168 |
| sf1/mid1/sel | wrap | 2 | 1.4 | 1.4 | 0 | 0 | 35 / 57 | 0.02 | 0.155 |
| sf1/mid1/sel | wrapx8 | 2 | 1.4 | 1.4 | 0 | 0 | 31 / 80 | 0.02 | 0.162 |
| sf1/small1/full | ctl | 4704 | 211.6 | 211.6 | 0 | 0 | 44 / 94 | 2.12 | 0.181 |
| sf1/small1/full | wrap | 294 | 211.6 | 211.6 | 0 | 0 | 47 / 107 | 1.50 | 0.159 |
| sf1/small1/full | wrapx8 | 37 | 211.6 | 211.6 | 0 | 0 | 50 / 108 | 1.46 | 0.201 |
| sf1/small1/proj | ctl | 1176 | 58.9 | 58.9 | 0 | 0 | 43 / 104 | 0.60 | 0.157 |
| sf1/small1/proj | wrap | 294 | 58.9 | 58.9 | 0 | 0 | 44 / 100 | 0.45 | 0.145 |
| sf1/small1/proj | wrapx8 | 266 | 73.3 | 58.9 | 25 | 0 | 45 / 92 | 0.47 | 0.154 |
| sf1/small1/sel | ctl | 8 | 0.5 | 0.5 | 0 | 0 | 44 / 62 | 0.03 | 0.137 |
| sf1/small1/sel | wrap | 4 | 0.5 | 0.5 | 0 | 0 | 39 / 53 | 0.03 | 0.147 |
| sf1/small1/sel | wrapx8 | 4 | 0.5 | 0.5 | 0 | 0 | 39 / 56 | 0.02 | 0.163 |

#### SF10 requests and bytes

| cell | arm | data GETs | fetched MiB | needed MiB | extra % | retries | first-byte p50 / p99 ms (median over runs) | CPU s | first unit s |
|---|---|---|---|---|---|---|---|---|---|
| sf10/files10/full | ctl | 960 | 1975.5 | 1975.5 | 0 | 0 | 44 / 138 | 13.99 | 0.386 |
| sf10/files10/full | wrap | 60 | 1975.5 | 1975.5 | 0 | 0 | 55 / 482 | 14.91 | 0.496 |
| sf10/files10/proj | ctl | 240 | 534.9 | 534.9 | 0 | 0 | 43 / 244 | 2.73 | 0.166 |
| sf10/files10/proj | wrap | 60 | 534.9 | 534.9 | 0 | 0 | 44 / 321 | 2.74 | 0.191 |
| sf10/files10/sel | ctl | 2 | 6.2 | 6.2 | 0 | 0 | 41 / 54 | 0.06 | 0.152 |
| sf10/files10/sel | wrap | 1 | 6.2 | 6.2 | 0 | 0 | 40 / 77 | 0.06 | 0.176 |
| sf10/mid10/full | ctl | 7824 | 2097.3 | 2097.3 | 0 | 0 | 46 / 105 | 14.65 | 0.246 |
| sf10/mid10/full | wrap | 489 | 2097.3 | 2097.3 | 0 | 0 | 48 / 146 | 14.75 | 0.176 |
| sf10/mid10/proj | ctl | 1956 | 643.6 | 643.6 | 0 | 0 | 46 / 121 | 3.68 | 0.142 |
| sf10/mid10/proj | wrap | 489 | 643.6 | 643.6 | 0 | 0 | 48 / 146 | 3.39 | 0.154 |
| sf10/mid10/sel | ctl | 12 | 4.6 | 4.6 | 0 | 0 | 48 / 101 | 0.06 | 0.130 |
| sf10/mid10/sel | wrap | 6 | 4.6 | 4.6 | 0 | 0 | 47 / 61 | 0.05 | 0.141 |

#### Repeated metadata reads (the `meta` query: footers only, 5 statements per run; rep 0 is cold)

| cell | arm | rep 0 med s | reps 1-4 med s | HEADs / footer GETs rep 0 | HEADs / footer GETs rep 1 | twin ratio rep 0 |
|---|---|---|---|---|---|---|
| sf1/files100/meta | duck | 0.396 | 0.301 | (not counted: DuckDB) | (not counted) | 1.00 |
| sf1/files100/meta | wrap | 0.774 | 0.660 | 100 / 100 | 100 / 100 | 1.06 |
| sf1/files100/meta | wrapc | 0.757 | 0.283 | 100 / 100 | 100 / 0 | 1.13 |
| sf1/small1/meta | duck | 0.121 | 0.100 | (not counted: DuckDB) | (not counted) | 1.20 |
| sf1/small1/meta | wrap | 0.105 | 0.102 | 1 / 2 | 1 / 2 | 1.00 |
| sf1/small1/meta | wrapc | 0.106 | 0.029 | 1 / 2 | 1 / 0 | 1.03 |

#### Scaling with CPU workers, SF10 `mid10` (one file, 489 row groups); speedup = median(1 worker) / median(N workers)

| query | arm | 1 worker s | 4 workers s | 8 workers s | speedup at 4 | speedup at 8 |
|---|---|---|---|---|---|---|
| full | duck | 13.41 | 3.34 | 2.97 | 4.01 | 4.51 |
| full | wrap | 13.40 | 6.25 | 3.36 | 2.14 | 3.99 |
| proj | duck | 9.01 | 2.23 | 1.26 | 4.03 | 7.15 |
| proj | wrap | 7.99 | 3.87 | 2.03 | 2.06 | 3.94 |

#### Peak RSS, one cell and one arm per FRESH process (full scans)

| cell | duck MiB | ctl MiB | wrap MiB |
|---|---|---|---|
| sf1/files100/full | 387 | 119 | 132 |
| sf1/small1/full | 271 | 68 | 84 |
| sf10/files10/full | 2509 | 742 | 1096 |
| sf10/mid10/full | 729 | 175 | 240 |

## Findings the tables show

- **The wrapper with ADR 0011's merging is as fast as DuckDB where the work is decode or bandwidth, and slower where DuckDB issues more parallel requests.** `wrap` / DuckDB: 0.76 to 1.13 on full scans of `big1`, `mid1`, `mid10`, `files10`; 1.21 to 1.59 on `files100` and `small1` full; 1.27 to 1.69 on `proj` of `small1`, `files100` and `mid10`; 0.63 to 1.02 on `sel` (except `files100`); 1.7 to 1.8 on `files100` `sel` and `meta` (100 files, footer-bound). Why (reasoned, not profiled): footers are opened 8 files ahead, each HEAD then footer GET, so 100 footers are 100 sequential round trips per slot; DuckDB's footer fan-out for the same cell was not measured.
- **Merging within a row group is the large effect.** `ctl` (unmerged) is 5.4 to 18 times slower than DuckDB on the many-row-group full scans (`small1` 13 to 14 s, `mid10` 23 to 25 s) and `wrap` is 1.1 to 12.6 times faster than `ctl` on full and proj cells (`wrap / ctl` column); request counts fall from 4,704 to 294 (`small1` full), 7,824 to 489 (`mid10` full) at identical fetched bytes (0 % extra everywhere). On `sel` and `big1` they tie within noise.
- **Cross-row-group merging (`wrapx8`) is not a clear win.** It is 2.6 to 2.7 times faster only on `small1` full (294 requests to 37) and 1.1 times on `small1` proj; it is 1.8 to 2.2 times SLOWER on `big1` full (4 row groups of 45 MiB: the merged unit is 181 MiB, split at 64 MiB, and decode of the first unit starts only after it arrives: first unit 0.89 s against 0.40 s), 1.2 to 1.3 times slower on `mid1` full and `big1` proj. On `small1` proj it fetched exactly 25 % extra bytes (73.2 against 58.6 MiB), the cap.
- **The 16-attempt cap and the unit window bound throughput.** `mid10` full: 2,097 MiB in 2.85 s (ARM64) and 3.36 s (x86-64) for the wrapper = 735 and 625 MiB/s, and in 2.71 and 2.97 s for DuckDB = 774 and 705 MiB/s. The wrapper needs 489 requests of 4.3 MiB, at most 16 at a time; at about 90 ms per request (46 ms first byte plus transfer) that is 16 × 4.3 MiB / 0.09 s = 760 MiB/s, which matches (reasoned, not profiled). With the spike's window of 2 × workers row-group units the wrapper reaches only 2.1 times at 4 workers (below). A local check with an injected 90 ms per request (fake endpoint, `small1` proj, 8 workers) gave 3.65 s with a window of 8 units, 1.99 s with 16, 1.95 s with 32 and 1.97 s with 64: the window helps up to the 16-attempt cap and then nothing (local fake, not GCS; CPU-limited at the high end).
- **Warm metadata removes footer GETs but not HEADs**: `wrapc` statements 2 to 5 do 100 HEADs and 0 footer GETs (`files100`): 0.21 s (ARM64) and 0.28 s (x86-64) against 0.53 and 0.66 to 0.77 s without the cache, and against DuckDB's repeated statement 0.24 s and 0.30 s (DuckDB's own cache behavior is not documented here; its repeated statements are about a quarter faster than its first).
- **Memory.** Peak RSS of a fresh process for a full scan: wrapper 76 to 84 MiB (`small1`), 114 to 132 (`files100`), 224 to 240 (`mid10` SF10), 1,020 to 1,096 (`files10` SF10, six 1M-row groups per file, up to 16 units of about 63 MiB in flight); unmerged control 67 to 742 MiB; DuckDB 260 to 2,509 MiB (2.3 to 3.4 times the wrapper). The idle runtime of the spike process is 5 to 9 MiB.
- **The shake-down agrees with the final x86-64 run** in the 60 non-`full` cells of SF1 and SF10 (median deviation of the cell medians 1.07, 90th percentile 1.16, worst 1.40 in `big1` proj `ctl`, a cell whose own twin ratio was 1.22 in the final run and 1.62 in the shake-down).

## Proposed frozen thresholds (card part 2, provisional rows)

Nothing below is frozen; the parent or owner decides. Each proposal states the rule it comes from, the evidence and the reason. Per-cell numbers are in the tables above and in `raw/`.

**Performance** (card: "cold end-to-end median latency ≤1.5x matched DuckDB and ≤1.2x matched spike policy under the same byte-waste cap; useful throughput ≥ reciprocal of those ratios. Warm metadata must remove footer GETs for unchanged objects (identity checks counted separately); warm latency must not regress outside paired noise").
1. *Against DuckDB*: keep **1.5x** for every cell where the spike wrapper itself is within 1.5x on both CPUs (18 of the 22 cells below). For the four cells where the spike wrapper is already above it (`files100` meta and sel at 1.77 to 1.79x and 1.70 to 1.78x, `small1` full at 1.44 and 1.59x, `mid10` proj at 1.61 and 1.69x) propose the ceiling in the table: 1.1 × the worse CPU's wrapper ratio, rounded up (2.00x, 2.00x, 1.75x, 1.90x). Reason: the target must be reachable by a reasonably tuned engine on the same policy; the ratios are the cell's own measured reference, so a production regression is visible against the same bar. These four cells also name the follow-up work (parallel footer open; request window) and a later tightening needs a dated amendment, not a silent change.
2. *Against the spike policy*: define "matched spike policy" as `wrap` (merged within a row group, ADR 0011), not `ctl`; `ctl` is the fallback control and `wrap` beats it by 1.1 to 12 times in every full and proj cell, so 1.2x against `ctl` would allow a large regression. Ceiling per cell and CPU = 1.2 × the `wrap` median in the table.
3. *Throughput*: the reciprocals follow; no separate number.
4. *Warm metadata*: freeze exactly **0 footer GETs and 1 identity lookup per object on a warm hit** (measured: 0 and 100 for 100 files), and warm latency ≤ the cold latency of the same cell and ≤ 1.2x DuckDB's repeated-statement median for `meta` (0.29 s and 0.36 s for `files100`, 0.11 s and 0.12 s for `small1`).
5. *Optional prefetch / cross-row-group merging* (rule 2): keep **off**. Rule 2's own test (≥1.2x faster outside twin spread on both CPUs in affected cells, no cell >1.05x slower outside noise, caps pass) fails: `wrapx8` is 1.8 to 2.2x slower on `big1` full on both CPUs (twin ratios 0.98 and 0.99) although 2.6x faster on `small1` full.

Per-cell ceilings proposed for the Performance row (DuckDB medians, spike wrapper medians and the 1.2x ceilings are from the final runs; "ARM64 / x86-64"):

| cell | DuckDB med s (ARM64 / x86-64) | spike wrapper (`wrap`) med s | wrap / DuckDB | proposed ceiling vs DuckDB | proposed ceiling vs wrapper (1.2x) s |
|---|---|---|---|---|---|
| sf1/big1/full | 0.475 / 0.585 | 0.504 / 0.507 | 1.06 / 0.87 | 1.5x | 0.605 / 0.608 |
| sf1/big1/meta | 0.068 / 0.084 | 0.075 / 0.074 | 1.10 / 0.88 | 1.5x | 0.090 / 0.088 |
| sf1/big1/proj | 0.236 / 0.289 | 0.216 / 0.220 | 0.92 / 0.76 | 1.5x | 0.260 / 0.263 |
| sf1/big1/sel | 0.183 / 0.212 | 0.162 / 0.184 | 0.89 / 0.87 | 1.5x | 0.195 / 0.220 |
| sf1/files100/full | 0.473 / 0.615 | 0.586 / 0.744 | 1.24 / 1.21 | 1.5x | 0.703 / 0.893 |
| sf1/files100/meta | 0.309 / 0.400 | 0.547 / 0.715 | 1.77 / 1.79 | 2.00x | 0.656 / 0.858 |
| sf1/files100/proj | 0.422 / 0.533 | 0.596 / 0.747 | 1.41 / 1.40 | 1.5x | 0.715 / 0.896 |
| sf1/files100/sel | 0.307 / 0.429 | 0.547 / 0.730 | 1.78 / 1.70 | 2.00x | 0.656 / 0.876 |
| sf1/mid1/full | 0.374 / 0.441 | 0.379 / 0.434 | 1.01 / 0.98 | 1.5x | 0.455 / 0.521 |
| sf1/mid1/meta | 0.060 / 0.068 | 0.082 / 0.095 | 1.37 / 1.40 | 1.5x | 0.099 / 0.114 |
| sf1/mid1/proj | 0.265 / 0.374 | 0.300 / 0.329 | 1.13 / 0.88 | 1.5x | 0.360 / 0.395 |
| sf1/mid1/sel | 0.152 / 0.174 | 0.150 / 0.177 | 0.99 / 1.02 | 1.5x | 0.180 / 0.212 |
| sf1/small1/full | 0.728 / 0.779 | 1.046 / 1.236 | 1.44 / 1.59 | 1.75x | 1.255 / 1.483 |
| sf1/small1/meta | 0.111 / 0.116 | 0.095 / 0.113 | 0.86 / 0.97 | 1.5x | 0.114 / 0.136 |
| sf1/small1/proj | 0.655 / 0.801 | 0.972 / 1.020 | 1.48 / 1.27 | 1.5x | 1.167 / 1.224 |
| sf1/small1/sel | 0.178 / 0.218 | 0.165 / 0.169 | 0.93 / 0.77 | 1.5x | 0.198 / 0.202 |
| sf10/files10/full | 2.073 / 2.367 | 1.583 / 2.271 | 0.76 / 0.96 | 1.5x | 1.899 / 2.725 |
| sf10/files10/proj | 0.763 / 0.915 | 0.629 / 0.707 | 0.82 / 0.77 | 1.5x | 0.755 / 0.848 |
| sf10/files10/sel | 0.244 / 0.295 | 0.163 / 0.187 | 0.67 / 0.63 | 1.5x | 0.196 / 0.225 |
| sf10/mid10/full | 2.709 / 2.973 | 2.852 / 3.357 | 1.05 / 1.13 | 1.5x | 3.423 / 4.028 |
| sf10/mid10/proj | 1.101 / 1.260 | 1.864 / 2.028 | 1.69 / 1.61 | 1.90x | 2.237 / 2.433 |
| sf10/mid10/sel | 0.192 / 0.222 | 0.156 / 0.173 | 0.81 / 0.78 | 1.5x | 0.187 / 0.208 |

cells where the spike wrapper itself is over 1.5x DuckDB on at least one CPU: sf1/files100/meta, sf1/files100/sel, sf1/small1/full, sf10/mid10/proj

**Scalability** (card: "bounded memory independent of total file count; SF1/SF10, 1/4/8 CPU workers ...; no universal remote CPU-speedup target ... Same-session reference freezes speedup per cell (network/CPU limited cells separate)"). Measured on SF10 `mid10` only (other cells and concurrent admitted queries **NOT MEASURED**): speedup of 1 worker over N workers (ARM64 / x86-64): wrapper `full` 2.1 / 2.1 at 4 workers and 4.05 / 3.99 at 8; wrapper `proj` 2.1 / 2.1 and 3.9 / 3.9; DuckDB `full` 3.6 / 4.0 and 3.9 / 4.5 (network-limited at about 700 MiB/s); DuckDB `proj` 4.0 / 4.0 and 7.1 / 7.2. Propose: classify `mid10` full as **network-limited at 8 workers** and `mid10` proj as **CPU- and request-window-limited**; freeze for Nata a speedup of at least 0.9 × the spike wrapper's (full ≥3.6 at 8, ≥1.9 at 4; proj ≥3.5 at 8, ≥1.85 at 4) and record that DuckDB reaches 7.1x on proj and 4.0x at 4 workers, so these are floors, not achievable ceilings; the 4-worker figure is held down by the spike's window of 2 × workers units (reasoned from the arithmetic above and the local injected-latency check), which production should size by the attempt cap. Memory independent of file count: the wrapper's RSS for a full scan of 100 files (114 / 132 MiB) is 1.50 / 1.57 times that of one file with 293 row groups (76 / 84 MiB): bounded by the unit window and row-group size, not by the number of files; propose the ceiling "RSS ≤ 1.25 × the spike wrapper's value for the cell, plus the cache's charged bytes" (table "Peak RSS").

**Reliability** (card: "target quiescence ≤1 s after cancel in fault fixtures"): keep **1 s**. Measured under 1 ms in the spike fixtures on three machines, but the production path adds a decode drain and bridge teardown that the spike does not have, so tightening has no evidence.

**Fault tolerance** (ADR 0011 numbers: 4 attempts, backoff 100 ms to 5 s equal jitter, 30 s attempt, 120 s operation, Retry-After inside the deadline): freeze **unchanged**. Evidence: asserted exactly by the fault cases (attempt counts, backoff gaps within half to full nominal, Retry-After honored and refused beyond the deadline, deadline respected); on the real store 6 retries in about 792,000 requests and per-run p99 first byte up to 1.8 s, far below 30 s. No evidence supports other values; none was optimized.

**Resource bounds** (provisional: ≤16 attempts per query and ≤32 per node; merged response ≤64 MiB, gap ≤1 MiB, ≤25 % extra; footer wire ≤16 MiB; cache ≤64 MiB charged and ≤1,024 entries): freeze the **caps unchanged**; do not raise them to buy throughput (rule 2). Evidence: peaks 16 and 32 verified; extra bytes 0 % in every cell and 25 % exactly at the cap in one (`wrapx8`), so the 25 % allowance is not binding for the default policy on this data and is binding for the optional one; the largest footer seen is 0.53 MB wire for 293 row groups (16 MiB is 30 times that); the cache charge is 3.8 to 5.5 times the wire size (2.0 MB for the `small1` footer), so 64 MiB holds about 32 such footers or 1,024 small ones and the entry cap binds for small footers. RSS envelope to declare: idle runtime 5 to 9 MiB + live response bytes (merged responses in flight, up to the grant) × 1.15 + cache charged bytes (≤64 MiB) + allocator slack; measured fresh-process peaks above are the check (on `files10` the measured peak is 1.01 to 1.09 × the live-bytes arithmetic: 16 units × 63 MiB = 1,008 MiB against 1,020 to 1,096 MiB). A 1-GiB node grant (the spike's default) is reached by a layout of six 63 MiB-row-group units with 16 in flight; whether the owner accepts that RSS is a decision, and if not the unit window should shrink with the row-group size rather than the request cap.

**Portability / Compatibility / Security / Operability**: no number to freeze. ARM64 and x86-64 agree in shape and in every verdict above (ratios within the noise); DuckDB's auth path differs from `gs://`; no secret, endpoint or bucket name is in any result (the job's leak scan found none; `check-leaks.sh --staged` clean).

## NOT MEASURED

Real S3 and MinIO (including the S3 credential chain); `gs://` through DuckDB's HMAC secret; DuckDB request and byte counts; concurrent admitted queries; scaling cells other than SF10 `mid10`; GCS tail beyond the p99 above at more than 5 rounds; cross-region and other storage classes; a real replace-during-scan on GCS; `cargo deny` and the unsafe inventory for `object_store` 0.14; the binder→execution handoff; Nata's own timings.

## Reproduce

```bash
# one run (ARM64 shown; x86-64: --machines c3d-highcpu-8). Needs gcs.env (bucket and a unique prefix) beside run-reference.sh and a heavy.steps listing
# the steps of run-reference.sh in order (build tests preflight data upload bench-sf1 bench-meta bench-rt bench-sf10 bench-rss bench-scale cleanup leakscan)
scripts/gcp/orchestrate.sh plan --job heavy --machines n4a-highcpu-8 --max-vm-hours 2.5 --data-gb 80 --vm-service-account <write-test SA> --vm-storage-scope read-write
scripts/gcp/orchestrate.sh run  --job heavy --machines n4a-highcpu-8 --max-vm-hours 2.5 --data-gb 80 --vm-service-account <write-test SA> --vm-storage-scope read-write --approved-usd 0.32
scripts/gcp/orchestrate.sh status --run-id <id>        # nothing left
# local trial of every step against the local fake endpoint (no cloud): NATA_OSB_MODE=trial, see the header of run-reference.sh
# tables: osb summarize <csv>   (the per-cell tables above were produced from the CSVs with a small script over the same columns)
```
