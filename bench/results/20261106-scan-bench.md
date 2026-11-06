# Slice 3 benchmark: pruning (R2) and scalability of the Parquet scan (2026-11-06)

Runs `scanbench-arm64` (`n4a-highcpu-8`, Neoverse-N3, 8 cores, $0.016) and `scanbench-x86` (`c3d-highcpu-8`, AMD EPYC 9B14, 4 cores x 2
threads, $0.012), job `scanbench`, code `a09bde7`, script `crates/nata-bench/scan-bench.sh`. The product command `nata scan --count`
(the real slice 3 scan: footer adapter, pruning, exact filter, parallel ordered scheduler; `--count` counts the result rows without rendering
CSV). Wall time of the process. Every block settled first (0 NOT SETTLED on either machine), 5 rounds, a twin of the 1- and 8-thread arms
as the noise control (0.99 to 1.03). Raw: `bench/results/raw/20261106-scanbench-{arm64,x86}/`.

## R2: pruning

TPC-H SF1 `lineitem` rewritten sorted by `l_shipdate` in 122,880-row groups (49 groups), columns `l_orderkey, l_extendedprice,
l_shipdate`, predicate `l_shipdate >= 1994-01-01 AND l_shipdate < 1995-01-01`, 8 threads:

| | ARM64 | x86-64 | target |
|---|---|---|---|
| row groups skipped | 41 of 49 (83.7%) | 41 of 49 (83.7%) | at least 60% (R2) |
| bytes read, pruned / unpruned | 16.4% | 16.4% | at most 40% |
| rows returned | 909,455, equal to DuckDB's in every run | same | equal |
| wall, pruned / unpruned | 0.016 / 0.030 s | 0.022 / 0.043 s | - |

**R2 is met on both machines**, with the same row-group count as the analytic model of step 1 (41 of 49).

## Scalability: the 7 Q1 columns of `lineitem`, no filter

Speed-up of 8 threads over 1 thread (best of window = threads and window = 2 x threads), and the effect of the window:

| data | machine | 1 thread s | 8 threads, window 8 | 8 threads, window 16 | best speed-up | target (5x) |
|---|---|---:|---|---|---:|---|
| SF10 (60 M rows) | ARM64 | 1.338 | 0.215 s (6.22x) | 0.149 s (8.98x) | 8.98x | MET |
| SF10 | x86-64 | 1.754 | 0.416 s (4.22x) | 0.303 s (5.79x) | 5.79x | MET |
| SF1 (6 M rows) | ARM64 | 0.139 | 0.045 s (3.09x) | 0.042 s (3.31x) | 3.31x | NOT MET |
| SF1 | x86-64 | 0.213 | 0.074 s (2.88x) | 0.069 s (3.09x) | 3.09x | NOT MET |

Peak RSS at 8 threads: 59 to 67 MB with window 8, 75 to 102 MB with window 16 (within the step 2b bound). SF10 throughput at 8 threads,
window 16: 403 M rows/s (ARM64), 198 M rows/s (x86-64; 4 physical cores).

Readings.
1. **The window matters more than expected.** A window of 2 x threads is 1.4x faster than window = threads at 8 threads on SF10 on both
   machines, and even helps at 1 thread (1.19x ARM64): with window = threads a worker waits while its finished row group is handed to the
   writer. **Decision (by the assistant under the owner's delegation): the default window of `nata scan` is 2 x threads** (commit with this
   record). Memory doubles with it, within the bound.
2. **SF10 meets the 5x target on both machines; SF1 does not** (3.1x to 3.3x). A whole SF1 scan takes 40 to 210 ms, and the 8-thread time
   is about 25 ms above a perfect split on ARM64 (0.042 s against 0.139 / 8 = 0.017 s): a serial part per run (process start, opening the
   file and reading its footer, which the scheduler does one file at a time, and the hand-off of the last row groups). Not separated here.
   This is a deviation from the card, recorded in its closeout: the target holds where there is enough data per thread, and the serial part
   is the follow-up (measure the scan inside one process; read footers in parallel when a scan has many files).
3. x86-64 scales less (5.8x at best on 8 threads) because 8 threads share 4 physical cores.

What this does not show: DuckDB's scan on the same columns (the R1 comparison against DuckDB is the baseline spike's,
`bench/results/20261105-parquet-scan-baseline.md`); more than 8 threads for the product scan (the 32- and 64-vCPU sweeps are of the spike's
pipeline, `bench/results/20261106-scale-sf10.md`); many files per scan; a filter during the scalability runs; object storage.
