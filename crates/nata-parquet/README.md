# nata-parquet

The Parquet scan of slice 3 (`docs/slices/0001-parquet-scan.md`): a plan over a list of Parquet files (footers read lazily, row groups pruned by their statistics), one
independent task per kept row group (the morsel of ADR 0001), an exact row filter with DuckDB's semantics, DuckDB-compatible CSV output for the developer command `nata scan`, and
since **step 2b** a parallel, ordered, cancellable, memory- and handle-bounded driver (`run_parallel`, on `crates/nata-sched`). `run_serial` stays as the single-thread
reference implementation that the parallel scan is tested against. The cloud benchmarks are a later step.

**Why a new crate and not `nata-scan`.** `nata-scan` is the pruning DECISION: pure, no dependencies, 53 mutants checked in about 90 seconds. This crate needs `parquet` and `arrow` (a compile
of minutes) and does I/O. Keeping them apart keeps the decision crate's checks fast and its claim ("no I/O, no dependencies, no `unsafe`") true. This crate depends on `nata-scan`.

```rust
let plan = nata_parquet::ScanPlan::open(&files, &ScanOptions { columns, filters, prune: true })?;   // footers, schema checks, filter typing, pruning
for task in plan.tasks() { let out = task.run()?; /* out.batches: projected, filtered, compacted */ }
let report = nata_parquet::run_serial(&plan, limit, |batch| /* sink */ Ok(()))?;                 // or the serial driver; report.line() is what `nata scan` prints
```

## What it reads

One decision table (`src/schema.rs`) for reading, filtering and pruning. A column that is not in it is `not supported yet: <what>` and an error only if the scan touches it (projects or filters it;
the default projection is all columns, so it touches all).

| Parquet | Arrow type handed out | Filter literal | Pruned on statistics |
|---|---|---|---|
| BOOLEAN | `Boolean` | `true` / `false` | yes |
| INT 8/16/32/64 (signed) | `Int8` .. `Int64` | integer, compared as 128-bit | yes |
| INT 8/16/32/64 (unsigned) | `UInt8` .. `UInt64` | integer | **no** (unsigned order is not mapped) |
| FLOAT, DOUBLE | `Float32`, `Float64` | number, `nan`, `inf`, `-inf` | yes, never on NaN-sensitive terms (no NaN count in Parquet) |
| DECIMAL(p<=38, s) on INT32, INT64, FIXED_LEN_BYTE_ARRAY or BYTE_ARRAY | `Decimal64` (p <= 18), `Decimal128` | decimal, EXACT, never rounded to the column's scale | yes |
| DATE | `Date32` | `YYYY-MM-DD` | yes |
| TIMESTAMP without time zone, MILLIS or MICROS | `Timestamp(ms\|us, None)` | `YYYY-MM-DD HH:MM:SS[.ffffff]` | yes (ms converted to us, checked) |
| STRING / UTF8 | `Dictionary(Int32, Utf8)` (kept, never expanded) | `'text'` | yes (truncated bounds handled by `nata-scan`) |
| BYTE_ARRAY without annotation | `Binary` | `X'00FF'` | yes (unsigned bytes) |
| everything else: INT96, FLOAT16, TIME, UUID, ENUM, JSON, TIMESTAMP with time zone, TIMESTAMP NANOS, DECIMAL above 38 digits, nested columns (struct, list, map) | `not supported yet: <what>` | | |

Row-group selection: one whole row group per task (ADR 0001, revision note of 2026-11-05); no page index, no bloom filter, no row selection inside a row group.

## Late materialization (register item P1, `ScanOptions::late_materialization`, `--late-materialization`)

Off by default until the benchmark of step 4b decides (`docs/slices/0002-select-where-limit.md`, part 5). With a pushed filter, a row-group task reads in two steps: (1) only the filter's columns of the
whole row group are decoded and the exact filter is evaluated on them, giving a Parquet `RowSelection`; (2) only the OUTPUT columns are decoded, only for the selected rows (nothing is read when no
row passed, and no selection is used when every row passed). Both steps share the task's one file handle. A column that is both filtered and selected is decoded in both steps (the synchronous parquet-rs
reader has no predicate cache). The rows, their order, the counters (`rows_read` counts the rows the filter looked at) and the errors are those of the default read; `bytes_read` can be lower. The cancel
is looked at between batches in both steps. Without a filter it changes nothing. `NATA_LATE_MATERIALIZATION=1` turns it on for `nata scan`/`nata sql` and for the test suites (`late_default()`), which is
how the DuckDB differential files and `nata-check` run in this mode. A finding: parquet-rs sizes the buffers of a string batch for the batch size, not for the rows selected, so a late batch of 200 rows can
hold about 1 MB (`tests/late.rs`).

