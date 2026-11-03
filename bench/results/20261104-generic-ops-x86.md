# Generic operators against hand-fused loops (ADR 0003, open check 5), x86-64

Code: `spikes/generic-ops/` (throwaway; delete once ADR 0003 is accepted). Run 2026-11-04 on the shared 8-vCPU x86-64 VM used throughout (12th Gen Intel Core i3-1220P). **ARM64 not yet run.**

## What was built

A small pipeline that follows ADR 0003:

- **Batch** = Arrow columns (`Decimal64(15,2)` for the decimals, `Date32`, dictionary strings with `Int32` keys) plus a **selection vector** (row indexes) and computed columns.
- **Operators**, separate stages: `FilterDate` / `FilterDec` (narrow the selection, no copying), `Project` (one primitive kernel per step: `1 - discount`, then `price * that`), and a sink (grouped or global `SUM`, `COUNT`).
- **Kernels** are checked and selection-aware: arithmetic overflow is an error only for a row that is selected and valid; a NULL input gives NULL; a NULL in a filter column drops the row; the overflow check is branch-free (a flag masked by validity).
- Morsel-driven workers, one row group per morsel, as in the earlier spikes.

Two queries over TPC-H SF1 `lineitem` (6,001,215 rows, 49 row groups):

- **Q1 (reduced):** `SELECT l_returnflag, l_linestatus, sum(l_quantity), sum(l_extendedprice), sum(l_extendedprice*(1-l_discount)), count(*) ... WHERE l_shipdate <= DATE '1998-09-02' GROUP BY 1,2`. 98.6% of rows survive the filter (unselective).
- **Q6:** `SELECT sum(l_extendedprice*l_discount) ... WHERE l_shipdate >= '1994-01-01' AND l_shipdate < '1995-01-01' AND l_discount BETWEEN 0.05 AND 0.07 AND l_quantity < 24`. About 2% of rows survive (selective).

The baseline for each is a **hand-fused loop** over the same batches (one loop per batch, no selection vector, no NULL handling, the best a query compiler could emit), at the same thread counts.

## Correctness

- Both queries, through the generic engine and the hand-fused loop, are **identical to DuckDB** digit for digit (exact decimals), checked by diffing the CSV; the result is the same at 1 and 4 threads.
- The NULL paths have no DuckDB oracle (the masked data is not in the file). They were checked against an **independent scalar reference** (plain row loops with `Option`, exact `i128`, sharing no code with the engine) with 10%, 50% and 100% of four columns made NULL deterministically: equal in all six cases, including the all-NULL cases (empty Q1, NULL Q6).
- 10 unit tests of the kernels, including: an overflow in an unselected row is not an error; an overflow in a NULL slot is not an error; a guard filter protects a later multiplication; filters drop NULLs and respect the incoming selection (masked and dense paths); subtraction follows the same overflow rules as multiplication; a randomized comparison of `mul` with a scalar reference over 3,000 cases with NULLs, selections and overflow.
- **Mutation check, run on the final code** (a script applies one deliberate breakage at a time to `kernels.rs`, runs the unit tests, `check` and `nullcheck`, and restores the file): 15 breakages (filter ignores NULLs; sum ignores NULLs; multiplication or subtraction overflow not masked by validity; output validity always true; validity of the second operand ignored; grouped sums count NULL values or miscount them; wrong group counts, dense and masked; filter ignores the incoming selection, both paths; overflow never reported, both paths; a filter bound off by one). First run: 14 caught, **1 survived** (subtraction overflow not masked by validity, which no test covered) and one more was caught only by the end-to-end check, not by a unit test (the dense filter ignoring the selection). Two tests were added for those, and both breakages are now caught by the unit tests. The earlier mutation run (7 breakages) was before the single-pass aggregation kernels existed and is superseded by this one.

## Result: end to end (Parquet decode included)

25 interleaved rounds after a warm-up, arms in rotating order; min / median ms; ratios are medians of paired per-round ratios with quartiles; the control is generic against generic.

