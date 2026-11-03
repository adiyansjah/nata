# Spike: pull/static versus push/morsel (ADR 0001)

Code: `spikes/pipeline-model/` (throwaway; delete once ADR 0001 is accepted).

## What was run

- Query shape: TPC-H Q1 on `lineitem`: scan -> filter (`l_shipdate <= 1998-09-02`) -> project -> hash aggregate by two columns, 4 groups.
- Data: TPC-H SF1 `lineitem.parquet`, 6,001,215 rows, 49 row groups, 205 MB, local disk.
- Engine under test: ad-hoc Rust spike on `arrow`/`parquet` crates. Not Nata.
- CPU: 12th Gen Intel(R) Core(TM) i3-1220P, 8 logical cores visible, 15.6 GiB RAM, Linux 6.12.113 (shared cloud VM, so timings are noisy).
- Timing: in-process wall clock for the whole query, 21 timed runs after 1 discarded warm-up. Warm OS page cache, never cold. Output is a 4-row result, so output cost is negligible.
- Designs: `pull/static` = each thread owns a contiguous range of row groups and pulls batches through `Box<dyn PullOp>` operators. `push/morsel` = threads take the next row group from a shared atomic counter and push batches through `Box<dyn Transform>` operators into a thread-local aggregate. Filter, project and aggregate logic is shared code.

## Result

| design | threads | min ms | median ms |
|---|---:|---:|---:|
| pull/static | 1 | 393 | 416 |
| pull/static | 2 | 214 | 321 |
| pull/static | 4 | 193 | 210 |
| pull/static | 8 | 166 | 185 |
| push/morsel | 1 | 438 | 488 |
| push/morsel | 2 | 256 | 354 |
| push/morsel | 4 | 189 | 211 |
| push/morsel | 8 | 162 | 212 |

Both designs return the same 4 rows as DuckDB 2.0.0 (identical at 2 decimals). Counts are exact; f64 sums differ in the last digits between thread counts because summation order changes (checked with a 1e-9 relative tolerance).

## What limits and what is excluded

- The first full run (7 timed runs) gave pull/1 = 477 ms against 393 ms here: run-to-run noise on this VM is about 20%. Differences smaller than that are not evidence.
- The workload is uniform: every row group costs about the same, so dynamic morsel assignment has nothing to win. It cannot show skew handling, spill, window functions or exchange behaviour.
- The spike is not tuned. Single-thread time per stage (`spike-pipeline-model <file> stages`, 4 runs, warm cache, timers around each call): decode 174-189 ms, filter 39-42 ms, project 93-100 ms, hash aggregate 76-86 ms, about 390 ms in total. Decode is the largest share (45%); why it is slow was not investigated. Filter copies 5.9M surviving rows instead of using a selection vector; project makes several passes with intermediate arrays; aggregate does one default-hasher HashMap lookup per row. These costs are the same in both designs. DuckDB ran the same query in 250-294 ms on 1 thread and 179-215 ms on 8 (whole process, startup included, 5-7 runs), so do not read the spike's absolute numbers as engine speed.
- Scaling stops near 2.4x at 8 threads. Cause not investigated (candidates: the 8 visible cores are hyperthreads, Parquet decode memory bandwidth, per-row-group file open).
- push/1 is about 10% slower than pull/1 in both runs. Cause not investigated; could be noise.

## Reading

Neither design shows a measurable cost on a simple uniform scan. The choice between them rests on properties this spike does not test; see ADR 0001.

## Follow-up: how much of the gap to DuckDB do cheap fixes close?

Same machine, data and method as above. Two fixes in the spike's operators (shared by both designs): an Fx multiply-rotate hasher instead of SipHash for the group-by map, and a project step that reads the three decimal columns in one loop and writes the three outputs directly. Results were checked against the baseline spike (counts exact, sums within 1e-9 relative), which was itself checked against DuckDB.

### Single-thread time per stage (best of 8 runs, ms)

| variant | decode | filter | project | aggregate | total |
|---|---:|---:|---:|---:|---:|
| baseline (SipHash, multi-pass project) | 192 | 42 | 101 | 83 | 419 |
| fast hasher | 184 | 41 | 98 | 36 | 359 |
| fused project, i128 to f64 | 200 | 44 | 108 | 88 | 440 |
| fused project, via i64 | 185 | 41 | 34 | 81 | 341 |
| fast hasher + fused project (via i64) | 181 | 40 | 31 | 35 | 286 |