## Footers read ahead (register item P2, `ParallelConfig::footers_ahead`, `--footers-ahead K`) and the schema check (P10, `ScanOptions::strict_schema`)

Off (1 = one file at a time, slice 3's behaviour) until the benchmark of step 4b decides. With `K`, up to K files may be pending (footer being read, read and waiting for an earlier file, or with row groups not yet started),
and idle workers read the next files' footers while earlier row groups run (`crates/nata-sched`). The rows, their order and the errors are those of `K = 1`; handles stay at one per worker, and the footers in memory at
`K` plus the files whose row groups run (`peak_footers <= threads + K + 1`, tested under `ulimit -n 256` on 10,000 files). `strict_schema` (set by `nata sql`, whose binder reads only the first footer) makes the scan require
every file to have the first file's top-level columns in order and types; the error names the file. `NATA_FOOTERS_AHEAD=<k>` sets the default for the CLI and the test suites.

## Page checksums (register item R4, cargo feature `page-crc`, on by default)

A Parquet page may carry a CRC-32 of its (compressed) payload in its header. With the feature, parquet-rs verifies it before it decodes the page; a mismatch is `ErrorKind::Corrupt` ("Page CRC checksum mismatch") naming the file and the
row group, never a value. parquet-rs 60.1.0 offers no run-time switch (the check is compiled in or out with its `crc` feature, which adds the `crc32fast` crate: 30 `unsafe` keywords in 17 files, SIMD paths; recorded in
`bench/results/20261106-slice4-carried-in.md`), and it cannot WRITE checksums, so the files with checksums come from pyarrow (`tests/fixtures/make_checksummed.py`, committed). `cargo build -p nata --no-default-features` is the build
without the verification (the cost arm of the benchmark and the known-bad build of the fuzz test). What it does not cover: the footer and the page headers are not covered by a page checksum, and a page that carries no CRC
(parquet-rs and DuckDB write none) is not checked; pages that late materialization skips are not decoded and so not checked.

## The footer adapter (`src/adapter.rs`)

Statistics are handed to `nata-scan` only when they are sound; otherwise the row group is kept. `min_value`/`max_value` only (the deprecated `min`/`max` are never used: the `parquet` crate
reports `is_min_max_deprecated()`); only a footer column order of `TYPE_DEFINED_ORDER` with the sort order the type needs (`UNDEFINED` = no `column_orders` in the footer, `IEEE_754_TOTAL_ORDER`
(what parquet-rs 60 writes for floats), `UNKNOWN`: no bounds); a bound is exact only if the footer says so; no NaN count; no bounds for INT96, unsigned, FLOAT16, nested, time zones, nanoseconds,
decimals above 38 digits, text bounds that are not UTF-8, decimal bytes that do not fit 128 bits, and a millisecond timestamp whose microseconds overflow.

## Row semantics (DuckDB's, not Arrow's; `src/filter.rs`)

NULL compares are never true (also `<>`); floats: NaN = NaN, NaN above everything, -0.0 = +0.0; decimals exact across scales; integers as 128-bit numbers (a literal is never cast to the column's
width); text and binary bytewise unsigned; dictionary columns compared without expanding (the predicate runs once per dictionary entry). The pruning of `nata-scan` is sound only under these
semantics, and every one of them is checked against DuckDB.

## Output

`nata scan FILE... [--columns a,b] [--filter 'col op literal' ...] [--limit N] --csv [--no-prune] [--threads N] [--unordered] [--window N]`. stdout: the CSV in DuckDB's `-csv` text (the format
`nata-check` compares); stderr: one report line, for example
`scan: files=1 row_groups=30 skipped=25 read=5 not_read=0 rows_read=10240 rows_returned=9484 bytes_read=965772 columns_read=16 morsel=row-group pruning=applied(2 of 2 terms) files_opened=1 abandoned=0 threads=4 order=ordered window=4 peak_inflight_bytes=3088904 largest_task_bytes=3088904 peak_open_files=2 peak_footers=1 peak_rss_kib=14100`.
`bytes_read` is the number of bytes requested from the files for the column chunks by the row-group tasks (counted at the file handle, cancelled tasks included; the footers are not counted).
Row-group counts are those of the files that were **opened**: a scan that stops early (`--limit`, an error) has not read the footers of the files it never reached (`files_opened < files`).
`not_read` = kept row groups that were not decoded to the end (never started, or abandoned: `abandoned=`). Filtered batches are COMPACTED (arrow `filter_record_batch`; dictionary columns keep
their dictionary); a batch where every row matches is passed through; a batch with no match is dropped. Exit codes: 0, 1 (file or data error), 2 (`not supported yet`, wrong use). A closed stdout
(`| head`) cancels the scan like a limit and exits 0 without a message.

