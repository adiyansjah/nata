# Parquet scan, step 2a: single-thread reader, footer adapter, exact row filter, `nata scan` (slice 3)

Date: 2026-11-06 (work of 2026-11-05/06). Code: `crates/nata-parquet` (new), `crates/nata` (`scan` subcommand), `crates/nata-check` (`-- scan:` directive), `check/queries/scan/`. Card: `docs/slices/0001-parquet-scan.md`.
Local only: no GCP, no VM, no push. Machine: x86-64 Linux, 8 cores, DuckDB 2.0.0 CLI, `parquet`/`arrow` 60.1.0 (the versions of the baseline spike), pyarrow 26.0.1 through `uvx --offline --with pyarrow` (plain `python3` has no pyarrow).

## What was built

- **Adapter** (`src/adapter.rs`): the `parquet` crate's row-group column metadata to `nata-scan`'s statistics. `min_value`/`max_value` only (deprecated `min`/`max` never; `is_min_max_deprecated()`), column order must be `TYPE_DEFINED_ORDER` with the sort order the type needs, exactness only from the footer's flags, no NaN count, no bounds for INT96 / unsigned / FLOAT16 / nested / time zones / nanoseconds / DECIMAL above 38 digits / non-UTF-8 text bounds; decimals from INT32, INT64, FIXED_LEN_BYTE_ARRAY and BYTE_ARRAY (two's complement, any length that fits 128 bits); millisecond timestamps converted with checked arithmetic.
- **Filter** (`src/filter.rs`): `col op literal` parser, literals typed from the column (errors name the column, say what is wrong, never guess), and the exact row filter with DuckDB semantics. Dictionary columns are compared without expanding (predicate once per dictionary entry).
- **Reader** (`src/reader.rs`): `ScanPlan::open` (footers, schema checks across files, filter typing, pruning decisions) and `RowGroupTask::run` (one row group, own file handle, nothing shared mutable, `Send`). `ArrowReaderOptions::with_schema` gives `Dictionary(Int32, Utf8)` strings and `Decimal64` decimals (ADR 0003, checked on every batch: the reader returns an error if another type came out). Filtered batches are COMPACTED. `bytes_read` is counted at the file handle.
- **`nata scan FILE... [--columns a,b] [--filter 'col op literal' ...] [--limit N] --csv [--no-prune]`**: CSV on stdout in DuckDB's `-csv` text, one report line on stderr. **`nata-check`** has the `-- scan: <args>` directive (candidate runs `nata scan <args> --csv`; all earlier behaviour and tests unchanged; 5 queries in `check/queries/scan` pass against DuckDB).
- **Build configuration** (ADR 0003 decision 6, which says it, but the workspace did not yet do it): dependencies are built WITHOUT overflow checks (`[profile.{release,test,dev}.package."*"]` in the root `Cargo.toml`), our crates with them. Reason: arrow-data panics on `i32::MAX` under a NULL slot in dictionary `concat` when its own overflow checks are on; the scan returns dictionary arrays. `tests/overflow_config.rs` holds both facts (the same two tests as `spikes/hash-ops`), and a mutant that puts the checks back on is caught. The `test` and `dev` profiles also get `opt-level = 2` for dependencies and for `nata-parquet` (debug-speed Arrow made the differential tests take 3 times as long).

## Exact semantics

