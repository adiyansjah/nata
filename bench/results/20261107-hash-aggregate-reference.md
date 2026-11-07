# Hash aggregate reference (2026-11-07)

**Reference captured; Nata was not timed. Card 0005 is unchanged.**

- **Performance references ready:** all unrestricted SF1/SF10 queries at 1/4/8
  threads on both CPUs have five settled control and twin rounds. Across all
  requested budgets, **126 complete cells and 18 OOM/incomplete cells** are
  retained; incomplete cells supply no denominator. SF100 adds five complete
  ARM cells, including the exact HAVING fixture at 4 GiB.
- **Existing scaling rule as written:** full Q1 ≥5× at SF10/8 versus 1 is
  reachable on ARM (5.801× control / 5.894× twin), **not supported on x86**
  (3.979× / 4.012×). SF100 ≥2.5× at 32 versus 8 is reachable on ARM
  (2.654× / 2.692×). The x86 SF100 machine could not be created.
- **Spill observed:** exact HAVING succeeds and spills in every timed round at
  SF1/64 MiB and SF10/256 MiB on both CPUs, and SF100/4 GiB on ARM at eight
  threads. SF100 constrained median **84.576 s**, twin **84.828 s**,
  control min/max **82.871–87.536 s**, peak temp **6,305,611,776 bytes** across
  both arms. Unrestricted median/twin **70.181 / 70.037 s**, zero temp spill.
- **Strict RSS target remains unresolved:** DuckDB's buffer-manager limit is
  not Nata's proposed all-live-reservations budget. SF100/4 GiB reaches about
  10.14 GiB RSS. Do not freeze or inflate Nata's +32 MiB allowance from this.
- **Cleanup complete:** all five run IDs show no instances/disks. One ARM
  SF100 sizing fix used exactly one rerun; no other rerun occurred. PostgreSQL
  wire cost and the partitioned-algorithm file/no-file experiment are unmeasured.

## Measured medians (seconds)

Full output, unrestricted memory. Each number is a five-round median; full spread, twin and memory data follow.

| CPU | SF | query | 1 thread | 4 threads | 8 threads |
|---|---:|---|---:|---:|---:|
| arm64 | 1 | q1 | 0.2342 | 0.0972 | 0.0780 |
| arm64 | 1 | agg_orderkey | 1.1496 | 0.8842 | 0.8426 |
| arm64 | 1 | agg_partkey | 0.5785 | 0.2863 | 0.2358 |
| arm64 | 1 | having | 1.0429 | 0.7812 | 0.7463 |
| arm64 | 10 | q1 | 1.9272 | 0.5402 | 0.3322 |
| arm64 | 10 | agg_orderkey | 11.3242 | 8.2587 | 7.9955 |
| arm64 | 10 | agg_partkey | 6.1681 | 2.6267 | 2.0193 |
| arm64 | 10 | having | 10.3144 | 7.2832 | 7.0011 |
| x86 | 1 | q1 | 0.2560 | 0.1274 | 0.1072 |
| x86 | 1 | agg_orderkey | 1.3571 | 1.1494 | 1.0072 |
| x86 | 1 | agg_partkey | 0.5092 | 0.3396 | 0.3146 |
| x86 | 1 | having | 1.2216 | 0.9892 | 0.9028 |
| x86 | 10 | q1 | 1.8791 | 0.5479 | 0.4723 |
| x86 | 10 | agg_orderkey | 12.4108 | 9.5156 | 9.4517 |
| x86 | 10 | agg_partkey | 5.3313 | 3.3079 | 4.0445 |
| x86 | 10 | having | 11.3880 | 8.4033 | 8.3146 |

## Scaling reference

| CPU | SF | threads | control speedup | twin speedup |
|---|---:|---|---:|---:|
| arm64 | 10 | 8 / 1 | 5.801x | 5.894x |
| x86 | 10 | 8 / 1 | 3.979x | 4.012x |
| arm64 | 100 | 32 / 8 | 2.654x | 2.692x |


## Proposed frozen thresholds (parent acceptance required)

The card's rules remain unchanged. These are proposals made before any Nata timing.
Current seconds establish a reachable reference, not a future Nata denominator.

| Provisional card row | PROPOSED threshold | Reference and reasoning / limitation |
|---|---|---|
| Full-Q1 performance, SF1/SF10, 1/4/8, both CPUs | Retain **Nata ≤1.5× paired DuckDB** | The full aggregate body, all result columns and CLI startup/CSV/file sink are measured in every unrestricted cell. Use the same-session DuckDB control with its twin and spread during future Nata work; do not use these historic seconds as fixed denominators. No Nata speed claim is made. |
| `agg_orderkey`, `agg_partkey` performance | Retain **≤2× paired DuckDB** at the stated sizes/threads | Complete unrestricted full-output references exist; the large output cost is included symmetrically. This is a reachable SQL/CLI reference, not a kernel-only ceiling. |
| Aggregate spill performance | Retain **≤2× matched-budget, matched-thread DuckDB only where both arms finish 5/5**; **unresolved for OOM/incomplete cells** | The full-cell and unsupported-cell tables define exactly which denominators exist. A successful checksum probe alone is insufficient. Do not substitute a larger DuckDB budget or lower its threads and call it matched. No universal spill/no-spill ratio is proposed. |
| Q1 SF10 8 versus 1 scaling | **ARM ≥5×; x86 ≥3.5×**, rather than freezing ≥5× on both | ARM reference 5.80× control / 5.89× twin supports five. C3D reference 3.98× / 4.01× does not; 3.5 leaves material headroom beyond the observed noise. C3D has four physical cores/eight SMT threads; ARM has eight physical cores. Topology is measured; attributing all of the difference to SMT would be an unisolated explanation. |
| Q1 SF100 32 versus 8 scaling | **ARM ≥2.5×; x86 unresolved** | ARM full-Q1 reference is 2.65× control / 2.69× twin at 32 versus 8, supporting 2.5 with modest headroom. No C3D 32-vCPU shape exists and the requested 30-vCPU attempt failed to create a VM. Do not freeze a both-CPU threshold from ARM or historical reduced-Q1 numbers. |
| High-cardinality completion/spill at SF1 64 MiB, SF10 256 MiB, SF100 4 GiB | **Do not call the strict query-wide completion budgets reference-validated yet**; retain them as proposed implementation goals | The exact HAVING fixture completes and spills under the matching DuckDB buffer-manager limits at all SF1/SF10 thread counts and at SF100/eight threads. Other low-budget matrix cells fail. Those successes do not cover all result/allocator memory or prove Nata's all-live-reservations contract. The parent needs bounded-budget spike/skeleton evidence before claiming the strict query-wide budgets are reference-validated. |
| RSS baseline + B + 32 MiB slack, stacks separate | **Unresolved; do not increase or freeze the allowance from DuckDB RSS** | No-data RSS and real full-query RSS are recorded. DuckDB allocates outside its configured budget and does not measure Nata reservations or stack bounds. In particular SF10/256 MiB can consume about 1.1 GiB RSS. That is evidence of non-equivalent contracts, not justification for a large Nata slack allowance. |