**Parallel semantics (step 2b; `src/parallel.rs`).**

* **Order.** Default: the rows come out in the order of the single-thread scan (file list, row groups, rows) for every thread count, through a bounded reorder window. `--unordered`: results are
  handed over as they finish (the same multiset of rows).
* **Window.** At most `window` row groups (default: the thread count) are in flight, finished and waiting, or in the writer's hands. A slow early row group makes the workers wait; the buffer cannot grow.
* **Limit.** `--limit N` counts rows in the delivered order. When the N-th row is out, no more row groups are claimed, running tasks stop at their next batch (8,192 rows), drop reader, handle and buffers,
  and all threads are joined before the process exits. Ordered mode prints exactly the first N rows.
* **Errors.** The error reported is the one of the lowest (file, row group) that failed (a file whose footer cannot be read has its place in the order), naming file and row group, never a cell value.
  Ordered mode prints every row before it first. Footers are read when the scan reaches the file, so a corrupt LATER file is found after the earlier files' rows were printed (the first file's footer is read
  before anything is printed). An error that comes after a satisfied limit is never reported.
* **Memory.** Decoded bytes in flight are at most `window x` the largest task by construction (`peak_inflight_bytes <= window * largest_task_bytes`, asserted); resident memory is bounded by
  `base + K x window x M + F` with `M` the decoded size of the largest row group, `K = 3` (the batch being filtered and its copy, allocator slack) and `F = 32 MiB`; K and F are empirical (see the
  record `bench/results/20261106-scan-parallel.md`), checked by `crates/nata/tests/resource_bounds.rs` with an unbounded control.
* **Handles.** At most one per worker thread plus the footer being read (`peak_open_files`), whatever the number of files.

**Partial output on an error.** A data error inside a row group is found when that row group is read; the rows before it have been printed, the process exits non-zero and says which file and row group.
A task never returns rows of the row group that failed.

## Commands

```sh
cargo test -p nata-parquet                       # debug; needs the duckdb command line for the differential files (they print SKIPPED without it), uvx + pyarrow optional
cargo test --release -p nata-parquet             # release (overflow checks on for our code, off for dependencies, as in the workspace)
NATA_REQUIRE_DUCKDB=1 NATA_REQUIRE_PYARROW=1 cargo test -p nata-parquet   # a missing DuckDB or pyarrow is an error, not a skip
scripts/local-heavy.sh crates/nata-parquet/run_thread_matrix.sh      # the DuckDB differential tests at threads 1,2,3,4,8, ordered and unordered (about 20 minutes)
NATA_SCENARIOS=50000 cargo test --release -p nata-parquet --test scenarios -- --nocapture      # more random cancel/error scenarios (default 5000, 12 s)
NATA_FUZZ_CASES=20000 cargo test --release -p nata-parquet --test fuzz_files -- --nocapture    # a larger byte-flip/truncation fuzz (default 3000 per shape x 5 shapes, 30 s)
cargo test --release -p nata --test resource_bounds -- --nocapture                            # 10,000 files under ulimit -n 256, and the RSS bound (10 s)
python3 crates/nata-parquet/mutate.py --jobs 3 --out /tmp/mutants-parquet.txt    # the mutants (111: 95 of step 2a + 16 `par-*` of step 2b); every line must say CAUGHT
python3 crates/nata-parquet/mutate.py --list     # the mutant names;  --dry: only check that each one compiles
```