| Topic | Rule | Checked by |
|---|---|---|
| NULL | a comparison with NULL is never true (also `<>`); `IS NULL` / `IS NOT NULL` look at validity | every differential file, 113 all-NULL cases |
| Float order | NaN = NaN, NaN above every number and infinity, -0.0 = +0.0 | `floats`, `floats_nonull` files (950 cases), 12,000 random predicates, mutants |
| Float pruning | Parquet has no NaN count, so float `>`, `>=`, `<>` are never pruned by a maximum; `<`, `<=`, `=` may be | designed cases `d > 5000` skip exactly 1 (the all-NULL group) on all three writers |
| Decimals | exact across scales, the literal is never rounded; if rescaling overflows 128 bits the sign decides | 2,104 decimal cases (every storage width, literals with more and fewer digits), mutants |
| Integers | 128-bit compare, a literal is never cast to the column's width (`i8 = 300` is empty, `i32 = 4294967296` is not 0) | 2,331 integer-family cases |
| Unsigned | read and filtered, never pruned | skip count asserted zero |
| Text / bytes | unsigned bytewise; truncated statistics handled by `nata-scan` | Unicode file, all 255 byte values, parquet-rs `truncate4` style |
| Timestamps | without time zone, ms or us; ms compared in us | pyarrow `ms` variant, parquet-rs ms file |
| Statistics | only `min_value`/`max_value`, only the expected type-defined order, exactness only from the footer's flag | `tests/adapter.rs` (15 tests) |
| Output | DuckDB's CSV text; floats read back to the same bits (text identical for 99.993% of doubles, 96.12% of floats, see the differences below) | 100,017 rows, all 128 ASCII characters, all 256 byte values |
| Errors | name the file and, where known, the row group; no cell value (text between quotes is dropped from library messages); a panic is caught and reported without its message | `tests/errors.rs` |

## Differential matrix (against DuckDB 2.0.0, compared IN ORDER, each case run pruned and unpruned)

Files: TPC-H `lineitem` SF0.01 (60,175 rows) sorted by `l_shipdate`, 2,048-row groups (30 row groups), written by DuckDB, by parquet-rs in 14 styles (plain / dictionary / delta-length / delta-byte-array / byte-stream-split; none, snappy, zstd, gzip, lz4-raw; data page v1 and v2; small pages; statistics truncated to 4 bytes) and by pyarrow in 5 variants (snappy, zstd v2 pages, delta + gzip, decimals as integers + lz4, timestamps in ms). Seven edge files (4 row groups each) written by DuckDB, five parquet-rs styles and 1 to 3 pyarrow variants.