FD ≤32, disk quota D, correctness, cleanup and security contracts are not newly
provisional numeric performance targets and were not tested by this DuckDB job.
No proposed rule weakens those contracts or treats an uninstrumented path as zero.

## Cloud runs, cost and cleanup

All running VMs were Spot in us-central1-a; failed C3D-30 creation also tried
us-central1-b/c/f. 8-vCPU jobs used 20 GB boot + 50 GB data; scale jobs planned
20 GB boot + 300 GB data. Every plan preceded its run and was below the brief's
$0.35 / $1.20 per-run caps. Raw plans, reports and explicit status outputs are
linked below. Costs here are **VM-compute estimates**, not invoices or totals
including disk/IP; the printed worst cases include those allowances.

| run ID | machine | counted duration | VM USD | plan worst USD | outcome / cleanup status |
|---|---|---:|---:|---:|---|
| aggref-arm64 | n4a-highcpu-8 | 46m 33s (2793s) | 0.076 | .304 | finished; [empty status](raw/20261107-aggref-arm64/cleanup-status.txt) |
| aggref-x86 | c3d-highcpu-8 | 48m 51s (2931s) | 0.056 | .205 | finished; [empty status](raw/20261107-aggref-x86/cleanup-status.txt) |
| aggref-sf100-arm64 | n4a-highcpu-32 | ~18 min (manual) | ~0.12 | .909 | stopped for matrix fix; runner undercounted 0; [empty status](raw/20261107-aggref-sf100-arm64/cleanup-status.txt) |
| aggref-sf100-x86 | c3d-highcpu-30 | 0 VM seconds | 0.000 | .627 | no VM / Spot capacity failure; [empty status](raw/20261107-aggref-sf100-x86/cleanup-status.txt) |
| aggref-sf100-arm64-r2 | n4a-highcpu-32 | 53m 43s (3223s) | 0.352 | .909 | finished; [empty status](raw/20261107-aggref-sf100-arm64-r2/cleanup-status.txt) |

The aborted ARM run's runner report says $0.000/zero seconds. This is an
undercount: logs put VM creation at 06:00:34 UTC, delete initiation at 06:18:29,
and delete completion by 06:19:01 (~1,075–1,107 s, ~$0.118–$0.121 compute).
The failed x86 run has zero **VM** compute but its temporary data disks still
incur a small unitemized charge. No resource was left running to wait for
billing. Total estimated VM compute across all attempts is about **$0.60**;
disk/IP charges are additional. No preemptions occurred.

## Inputs and constrained HAVING resources

Input byte counts are compressed lineitem file sizes; full SHA256 is in each
raw `results/aggref/inputs.txt`. Output bytes and every successful cell's RSS/temp
are in the full table below; individual failed processes remain in `runs.csv`.

| CPU | SF | input bytes | input SHA256 prefix |
|---|---:|---:|---|
| arm64 | 1 | 205471331 | 4c75548921563f9c |
| arm64 | 10 | 2206265380 | df61ce72298ad791 |
| x86 | 1 | 205482956 | c87ad8d103896e33 |
| x86 | 10 | 2206307042 | d64d7da3a7908c8a |
| arm64 | 100 | 22622079884 | 2b089d1586c96a04 |

| CPU | SF | limit | threads | control median s | twin median s | max RSS MiB | max temp MiB |
|---|---:|---|---:|---:|---:|---:|---:|
| arm64 | 1 | 64MiB | 8 | 0.9796 | 0.9850 | 224.2 | 79.6 |
| arm64 | 10 | 256MiB | 8 | 8.9427 | 8.9727 | 1236.3 | 619.8 |
| x86 | 1 | 64MiB | 8 | 1.1828 | 1.1882 | 255.3 | 73.9 |
| x86 | 10 | 256MiB | 8 | 10.5006 | 10.5090 | 1347.9 | 580.8 |
| arm64 | 100 | 4GiB | 8 | 84.5760 | 84.8279 | 10387.3 | 6013.5 |

No-data RSS samples/topology are retained per run in `baseline-topology.txt`.
SF100 HAVING was tested at **eight threads only**; it is not evidence for its
16/32-thread completion or a full SF100 hash-query sweep. The Q1 scaling sweep
alone covers 8/16/32. Every constrained HAVING round has nonzero peak temp bytes.

## Method and limits

Dedicated `aggref` and `aggref-scale` jobs call `crates/nata-bench/aggref.py`.
A registered job gives a reproducible matrix and fails on a harness error, unlike
`heavy`'s intentionally non-gating individual command exit statuses. SF1/SF10 code is
committed at `4e58dd0`; code-tree identity is
`ce0de372c3cd4588de84e6660a6d26ae4756021aca5ebfaf9bcbff0350a1c3ba`.
The working tree shipped by the orchestrator has that same code identity; result
records are excluded by `scripts/code-tree.sh`.

DuckDB CLI 2.0.0, full Q1 aggregate body, the two historical named aggregate
queries, and the exact card HAVING body read `scripts/gen-tpch.sh` Parquet.
Each budget/thread configuration first runs a separate result probe:
`SELECT count(*), sum(hash(t)) FROM (<full body>) t`. Its count/checksum must
match the unrestricted one-thread reference before timing. This checks all
columns but is a probabilistic checksum, not an exact sorted multiset oracle.
DuckDB DOUBLE AVG is not an oracle for Nata's exact decimal AVG.

Five fresh-process rounds alternate identical control and twin arms, reversing
which comes first on odd rounds. Timings include startup, settings, view binding,
planning, Parquet read/decode, profiling, full CLI CSV formatting, and writing
output to a regular file on the data disk. The sink is deleted after each run;
only its byte count is retained. Unordered group output can differ in order.
No checksum wrapper appears in timed SQL. The OS file cache is warmed by probes;
it is not flushed. Each SF/thread sweep settles below load 0.5 before its timed
block; individual queries within a sweep are not independently settled.
JSON profiling is enabled symmetrically in both arms. GNU time records process
peak RSS. The profiler's `system.peak_temp_dir_size` is peak live temporary file
bytes, not cumulative bytes written; `peak_buffer_memory` is separate from RSS.
The input byte count is the whole compressed lineitem Parquet file size, not a
claim about physical disk reads or projected column bytes. Input SHA256 is retained.