| Test file | What it shows |
|---|---|
| `src/*` unit tests | filter syntax and literal typing, DuckDB's float and decimal order, calendar arithmetic, CSV text rules, redaction of error text |
| `tests/adapter.rs` | each rule of the adapter on hand-made column metadata: deprecated fields, column order, exactness flags, every decimal storage form, date sentinels, millisecond timestamps, unmappable types, missing statistics |
| `tests/differential_lineitem.rs` | TPC-H `lineitem` SF0.01 sorted by `l_shipdate` (30 row groups), written by DuckDB, parquet-rs (14 styles: plain/dictionary/delta/byte-stream-split, snappy/zstd/gzip/lz4/none, page v1/v2, truncated statistics) and pyarrow (5 variants); projections x filters, compared IN ORDER with DuckDB, pruned and unpruned, with the skip count asserted (nonzero where designed to prune, zero where designed not to) |
| `tests/differential_edge.rs` | NaN, -0.0, infinities, an all-NULL group, NULLs in some groups only, decimals of every storage width, Unicode and truncated text, every integer width, unsigned, dates, timestamps, booleans, bytes with the high bit set: 4 row groups each, 3 writers |
| `tests/special_files.rs` | empty and one-row files, zero row groups, all-NULL columns of every type, every type in one file, the CSV text of all 256 byte values and 100,000 floats, timestamps in milliseconds, the known difference of exact decimal comparison |
| `tests/soundness_files.rs` | PROPERTY: random files and predicates, pruned scan = unpruned scan = DuckDB (count, sum of ids, sum of squares), with a check that many predicates do prune |
| `tests/report.rs` | the report (counts, bytes equal to the footer's chunk sizes), file order, `--limit`, tasks run independently from other threads, schema differences, usage errors |
| `tests/errors.rs` | truncation at every interesting offset, garbage footers, cut column chunks, a damaged row group, 600 random byte flips and zeroed ranges: no panic, no hang, the error names the file (and the row group), no cell value in any message; types outside the card |
| `tests/overflow_config.rs` | ADR 0003 decision 6: dependencies are built without overflow checks (the arrow-data panic on a dictionary key under a NULL slot), our code with them |
| `tests/parallel.rs` | the parallel scan against the serial one on generated files: byte-identical ordered output for threads 1..8 and windows, the same multiset unordered, an exact `--limit`, lazy footers, the lowest error with injected faults, a closed consumer, 400 small files with bounded handles and footers, a slow task stopped at a batch boundary |
| `tests/scenarios.rs` | 5,000 random scenarios with a printed seed (limit, closed consumer, external cancel, I/O error, panic, delays): nothing left alive (handles, footers, bytes, threads), output a prefix of the serial output, the serial scan's error |
| `tests/fuzz_files.rs` | 3,000 byte-flip/truncation/zero/random-range cases per file shape (5 shapes) at 1 and 4 threads: no hang, no leak, errors name the file and carry no cell value, 1 and 4 threads agree; outcomes classified |
| `tests/common/faults.rs` | the fault-injecting, handle-counting file opener (`Tracked` pattern) |
| `crates/nata/tests/resource_bounds.rs` | 10,000 small files under `ulimit -n 256` with 1 and 8 threads; resident memory under the bound with a slow consumer, and over it with an unbounded window |
| `mutate.py` | ~111 mutants of the code above, each ONE text edit on a private copy; CAUGHT / SURVIVED / INVALID |

Environment variables: `NATA_DUCKDB` (path of the `duckdb` command line), `NATA_REQUIRE_DUCKDB`, `NATA_REQUIRE_PYARROW`, `NATA_TPCH_DIR` (directory with `lineitem.parquet` of SF0.01; the tests run
`scripts/gen-tpch.sh` if it is missing), `NATA_SCAN_FILES` / `NATA_SCAN_PREDICATES` / `NATA_SCAN_SEED` (the soundness property: default 30 files x 40 predicates), `NATA_ERROR_CASES` / `NATA_ERROR_SEED`
(the damaged-file sample: default 600), `NATA_SCAN_THREADS` / `NATA_SCAN_UNORDERED` (run the shared test helper `scan` through `run_parallel`: see `run_thread_matrix.sh`), `NATA_SCENARIOS` / `NATA_SCENARIO_SEED` /
`NATA_SCENARIO_ONLY`, `NATA_FUZZ_CASES` / `NATA_FUZZ_SEED` / `NATA_FUZZ_ONLY=<shape>:<case>`, `NATA_FILES` / `NATA_BIG_GROUPS` (resource tests), `NATA_DISCOVER=1` (the edge test prints every case with its skip count instead of asserting it).

## Known differences from DuckDB 2.0.0 (listed, not hidden)

1. **Exact decimal comparison.** DuckDB fails with a conversion error when a `DECIMAL(38,0)` value of 38 nines is compared with a literal of scale 1 (`d > 99.9`: it needs `DECIMAL(38,1)`).
   `docs/decimal-rules.md` section 5 defines the comparison as exact, and so does the scan. Pinned both ways by `known_difference_exact_decimal_comparison_where_duckdb_errors`.
2. **DuckDB's own row-group skipping is unsound for NaN.** On files written by pyarrow (whose statistics leave NaN out, as the Parquet format asks) DuckDB 2.0.0 answers `d > 5000`, `d >= 5000`,
   `d = 'NaN'`, `d >= 'NaN'`, `d > 'inf'` without the NaN rows, because it trusts the maximum. The tests therefore take DuckDB's answer with a `LIMIT` barrier that disables the pushdown, and count the
   differences (`DUCKDB-PUSHDOWN-DIFFERENCE` lines of the test output). The scan never skips a row group on such a term (rule 10 of `nata-scan`).
3. **Float text.** Both texts read back to the same number; in about 4% of random `FLOAT` values and 0.007% of `DOUBLE` values DuckDB prints other digits than the shortest ones (see `src/csv.rs`).
   Use `-- tolerance:` in `nata-check` queries over float columns.
4. Timestamps with a time zone and nanosecond timestamps are `not supported yet` (DuckDB reads them).