| query | threads | generic ms | hand-fused ms | generic / fused | control |
|---|---:|---|---|---|---|
| q1 | 1 | 114 / 129 | 103 / 118 | 1.10 [1.01, 1.14] | 0.99 [0.96, 1.05] |
| q1 | 2 | 67 / 71 | 61 / 64 | 1.09 [1.06, 1.11] | 1.00 [0.98, 1.04] |
| q1 | 4 | 44 / 49 | 42 / 44 | 1.12 [1.08, 1.14] | 1.00 [0.94, 1.03] |
| q1 | 8 | 33 / 35 | 31 / 34 | 1.06 [1.03, 1.09] | 1.01 [0.99, 1.04] |
| q6 | 1 | 77 / 81 | 87 / 91 | 0.89 [0.88, 0.90] | 1.00 [0.99, 1.02] |
| q6 | 2 | 43 / 44 | 47 / 48 | 0.91 [0.89, 0.92] | 1.00 [0.99, 1.01] |
| q6 | 4 | 29 / 32 | 31 / 36 | 0.94 [0.87, 0.97] | 1.01 [0.93, 1.05] |
| q6 | 8 | 21 / 21 | 22 / 24 | 0.92 [0.89, 0.95] | 1.01 [0.97, 1.05] |

## Result: engine only (batches already decoded and held in memory)

Parquet decode is a fixed cost shared by both arms (about 60-70 ms of the single-thread time here) and hides the difference, so this is the compute part alone.

| query | threads | generic ms | hand-fused ms | generic / fused | control |
|---|---:|---|---|---|---|
| q1 | 1 | 53 / 58 | 35 / 39 | 1.51 [1.48, 1.55] | 1.00 [0.98, 1.02] |
| q1 | 2 | 29 / 31 | 20 / 24 | 1.46 [1.30, 1.51] | 1.01 [0.98, 1.06] |
| q1 | 4 | 20 / 21 | 14 / 15 | 1.38 [1.33, 1.42] | 1.01 [0.97, 1.04] |
| q1 | 8 | 13 / 15 | 11 / 12 | 1.20 [1.17, 1.22] | 1.00 [0.97, 1.03] |
| q6 | 1 | 12 / 13 | 17 / 20 | 0.67 [0.63, 0.71] | 1.02 [0.99, 1.07] |
| q6 | 2 | 6 / 8 | 10 / 12 | 0.68 [0.63, 0.82] | 1.07 [0.91, 1.17] |
| q6 | 4 | 5 / 6 | 6 / 7 | 0.79 [0.77, 0.88] | 1.00 [0.95, 1.03] |
| q6 | 8 | 4 / 4 | 5 / 5 | 0.82 [0.80, 0.83] | 1.00 [0.98, 1.01] |

## Against DuckDB (`spikes/generic-ops/duckdb-time.sh`; in-process timer, 16 runs, the first run in time order dropped; min / median ms)

| | 1 thread | 2 | 4 | 8 |
|---|---:|---:|---:|---:|
| DuckDB Q1 | 121 / 128 | 71 / 77 | 54 / 55 | 42 / 44 |
| generic Q1, end to end | 114 / 129 | 67 / 71 | 44 / 49 | 33 / 35 |
| DuckDB Q6 | 67 / 69 | 38 / 40 | 31 / 32 | 26 / 28 |
| generic Q6, end to end | 77 / 81 | 43 / 44 | 29 / 32 | 21 / 21 |

Min against min, the generic engine takes 0.79-0.94 of DuckDB's time on Q1 and 0.81-1.15 on Q6 (slower at 1-2 threads, faster at 4-8). DuckDB's number includes parsing, planning and result handling, which the engine's does not. The two sets of numbers were taken at different times on a shared VM (drift of 10-30% is normal), not interleaved, so treat this as "comparable", not as a ranking.

**Correction (2026-11-04).** The first DuckDB timing (Q1 135 / 80 / 57 / 46, Q6 70 / 40 / 33 / 27 ms) was wrong in a small way: its script dropped the fastest run (`sort | tail -n +2`) instead of the first, which made DuckDB look 4-10% slower. The table above replaces it. The ratio range moved from "0.72-0.84 on Q1, 0.78-1.10 on Q6" to the figures above.