The unrestricted reference is explicitly 4 GiB at SF1/SF10 and 32 GiB at SF100;
these are not DuckDB defaults. Matched small limits are 64/256 MiB at SF1/SF10,
4 GiB at SF100, without reducing thread count or changing query settings to
rescue a failure. Failed probes are retained as unsupported budget cells and
have no timed denominator. Full-output failures are retained too.

DuckDB's limit applies to its buffer manager; vectors, query results and other
allocations can fall outside it. RSS can exceed the configured limit. This is
not equivalent to Nata's proposed query-wide live reservation limit. See the
[DuckDB limits documentation](https://www.duckdb.org/docs/current/operations_manual/limits)
and [memory-management description](https://duckdb.org/2024/07/09/memory-management).
A DuckDB RSS observation cannot justify multiplying Nata's budget by workers.

## Local trial and checks

TMPDIR was `$HOME/tmp-aggref` on disk; no /tmp scratch. AI_AGENT was unset for
DuckDB (also CODEX_CI and CLAUDECODE, to prevent CLI agent-mode output).
The first SF0.01/one-thread/one-round trial failed in reduction: the parser
expected the older flat profiler schema. No cloud launched. After correcting to
DuckDB 2.0's nested `system` metrics, the same trial passed all 24 timed cells
(four queries, three budgets, two arms), with matching checksum probes and zero
unsupported cells. It is a smoke check, not a target reference.

Reproduce locally, one command/check at a time:

```sh
mkdir -p "$HOME/tmp-aggref"
TMPDIR="$HOME/tmp-aggref" python3 crates/nata-bench/aggref.py --scales 0.01 --threads 1 --rounds 1 --trial --out "$HOME/tmp-aggref/trial"
TMPDIR="$HOME/tmp-aggref" bash scripts/gcp/test/test-job-chains.sh
TMPDIR="$HOME/tmp-aggref" python3 -m py_compile crates/nata-bench/aggref.py
bash -n scripts/gcp/job.sh scripts/gcp/orchestrate.sh scripts/gcp/vm-startup.sh scripts/gcp/test/test-job-chains.sh
```

All passed, including registered-job isolation and the existing chain tests.
No workspace Rust code changed; no Rust build/test or Nata benchmark is claimed.

The supplemental no-data probe runs five fresh `duckdb -csv -c 'SELECT 1;'`
processes under `/usr/bin/time -f 'baseline_max_rss_kib=%M'`, with AI_AGENT,
CODEX_CI and CLAUDECODE unset and disk TMPDIR. `lscpu` records threads per core,
cores per socket and sockets. These small probes were launched during the job,
not under an independent settle gate; their RSS is diagnostic and they may
briefly overlap the first sweep. Twin/min/max spreads retain any disturbance.
No no-data latency or engine time is claimed.

PostgreSQL/default-psql wire cost is **not measured**. `psql` is absent from these
job images (recorded in baseline-topology.txt). No synthetic wire substitute is
presented as a real PostgreSQL fixture. Installing and validating that fixture
would require another job revision/trial; it was not added to the running jobs.
The overlapping spike/identical partitioned-algorithm file/no-file bake-off in
card part 4 is also unmeasured: this brief covers the DuckDB reference, not a new
Nata or spike implementation.

A checksum probe can succeed when the full-output query fails: its small final
collector is not the same allocation pattern as full CSV output. Every timed
process retains its exit status. `medians.csv` can contain diagnostic medians
with `n < 5`; those are **incomplete cells, not references**. The tables and
proposed denominators below require five successful control AND five successful
twin rounds. Failed cells are not rescued by `preserve_insertion_order=false`,
thread reduction, a larger budget, or filtering out failed rounds.


The first SF100 ARM attempt was stopped after generation/probes and before
retained timing rows. Its original matrix unnecessarily included the two named
SF100 hash queries, threatening the cap before the last Q1 thread block. This
was a sizing mistake. Commit `16af98e` narrows `--scaling`: Q1 at 8/16/32 first
(32 GiB), then HAVING at eight threads under 32 GiB and 4 GiB. It passed its own
SF0.01/one-round trial (six timed rows, no failures), job-chain check, Python
compile and shell syntax check before the one permitted rerun. Rerun code-tree:
`e137330817e1b50fbae1dea0f4134c1a64637a13041dd339a0f01576d59274a9`.
No other run was rerun. The 8-vCPU runs kept their original committed source.

SF100 x86 creation failed in all four configured zones; the first detailed
failure was observed as `ZONE_RESOURCE_POOL_EXHAUSTED_WITH_DETAILS`, not a
confirmed quota error. The orchestrator's per-zone attempt log is retained.
Caution: `status` truncates its run's gcloud.log, so the original detailed error
was lost before copying; this error-code statement is a contemporaneous
observation, not a claim that the preserved raw log still contains it.
There was no x86 SF100 VM, and no substitution for 32 versus 8 scaling.

These are reachability references for freezing rules, not future cross-session
Nata denominators. Future Nata process timings must pair with DuckDB again on
the **same physical Parquet files** in that session, same threads, sink and
cache conditions. Independent generation can change compressed bytes/row-group
layout between CPUs; the input manifests deliberately retain each file's actual
bytes and SHA256 rather than assuming bit-identical generated Parquet.
Failed executions have unknown temp peaks when DuckDB emits no final profiler;
a blank counter in a failed row is not zero spill.

Plans and VM-compute estimates use `prices.env`'s cached November 3/4 catalog
rates (N4A 8/32: $0.0984/$0.3936 per hour, C3D 8/30: $0.0687/$0.2568).
They are estimates under that rate model, not billing invoices. A later check of
Google's [Spot pricing page](https://cloud.google.com/spot-vms/pricing?hl=en)
timed out; live per-shape rates were not reconfirmed. Every printed worst case
includes disks/IP and, for Hyperdisk, the runner's extra allowance. No cap was
exceeded by a printed plan, and no unlabelled resource, bucket or service account
was touched. All VMs use no service account and no scopes.

The largest absolute control/twin median difference across complete SF1/SF10
cells is 3.55% ARM64 and 3.12% x86-64. Every timed sweep settled. These are
observed five-round spreads, not confidence intervals. No exact allocation
profile isolates the cause of RSS excess or the x86 part-key eight-thread
slowdown; neither is assigned a speculative cause. Thread stacks were not
measured separately, another reason the provisional Nata RSS rule remains open.

The independent result-record check also passes on redacted raw copies:
`python3 bench/results/raw/20261107-aggref-local/verify.py`. It requires all
unrestricted cells/arms to have exactly rounds 0–4, successful and settled,
rejects a planted missing round, and checks full-cell min/quartiles/median/max
against raw sorted order statistics. Redaction additionally asserts that every
CSV cell is byte-for-byte identical before and after redaction.

CSV output is closed but **not fsynced** before the timer stops. Kernel dirty
pages/writeback can carry across rounds, and deleting the sink can discard
unwritten pages. Thus output bytes are logical CSV file bytes, not durable
physical write traffic or a disk-throughput benchmark. The same sink semantics
must be used for a later paired Nata comparison. Spill peaks likewise describe
live temporary file size, not a direct measurement of physical I/O traffic.

Final record checks: Python compile, shell syntax, both local trial modes,
job chains, raw order-statistic/completeness checks (including the missing-round
canary and nonzero spill in every constrained HAVING round), staged whitespace
check and staged leak scan passed. A separate staged scan also checked all new
files for IP addresses, email addresses, keys and project identifiers. Raw line
endings/trailing whitespace were normalized; CSV cells were asserted unchanged.
Branch: `t3/duckdb-aggregate-reference`. Source commits: `4e58dd0`, `16af98e`.
No push or merge was performed.

## Full cell record

Min, Q1, median, Q3, max are control wall seconds. Twin is its independent five-round median. RSS and temp are maxima across both arms, not sums. `True` means every sample in the cell started within a sweep whose settle gate passed.

| CPU | SF | threads | query | limit | min / Q1 / median / Q3 / max s | twin median s | peak RSS MiB | peak temp MiB | output bytes | settled |
|---|---:|---:|---|---|---|---:|---:|---:|---:|---|
| arm64 | 1 | 1 | agg_orderkey | 256MiB | 1.1810 / 1.1840 / 1.1849 / 1.1888 / 1.1919 | 1.1800 | 202.7 | 0.0 | 46417318 | True |
| arm64 | 1 | 1 | agg_orderkey | 4GiB | 1.1468 / 1.1469 / 1.1496 / 1.1564 / 1.1592 | 1.1525 | 243.5 | 0.0 | 46417318 | True |
| arm64 | 1 | 1 | agg_orderkey | 64MiB | 1.2773 / 1.2837 / 1.2855 / 1.2978 / 1.2982 | 1.2817 | 173.5 | 38.0 | 46417318 | True |
| arm64 | 1 | 1 | agg_partkey | 256MiB | 0.5687 / 0.5693 / 0.5697 / 0.5722 / 0.5953 | 0.5899 | 114.1 | 0.0 | 5813274 | True |
| arm64 | 1 | 1 | agg_partkey | 4GiB | 0.5693 / 0.5744 / 0.5785 / 0.5853 / 0.5909 | 0.5892 | 123.1 | 0.0 | 5813274 | True |
| arm64 | 1 | 1 | agg_partkey | 64MiB | 0.6043 / 0.6105 / 0.6112 / 0.6188 / 0.6229 | 0.6049 | 110.1 | 0.0 | 5813274 | True |
| arm64 | 1 | 1 | having | 256MiB | 1.0735 / 1.0820 / 1.0850 / 1.0938 / 1.0958 | 1.0835 | 204.5 | 0.0 | 39926992 | True |
| arm64 | 1 | 1 | having | 4GiB | 1.0393 / 1.0394 / 1.0429 / 1.0469 / 1.0492 | 1.0480 | 249.5 | 0.0 | 39926992 | True |
| arm64 | 1 | 1 | having | 64MiB | 1.1817 / 1.1844 / 1.1875 / 1.1925 / 1.6048 | 1.1867 | 176.2 | 38.8 | 39926992 | True |
| arm64 | 1 | 1 | q1 | 256MiB | 0.2310 / 0.2312 / 0.2338 / 0.2344 / 0.2353 | 0.2323 | 96.6 | 0.0 | 636 | True |
| arm64 | 1 | 1 | q1 | 4GiB | 0.2309 / 0.2322 / 0.2342 / 0.2344 / 0.2352 | 0.2319 | 95.3 | 0.0 | 636 | True |
| arm64 | 1 | 1 | q1 | 64MiB | 0.2429 / 0.2450 / 0.2451 / 0.2455 / 0.2458 | 0.2434 | 92.0 | 0.0 | 636 | True |
| arm64 | 1 | 4 | agg_orderkey | 256MiB | 0.8765 / 0.8782 / 0.8793 / 0.8803 / 0.8889 | 0.8826 | 238.2 | 0.0 | 46417318 | True |
| arm64 | 1 | 4 | agg_orderkey | 4GiB | 0.8822 / 0.8835 / 0.8842 / 0.8845 / 0.8856 | 0.8846 | 301.5 | 0.0 | 46417318 | True |
| arm64 | 1 | 4 | agg_orderkey | 64MiB | 1.0595 / 1.0604 / 1.0647 / 1.0670 / 1.0673 | 1.0640 | 203.1 | 47.6 | 46417318 | True |
| arm64 | 1 | 4 | agg_partkey | 256MiB | 0.3905 / 0.3915 / 0.3936 / 0.3952 / 0.3962 | 0.3973 | 354.9 | 112.7 | 5813274 | True |
| arm64 | 1 | 4 | agg_partkey | 4GiB | 0.2844 / 0.2854 / 0.2863 / 0.2884 / 0.2891 | 0.2886 | 380.5 | 0.0 | 5813274 | True |
| arm64 | 1 | 4 | agg_partkey | 64MiB | 0.8724 / 0.8743 / 0.8880 / 0.8942 / 0.9134 | 0.8735 | 153.2 | 268.5 | 5813274 | True |
| arm64 | 1 | 4 | having | 256MiB | 0.7766 / 0.7772 / 0.7785 / 0.7822 / 0.8051 | 0.7799 | 275.5 | 0.0 | 39926992 | True |
| arm64 | 1 | 4 | having | 4GiB | 0.7794 / 0.7804 / 0.7812 / 0.7813 / 0.7824 | 0.7844 | 332.3 | 0.0 | 39926992 | True |
| arm64 | 1 | 4 | having | 64MiB | 0.9654 / 0.9678 / 0.9682 / 0.9693 / 0.9726 | 0.9665 | 218.1 | 42.9 | 39926992 | True |
| arm64 | 1 | 4 | q1 | 256MiB | 0.0965 / 0.0966 / 0.0974 / 0.0978 / 0.0984 | 0.0974 | 124.1 | 0.0 | 636 | True |
| arm64 | 1 | 4 | q1 | 4GiB | 0.0962 / 0.0967 / 0.0972 / 0.0978 / 0.0979 | 0.0971 | 144.8 | 0.0 | 636 | True |
| arm64 | 1 | 4 | q1 | 64MiB | 0.0979 / 0.0984 / 0.0988 / 0.0991 / 0.0993 | 0.0994 | 110.3 | 0.0 | 636 | True |
| arm64 | 1 | 8 | agg_orderkey | 256MiB | 0.8410 / 0.8435 / 0.8478 / 0.8494 / 0.8511 | 0.8505 | 275.3 | 0.0 | 46417318 | True |
| arm64 | 1 | 8 | agg_orderkey | 4GiB | 0.8414 / 0.8421 / 0.8426 / 0.8432 / 0.8460 | 0.8420 | 371.2 | 0.0 | 46417318 | True |
| arm64 | 1 | 8 | agg_orderkey | 64MiB | 1.0685 / 1.0752 / 1.0774 / 1.0779 / 1.0783 | 1.0822 | 227.9 | 83.3 | 46417318 | True |
| arm64 | 1 | 8 | agg_partkey | 256MiB | 0.3705 / 0.3811 / 0.3813 / 0.3865 / 0.3870 | 0.3867 | 366.5 | 119.0 | 5813274 | True |
| arm64 | 1 | 8 | agg_partkey | 4GiB | 0.2317 / 0.2344 / 0.2358 / 0.2360 / 0.2411 | 0.2339 | 575.4 | 0.0 | 5813274 | True |
| arm64 | 1 | 8 | having | 256MiB | 0.7402 / 0.7407 / 0.7422 / 0.7431 / 0.7433 | 0.7470 | 300.5 | 0.0 | 39926992 | True |
| arm64 | 1 | 8 | having | 4GiB | 0.7453 / 0.7460 / 0.7463 / 0.7488 / 0.7577 | 0.7477 | 403.9 | 0.0 | 39926992 | True |
| arm64 | 1 | 8 | having | 64MiB | 0.9761 / 0.9793 / 0.9796 / 0.9858 / 0.9896 | 0.9850 | 224.2 | 79.6 | 39926992 | True |
| arm64 | 1 | 8 | q1 | 256MiB | 0.0740 / 0.0750 / 0.0773 / 0.0781 / 0.0797 | 0.0761 | 158.8 | 0.0 | 636 | True |
| arm64 | 1 | 8 | q1 | 4GiB | 0.0762 / 0.0779 / 0.0780 / 0.0796 / 0.0819 | 0.0767 | 206.4 | 0.0 | 636 | True |
| arm64 | 1 | 8 | q1 | 64MiB | 0.0766 / 0.0783 / 0.0789 / 0.0795 / 0.0800 | 0.0763 | 140.7 | 0.0 | 636 | True |
| arm64 | 10 | 1 | agg_orderkey | 256MiB | 12.7375 / 12.7846 / 12.7881 / 12.8464 / 12.8820 | 12.7826 | 1040.4 | 650.2 | 479114908 | True |
| arm64 | 10 | 1 | agg_orderkey | 4GiB | 11.2856 / 11.2944 / 11.3242 / 11.3812 / 11.4495 | 11.3338 | 1429.4 | 0.0 | 479114908 | True |
| arm64 | 10 | 1 | agg_partkey | 256MiB | 14.3758 / 14.5616 / 14.5981 / 14.7403 / 14.7450 | 14.5418 | 360.4 | 2915.1 | 60126169 | True |
| arm64 | 10 | 1 | agg_partkey | 4GiB | 6.1388 / 6.1663 / 6.1681 / 6.2022 / 6.3040 | 6.1894 | 288.2 | 0.0 | 60126169 | True |
| arm64 | 10 | 1 | agg_partkey | 64MiB | 16.1865 / 16.3326 / 16.3402 / 16.3999 / 16.4541 | 16.5331 | 231.9 | 2954.3 | 60126169 | True |
| arm64 | 10 | 1 | having | 256MiB | 11.6813 / 11.6890 / 11.7248 / 11.7551 / 12.3244 | 11.7004 | 1037.9 | 645.5 | 411956752 | True |
| arm64 | 10 | 1 | having | 4GiB | 10.2583 / 10.2640 / 10.3144 / 10.3323 / 10.4485 | 10.3287 | 1407.6 | 0.0 | 411956752 | True |
| arm64 | 10 | 1 | q1 | 256MiB | 1.9172 / 1.9190 / 1.9224 / 1.9273 / 1.9285 | 1.9245 | 97.5 | 0.0 | 657 | True |
| arm64 | 10 | 1 | q1 | 4GiB | 1.9230 / 1.9244 / 1.9272 / 1.9296 / 1.9374 | 1.9272 | 98.3 | 0.0 | 657 | True |
| arm64 | 10 | 1 | q1 | 64MiB | 2.0595 / 2.0624 / 2.0658 / 2.0679 / 2.0716 | 2.0614 | 97.8 | 0.0 | 657 | True |
| arm64 | 10 | 4 | agg_orderkey | 256MiB | 8.5662 / 8.5784 / 8.5922 / 8.6137 / 8.8502 | 8.6123 | 1151.9 | 631.0 | 479114908 | True |
| arm64 | 10 | 4 | agg_orderkey | 4GiB | 8.2477 / 8.2492 / 8.2587 / 8.2648 / 8.2691 | 8.2475 | 1313.9 | 0.0 | 479114908 | True |
| arm64 | 10 | 4 | agg_partkey | 256MiB | 9.7782 / 9.8431 / 9.8783 / 10.0997 / 10.1954 | 10.0581 | 382.7 | 2951.2 | 60126169 | True |
| arm64 | 10 | 4 | agg_partkey | 4GiB | 2.5818 / 2.6024 / 2.6267 / 2.6563 / 2.6606 | 2.5925 | 3759.1 | 0.0 | 60126169 | True |
| arm64 | 10 | 4 | having | 256MiB | 7.6022 / 7.6042 / 7.6264 / 7.6378 / 7.6705 | 7.6093 | 1206.1 | 628.8 | 411956752 | True |
| arm64 | 10 | 4 | having | 4GiB | 7.2481 / 7.2762 / 7.2832 / 7.3094 / 7.3497 | 7.2790 | 1425.9 | 0.0 | 411956752 | True |
| arm64 | 10 | 4 | q1 | 256MiB | 0.5429 / 0.5511 / 0.5526 / 0.5541 / 0.5662 | 0.5590 | 145.9 | 0.0 | 657 | True |
| arm64 | 10 | 4 | q1 | 4GiB | 0.5368 / 0.5394 / 0.5402 / 0.5418 / 0.5421 | 0.5397 | 164.6 | 0.0 | 657 | True |
| arm64 | 10 | 4 | q1 | 64MiB | 0.5946 / 0.5966 / 0.5979 / 0.6003 / 0.6006 | 0.5967 | 116.7 | 0.0 | 657 | True |
| arm64 | 10 | 8 | agg_orderkey | 256MiB | 9.8847 / 9.9026 / 9.9294 / 9.9389 / 9.9497 | 9.8928 | 1197.4 | 614.1 | 479114908 | True |
| arm64 | 10 | 8 | agg_orderkey | 4GiB | 7.9933 / 7.9950 / 7.9955 / 8.0032 / 8.0301 | 8.0099 | 1507.8 | 0.0 | 479114908 | True |
| arm64 | 10 | 8 | agg_partkey | 256MiB | 7.7902 / 7.8992 / 7.9625 / 8.0894 / 8.8343 | 7.8761 | 387.6 | 2739.9 | 60126169 | True |
| arm64 | 10 | 8 | agg_partkey | 4GiB | 1.9718 / 2.0122 / 2.0193 / 2.0437 / 2.1105 | 2.0448 | 4346.7 | 376.3 | 60126169 | True |
| arm64 | 10 | 8 | having | 256MiB | 8.9214 / 8.9335 / 8.9427 / 9.0041 / 9.1708 | 8.9727 | 1236.3 | 619.8 | 411956752 | True |
| arm64 | 10 | 8 | having | 4GiB | 6.9323 / 6.9931 / 7.0011 / 7.0038 / 7.0146 | 6.9793 | 1663.3 | 0.0 | 411956752 | True |
| arm64 | 10 | 8 | q1 | 256MiB | 0.3228 / 0.3253 / 0.3261 / 0.3262 / 0.3267 | 0.3276 | 185.9 | 0.0 | 657 | True |
| arm64 | 10 | 8 | q1 | 4GiB | 0.3270 / 0.3279 / 0.3322 / 0.3346 / 0.3397 | 0.3270 | 239.5 | 0.0 | 657 | True |
| arm64 | 10 | 8 | q1 | 64MiB | 0.3696 / 0.3697 / 0.3751 / 0.3766 / 0.3833 | 0.3752 | 146.2 | 0.0 | 657 | True |
| x86 | 1 | 1 | agg_orderkey | 256MiB | 1.4057 / 1.4079 / 1.4137 / 1.4272 / 1.4531 | 1.4112 | 215.7 | 0.0 | 46417318 | True |
| x86 | 1 | 1 | agg_orderkey | 4GiB | 1.3436 / 1.3550 / 1.3571 / 1.3693 / 1.3819 | 1.3793 | 263.8 | 0.0 | 46417318 | True |
| x86 | 1 | 1 | agg_orderkey | 64MiB | 1.5602 / 1.5666 / 1.5800 / 1.5929 / 1.6057 | 1.5725 | 196.1 | 33.4 | 46417318 | True |
| x86 | 1 | 1 | agg_partkey | 256MiB | 0.4941 / 0.5151 / 0.5214 / 0.5245 / 0.5534 | 0.5202 | 135.5 | 0.0 | 5813274 | True |
| x86 | 1 | 1 | agg_partkey | 4GiB | 0.4996 / 0.5011 / 0.5092 / 0.5176 / 0.5213 | 0.5220 | 139.5 | 0.0 | 5813274 | True |
| x86 | 1 | 1 | agg_partkey | 64MiB | 0.5496 / 0.5590 / 0.5732 / 0.5761 / 0.5766 | 0.5594 | 117.4 | 0.0 | 5813274 | True |
| x86 | 1 | 1 | having | 256MiB | 1.2610 / 1.2755 / 1.2840 / 1.2972 / 1.3019 | 1.2765 | 216.8 | 0.0 | 39926992 | True |
| x86 | 1 | 1 | having | 4GiB | 1.1998 / 1.2173 / 1.2216 / 1.2306 / 1.2313 | 1.2216 | 263.7 | 0.0 | 39926992 | True |
| x86 | 1 | 1 | having | 64MiB | 1.3967 / 1.4439 / 1.4452 / 1.4532 / 1.4541 | 1.4366 | 196.2 | 34.6 | 39926992 | True |
| x86 | 1 | 1 | q1 | 256MiB | 0.2440 / 0.2456 / 0.2457 / 0.2480 / 0.2510 | 0.2459 | 98.4 | 0.0 | 636 | True |
| x86 | 1 | 1 | q1 | 4GiB | 0.2510 / 0.2538 / 0.2560 / 0.2641 / 0.2692 | 0.2523 | 99.9 | 0.0 | 636 | True |
| x86 | 1 | 1 | q1 | 64MiB | 0.2526 / 0.2559 / 0.2575 / 0.2578 / 0.2737 | 0.2570 | 98.5 | 0.0 | 636 | True |
| x86 | 1 | 4 | agg_orderkey | 256MiB | 1.0971 / 1.1146 / 1.1177 / 1.1196 / 1.1242 | 1.1023 | 262.3 | 0.0 | 46417318 | True |
| x86 | 1 | 4 | agg_orderkey | 4GiB | 1.1145 / 1.1222 / 1.1494 / 1.1565 / 1.1605 | 1.1248 | 319.0 | 0.0 | 46417318 | True |
| x86 | 1 | 4 | agg_orderkey | 64MiB | 1.3702 / 1.3788 / 1.3898 / 1.3919 / 1.4159 | 1.4080 | 223.9 | 41.0 | 46417318 | True |
| x86 | 1 | 4 | agg_partkey | 256MiB | 0.5007 / 0.5117 / 0.5134 / 0.5217 / 0.5472 | 0.5051 | 364.2 | 107.6 | 5813274 | True |
| x86 | 1 | 4 | agg_partkey | 4GiB | 0.3333 / 0.3336 / 0.3396 / 0.3415 / 0.3416 | 0.3390 | 426.6 | 0.0 | 5813274 | True |
| x86 | 1 | 4 | agg_partkey | 64MiB | 0.9364 / 0.9376 / 0.9442 / 0.9473 / 0.9792 | 0.9663 | 160.0 | 242.7 | 5813274 | True |
| x86 | 1 | 4 | having | 256MiB | 0.9404 / 0.9963 / 1.0011 / 1.0093 / 1.0113 | 0.9886 | 298.5 | 0.0 | 39926992 | True |
| x86 | 1 | 4 | having | 4GiB | 0.9832 / 0.9881 / 0.9892 / 1.0057 / 1.0280 | 1.0013 | 362.3 | 0.0 | 39926992 | True |
| x86 | 1 | 4 | having | 64MiB | 1.2664 / 1.2664 / 1.2667 / 1.2732 / 1.2764 | 1.2674 | 236.4 | 39.4 | 39926992 | True |
| x86 | 1 | 4 | q1 | 256MiB | 0.1159 / 0.1196 / 0.1229 / 0.1235 / 0.1365 | 0.1250 | 129.8 | 0.0 | 636 | True |
| x86 | 1 | 4 | q1 | 4GiB | 0.1218 / 0.1226 / 0.1274 / 0.1301 / 0.1350 | 0.1278 | 159.3 | 0.0 | 636 | True |
| x86 | 1 | 4 | q1 | 64MiB | 0.1226 / 0.1255 / 0.1282 / 0.1296 / 0.1370 | 0.1258 | 109.2 | 0.0 | 636 | True |
| x86 | 1 | 8 | agg_orderkey | 256MiB | 1.0053 / 1.0226 / 1.0313 / 1.0336 / 1.0423 | 1.0189 | 291.7 | 0.0 | 46417318 | True |
| x86 | 1 | 8 | agg_orderkey | 4GiB | 1.0046 / 1.0050 / 1.0072 / 1.0075 / 1.0173 | 1.0219 | 391.2 | 0.0 | 46417318 | True |
| x86 | 1 | 8 | agg_orderkey | 64MiB | 1.2974 / 1.2975 / 1.3061 / 1.3089 / 1.3169 | 1.3109 | 237.2 | 76.5 | 46417318 | True |
| x86 | 1 | 8 | agg_partkey | 256MiB | 0.4731 / 0.4737 / 0.4770 / 0.4823 / 0.4847 | 0.4778 | 377.0 | 115.5 | 5813274 | True |
| x86 | 1 | 8 | agg_partkey | 4GiB | 0.3067 / 0.3093 / 0.3146 / 0.3165 / 0.3176 | 0.3161 | 598.2 | 0.0 | 5813274 | True |
| x86 | 1 | 8 | having | 256MiB | 0.9040 / 0.9063 / 0.9067 / 0.9080 / 0.9091 | 0.9161 | 332.1 | 0.0 | 39926992 | True |
| x86 | 1 | 8 | having | 4GiB | 0.8847 / 0.9014 / 0.9028 / 0.9051 / 0.9070 | 0.8947 | 433.6 | 0.0 | 39926992 | True |
| x86 | 1 | 8 | having | 64MiB | 1.1722 / 1.1806 / 1.1828 / 1.1877 / 1.1952 | 1.1882 | 255.3 | 73.9 | 39926992 | True |
| x86 | 1 | 8 | q1 | 256MiB | 0.1004 / 0.1012 / 0.1037 / 0.1050 / 0.1074 | 0.1041 | 161.6 | 0.0 | 636 | True |
| x86 | 1 | 8 | q1 | 4GiB | 0.1037 / 0.1057 / 0.1072 / 0.1106 / 0.1134 | 0.1092 | 210.2 | 0.0 | 636 | True |
| x86 | 1 | 8 | q1 | 64MiB | 0.1060 / 0.1063 / 0.1097 / 0.1099 / 0.1101 | 0.1099 | 142.8 | 0.0 | 636 | True |
| x86 | 10 | 1 | agg_orderkey | 256MiB | 13.9274 / 13.9301 / 13.9679 / 14.2721 / 14.7106 | 13.8517 | 1156.3 | 632.6 | 479114908 | True |
| x86 | 10 | 1 | agg_orderkey | 4GiB | 12.3473 / 12.3871 / 12.4108 / 12.4173 / 12.4256 | 12.4132 | 1538.1 | 0.0 | 479114908 | True |
| x86 | 10 | 1 | agg_partkey | 256MiB | 13.3234 / 13.4878 / 13.6055 / 13.6877 / 13.7731 | 13.4702 | 373.1 | 2852.4 | 60126169 | True |
| x86 | 10 | 1 | agg_partkey | 4GiB | 5.2743 / 5.3093 / 5.3313 / 5.3418 / 5.3982 | 5.3220 | 299.3 | 0.0 | 60126169 | True |
| x86 | 10 | 1 | agg_partkey | 64MiB | 14.6638 / 14.7433 / 14.8894 / 14.9227 / 14.9419 | 14.6851 | 257.1 | 2879.2 | 60126169 | True |
| x86 | 10 | 1 | having | 256MiB | 12.7252 / 12.7293 / 12.7326 / 12.7576 / 13.1782 | 12.7890 | 1154.1 | 627.9 | 411956752 | True |
| x86 | 10 | 1 | having | 4GiB | 11.3684 / 11.3747 / 11.3880 / 11.4184 / 11.7137 | 11.4064 | 1503.9 | 0.0 | 411956752 | True |
| x86 | 10 | 1 | q1 | 256MiB | 1.8478 / 1.8515 / 1.8526 / 1.8589 / 1.8615 | 1.8507 | 104.8 | 0.0 | 657 | True |
| x86 | 10 | 1 | q1 | 4GiB | 1.8447 / 1.8504 / 1.8791 / 1.8859 / 1.9034 | 1.8823 | 104.5 | 0.0 | 657 | True |
| x86 | 10 | 1 | q1 | 64MiB | 1.9590 / 1.9594 / 1.9612 / 1.9766 / 1.9874 | 1.9753 | 103.6 | 0.0 | 657 | True |
| x86 | 10 | 4 | agg_orderkey | 256MiB | 10.0127 / 10.0389 / 10.0928 / 10.1155 / 10.1251 | 10.1033 | 1265.0 | 591.8 | 479114908 | True |
| x86 | 10 | 4 | agg_orderkey | 4GiB | 9.4869 / 9.5154 / 9.5156 / 9.5518 / 9.6702 | 9.5142 | 1329.4 | 0.0 | 479114908 | True |
| x86 | 10 | 4 | agg_partkey | 256MiB | 7.9619 / 7.9972 / 8.2085 / 8.2899 / 8.3157 | 8.1218 | 388.8 | 2574.4 | 60126169 | True |
| x86 | 10 | 4 | agg_partkey | 4GiB | 3.1725 / 3.1905 / 3.3079 / 3.3223 / 3.3284 | 3.2046 | 3823.0 | 0.0 | 60126169 | True |
| x86 | 10 | 4 | having | 256MiB | 8.7975 / 8.8400 / 8.8449 / 8.9155 / 8.9522 | 8.8799 | 1324.4 | 590.8 | 411956752 | True |
| x86 | 10 | 4 | having | 4GiB | 8.3590 / 8.3847 / 8.4033 / 8.4081 / 8.4165 | 8.3924 | 1449.2 | 0.0 | 411956752 | True |
| x86 | 10 | 4 | q1 | 256MiB | 0.5420 / 0.5422 / 0.5449 / 0.5471 / 0.5515 | 0.5563 | 147.2 | 0.0 | 657 | True |
| x86 | 10 | 4 | q1 | 4GiB | 0.5438 / 0.5462 / 0.5479 / 0.5584 / 0.5766 | 0.5521 | 171.2 | 0.0 | 657 | True |
| x86 | 10 | 4 | q1 | 64MiB | 0.6081 / 0.6191 / 0.6191 / 0.6284 / 0.6725 | 0.6165 | 117.8 | 0.0 | 657 | True |
| x86 | 10 | 8 | agg_orderkey | 256MiB | 11.5210 / 11.5309 / 11.5399 / 11.5569 / 11.5790 | 11.5804 | 1314.9 | 575.1 | 479114908 | True |
| x86 | 10 | 8 | agg_orderkey | 4GiB | 9.4188 / 9.4420 / 9.4517 / 9.4533 / 9.5143 | 9.4392 | 1476.0 | 0.0 | 479114908 | True |
| x86 | 10 | 8 | agg_partkey | 256MiB | 8.2813 / 8.3155 / 8.3269 / 8.4995 / 8.9021 | 8.2527 | 389.4 | 2520.5 | 60126169 | True |
| x86 | 10 | 8 | agg_partkey | 4GiB | 3.6679 / 3.8517 / 4.0445 / 4.1793 / 4.1910 | 4.0553 | 4412.6 | 351.5 | 60126169 | True |
| x86 | 10 | 8 | having | 256MiB | 10.4494 / 10.4645 / 10.5006 / 10.5169 / 10.6132 | 10.5090 | 1347.9 | 580.8 | 411956752 | True |
| x86 | 10 | 8 | having | 4GiB | 8.2545 / 8.3026 / 8.3146 / 8.3274 / 8.4688 | 8.2957 | 1640.8 | 0.0 | 411956752 | True |
| x86 | 10 | 8 | q1 | 256MiB | 0.4568 / 0.4609 / 0.4613 / 0.4630 / 0.4651 | 0.4616 | 183.4 | 0.0 | 657 | True |
| x86 | 10 | 8 | q1 | 4GiB | 0.4678 / 0.4684 / 0.4723 / 0.4762 / 0.4768 | 0.4691 | 243.0 | 0.0 | 657 | True |
| x86 | 10 | 8 | q1 | 64MiB | 0.5509 / 0.5560 / 0.5570 / 0.5608 / 0.5630 | 0.5584 | 148.1 | 0.0 | 657 | True |
| arm64 | 100 | 16 | q1 | 32GiB | 1.4838 / 1.4853 / 1.4874 / 1.4901 / 1.4910 | 1.4924 | 638.1 | 0.0 | 679 | True |
| arm64 | 100 | 32 | q1 | 32GiB | 0.9809 / 0.9887 / 0.9913 / 0.9933 / 1.0145 | 0.9772 | 939.4 | 0.0 | 679 | True |
| arm64 | 100 | 8 | having | 32GiB | 70.1164 / 70.1543 / 70.1814 / 70.1822 / 70.2223 | 70.0370 | 11999.3 | 0.0 | 4247630601 | True |
| arm64 | 100 | 8 | having | 4GiB | 82.8713 / 83.9376 / 84.5760 / 85.1259 / 87.5357 | 84.8279 | 10387.3 | 6013.5 | 4247630601 | True |
| arm64 | 100 | 8 | q1 | 32GiB | 2.6178 / 2.6311 / 2.6312 / 2.6340 / 2.6442 | 2.6303 | 474.6 | 0.0 | 679 | True |

## Unsupported or incomplete budget cells

| CPU | SF | threads | query | limit | successful control / twin rounds | reason |
|---|---:|---:|---|---|---|---|
| arm64 | 1 | 8 | agg_partkey | 64MiB | 0 / 2 | full-output OOM/failure (see errors.txt) |
| arm64 | 10 | 1 | having | 64MiB | 0 / 0 | probe OOM |
| arm64 | 10 | 1 | agg_orderkey | 64MiB | 0 / 0 | probe OOM |
| arm64 | 10 | 4 | having | 64MiB | 0 / 0 | probe OOM |
| arm64 | 10 | 4 | agg_orderkey | 64MiB | 0 / 0 | probe OOM |
| arm64 | 10 | 4 | agg_partkey | 64MiB | 0 / 0 | probe OOM |
| arm64 | 10 | 8 | having | 64MiB | 0 / 0 | probe OOM |
| arm64 | 10 | 8 | agg_orderkey | 64MiB | 0 / 0 | probe OOM |
| arm64 | 10 | 8 | agg_partkey | 64MiB | 0 / 0 | probe OOM |
| x86 | 1 | 8 | agg_partkey | 64MiB | 1 / 3 | full-output OOM/failure (see errors.txt) |
| x86 | 10 | 1 | having | 64MiB | 0 / 0 | probe OOM |
| x86 | 10 | 1 | agg_orderkey | 64MiB | 0 / 0 | probe OOM |
| x86 | 10 | 4 | having | 64MiB | 0 / 0 | probe OOM |
| x86 | 10 | 4 | agg_orderkey | 64MiB | 0 / 0 | probe OOM |
| x86 | 10 | 4 | agg_partkey | 64MiB | 0 / 0 | probe OOM |
| x86 | 10 | 8 | having | 64MiB | 0 / 0 | probe OOM |
| x86 | 10 | 8 | agg_orderkey | 64MiB | 0 / 0 | probe OOM |
| x86 | 10 | 8 | agg_partkey | 64MiB | 0 / 0 | probe OOM |