Findings:
- The hasher halves the aggregate (83 to 36 ms).
- Fusing the project step alone did nothing (101 to 108 ms). The cost was the `i128 -> f64` conversion, which has no hardware instruction (17.7M conversions). Converting through `i64` (values fit; TPC-H decimals are 15 digits) took project from about 100 to about 31 ms. The earlier explanation, intermediate arrays and extra passes, was wrong.
- Decode (181 ms, 63% of what remains) and filter (40 ms, copies 5.9M rows instead of using a selection vector) are untouched.

### End to end, push/morsel (15 timed runs after warm-up, ms; min / median)

| threads | baseline | cheap fixes | DuckDB 2.0.0, whole process | DuckDB minus startup |
|---:|---:|---:|---:|---:|
| 1 | 365 / 389 | 298 / 319 | 255 / 270 | about 205-220 |
| 2 | 209 / 220 | 160 / 178 | 166 / 175 | about 120 |
| 4 | 160 / 170 | 114 / 118 | 140 / 145 | about 90 |
| 8 | 124 / 130 | 95 / 107 | 135 / 142 | about 85-90 |

DuckDB was timed as a separate process (11 runs, warm cache, `SET threads=N`) right after the spike runs. Its process startup, measured with `SELECT 1`, is 46 ms min / 57 ms median, and the last column subtracts about 50 ms from the min. That subtraction is approximate: the Parquet extension load also sits inside the DuckDB figure, so DuckDB's real compute is probably lower, and the gap probably larger, than shown.

Reading: the cheap fixes take the spike from about 1.8x DuckDB's compute time to about 1.4x at 1 thread, and from about 1.4x to about 1.1x at 8 threads. The remaining 1-thread gap is mostly Parquet decode and the filter copy. The spike is still not an engine: it handles one query shape, no NULLs, and a fixed schema.

## Follow-up: where the Parquet decode time goes

Same machine, data and method. Decode-only runs: single thread, all 49 row groups, no operators, best of 6, warm cache. `perf` is installed but blocked by `perf_event_paranoid=3`, and the setting was not changed, so causes were isolated by experiments instead of a profile.

### What is in the file (DuckDB-written, 6 columns used)

| column | physical type | encoding | compressed MB | uncompressed MB |
|---|---|---|---:|---:|
| l_returnflag | BYTE_ARRAY (UTF8) | dictionary | 1.4 | 1.5 |
| l_linestatus | BYTE_ARRAY (UTF8) | dictionary | 0.8 | 0.8 |
| l_quantity | INT64 (decimal 15,2) | dictionary | 4.5 | 4.5 |
| l_extendedprice | INT64 (decimal 15,2) | plain | 31.0 | 48.0 |
| l_discount | INT64 (decimal 15,2) | dictionary | 3.0 | 3.0 |
| l_shipdate | INT32 (date) | dictionary | 9.5 | 9.5 |

All columns Snappy-compressed.

### Per column, decode only (ms)

| column | snappy file | uncompressed copy of the same file |
|---|---:|---:|
| l_returnflag | 37 | 39 |
| l_linestatus | 36 | 37 |
| l_quantity | 20 | 16 |
| l_extendedprice | 49 | 12 |
| l_discount | 13 | 14 |
| l_shipdate | 14 | 14 |
| all 6 | 167 | 128 |

The per-column times sum to the all-columns time, so columns do not interact.

### Causes found

1. **String expansion, about 65-70 ms.** The two 1-character columns have 3 distinct values each, yet the reader expands 6M dictionary indexes into 6M separate strings per column. Same two columns, decode only: `Utf8` 80-85 ms, `Utf8View` 24-25 ms, `Dictionary(Int8, Utf8)` 13-14 ms, `Dictionary(Int32, Utf8)` 14 ms. Keeping the dictionary costs the same with 32-bit keys as with 8-bit keys.
2. **Snappy decompression, about 37-39 ms**, nearly all of it on `l_extendedprice` (49 ms compressed, 12 ms uncompressed; 48 MB uncompressed in about 37 ms is about 1.3 GB/s). Not fixable by an engine choice for a file someone else wrote; a faster Snappy implementation is untested.
3. **Batch size**: 1024 to 8192 rows is flat (171-177 ms); 32768 rows takes 233 ms and 131072 rows takes 336 ms, so large batches fall out of cache. Keep batches near 4K-8K rows.

