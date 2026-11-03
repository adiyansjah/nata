# Pull versus push, corrected (ADR 0001, open check 2, x86-64 part)

Code: `spikes/pipeline-model/` (`spike-pipeline-model <lineitem.parquet> pullpush 25`; throwaway, delete once ADR 0001 is accepted). Run 2026-11-04 on the 8-vCPU shared x86-64 VM used throughout (12th Gen Intel Core i3-1220P, KVM). **ARM64 not run.**

## Why this replaces the first comparison

The first comparison (`20261103-pipeline-model-spike.md`) had four faults, found in review and checked in the code: pull ran with static partitions while push used a shared morsel counter; the pull "8 threads" row used 7 threads; all pull timings ran before all push timings; and the operators were the slow, unoptimized ones. This version fixes each.

## What was run

- **Same query and data as before:** TPC-H Q1 shape over SF1 `lineitem.parquet` (6,001,215 rows, 49 row groups, Snappy), dictionary-preserving strings, fused project, fast hasher: the optimized operators, separate and chained (filter, then project, then aggregate), not the single fused loop.
- **Pull with the same morsel queue** (`run_pull_morsel`): each worker takes the next row group from a shared counter, as push does, and pulls batches through nested `Box<dyn PullOp>` operators. **Push** (`run_push_with`): the same counter, then a list of `Box<dyn Transform>`. Only the dispatch style differs. Exactly the requested number of worker threads in both.
- **Interleaved with a control:** each round runs three arms in a rotating order: pull, push and a second pull ("pull2"). The ratio pull2/pull measures how much two identical designs differ by noise alone; push/pull is read against it.
- 25 timed rounds after one discarded warm-up per cell. Every run's result is checked against a reference (counts exact, sums within 1e-9 relative) before it is counted.
- Batch sizes 8192, 1024 and 256 rows, to magnify any per-batch dispatch cost; 1, 2, 4 and 8 threads.

## Result

| batch rows | threads | pull ms min / median | push ms min / median | push / pull, median of paired ratios [Q1, Q3] | control: pull2 / pull [Q1, Q3] |
|---:|---:|---|---|---|---|
| 8192 | 1 | 141 / 147 | 140 / 145 | 0.989 [0.970, 1.031] | 1.006 [0.994, 1.041] |
| 8192 | 2 | 82 / 84 | 81 / 83 | 0.982 [0.944, 0.992] | 0.994 [0.930, 1.008] |
| 8192 | 4 | 57 / 62 | 55 / 61 | 0.979 [0.944, 1.025] | 0.989 [0.941, 1.040] |
| 8192 | 8 | 77 / 83 | 76 / 83 | 0.972 [0.953, 1.038] | 0.997 [0.968, 1.042] |
| 1024 | 1 | 175 / 181 | 174 / 184 | 1.010 [0.990, 1.038] | 1.001 [0.988, 1.021] |
| 1024 | 2 | 95 / 102 | 95 / 101 | 0.990 [0.949, 1.022] | 1.000 [0.951, 1.034] |
| 1024 | 4 | 67 / 73 | 67 / 75 | 0.995 [0.980, 1.066] | 1.008 [0.990, 1.031] |
| 1024 | 8 | 63 / 66 | 61 / 65 | 0.987 [0.974, 1.016] | 0.995 [0.962, 1.009] |
| 256 | 1 | 263 / 274 | 258 / 268 | 0.996 [0.980, 1.007] | 1.001 [0.985, 1.015] |
| 256 | 2 | 142 / 148 | 141 / 147 | 0.993 [0.956, 1.002] | 0.997 [0.935, 1.018] |
| 256 | 4 | 100 / 108 | 103 / 106 | 0.971 [0.932, 1.029] | 1.004 [0.945, 1.052] |
| 256 | 8 | 73 / 88 | 73 / 88 | 1.020 [0.981, 1.042] | 0.998 [0.982, 1.043] |

Ratios are medians of the per-round paired ratios, with the first and third quartile in brackets. Times are milliseconds for the whole query.

## Reading

- **Push is indistinguishable from pull at this granularity.** The median paired ratio push/pull lies between 0.971 and 1.020 in all 12 cells. The control (pull against itself) lies between 0.989 and 1.008 and its quartiles spread about ±3% to ±5%, so differences of a few percent cannot be told apart from noise here.
- **At 256-row batches** (about 23,000 batches; the whole query takes about 260 ms on one thread, so about 11 microseconds per batch) the ratio is 0.996 [0.980, 1.007]: any dispatch difference is under about 2% of 11 microseconds, so under roughly 0.2 microseconds per batch.
- **The earlier "push is 10-20% slower at 1-2 threads" does not reproduce.** The likely cause is run order (the first comparison was not interleaved) combined with its different scheduling; this experiment cannot confirm that.
- Absolute times drift: the 8-thread, 8192-row cell was 38 ms in a 2-round smoke run and 77 ms in the full run on the same machine. That is the reason for paired, interleaved ratios, and a reason not to quote absolute times from this VM.

## Limits

- **x86-64 only**, one shared VM (noise of about ±3-5% per paired ratio), one query shape.
- Both arms use dynamic dispatch through boxed trait objects on a linear chain. Static dispatch, branching pipelines and join build/probe are not compared here (the join and early-termination cases are in `20261103-operator-trait-prototype.md`).
- The kernels are Arrow-based and allocate per batch, which costs microseconds and hides nanosecond-scale dispatch differences; with much cheaper kernels, or a fused single-loop pipeline, the dispatch share would be larger. This does not rule out a difference there.