| Test | Cases compared with DuckDB |
|---|---|
| `lineitem_written_by_duckdb` (6 projections x 52 filters, all operators, ANDs, the card's year range, filter columns not projected) | 438 |
| `lineitem_written_by_parquet_rs_in_every_style` (4 cases x 14 styles + the matrix on 3 styles) | 350 |
| `lineitem_written_by_pyarrow` | 495 |
| `filter_columns_that_are_not_projected` | 6 |
| edge: floats / floats without an all-NULL group / decimals / strings / NULLs in some groups / ints, dates, timestamps, booleans / blobs | 824 / 126 / 2,104 / 1,432 / 434 / 2,331 / 469 |
| special: empty and one-row files, zero row groups / all-NULL columns of 14 types / every type / ms timestamps / ASCII + bytes | 20 / 113 / 34 / 46 / 1 |
| **Total compared with DuckDB** | **about 9,260 cases** (each checked pruned AND unpruned, row for row) |
| `soundness_files` (random files and predicates; count, sum and sum of squares of ids) | 1,200 in the default run; **12,000 over 300 files (seed 777): 0 differences, 8,962 predicates skipped at least one row group, 19,601 of 42,240 row groups skipped** |
| float text, random bit patterns | 100,017 rows x 2 columns, every value identical, text identical for 99.993% (double) and 96.12% (float) |

Skip counts are asserted in every designed case: nonzero where the case is designed to prune (the year range skips at least 20 of 30 row groups on all three writers), exactly zero where it is designed not to (NaN-blind `>` on a float column, a range that contains the whole column, unsigned columns, `<>` on a non-constant group, ranges wider than the data). Where writers differ the expectation is per writer and written in the test (float bounds are unusable for parquet-rs 60, which writes the IEEE total order; DuckDB, parquet-rs and pyarrow all write exactness flags, so a constant group is skipped for `<>`).

## Known differences from DuckDB 2.0.0 (also in the crate README; the card has no known-differences section)

1. **Exact decimal comparison** (decimal-rules section 5): DuckDB fails with a conversion error when a `DECIMAL(38,0)` value of 38 nines meets a literal of scale 1 (`d > 99.9`: `Casting value ... to type DECIMAL(38,1) failed: value is out of range`). The scan is exact. `known_difference_exact_decimal_comparison_where_duckdb_errors` pins both sides; the differential tests do not compare such literals.
2. **NEW, found by this step: DuckDB's own row-group skipping is unsound for NaN.** On files from pyarrow (statistics leave NaN out, as the format asks) DuckDB answers `d > 5000`, `d >= 5000`, `d >= inf`, `d = 'NaN'`, `d >= 'NaN'`, `d > 'inf'` without the 205 NaN rows (for example 512 rows instead of 717), because it trusts the maximum. The same query over DuckDB's own file or with a `LIMIT` barrier (which stops the pushdown) is right. The tests use the barrier as the oracle and print every difference (`DUCKDB-PUSHDOWN-DIFFERENCE`; 25 lines per run). This is exactly the soundness hole of rule 10 of `nata-scan`; the scan does not have it. Not reported upstream.
3. **Float text.** Both texts read back to the same number; where two shortest strings are equally close DuckDB takes the even digit (`...721.2`, Rust `...721.3`), and many `FLOAT` values of magnitude above ~1e6 print with more digits in DuckDB (`160227392.0`; the shortest is `160227400.0`). I did not find DuckDB's rule (the exact 9-digit form is printed sometimes and the shortest other times) and implemented shortest digits. Consequence: `nata-check` queries over float columns need `-- tolerance:` (and for FLOAT a relative 1e-7 or more, because `nata-check` compares as f64). NaN with the sign bit set prints `-nan` in both.
4. `TIMESTAMP WITH TIME ZONE`, nanosecond timestamps, nested columns, FLOAT16, TIME, UUID, INT96, DECIMAL above 38 digits and encrypted files answer `not supported yet: <what>` (DuckDB reads most of them).

## Fault tolerance and security (`tests/errors.rs`, `tests/overflow_config.rs`)

- Every proper prefix of a valid file tried (21 fixed cuts and 60 random ones): an error naming the file, at open, with nothing returned. 14 footer and magic cases (garbage, zero, reversed, huge or short length field, wrong magic at either end, empty file, text file, a CSV file, `PARE` = encrypted: `not supported yet: Parquet encryption`).
- Column chunks cut out of the middle with the footer kept: errors in several row groups, each naming the row group that was damaged. A damaged row group in the middle: error naming file and row group 1, only row group 0 reached the sink, row group 2 never read.
- 600 random single-bit flips, random bytes, zeroed ranges and footer-region inversions of a 3-row-group file (seed fixed, `NATA_ERROR_CASES`): 252 errors, 348 scans that finished (a flipped value byte in an unchecksummed page decodes to another valid value, as the card says), 0 panics (no `Internal` error), 0 hangs (30 s watchdog), and no message contained the planted marker value that is in every row of the text column.
- Found and fixed by these tests: `parquet-rs` does not check the leading magic number (a file with a damaged header still decoded), so the reader checks both magic numbers itself.
- Types outside the card: nested, nanosecond, time-zone, TIME, DECIMAL(50,2) files scan fine when the unsupported column is not touched (it is not even read: `columns_read=1`) and fail with `not supported yet: column "x": ...` naming the file when it is projected, filtered or part of the default projection.
- `unsafe` stays forbidden (workspace lint). `cargo geiger` of the `parquet` dependency was NOT run in this step.

## Mutants of the new code (`crates/nata-parquet/mutate.py`, release build, private copies, fast groups first)

95 mutants, each ONE exact text edit (two or three edits for a few). All 95 CAUGHT in the end. The honest history: the first full run (4 jobs, 2,420 s) gave 90 CAUGHT and 5 SURVIVED; the survivors were `filter-text-compared-as-signed-bytes` and `filter-unsigned-64-compared-as-signed` (the plain-string and `UInt64` arms of the row filter are dead code in the scan, which only ever sees dictionary strings and the arms were untested: unit tests on batches added), `reader-row-group-numbers-off-by-one` (the error path of `build()` was never reached by a file: `parquet_error` unit-tested), `reader-no-error-on-wrong-types` (the type guard was inline: made a function and unit-tested) and `reader-limit-does-not-stop-between-row-groups`, which is an EQUIVALENT mutant (the next batch stops the scan one step later with the same output) and was removed from the list. One mutant (`filter-dictionary-text-compared-as-signed-bytes`) was added. The 90 caught in the first run ran against the test set of that moment, which only grew afterwards; the 6 changed or added ones were re-run on the final tests. The final full list was not re-run in one go (about 40 minutes). No INVALID mutant remains (`--dry`: 95 of 95 compile).

| Mutant | Result | First failing test group and test |
|---|---|---|
| `adapter-reads-deprecated-min-max` | CAUGHT | (--test adapter) deprecated_min_max_are_never_used |
| `adapter-ignores-column-order` | CAUGHT | (--test adapter) only_the_expected_type_defined_order_gives_bounds |
| `adapter-trusts-ieee-total-order` | CAUGHT | (--lib) adapter::tests::only_the_expected_type_defined_order_is_usable |
| `adapter-sort-order-not-compared` | CAUGHT | (--lib) adapter::tests::only_the_expected_type_defined_order_is_usable |
| `adapter-inexact-bound-taken-as-exact` | CAUGHT | (--test adapter) a_missing_exactness_flag_means_not_exact, an_exactness_flag_per_side |
| `adapter-text-bound-inexact-taken-as-exact` | CAUGHT | (--test adapter) a_missing_exactness_flag_means_not_exact, an_exactness_flag_per_side |
| `adapter-millisecond-timestamps-not-converted` | CAUGHT | (--test adapter) dates_and_timestamps |
| `adapter-millisecond-conversion-wraps` | CAUGHT | (--test adapter) dates_and_timestamps |
| `adapter-invents-a-nan-count` | CAUGHT | (--test adapter) nan_count_is_never_invented, only_the_expected_type_defined_order_gives_bounds |
| `adapter-missing-null-count-as-zero` | CAUGHT | (--test adapter) a_missing_null_count_is_not_zero |
| `adapter-decimal-sign-extension-lost` | CAUGHT | (--lib) adapter::tests::twos_complement_decoding |
| `adapter-date-sentinels-trusted` | CAUGHT | (--test adapter) dates_and_timestamps |
| `adapter-fixed-length-bound-of-wrong-width-trusted` | CAUGHT | (--test adapter) decimals_from_every_storage_form |
| `adapter-negative-row-count-accepted` | CAUGHT | (--test adapter) row_group_stats_of_a_real_schema_and_a_negative_row_count |
| `adapter-column-order-list-indexed-unchecked` | CAUGHT | (--test adapter) the_column_order_of_a_short_or_missing_list_is_undefined_and_never_panics |
| `adapter-statistics-of-the-next-column` | CAUGHT | (--test adapter) row_group_stats_of_a_real_schema_and_a_negative_row_count |
| `schema-unsigned-integers-pruned-as-signed` | CAUGHT | (--lib) schema::tests::unsigned_integers_are_never_pruned |
| `schema-nested-column-accepted` | CAUGHT | (--test errors) types_outside_the_card_are_not_supported_yet_and_only_when_touched |
| `schema-time-zone-timestamps-accepted` | CAUGHT | (--test adapter) types_that_cannot_be_mapped_soundly_have_no_bounds |
| `schema-nanosecond-timestamps-as-micros` | CAUGHT | (--test adapter) types_that_cannot_be_mapped_soundly_have_no_bounds |
| `parse-ne-as-eq` | CAUGHT | (--lib) filter::tests::syntax_accepts_the_documented_forms |
| `parse-le-after-lt` | CAUGHT | (--lib) filter::tests::syntax_accepts_the_documented_forms |
| `parse-string-without-quotes-accepted` | CAUGHT | (--lib) filter::tests::literals_are_typed_from_the_column_and_nothing_is_guessed |
| `parse-integer-fraction-rounded` | CAUGHT | (--lib) filter::tests::literals_are_typed_from_the_column_and_nothing_is_guessed |
| `parse-decimal-literal-rounded-to-two-digits` | CAUGHT | (--lib) filter::tests::literals_are_typed_from_the_column_and_nothing_is_guessed |
| `parse-float-overflow-accepted` | CAUGHT | (--lib) filter::tests::literals_are_typed_from_the_column_and_nothing_is_guessed |
| `parse-timestamp-fraction-rounded` | CAUGHT | (--lib) civil::tests::dates_and_timestamps_have_duckdbs_text |
| `parse-date-day-not-validated` | CAUGHT | (--lib) civil::tests::dates_round_trip_over_the_whole_supported_range, filter::tests::literals_are_typed_from_the_column_and_nothing_is_guessed |
| `parse-date-leap-rule-wrong` | CAUGHT | (--lib) civil::tests::dates_round_trip_over_the_whole_supported_range |
| `filter-nan-not-equal-to-itself` | CAUGHT | (--lib) filter::tests::float_order_is_duckdbs |
| `filter-nan-below-numbers` | CAUGHT | (--lib) filter::tests::float_order_is_duckdbs |
| `filter-nan-not-above-infinity` | CAUGHT | (--lib) filter::tests::float_order_is_duckdbs |
| `filter-negative-zero-below-positive-zero` | CAUGHT | (--lib) filter::tests::float_order_is_duckdbs |
| `filter-decimal-literal-rounded-to-the-column-scale` | CAUGHT | (--test soundness_files) pruned_equals_unpruned_equals_duckdb_on_random_files_and_predicates |
| `filter-decimal128-literal-rounded-to-the-column-scale` | CAUGHT | (--test special_files) known_difference_exact_decimal_comparison_where_duckdb_errors |
| `filter-decimal-overflow-sign-flipped` | CAUGHT | (--lib) filter::tests::decimals_compare_exactly_across_scales |
| `filter-decimal-rescale-wraps` | CAUGHT | (--lib) filter::tests::decimals_compare_exactly_across_scales |
| `filter-null-rows-compare-true` | CAUGHT | (--test special_files) all_null_columns_of_every_type, csv_text_of_every_ascii_character_and_every_byte, known_difference_exact_decimal_comparison_where_duckdb_errors |
| `filter-is-null-inverted` | CAUGHT | (--test special_files) all_null_columns_of_every_type, empty_and_one_row_files, every_type_in_one_file |
| `filter-is-not-null-keeps-nulls` | CAUGHT | (--test special_files) all_null_columns_of_every_type |
| `filter-text-compared-as-signed-bytes` | CAUGHT | (--lib) filter::tests::plain_text_binary_and_unsigned_columns_use_unsigned_bytes_and_unsigned_numbers |
| `filter-binary-compared-as-signed-bytes` | CAUGHT | (--test special_files) csv_text_of_every_ascii_character_and_every_byte, every_type_in_one_file |
| `filter-dictionary-text-compared-as-signed-bytes` | CAUGHT | (--test special_files) csv_text_of_every_ascii_character_and_every_byte |
| `filter-dictionary-table-one-short` | CAUGHT | (--test special_files) csv_text_of_every_ascii_character_and_every_byte, empty_and_one_row_files |
| `filter-dictionary-key-under-null-slot-used` | CAUGHT | (--test special_files) all_null_columns_of_every_type |
| `filter-terms-or-ed` | CAUGHT | (--test report) limit_stops_early_and_reports_what_was_not_read, tasks_are_independent_row_group_morsels, the_file_list_is_scanned_in_the_order_given, the_report_counts_files_row_groups_rows_and_bytes |
| `filter-int-literal-cast-to-32-bits` | CAUGHT | (--test soundness_files) pruned_equals_unpruned_equals_duckdb_on_random_files_and_predicates |
| `filter-int8-literal-cast-to-8-bits` | CAUGHT | (--test differential_edge) integers_dates_timestamps_booleans |
| `filter-unsigned-64-compared-as-signed` | CAUGHT | (--lib) filter::tests::plain_text_binary_and_unsigned_columns_use_unsigned_bytes_and_unsigned_numbers |
| `filter-millisecond-timestamps-not-scaled` | CAUGHT | (--test special_files) timestamps_in_milliseconds |
| `filter-float32-compared-against-narrowed-literal` | CAUGHT | (--test differential_edge) floats_nan_zeros_infinities_and_an_all_null_group |
| `filter-boolean-order-reversed` | CAUGHT | (--test soundness_files) pruned_equals_unpruned_equals_duckdb_on_random_files_and_predicates |
| `reader-pruning-never-applied` | CAUGHT | (--test report) limit_stops_early_and_reports_what_was_not_read, tasks_are_independent_row_group_morsels, the_report_counts_files_row_groups_rows_and_bytes |
| `reader-second-term-not-used-for-pruning` | CAUGHT | (--test differential_lineitem) lineitem_written_by_duckdb, lineitem_written_by_parquet_rs_in_every_style, lineitem_written_by_pyarrow |
| `reader-skipped-row-groups-are-read` | CAUGHT | (--test report) limit_stops_early_and_reports_what_was_not_read, tasks_are_independent_row_group_morsels, the_report_counts_files_row_groups_rows_and_bytes |
| `reader-skips-the-last-row-group` | CAUGHT | (--test report) limit_stops_early_and_reports_what_was_not_read, the_file_list_is_scanned_in_the_order_given, the_report_counts_files_row_groups_rows_and_bytes |
| `reader-skips-the-first-row-group` | CAUGHT | (--test report) limit_stops_early_and_reports_what_was_not_read, tasks_are_independent_row_group_morsels, the_file_list_is_scanned_in_the_order_given, the_report_counts_files_row_groups_rows_and_bytes |
| `reader-row-groups-in-reverse-order` | CAUGHT | (--test report) limit_stops_early_and_reports_what_was_not_read, tasks_are_independent_row_group_morsels, the_file_list_is_scanned_in_the_order_given |
| `reader-reads-the-next-row-group` | CAUGHT | (--test report) limit_stops_early_and_reports_what_was_not_read, tasks_are_independent_row_group_morsels, the_file_list_is_scanned_in_the_order_given, the_report_counts_files_row_groups_rows_and_bytes |
| `reader-row-group-numbers-off-by-one` | CAUGHT | (--lib) reader::tests::parquet_errors_name_the_file_and_the_row_group_and_keep_no_quoted_text |
| `reader-files-sorted-by-name` | CAUGHT | (--test report) the_file_list_is_scanned_in_the_order_given |
| `reader-repeated-file-scanned-once` | CAUGHT | (--test report) the_file_list_is_scanned_in_the_order_given |
| `reader-filter-column-not-read-unless-projected` | CAUGHT | (--test report) the_report_counts_files_row_groups_rows_and_bytes |
| `reader-projects-before-filtering` | CAUGHT | (--test special_files) all_null_columns_of_every_type, csv_text_of_every_ascii_character_and_every_byte, empty_and_one_row_files, every_type_in_one_file |
| `reader-later-files-not-checked-against-the-first` | CAUGHT | (--test report) schema_differences_between_files_name_the_file |
| `reader-magic-numbers-not-checked` | CAUGHT | (--test errors) garbage_footers_and_garbage_files |
| `reader-no-error-on-wrong-types` | CAUGHT | (--lib) reader::tests::expanded_strings_are_not_the_requested_dictionary_type |
| `reader-limit-one-too-many` | CAUGHT | (--test report) limit_stops_early_and_reports_what_was_not_read |
| `reader-limit-zero-still-reads` | CAUGHT | (--test report) limit_stops_early_and_reports_what_was_not_read |
| `report-skipped-counted-twice` | CAUGHT | (--test report) limit_stops_early_and_reports_what_was_not_read, the_report_counts_files_row_groups_rows_and_bytes |
| `report-total-counts-only-kept-row-groups` | CAUGHT | (--test report) the_report_counts_files_row_groups_rows_and_bytes |
| `report-read-count-not-incremented` | CAUGHT | (--test report) limit_stops_early_and_reports_what_was_not_read, the_report_counts_files_row_groups_rows_and_bytes |
| `report-bytes-not-added` | CAUGHT | (--test report) tasks_are_independent_row_group_morsels, the_report_counts_files_row_groups_rows_and_bytes |
| `report-bytes-of-chunk-reads-not-counted` | CAUGHT | (--test report) the_report_counts_files_row_groups_rows_and_bytes |
| `report-rows-read-counts-returned` | CAUGHT | (--test report) limit_stops_early_and_reports_what_was_not_read, tasks_are_independent_row_group_morsels, the_file_list_is_scanned_in_the_order_given, the_report_counts_files_row_groups_rows_and_bytes |
| `report-pruning-flag-always-applied` | CAUGHT | (--test report) the_report_counts_files_row_groups_rows_and_bytes |
| `report-not-read-counts-skipped-too` | CAUGHT | (--test report) the_report_counts_files_row_groups_rows_and_bytes |
| `csv-null-printed-empty` | CAUGHT | (--test special_files) all_null_columns_of_every_type, csv_text_of_every_ascii_character_and_every_byte, every_type_in_one_file |
| `csv-delete-character-not-quoted` | CAUGHT | (--lib) csv::tests::fields_are_quoted_like_duckdb |
| `csv-non-ascii-not-quoted` | CAUGHT | (--lib) csv::tests::fields_are_quoted_like_duckdb |
| `csv-embedded-quote-not-doubled` | CAUGHT | (--lib) csv::tests::fields_are_quoted_like_duckdb |
| `csv-float-positional-threshold-moved` | CAUGHT | (--lib) csv::tests::floats_have_duckdbs_text |
| `csv-negative-nan-printed-positive` | CAUGHT | (--lib) csv::tests::floats_have_duckdbs_text |
| `csv-negative-zero-printed-positive` | CAUGHT | (--lib) csv::tests::floats_have_duckdbs_text |
| `csv-decimal-scale-dropped` | CAUGHT | (--lib) csv::tests::decimals_dates_and_blobs |
| `csv-blob-quote-not-escaped` | CAUGHT | (--lib) csv::tests::decimals_dates_and_blobs |
| `csv-blob-escape-lowercase` | CAUGHT | (--lib) csv::tests::decimals_dates_and_blobs |
| `csv-timestamp-fraction-zeros-kept` | CAUGHT | (--lib) civil::tests::dates_and_timestamps_have_duckdbs_text |
| `csv-date-before-year-1-without-bc` | CAUGHT | (--lib) civil::tests::dates_and_timestamps_have_duckdbs_text |
| `error-message-not-redacted` | CAUGHT | (--lib) error::tests::redact_drops_quoted_text_and_control_characters |
| `error-without-row-group` | CAUGHT | (--lib) error::tests::display_names_file_and_row_group |
| `error-without-file-name` | CAUGHT | (--lib) error::tests::display_names_file_and_row_group |
| `error-not-supported-without-the-words` | CAUGHT | (--lib) error::tests::display_names_file_and_row_group |
| `build-dependencies-overflow-checks-on-in-release` | CAUGHT | (--test overflow_config) concat_of_dictionaries_with_an_extreme_key_under_a_null_slot_does_not_panic |
| `build-workspace-overflow-checks-off-in-release` | CAUGHT | (--test overflow_config) our_own_arithmetic_still_panics_on_overflow |

## Test counts and run time

`cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` clean. `cargo test --workspace` (debug) all pass: `nata-parquet` 71 tests (20 unit, 15 adapter, 7 + 4 differential, 8 errors, 3 overflow_config, 6 report, 1 soundness_files, 7 special_files), `nata` 2 + 3 (CLI), `nata-check` 20 + 5 (3 + 2 new), `nata-kernels` and `nata-scan` unchanged and green. Wall time of the whole workspace in debug: 4 min 32 s including a cold build of the dependencies at `opt-level = 2`; the `nata-parquet` test binaries alone take about 135 s (edge differential 88 s, lineitem 31 s, in parallel threads each). Release (`nata-parquet` and `nata-scan`): all pass, 2 min 26 s. DuckDB tests print `SKIPPED` without the `duckdb` command line and fail with `NATA_REQUIRE_DUCKDB=1`; pyarrow tests are skipped without `uvx`, and fail with `NATA_REQUIRE_PYARROW=1`. Slower options behind variables: `NATA_SCAN_FILES` (the 12,000-predicate run took 63 s release), `NATA_ERROR_CASES`.

## Design decisions to review

1. A new crate `nata-parquet` instead of growing `nata-scan` (which stays pure and dependency-free).
2. Only `TYPE_DEFINED_ORDER` is trusted. Consequence: **float statistics written by parquet-rs 60 are never used** (it writes `IEEE_754_TOTAL_ORDER`; two non-NaN bounds under that order would even prove "no NaN rows", which would be a worthwhile later rule).
3. An absent exactness flag means "not exact" (the `parquet` crate cannot tell absent from false); all three writers tested write the flags.
4. Footers of all files are read before the first row is printed (corrupt or mismatching files produce no output at all); a data error inside a row group is found when it is read, so earlier row groups' rows may already be on stdout and the exit status is non-zero. A task never returns rows of a failed row group. A buffered CLI would change this.
5. Filtered batches are compacted (no selection vector is returned yet).
6. Unsupported types are errors only when touched; the default projection touches all.
7. Date and timestamp literals are written `YYYY-MM-DD` / `YYYY-MM-DD HH:MM:SS[.ffffff]` (quotes optional), years 0001 to 9999; a date literal for a timestamp column is refused, not widened.
8. `nata-check` splits the `-- scan:` arguments like a shell without escapes (quotes only).
9. A caught panic is reported as an `Internal` error without its message, and the CLI prints only the location of a panic.

## What this does not show

- **No parallelism** (the task interface is ready; it is exercised from threads in one test, `tasks_are_independent_row_group_morsels`, nothing more), **no `LIMIT` cancellation** (`--limit` just stops reading after N rows, the first row group to satisfy it is read whole), **no memory or file-handle bounds** (a whole row group is decoded and kept before it is returned; 10,000 files under `ulimit -n` untested), **no fuzz at the card's size** (600 cases of one file shape, not 3,000 per shape), **no cloud and no timing** at all (no R1/R2 number, no byte-share measurement on SF1, nothing on ARM64).
- Only the writers actually used: DuckDB 2.0.0, parquet-rs 60.1.0 and pyarrow 26.0.1. No Java-written file (the card says so), no Spark, no Impala; nothing from writers that omit `column_orders` (those files are scanned correctly and never pruned, tested only with hand-made metadata).
- No nested types, no page index, no column index, no bloom filter, no encrypted files (they answer `not supported yet`), no INT96, no row selection inside a row group.
- Dictionary-aware filtering exists only for `Dictionary(Int32, Utf8)` (the one dictionary type the reader produces); other columns are compared on their values.
- The row-count check (rows decoded must equal the footer's count) has no mutant: no test file makes the pages and the footer disagree on the count.
- The float text of `FLOAT` differs from DuckDB's in about 4% of random values (value identical).
- Datasets are SF0.01 and synthetic edge files: all "reads a fifth of the row groups" claims are layout claims on 30 row groups, not SF1.
- The mutant run was not repeated in one piece after the last test additions (see above).