### End to end with dictionary-preserving strings

The aggregate groups on dictionary keys through a per-batch lookup table; filter and project pass the dictionary arrays through.

Single thread, best of 8, ms:

| variant | decode | filter | project | aggregate | total |
|---|---:|---:|---:|---:|---:|
| baseline | 179 | 40 | 94 | 84 | 396 |
| fast hasher + fused project | 177 | 39 | 30 | 48 | 294 |
| ... + dictionary strings | 122 | 21 | 31 | 49 | 224 |

Filter halves too (40 to 21 ms), because filtering a dictionary array copies only the small keys.

End to end, push/morsel, 15 timed runs, min ms, set beside DuckDB 2.0.0 timed immediately afterwards (whole process, 11 runs, `SET threads=N`; startup measured at 36 ms min with `SELECT 1` and subtracted):

| threads | baseline | cheap fixes | + dictionary strings | DuckDB, minus startup |
|---:|---:|---:|---:|---:|
| 1 | 399 | 311 | 223 | about 175 |
| 2 | 216 | 173 | 130 | about 125 |
| 4 | 164 | 123 | 91 | about 95 |
| 8 | 130 | 97 | 75 | about 85 |

DuckDB's own `EXPLAIN ANALYZE` on one thread gives scan with filter 0.10 s, projection 0.01 s, group-by 0.07 s (about 0.18 s in total).

### Reading and limits

- With all three changes the spike is about 1.3x DuckDB's compute time on one thread and level with it (75 against about 85 ms) on 8 threads. Do not read the 8-thread comparison as "faster than DuckDB": the subtraction of startup is approximate, the 8 visible cores of this shared VM may be hyperthreads, and DuckDB's own 8-thread time moved between 121 and 142 ms across sessions.
- This is one query shape on one file: fixed schema, no NULLs, three distinct string values, a dictionary-friendly layout. A column with many distinct strings would not gain from dictionary-preserving reads the same way, and the spike's `Int8` dictionary keys would overflow beyond 127 distinct values; a real engine needs `Int32` keys or a runtime choice (decode cost was the same at 14 ms).
- Remaining decode time (122 ms) is mostly `l_extendedprice` (49 ms, of which about 37 ms is Snappy), then `l_quantity` 20 ms, `l_shipdate` 14 ms, the two dictionary string columns 14 ms, and `l_discount` 13 ms.

## Correction: the DuckDB comparison above was biased

The "DuckDB, minus startup" columns in the two follow-ups above were computed as whole-process wall time minus the time of `duckdb -c "SELECT 1"`. That left other per-process costs inside DuckDB's number, so it overstated DuckDB's time. Timed inside one process with `.timer on` (12 runs per thread count, first run dropped, warm cache, `SET threads=N`), DuckDB 2.0.0 takes, in ms:

| threads | min | median |
|---:|---:|---:|
| 1 | 148 | 158 |
| 2 | 94 | 98 |
| 4 | 68 | 74 |
| 8 | 54 | 56 |

Against this, the earlier best spike variant (dictionary strings, fast hasher, fused project; 223 / 125 / 89 / 75 ms at 1 / 2 / 4 / 8 threads in the same session) was about 1.5x DuckDB at 1 thread and about 1.4x at 8, not "level at 8 threads" as the earlier text said. Use the in-process table for any comparison.

## Follow-up: profile-guided optimization (v2)

`perf record -e cpu-clock` (this VM has no hardware counters) on the best variant so far, single thread, showed: Snappy decompress 20%, group-by update 18% plus dictionary key lookup 4%, project 13%, dictionary-index decoding about 11%, `i64 -> Decimal128` widening 5%, `memcpy` about 7%, kernel read copy 3%. Changes made, all in the spike:

1. Filter, project and aggregate fused in one loop over the decoded batch: no filtered copy, no intermediate arrays.
2. Exact integer decimal arithmetic: sums of the scale-2 values, and of `price * (100 - disc)` at scale 4, in `i128` accumulators, with the per-row product done in `i64` and an `i128` fallback on overflow. No float conversion.
3. Dense group accumulation indexed by the two dictionary keys (3x3 array per batch, merged into a small map once per batch), instead of one hash lookup per row.
4. Reader asked for `Decimal64` (the file stores INT64) to skip the widening to `i128`.
5. Whole file held in memory, tried as a variant.

Limits of v2: it is hand-specialized to this one query. It assumes decimals of scale 2 and precision up to 18, dictionary strings with `Int8` keys, and no NULLs. A real kernel has to handle all of those, and a general engine pays for that generality, so v2 is a ceiling for what specialization can reach, not an engine.

### Result (SF1, 15 timed runs after warm-up, min / median ms, same session as the DuckDB in-process timings above)

| variant | 1 thread | 2 | 4 | 8 |
|---|---:|---:|---:|---:|
| baseline spike | 371 / 389 | 205 / 221 | 157 / 169 | 117 / 125 |
| + dictionary strings (previous best) | 223 / 250 | 125 / 129 | 89 / 94 | 75 / 79 |
| v2, Decimal128 | 133 / 145 | 77 / 81 | 52 / 55 | 37 / 40 |
| v2, reader returns Decimal64 | 129 / 135 | 73 / 76 | 48 / 52 | 35 / 41 |
| v2, file held in memory | 139 / 149 | 77 / 79 | 54 / 58 | 47 / 54 |
| v2, Decimal64, file in memory | 132 / 140 | 71 / 73 | 46 / 50 | 32 / 36 |
| DuckDB 2.0.0, in-process timer | 148 / 158 | 94 / 98 | 68 / 74 | 54 / 56 |

The v2 result equals DuckDB's output digit for digit, including the scale-4 column, which the float versions were only checked against within a 1e-9 relative tolerance (checked on the Decimal128 run's printed result against `duckdb -csv`).

Holding the file in memory gives no consistent gain (differences are within the run-to-run noise of about 10-20%).

### What is left

Profile of v2 (single thread, Decimal64): Snappy decompress 33%, the fused loop 24%, dictionary-index decoding about 25% (five of six columns are dictionary-encoded), kernel and libc copies about 9%.

Snappy is the one clear lever. On an uncompressed copy of the file v2 takes 83 ms at 1 thread (22 at 8) against 129 (35) on the Snappy file, so the `snap` crate costs about 46 ms per thread. DuckDB goes from 148 to 131 ms at 1 thread. That difference is not a reliable measure of DuckDB's Snappy cost: it compares two different files (390 MB against 205 MB read), and the next section replaces it with a direct profile.

## Follow-up: a faster Snappy decoder (result: no end-to-end gain)

Snappy was the largest single cost in v2 (33% of a single-thread profile), so decoders were swapped in behind the `parquet` crate without forking it: a shim crate named `snap` (`spikes/pipeline-model/shim-snap`, patched in with `[patch.crates-io]`) selects the decoder with `SNAP_IMPL=orig|cpp` and times every decompress call. The 539 real compressed pages of the six columns (50.3 MB in, 67.4 MB out, median 16 KB, max 960 KB output) were dumped and replayed in a standalone C++ benchmark (`snappy-bench/bench.cc`) and inside the Rust process (`snapreplay`). The original `snap` crate is vendored in `vendor/snap-orig` (BSD-3-Clause, `COPYING` kept).

### What DuckDB spends

`perf record -e cpu-clock` on DuckDB 2.0.0, one thread, the same query run 25 times: `duckdb_snappy::DecompressBranchless` is 18.9% of its samples, about 28 ms of its 148 ms per query, or about 2.4 GB/s on this data. That replaces the earlier "17 ms" estimate. Its other large costs are the hash aggregate (about 9% hash probe, 7% string-key match, 6% clustered aggregate) and dictionary slicing (5%).

### Decoder speed on the real pages (GB/s of decompressed output)

| decoder | standalone, hot output buffer | inside the Parquet reader |
|---|---:|---:|
| `snap` 1.1.2 (pure Rust), generic x86-64 | 1.65-1.95 | 1.4-1.55 |
| `snap`, built with `-C target-cpu=x86-64-v3` | 1.73-1.75 | not run |
| Google snappy 1.3.1 via `snappy_src`, default build flags | 1.36 | 0.8-0.9 |
| same, tuned build (`-DNDEBUG`, `HAVE_*` macros, `-mavx`, `-fPIE`) | 2.05-2.22 | 1.7-1.75 |