## How the cost of generality was found and reduced

The first working version was slower:

| step | engine-only generic Q1, 1 thread | generic / fused |
|---|---:|---:|
| first version (five separate passes in the aggregation sink) | 72 ms | 2.10 |
| profile (`perf`): 71% of the time in the sink, 14% in `Project`, 7% in the filter | | |
| one pass computing group id, count and all three sums together | 49 ms | 1.47 |
| the finished code above (run-to-run drift) | 53 ms | 1.51 |

The selection vector and the primitive kernels were not the expensive part; the structure of the aggregation was. Q6 was never slower than the hand-fused loop.

## The cost of NULL handling (engine only, 10% NULLs in four columns, the same query on masked and unmasked batches held in memory)

| query | threads | no NULLs ms | 10% NULLs ms | ratio |
|---|---:|---|---|---|
| q1 | 1 | 49 / 52 | 115 / 122 | 2.33 [2.29, 2.42] |
| q1 | 8 | 12 / 13 | 25 / 27 | 2.11 [2.09, 2.16] |
| q6 | 1 | 12 / 13 | 29 / 31 | 2.46 [2.40, 2.59] |
| q6 | 8 | 4 / 4 | 7 / 7 | 1.84 [1.83, 1.88] |

This is the largest cost of generality found. Three ways of reading the validity bitmap were tried (all bits, all expanded to bytes once per batch, and a hybrid that expands only when the selection is dense), and none got below about 1.8x:

| representation | q1 1 thread, absolute ms with NULLs | q6 1 thread, absolute ms with NULLs |
|---|---:|---:|
| bit test per use (with the earlier, multi-pass sink) | 143 | 27 |
| bytes, expanded for every column of every batch | 110 | 41 |
| hybrid: bytes if at least a quarter of the rows are selected, else bits | 115 | 29 |

Expanding every column wastes work when a selective query reads later columns for 2% of the rows (Q6 got worse); testing bits wastes work when most rows are read. An earlier attempt to time NULLs on the Parquet path was wrong, because masking inside the timed region cost as much as the query; the figures above preload the masked batches.

## Reading

- **Generality is cheap on end-to-end time and not free on compute.** Through Parquet, Q1 costs 1.06-1.12x the hand-fused loop and Q6 is 6-11% faster. With decode removed, Q1 costs 1.2-1.5x. Compared with the earlier micro-benchmark, where arrow's two checked kernels were 2.4-2.9x a fused loop, owning the kernels and the selection vector recovers most of the gap.
- **The hand-fused loop is not a ceiling.** On the selective query the selection vector lets the generic pipeline skip later columns for rejected rows, and it beats a loop that evaluates every predicate on every row (a more careful fused loop with early exits might do better; none was written).
- **Aggregation structure mattered more than kernel design:** one pass per aggregate set instead of one pass per aggregate cut generic Q1 by a third.
- **NULL handling roughly doubles the engine cost** at 10% NULLs, whatever the representation tried; this needs its own design (see ADR 0003, consequences).
- The group-by here keeps the dictionary columns and groups on dictionary codes (interned to dense ids, at most 32 distinct values per run). That is the "dictionary-aware group-by" the ADR assigned to a later tier; it is what keeps Q1 cheap, and it was simple, but a general group-by (many distinct values, plain strings) was not built.

## Limits

- x86-64 only, one shared VM (the paired-ratio quartiles are about ±1-5%); one data set, two query shapes, no joins.
- Synthetic NULLs (10% uniformly at random in four columns).
- The fused baselines have no NULL handling and no selection vector, by design.
- Buffers (selection vectors, computed columns) are allocated per batch with no pooling; no `LIMIT`, no spill, no operator protocol scheduler (the earlier prototype covers that); the engine runs its own simple morsel loop.
- The group-by supports at most 32 distinct key values per run and two dictionary key columns.
