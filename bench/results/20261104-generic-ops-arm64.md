# Generic operators against hand-fused loops, ARM64 (ADR 0003, open check 5)

Same code as `bench/results/20261104-generic-ops-x86.md` (`spikes/generic-ops/`, run by `scripts/gcp/job.sh` job `genops`). Read that file for the method, the queries and the baselines; this one has the ARM64 numbers and what differs.

## Machine, place and cost

| | |
|---|---|
| Machine | GCP `n4a-highcpu-8` Spot, us-central1-a, Neoverse-N3, 8 vCPU, 16 GB, Debian 12 arm64, kernel 6.1 |
| When | 2026-11-04 (VM clock 2026-11-03 22:53 UTC), run id `genops-arm64`, one VM, 0 preemptions, 12 steps |
| VM time | 196 s counted; estimated VM cost about 0.005 USD from catalog prices (not checked against a bill); approved cap was 0.20 USD |
| Cleanup | verified: nothing labelled for this run remained |

The machine was quiet: paired-ratio quartiles are within about ±1%, much tighter than on the shared x86-64 VM.

## Correctness

- 10 of 10 unit tests pass.
- Q1 and Q6 results are byte-identical to the x86-64 output, which matched DuckDB, at 1 and 4 threads and against the hand-fused loop. (The DuckDB comparison itself was not re-run on ARM64; the equality is with the x86-64 output.)
- All six NULL cases (10%, 50%, 100% NULLs, both queries) equal the independent scalar reference.
- The mutation check was not re-run on ARM64 (the kernels are the same source; it tests the tests, not the machine).

## End to end (Parquet decode included), 25 interleaved rounds; min / median ms

| query | threads | generic | hand-fused | generic / fused | control |
|---|---:|---|---|---|---|
| q1 | 1 | 119 / 121 | 93 / 94 | 1.29 [1.28, 1.29] | 1.00 |
| q1 | 2 | 60 / 61 | 48 / 48 | 1.27 [1.27, 1.27] | 1.00 |
| q1 | 4 | 32 / 32 | 25 / 25 | 1.25 [1.25, 1.26] | 1.00 |
| q1 | 8 | 17 / 17 | 14 / 14 | 1.20 [1.20, 1.21] | 1.00 |
| q6 | 1 | 62 / 63 | 73 / 74 | 0.85 [0.84, 0.85] | 1.00 |
| q6 | 2 | 31 / 32 | 37 / 37 | 0.85 [0.84, 0.85] | 1.00 |
| q6 | 4 | 16 / 17 | 19 / 20 | 0.85 [0.85, 0.85] | 1.00 |
| q6 | 8 | 9 / 9 | 10 / 11 | 0.85 [0.85, 0.86] | 1.00 |

## Engine only (decoded batches in memory)

| query | threads | generic | hand-fused | generic / fused | control |
|---|---:|---|---|---|---|
| q1 | 1 | 61 / 61 | 31 / 31 | 1.96 [1.95, 1.97] | 1.00 |
| q1 | 2 | 31 / 31 | 16 / 16 | 1.89 [1.88, 1.89] | 1.00 |
| q1 | 4 | 15 / 16 | 9 / 9 | 1.78 [1.77, 1.79] | 1.00 |
| q1 | 8 | 8 / 8 | 5 / 5 | 1.53 [1.49, 1.57] | 1.00 |
| q6 | 1 | 14 / 15 | 20 / 20 | 0.76 [0.75, 0.76] | 0.99 |
| q6 | 2 | 7 / 8 | 10 / 10 | 0.75 [0.74, 0.76] | 1.00 |
| q6 | 4 | 4 / 4 | 5 / 5 | 0.77 [0.75, 0.77] | 1.01 |
| q6 | 8 | 2 / 2 | 3 / 3 | 0.77 [0.75, 0.78] | 1.00 |

## NULL cost (engine only, 10% NULLs in four columns)

| query | threads | no NULLs ms | 10% NULLs ms | ratio |
|---|---:|---|---|---|
| q1 | 1 | 61 / 61 | 133 / 133 | 2.18 [2.17, 2.18] |
| q1 | 8 | 8 / 8 | 17 / 17 | 2.13 [2.12, 2.14] |
| q6 | 1 | 14 / 15 | 40 / 40 | 2.69 [2.67, 2.70] |
| q6 | 8 | 2 / 2 | 5 / 5 | 2.45 [2.42, 2.49] |

## DuckDB on the same VM (`duckdb-time.sh`, DuckDB 2.0.0 arm64, in-process timer, first run dropped; min / median ms)

Run in the same job, after the generic benchmarks (not interleaved with them).

| | 1 thread | 2 | 4 | 8 |
|---|---:|---|---|---|
| DuckDB Q1 | 154 / 155 | 80 / 81 | 43 / 44 | 25 / 26 |
| generic Q1, end to end | 119 / 121 | 60 / 61 | 32 / 32 | 17 / 17 |
| DuckDB Q6 | 71 / 74 | 38 / 40 | 22 / 22 | 14 / 15 |
| generic Q6, end to end | 62 / 63 | 31 / 32 | 16 / 17 | 9 / 9 |

Min against min the generic engine takes 0.68-0.77 of DuckDB's time on Q1 and 0.64-0.87 on Q6 on this machine. The caveat about DuckDB including parsing and planning applies here too.

## Against x86-64

| | x86-64 | ARM64 |
|---|---|---|
| Q1 generic / fused, end to end | 1.06-1.12 | 1.20-1.29 |
| Q1 generic / fused, engine only | 1.20-1.51 | 1.53-1.96 |
| Q6 generic / fused, end to end | 0.89-0.94 | 0.85 |
| Q6 generic / fused, engine only | 0.67-0.82 | 0.75-0.77 |
| NULL penalty at 10% NULLs (engine only) | 1.8-2.5 | 2.1-2.7 |

## Reading

- The conclusion from x86-64 holds in direction and gets weaker for the unselective query: Q1 costs 1.2-1.3x the hand-fused loop end to end on ARM64 (1.06-1.12x on x86-64), and the selective query is still faster than the fused loop (0.85x).
- **The cause of the larger Q1 gap on ARM64 is not known.** The hand-fused Q1 loop is 1.9-2.3x faster engine-only on ARM64 than the generic pipeline, while the generic pipeline on ARM64 is about as fast as on x86-64 (61 against 53 ms single thread). So the fused loop benefits more on this machine, and I did not profile it. Possibilities, all untested: LLVM vectorises or schedules the fused loop better for Neoverse-N3; the generic pipeline's gather through the selection vector and `i128` accumulation cost relatively more there; the memory system. Do not take any of these as the answer.
- The N3 core (N4A) is a different microarchitecture from the V2 core (C4A) used for earlier ARM64 runs, so a V2 run could differ.
- The NULL penalty is a little larger on ARM64 (2.1-2.7x), so the NULL path remains the biggest known cost on both architectures.
- Against DuckDB on this machine the generic engine takes less time in every cell (0.64-0.87 of it). That comparison is not interleaved and DuckDB's includes planning; it supports "competitive with DuckDB on these two queries", not a ranking.

## Limits

One ARM64 machine type (N3), one run, same query shapes, data set and synthetic NULLs as the x86-64 file; no profile; the V2 core not covered. The fused baseline has no NULL handling and no selection vector.