(Absolute numbers drift by about 15% between sessions on this shared VM; each row pair was measured back to back.)

What decided the C++ numbers, from standalone builds of the same source on the same pages:
- `-fPIC`, which the `cc` crate always adds, makes GCC treat the hot helpers (`MemCopy64`, `DeferMemCopy`) as interposable: they are not inlined and `memmove` goes through the PLT. `-fPIE` fixes it: 1.57 to 2.22 GB/s with AVX on.
- AVX: 1.4 to 2.1 GB/s (`-mavx`; `-mavx2`, `-march=x86-64-v3` and `-march=native` are the same). SSSE3 and BMI2 alone did nothing or slightly hurt.
- `-DNDEBUG` and the `HAVE_*` macros had no measurable effect.
- Rust `snap` gets nothing from AVX codegen.

### End to end

v2 with the Decimal64 reader, min / median ms, three alternating rounds:

| threads | `snap` | tuned C++ snappy |
|---:|---|---|
| 1 | 128 / 136, 136 / 145, 129 / 140 | 139 / 145, 134 / 140, 135 / 140 |
| 8 | 34 / 35, 37 / 39, 37 / 40 | 36 / 40, 38 / 43, 40 / 44 |

No improvement; the C++ decoder is equal or slightly slower. The decode-only timing shows it saving about 6 ms of Snappy per pass (38-40 against 44-48 ms), but that does not survive into the pipeline within the roughly 5-10% noise.

### Reading

- A faster Snappy decoder is not a worthwhile lever: the best build tried is 15-25% faster on the decoder alone, which is under 5% of the total and not visible end to end, and it adds a C++ toolchain, AVX-specific builds and runtime CPU dispatch.
- A pure-Rust decoder was not written; it would have to beat tuned C++ by a wide margin to matter.
- DuckDB's decoder is about as fast as the tuned C++ one (2.4 against 2.1-2.2 GB/s), so the earlier "2.5x" gap was wrong, and the remaining distance to DuckDB on one thread is not Snappy.
- Anyone building `snappy_src` should know the default `cc` build is about 2x slower than it needs to be.

## Follow-up: independent review, interleaved accumulators, scaling ceiling

An independent review of this file (an Opus agent, read-only, no measurements) made the points under "Corrections" below; the two experiments under "Measured" were run in response. Same-session means measured back to back in one session; this VM changes speed between sessions by 6-30%, so only same-session ratios are comparable.

### Measured

**Interleaved accumulators (no gain).** The review suspected that the fused loop (about 5 ns per row) is slowed by a dependency chain through memory, because consecutive rows often fall in the same group and each row does four 128-bit load-add-store sequences on one cache line. Using 4 copies of each group's accumulators, with consecutive rows going to different copies, 1-thread v2 (Decimal64), min of 12 runs, alternating, 4 rounds: plain 98 / 98 / 100 / 101 ms, interleaved 100 / 103 / 100 / 104 ms. The hypothesis is not supported; the loop is not limited that way.

**Scaling.** `lscpu` inside the KVM guest reports 8 cores with 1 thread each and an invented L2 size (32 MiB in 8 instances), so the real topology (the host chip is a 12th-gen mobile i3 with performance and efficiency cores) cannot be read from it. Measured instead:
- 1-thread v2 pinned to each of the 8 vCPUs in turn: 104-109 ms. No visible fast and slow vCPUs.
- 8 independent single-thread processes at once, one per vCPU: 123-166 ms each (mean 138) against 104 ms alone. That is an aggregate of about 6.0x one process (about 5.0x if limited by the slowest).
- One process with 8 threads, same session: 100 ms at 1 thread, 25 ms at 8, which is 4.0x.

So the hardware allows about 6x here, and the in-process threaded version reaches 4x. Some of the gap is the tail: 49 row groups over 8 threads is 7 rounds, a best case of about 87% efficiency. The rest is not explained. A plausible cause, untested, is contention inside one process: the parquet reader allocates new buffers per page and every thread takes page faults on the same address space, which independent processes do not share. Finer morsels and a reused buffer pool would test it.

