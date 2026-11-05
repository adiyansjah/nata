# Parquet scan baseline spike (slice 3): reuse parquet-rs, morsel = a whole row group (2026-11-05)

Card: `docs/slices/0001-parquet-scan.md`, section 4. Rules R1 and R3 were fixed in the card (accepted by the owner on 2026-11-05) before anything was measured. R2 (pruning) is not part of this spike.
Code: `spikes/parquet-scan` (commit `18ff19a`; the orchestrator's tarball has no `.git`, so the machine files say `git: unknown`). Raw output: `raw/20261105-parquet-scan-arm64/` and `raw/20261105-parquet-scan-x86/`.

## Result in one paragraph

**R1: reuse `parquet-rs`.** It hands back dictionary arrays for the string columns of `lineitem` without expanding them, and with that its single-thread decode plus a cheap consumer takes 0.37 to 0.65 of DuckDB's time on both CPU types (the rule asked for at most 1.25). **R3: the morsel is a whole row group.** On the rule's layout (`big1`, written by DuckDB) a quarter of a row group is slower (0.64 to 0.73x of the row group's throughput at 8 threads, 0.40 to 0.46x at 1 to 4 threads) and uses 2.2x the memory; the rule wanted at least 1.15x and at most +25% memory. One observation next to the rule, not instead of it: with a Parquet page index a quarter is 1.55 to 1.80x faster at 8 threads, but only because `big1` has 4 row groups for 8 threads.

## Method

- `pscan` (`spikes/parquet-scan`): the arms below read the same columns and fold every value into a checksum (numbers and dates by value, decimals by unscaled value, strings by first byte, a dictionary column by key counts). Every timed run is compared with a reference; **a gate runs first** and no time is kept unless every arm equals DuckDB's SQL result on the same file (count and per-column sums, 6,001,215 rows).
- Arms for R1 (one thread, two projections: Q1 = 7 columns, Q6 = 4 columns): `plain` (parquet-rs defaults: expanded strings, Decimal128), `engine` (`with_schema`: `Dictionary(Int32, Utf8)` for `l_returnflag` and `l_linestatus`, `Decimal64(15,2)` for the four decimal columns), `engine-twin` (the same arm again, the noise control), `duckdb` (DuckDB 2.0.0 CLI, `SET threads=1`, its own timer, `read_parquet` plus `sum`/`count` over the same columns). 25 rounds after one discarded warm-up round, the arm that goes first rotates.
- Arms for R3: the morsel is a row group (`rg`), a quarter of a row group (`quarter`: four contiguous row ranges, via `RowSelection`) or `rg` again (twin), 1/2/4/8 threads (an atomic morsel counter, no stealing), 25 rounds. Two files with the same rows: `big1` (DuckDB, 4 row groups of about 1.5 million rows, **no page index**; the layout named in the rule) and `big1-pi` (rewritten by parquet-rs, 4 row groups, 20,000-row pages, page statistics, **with** a page index).
- Memory: one process per run, 5 runs per arm, the median of the peak resident size above the size before the scan, at 8 threads.
- Machines: `n4a-highcpu-8` (Neoverse-N3, 8 cores, ARM64) and `c3d-highcpu-8` (AMD EPYC 9B14, 4 cores x 2 threads), Spot, each run alone on its VM. Every block waited for load below 0.5 (ADR 0006 rule 7); start loads 0.46 to 0.49, printed in the raw summaries. The files are in the page cache after the warm-up round: this measures decoding, not I/O.
- Cost: about $0.017 (n4a) plus $0.014 (c3d), estimated by the orchestrator; caps were $0.138 and $0.080. Resources left after cleanup: none (checked by label).
- Before the runs: 8 tests (`tests/scan.rs`) and 3 mutants (`mutants/run.sh`: a split off by one row, the last thread dropped, the last dictionary key skipped) pass or are caught on both VMs; a build error would have been INVALID, not a catch. The gate caught one of the mutants by accident on the development machine (a stale binary), which is how the runner learned to rebuild clean afterwards.

## R1: single-thread decode against DuckDB (median [q1, q3] in ms; ratios are per-round medians of times)

| machine | projection | plain | engine | engine twin | DuckDB | engine / twin | engine / DuckDB | plain / DuckDB | plain / engine |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| n4a | Q1, 7 columns | 142.0 [139.8, 145.3] | 98.0 [97.0, 98.5] | 97.6 | 207.0 [206, 209] | 1.00 | **0.47** [0.47, 0.48] | 0.69 | 1.45 |
| n4a | Q6, 4 columns | 72.1 [71.5, 72.8] | 60.6 [60.2, 61.3] | 60.6 | 162.0 [161, 163] | 1.00 | **0.37** [0.37, 0.38] | 0.45 | 1.19 |
| c3d | Q1, 7 columns | 192.2 [190.5, 201.1] | 132.1 [130.5, 133.8] | 131.1 | 204.0 [202, 205] | 1.01 | **0.65** [0.64, 0.66] | 0.95 | 1.45 |
| c3d | Q6, 4 columns | 100.8 [99.4, 102.0] | 87.1 [86.2, 88.6] | 87.1 | 163.0 [161, 165] | 1.00 | **0.54** [0.53, 0.54] | 0.62 | 1.16 |

- **R1 (i) PASS on both CPU types:** the gate checks that `l_returnflag` comes back as a `Dictionary` array (it does, on `lineitem` and on `big1`).
- **R1 (ii) PASS on both CPU types and both projections:** the engine arm needs 0.37 to 0.65 of DuckDB's time; the rule's limit was 1.25 (0.8x of DuckDB's speed). The margin is at least 1.9x, so the decision does not depend on the caveats below. Even the plain arm passes.
- The twin ratio is 1.00 to 1.01, so the noise of a pair is about 1%; the rest of the table is far above it.
- Keeping dictionaries and `Decimal64` makes the scan 1.16x to 1.45x faster than parquet-rs defaults (more with more string columns), the same direction as ADR 0003's 13-14 ms against 80-85 ms for the string columns.

**Caveats.** DuckDB's time includes its aggregates and its own work on the string and decimal columns (`sum(ascii(...))`, a cast of decimals), which is not the same work as our fold, so the ratio is "decode plus a cheap consumer", not a pure decode comparison; the margin is large but the number is not a decode speed. One file (SF1 `lineitem`), whose two string columns have 3 and 2 distinct values, the best case for dictionaries; high-cardinality string columns (`l_comment`) were not measured. Only INT64-backed `DECIMAL(15,2)` as DuckDB writes it was read; whether other writers' physical types (fixed-length byte arrays) also arrive as `Decimal64` without a widening copy was not tested. The files were read from the page cache.

## R3: morsel size (throughput; row group time / quarter time, so below 1 means the quarter is slower)

`big1` (the rule's layout, DuckDB-written, no page index; 4 row groups, so 4 morsels or 16 quarters):

| threads | n4a row group ms | n4a quarter ms | n4a gain | c3d row group ms | c3d quarter ms | c3d gain |
|---|---:|---:|---:|---:|---:|---:|
| 1 | 96.8 | 244.3 | 0.40 | 127.8 | 279.4 | 0.46 |
| 2 | 48.7 | 121.5 | 0.40 | 68.8 | 149.1 | 0.46 |
| 4 | 25.5 | 63.5 | 0.41 | 39.8 | 88.7 | 0.44 |
| 8 | 25.7 | 35.4 | **0.73** [0.71, 0.75] | 44.2 [40.3, 49.4] | 70.7 | **0.64** [0.57, 0.70] |

Peak memory above the opened file at 8 threads (median of 5 runs, KiB; runs within 3% of each other): n4a 82,536 (row group) against 183,740 (quarter) = **+122.6%**; c3d 84,676 against 182,092 = **+115.0%**.

**R3 reading: row group, on both CPU types and on both criteria** (gain 0.73x and 0.64x against 1.15x; memory +123% and +115% against +25%). The decision is therefore **a whole row group per morsel**.

`big1-pi` (same rows, with a Parquet page index), next to the rule and not instead of it:

| threads | n4a gain | c3d gain |
|---|---:|---:|
| 1 | 0.96 | 0.98 |
| 2 | 0.96 | 0.98 |
| 4 | 0.98 | 0.97 |
| 8 | **1.80** [1.73, 1.84] | **1.55** [1.39, 1.68] |

Memory: n4a 9,588 against 16,048 KiB (+67.4%), c3d 11,680 against 16,376 (+40.2%).

What the numbers say, and what they do not:

- **Without a page index a quarter costs about 2.2 to 2.5x the work.** The one-thread time of the quarter arm is 2.52x (n4a) and 2.19x (c3d) the row group's. That is what four quarters would cost if each had to decode its row group from the start up to its own rows ((1+2+3+4)/4 = 2.5x), and the memory is about 2.2x. The mechanism is consistent with the numbers; it was not shown directly (no instrumentation of the pages read).
- **With a page index a quarter costs 2 to 4% at 1 to 4 threads** (more morsels to set up) and **wins at 8 threads by 1.55x to 1.80x, only because there are 4 row groups for 8 threads**: the row-group arm runs 4 threads busy and 4 idle (its 8-thread time equals its 4-thread time on n4a: 25.7 and 25.5 ms). That is the "fewer morsels than threads" case, not a statement about the best morsel size. For a normal file with dozens of row groups (the 49 of `lineitem`) it does not arise. It will matter for a single file with few big row groups on a machine with many cores (item 32, and a later slice).
- On c3d the row-group arm is slower at 8 threads than at 4 (44.2 [40.3, 49.4] against 39.8 ms) with 4 morsels only; not investigated (the x86 thread-placement effect of `20261105-pin-check.md` is a candidate, not a conclusion).
- Scaling of the row-group morsel from 1 to 4 threads: 96.8 / 25.5 = 3.8x (n4a), 127.8 / 39.8 = 3.2x (c3d). **The card's scalability target (5x at 8 threads) was not measured here**: 4 morsels cannot use 8 threads. It needs the 49-row-group file and belongs to the slice's own benchmark.

## What this changes

- ADR 0003: its open point "the `parquet` crate expands dictionaries for some types, which may need our own page decoding" is answered for the columns of `lineitem`: **reuse `parquet-rs`**, with `ArrowReaderOptions::with_schema` asking for `Dictionary(Int32, Utf8)` and `Decimal64(15,2)`. Not shown: high-cardinality strings, other writers' decimal encodings, `Date32`/`Timestamp` variants beyond `l_shipdate`. Revision note added.
- ADR 0001: the open "morsel size" is decided as **one row group**; the finer slice is not adopted. A page-index-based split is recorded as an option for files with fewer row groups than threads, to be tested when that case matters. Revision note added.
- Slice 3: card section 4 now carries this result; R2 and the rest of the card are untouched.

## Not shown

R2 (pruning); 32 and more cores; SF10; files written by pyarrow or Java; cold reads (I/O); the 5x scaling target; a pure decode comparison with DuckDB (a consumer with identical work); memory above 8 threads or on other layouts than the two used.
