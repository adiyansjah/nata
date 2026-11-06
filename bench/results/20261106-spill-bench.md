# What the grace hash join's spill path costs (plan item 15, 2026-11-06)

Runs `spillbench-arm64` (`n4a-highcpu-8`, Neoverse-N3, $0.024) and `spillbench-x86` (`c3d-highcpu-8`, AMD EPYC 9B14, 4 cores x 2 threads,
$0.017), job `spillbench`, VM time only, code `b8cedbe`. Benchmark: `spikes/operators-beyond/spill-bench.sh` (the spike's real operators,
`opsbeyond` binary). Per (query, threads) block the machine settled first (every one of the 9 SF1 blocks and 3 SF10 blocks on both machines
settled below 0.5, raw `settle.txt`), then interleaved rounds (7 at SF1, 3 at SF10) of: `mem` (in memory, no budget), `mem2` (the same again:
the noise control), `g256` / `g64` (grace join, 32 partitions, 256 MB / 64 MB budget), and DuckDB on the same Parquet files and SQL. Wall time
is process start to end for every arm alike (Parquet decode included); peak RSS from GNU time. Queries: `join-inner`, `join-left`
(lineitem with orders, counting results) and `q13` (customer left join orders, then grouping). Correctness against DuckDB is not re-checked here;
it is the `opsbeyond` job's (31 and 30 comparisons, 21 of 21 mutants). Here every arm of a query returned the same row count. Raw outputs:
`bench/results/raw/20261106-spillbench-{arm64,x86}/`.

## 1. SF1 (median seconds; ratio arm / mem with quartiles; the twin `mem2` stayed within 0.95-1.05)

| query | threads | mem s (ARM / x86) | g256 / mem (ARM) | g256 / mem (x86) | DuckDB / mem (ARM) | DuckDB / mem (x86) | peak RSS mem / g256 MB (ARM) |
|---|---:|---|---|---|---|---|---|
| join-inner | 1 | 0.527 / 0.230 | 0.44 [0.44, 0.44] | 0.90 [0.85, 0.90] | 0.12 | 0.30 | 108 / 46 |
| join-inner | 8 | 0.419 / 0.183 | 0.18 [0.18, 0.19] | 0.51 [0.47, 0.57] | 0.13 | 0.36 | 123 / 73 |
| join-left | 1 | 0.512 / 0.252 | 0.46 [0.44, 0.48] | 0.85 [0.76, 0.88] | 0.22 | 0.37 | 108 / 48 |
| join-left | 8 | 0.410 / 0.180 | 0.18 [0.17, 0.20] | 0.53 [0.50, 0.56] | 0.17 | 0.39 | 126 / 75 |
| q13 | 1 | 0.641 / 0.330 | 0.60 [0.59, 0.60] | 0.93 [0.91, 0.94] | 0.30 | 0.41 | 121 / 59 |
| q13 | 8 | 0.585 / 0.296 | 0.26 [0.25, 0.26] | 0.51 [0.49, 0.52] | 0.14 | 0.32 | 140 / 73 |

The 64 MB arm gave the same numbers as 256 MB at SF1 (24 MB spilled in 64 files, at most 32 to 60 open). The 4-thread rows are in the raw
summaries and lie between the 1- and 8-thread rows.

## 2. SF10, 8 threads

| query | mem s (ARM / x86) | mem peak RSS MB | g256 / mem (ARM / x86) | g256 peak RSS MB | spilled | DuckDB / mem (ARM / x86) |
|---|---|---:|---|---:|---|---|
| join-inner | 4.451 / 2.304 | 1,227 | 0.14 / 0.32 | 639 | 241 MB, 64 files | 0.04 / 0.10 |
| join-left | 4.499 / 2.321 | 1,236 | 0.14 / 0.31 | 645 | 241 MB, 64 files | 0.04 / 0.10 |
| q13 | 5.336 / 2.989 | 1,284 | 0.28 / 0.50 | 625 | 241 MB, 64 files | 0.06 / 0.13 |

**With a 64 MB budget every SF10 run failed, on both machines, loudly**: "join partition N needs about 31 MB, over the memory budget: more
partitions or recursive partitioning needed". Eight partitions are joined at once and each needs about 31 MB; the join has no recursive
repartitioning. No wrong result was produced. A failed query leaves no spill file behind: checked on the development machine with the same
failure forced at SF1 (1 MB budget, 2 threads): 0 files in the spill directory after the process exited. (The GCP run's "leftover spill
files" lines were a defect of the benchmark script, which read the count from the binary's last line, a line a failed run does not print;
fixed to count the files on disk.)

## Readings

1. **The cost of spilling cannot be read from this comparison, because the in-memory join is the slower algorithm.** It builds ONE hash table
   on one thread in its sink's `finalize` (`spikes/operators-beyond/src/join.rs`, `JoinBuild::finalize`), so it barely gains from threads
   (ARM64 join-inner 0.527 s at 1 thread, 0.419 s at 8). The grace join builds and probes one partition pair per task, in parallel, so it is
   faster even when it writes to disk: 0.14x to 0.60x of the in-memory time on ARM64 and 0.31x to 0.93x on x86-64, with half the memory.
   **Consequence for slice 7 (hash join):** the build must be partitioned and parallel from the start; a single global build is not a baseline
   worth keeping. The spill path's own cost needs an in-memory PARTITIONED arm (the same algorithm without the files), which this spike lacks.
2. **Memory:** spilling halves peak RSS at SF10 (about 1.23 GB to 0.64 GB with a 256 MB budget; the process also holds Parquet decode buffers).
3. **A tight budget fails instead of degrading:** recursive repartitioning (or fewer partitions in flight) is needed before a budget below
   about 8 x the largest partition can work. Slice 7's card must require it, with a test at a budget well below the partition size.
4. **DuckDB is 3 to 25 times faster** on these joins (0.04x to 0.41x of our in-memory time). Not a spill finding, but it bounds expectations.

## What this does not show

Correctness (that is the `opsbeyond` job); the spill path against an in-memory partitioned join (no such arm); recursive partitioning (not
built); budgets between 64 and 256 MB; more than 8 threads; skewed keys; another disk type (Hyperdisk Balanced only); the sort and window
spill paths (only the joins and q13 were measured); one run per machine.