**Same-session comparison with DuckDB 2.0.0** (in-process timer, 12 runs, first dropped; spike min of 15 timed runs), ms:

| threads | spike v2 (Decimal64) | DuckDB min / median |
|---:|---:|---:|
| 1 | 100 / 103 | 139 / 140 |
| 2 | 54 / 56 | 76 / 79 |
| 4 | 35 / 36 | 62 / 65 |
| 8 | 25 / 26 | 46 / 48 |

In the earlier session the ratio was 129 against 148 at 1 thread and 35 against 54 at 8, so the direction held in both sessions but the size of the lead varies from 13% to 28% at 1 thread. Treat the lead as roughly 1.1x to 1.4x at 1 thread and 1.5x to 1.8x at 8, for this one hand-specialized query.

### Corrections to earlier claims in this file

- "Ceiling" (v2) is too strong. v2 is one hand-specialized reference point; ideas not tried (aggregating on dictionary codes, evaluating the date predicate once per dictionary entry, picking a narrower accumulator from row-group statistics) go beyond it.
- The 1-thread lead over DuckDB in the first session (129 against 148 ms) was inside the stated 10-20% noise. Only the 8-thread lead and the same-session figures above clear it.
- Variants in the v2 table were timed one after another, not interleaved. The 8-thread "file held in memory" cell (47 / 54 ms, against 37 / 40) was probably drift, since the Decimal64 variant with the file in memory measured 32 / 36 in the same table; the conclusion "no consistent gain" was right but not properly supported. Only the Snappy comparison alternated.
- "Faster Snappy: no end-to-end gain" should read "an expected 4-8 ms is below this setup's resolution".
- The spike's timed region excludes Parquet footer parsing (done once in `Source::open`); DuckDB's timer includes parsing, planning and footer reads. Both are small, and both favour the spike.
- The 18.9% DuckDB Snappy share comes from a 25-query process that includes startup, so "about 28 ms" and "2.4 GB/s" are approximate.
- "Large batches fall out of cache" is untested; page-fault and zeroing costs for allocations above the allocator's mmap threshold are an equally plausible cause of the slower 32K and 128K batch sizes.
- The overflow comment in `accumulate` said overflow is an error; it is not (the `i128` sums wrap in release builds and `p as i64` truncates for wide input). Comment corrected; the shortcut is valid only under the precision <= 18 assumption.
- Dense group accumulation from dictionary keys needs a cap on the product of the key cardinalities, with a hash fallback above it.

## Corrections found by an independent review of ADRs 0001-0003

An independent reviewer (a second agent given the ADRs, this file and the code, asked to critique) found the following; each was checked against the code or this file before being accepted.

- **The 8-thread pull/static row used 7 threads.** `run_pull` splits the 49 row groups into chunks of `div_ceil(49, threads)`: 25, 13 and 7 row groups for 2, 4 and 8 threads, which yields 2, 4 and 7 threads (1 gives 1). The first section's 8-thread comparison (pull 166 / 185 ms, push 162 / 212 ms) therefore compared 7 threads of pull with 8 of push.
- **The 2-thread row was left out of the summary.** Push/morsel was slower there by 20% at the minimum (256 against 214 ms) and 10% at the median (354 against 321 ms). At 1 thread the figures are 11% (min, 438 against 393) and 17% (median, 488 against 416). Only the 4-thread row (193 / 210 against 189 / 211) shows the two designs equal within noise.
- **Two things changed at once.** The comparison varies both the scheduling (static partitions against a shared morsel counter) and the operator-dispatch style (pull iterators against push transforms). A pull design with a morsel queue was not run, so the comparison cannot attribute a difference to either.
- **Mixed sessions in a headline.** "399 -> 129 ms" took the baseline from the first session and v2 from a later one. In the same session as the v2 run the baseline was 365-371 ms, so the speedup is about 2.9x.
- **"Exact integer sums were also faster" was never isolated.** v2 changed five things at once (fused loop, exact integers, dense groups, `Decimal64` reader, no filtered copy). The one isolated result is that avoiding the `i128 -> f64` conversion took the project step from about 100 to about 31 ms.
- **Not in this file:** the "26-33%" Snappy share used in ADR 0002. This file has 20% (profile before v2), 33% (v2 profile) and, from the uncompressed-file comparison, about 36% (46 of 129 ms).
