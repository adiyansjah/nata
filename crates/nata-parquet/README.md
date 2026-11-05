# nata-parquet

The Parquet scan of slice 3 (`docs/slices/0001-parquet-scan.md`), **single-thread step (2a)**: a plan over a list of Parquet files (footers read, row groups pruned by their statistics), one
independent task per kept row group (the morsel of ADR 0001), an exact row filter with DuckDB's semantics, and DuckDB-compatible CSV output for the developer command `nata scan`.
Parallel morsels, `LIMIT` cancellation, memory and file-handle bounds, the fuzz of the card and the cloud benchmarks are later steps; the task interface is built so that they can be added
without changing it (`ScanPlan::tasks()` yields `RowGroupTask`s, each of which reads one row group with its own file handle and shares nothing mutable).

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

`nata scan FILE... [--columns a,b] [--filter 'col op literal' ...] [--limit N] --csv [--no-prune]`. stdout: the CSV in DuckDB's `-csv` text (the format `nata-check` compares); stderr: one report line,
`scan: files=1 row_groups=30 skipped=25 read=5 not_read=0 rows_read=10240 rows_returned=9484 bytes_read=965772 columns_read=16 morsel=row-group pruning=applied(2 of 2 terms)`.
`bytes_read` is the number of bytes requested from the file for the column chunks of the row groups read (counted at the file handle, and equal to the sum of the chunk sizes in the footer; the
footer itself is not counted). Filtered batches are COMPACTED (arrow `filter_record_batch`; dictionary columns keep their dictionary); a batch where every row matches is passed through; a batch with no
match is dropped. Exit codes: 0, 1 (file or data error), 2 (`not supported yet`, wrong use).

**Partial output on an error.** Footers of all files are read before any row is printed (so a corrupt or mismatching file is an error with nothing on stdout). A data error inside a row group is found
when that row group is read: the rows of the earlier row groups have already been printed, the process exits non-zero and says which file and row group. A task never returns rows of the row group that failed.

## Commands

```sh
cargo test -p nata-parquet                       # debug; needs the duckdb command line for the differential files (they print SKIPPED without it), uvx + pyarrow optional
cargo test --release -p nata-parquet             # release (overflow checks on for our code, off for dependencies, as in the workspace)
NATA_REQUIRE_DUCKDB=1 NATA_REQUIRE_PYARROW=1 cargo test -p nata-parquet   # a missing DuckDB or pyarrow is an error, not a skip
python3 crates/nata-parquet/mutate.py --jobs 4 --out /tmp/mutants-parquet.txt   # the mutants (about 95); every line must say CAUGHT
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
| `mutate.py` | ~95 mutants of the code above, each ONE text edit on a private copy; CAUGHT / SURVIVED / INVALID |

Environment variables: `NATA_DUCKDB` (path of the `duckdb` command line), `NATA_REQUIRE_DUCKDB`, `NATA_REQUIRE_PYARROW`, `NATA_TPCH_DIR` (directory with `lineitem.parquet` of SF0.01; the tests run
`scripts/gen-tpch.sh` if it is missing), `NATA_SCAN_FILES` / `NATA_SCAN_PREDICATES` / `NATA_SCAN_SEED` (the soundness property: default 30 files x 40 predicates), `NATA_ERROR_CASES` / `NATA_ERROR_SEED`
(the damaged-file sample: default 600), `NATA_DISCOVER=1` (the edge test prints every case with its skip count instead of asserting it).

## Known differences from DuckDB 2.0.0 (listed, not hidden)

1. **Exact decimal comparison.** DuckDB fails with a conversion error when a `DECIMAL(38,0)` value of 38 nines is compared with a literal of scale 1 (`d > 99.9`: it needs `DECIMAL(38,1)`).
   `docs/decimal-rules.md` section 5 defines the comparison as exact, and so does the scan. Pinned both ways by `known_difference_exact_decimal_comparison_where_duckdb_errors`.
2. **DuckDB's own row-group skipping is unsound for NaN.** On files written by pyarrow (whose statistics leave NaN out, as the Parquet format asks) DuckDB 2.0.0 answers `d > 5000`, `d >= 5000`,
   `d = 'NaN'`, `d >= 'NaN'`, `d > 'inf'` without the NaN rows, because it trusts the maximum. The tests therefore take DuckDB's answer with a `LIMIT` barrier that disables the pushdown, and count the
   differences (`DUCKDB-PUSHDOWN-DIFFERENCE` lines of the test output). The scan never skips a row group on such a term (rule 10 of `nata-scan`).
3. **Float text.** Both texts read back to the same number; in about 4% of random `FLOAT` values and 0.007% of `DOUBLE` values DuckDB prints other digits than the shortest ones (see `src/csv.rs`).
   Use `-- tolerance:` in `nata-check` queries over float columns.
4. Timestamps with a time zone and nanosecond timestamps are `not supported yet` (DuckDB reads them).
