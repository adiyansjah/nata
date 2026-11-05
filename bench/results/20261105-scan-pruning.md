# Row-group pruning decision (slice 3, implementation step 1): `crates/nata-scan`

Date: 2026-11-05. Card: `docs/slices/0001-parquet-scan.md` (rows Correctness, Fault tolerance, Security; decision rule R2). ADRs relied on: 0005 rules 3 and 5 (pruning is sound; a missing statistic is allowed, a wrong one is not).
Machine: the development machine (Linux x86-64, 8 cores), DuckDB 2.0.0 command line (`v2.0.0 (Cyanoptera) d54cbbe3df`). No cloud was used, no number here is a performance claim.

## What was built

A new workspace member `crates/nata-scan`: a library with no dependencies and no `unsafe` code. `can_skip(row_group_stats, predicate) -> Decision::Skip(reason) | Decision::Keep(reason)`.
Statistics: row count, null count, NaN count (optional), min and max with an `exact` flag, for bool, int32, int64, float32, float64, decimal (unscaled `i128`, precision, scale), date32, timestamp (microseconds), text and binary.
Predicate: an AND of `column <op> literal` (`<`, `<=`, `>`, `>=`, `=`, `<>`), `IS NULL`, `IS NOT NULL`. The contract: Skip only when the statistics prove that no row can match under DuckDB semantics; anything missing, unsound, NaN-affected, truncated beyond use or unsupported is Keep.
The rule table (27 rules, each with the DuckDB check that confirmed its semantics), the type rules and the list of what is not covered are the crate documentation (`crates/nata-scan/src/lib.rs`). This file records the evidence.

## The rules in one paragraph

Compare against the bounds (`x < v` skips when `min >= v`, `x <= v` when `min > v`, `x > v` when `max <= v`, `x >= v` when `max < v`, `x = v` outside `[min, max]`, `x <> v` only when `min = max = v` with both bounds exact);
an empty group is skipped; an all-NULL group (`null_count = rows`) is skipped for every comparison and for `IS NOT NULL`; `IS NULL` is skipped when `null_count = 0` (an unknown count is not zero).
Floats: Parquet leaves NaN out of the bounds, NaN is above every number in DuckDB, so `>`, `>=` and `<>` are kept unless the NaN count is 0 (`max < 5` does NOT prove that `x > 5` has no match), `<`, `<=` and `=` prune as for any number, `x > NaN` is always empty, a bound that is NaN is not trusted, zeros compare as numbers (so `-0.0` and `+0.0` bounds need no widening).
Text and binary: bytewise unsigned order; a truncated maximum is trusted only up to its increment (exclusive), so it is sound against writers that increment and against writers that forget to; a truncated bound never makes a "constant group".
Decimals: exact comparison across scales and against integers, the literal is never rounded to the column's scale, a rescale that does not fit 128 bits is decided by sign. Integer literals of the other width are compared exactly (no cast).

## DuckDB checks: what was confirmed and what was contradicted

`tests/duckdb_oracle.rs` runs two checks against the real DuckDB (2.0.0 command line, found automatically; the test says SKIPPED if it is absent and `NATA_REQUIRE_DUCKDB=1` makes that an error):

1. **53 single facts** behind the rules, all confirmed: `NaN > 5`, `NaN > Infinity`, `NaN = NaN`, `NaN >= NaN`, `NaN <= NaN`, `NaN <> 5` true; `NaN < NaN`, `NaN > NaN`, `NaN < 5`, `NaN <= 5`, `NaN = 5` false; `5 < NaN` true; the same for FLOAT; `-0.0 = 0.0` true and `-0.0 < 0.0`, `0.0 > -0.0` false; `0.1::FLOAT = 0.1::DOUBLE` false (float widens exactly);
   comparisons with NULL are NULL (`=`, `<>`, `<`); decimals are exact across scales (`1.50::DECIMAL(5,2) = 1.505::DECIMAL(5,3)` false, `<` true, `= 1.5::DECIMAL(2,1)` true) and against integers; integer widths (`0::INT = 4294967296::BIGINT` false); `false < true`; text bytewise (`'é' > 'z'`, `'日' > 'é'`, `'𝄞' > '日'`, `'Z' < 'a'`, `'' < 'a'`); binary unsigned bytewise (`x'80' > x'7f'`, `x'41ff' < x'42'`); dates and timestamps.
2. **The row-by-row oracle of the soundness test against DuckDB**: random tables and predicates (all types, NULLs, NaN, zeros, extremes, cross-width and cross-scale literals) evaluated by DuckDB and by the oracle. Default run 2,000 cases; the recorded run 20,000 draws: **19,925 compared, 0 mismatches** (a second seed, 5,000 draws: 4,988 compared, 0 mismatches).
   The check can fail: the same cases evaluated with IEEE rules for NaN (Arrow compute, pyiceberg) differ from DuckDB in 732 of the 19,925 cases, and the test asserts at least one such case.

Contradictions and limits found (none changes a rule, all are listed in the crate docs):
- **DuckDB errors where the decimal rules define an exact answer**: `99999999999999999999999999999999999999::DECIMAL(38,0) > 0.5::DECIMAL(3,1)` fails with a conversion error ("out of range", common type DECIMAL(38,1)). DuckDB rejected 75 of the 20,000 random draws for this reason (a few such comparisons also appear in the 2,000-draw default run). The pruning is exact (decimal-rules section 5); the differential tests must not compare such queries (the card's "known differences" list should gain this row).
- DuckDB cannot hold a NUL in text, and the extreme values of a date or timestamp integer are DuckDB's infinity sentinels (`i32::MAX` days, `i64::MAX` microseconds): 7,977 draws that contain them were not written to DuckDB; the oracle for those cases is unchecked (they are in the soundness run).
- DuckDB's own row-group pruning on parquet files was NOT part of this step (it was checked on our writer's files in `spikes/parquet-write/xcheck.py`).

## Tests and counts

51 tests (35 rule tests, 7 soundness-and-controls, 6 effectiveness, 2 DuckDB, 1 doctest). All pass in debug and release.

| Check | Cases | Result |
|---|---|---|
| Soundness property vs row-by-row oracle (release, default seed 0x4E41_5441_5343_414E) | 400,000 per run (40,000 in debug) | 151,274 skipped (37.8%), 0 unsound. Skips by reason: OutOfRange 65,899; AllNull 51,348; NoNulls 18,548; EmptyRowGroup 12,163; Unsatisfiable 2,545; NoNan 771. The test requires each reason to occur at least 1 in 2,000 cases |
| Same, five other seeds (1, 2, 3, 0xDEADBEEF, 987654321) | 1,000,000 each, release | 0 unsound each (about 380,000 skipped each) |
| Tightness, integers/dates/timestamps/booleans/decimals (brute force over every value of `[min, max]`) | 200,000 (40,000 debug) | decision equals brute force in every case (88,052 skips) |
| Tightness, floats (with and without NaN count, both zeros, infinities, NaN literal) | 200,000 | equal in every case (52,927 skips) |
| Tightness, text (exact bounds, non-ASCII) | 200,000 | equal in every case (63,813 skips) |
| R2 on sorted dates: TPC-H SF1 shape (6,001,215 rows, 122,880 per row group = 49 groups), nine ranges | 9 x 49 groups | one-year ranges skip 41 of 49 (83.7%; 42 for the last year), a month and a day skip 48 of 49; in every case exactly the groups without a matching row (R2 asks for at least 60%) |
| R2 on sorted integers (1,000,000 keys in groups of 8,192 = 123 groups, a range of 14% of the keys), 200 random ranges | 200 x 123 | skipped = the disjoint groups, every time, and at least 60% each time |
| Clustered random data (2,000 groups x 1,000 values in a random window of width 1,000, 500 ranges of width 10,000) | 1,000,000 group decisions | 98.90% skipped; 100.00% of the groups without a match; stated minimum 95% and 97% |
| Generator coverage over 20,000 cases | | NaN rows 3,773; both zero signs 2,089; truncated maximum 2,538; all-NULL columns 4,908; empty tables 589; NaN bounds 487; missing statistics 14,149 (each must be at least 100) |

Test time (development machine, 8 cores, wall clock of `cargo test -p nata-scan`, offline): release 7.8 s from a clean build of the crate and 6.4 s rerun (soundness 2.2 s, DuckDB 2.9 s, effectiveness 1.0 s, rules under 0.01 s); debug 14.0 s clean and 12.4 s rerun (effectiveness 7.8 s). The mutation run: 82 s for 53 mutants with 6 parallel workers.

## Negative controls: the soundness test can fail

Five deliberately unsound readers, built by corrupting the statistics before the call or written out independently, each rejected within the first 1,300 cases of the default seed (`tests/soundness.rs`):

| Control | Models | Rejected at case |
|---|---|---|
| `nan-blind` | a reader that thinks the float bounds describe all rows (NaN count forced to 0) | 191 |
| `truncated-as-exact` | a reader that takes a truncated maximum for the maximum | 543 |
| `missing-null-count-as-zero` | an unknown null count read as 0 | 1,270 |
| `missing-bound-as-zero` | an absent min or max read as 0 | 40 |
| `nan-bound-trusted` | an independent decider that takes a NaN minimum as the largest number | 213 |

## Mutants

53 mutants, each one text edit (or, for `nan-bounds-trusted`, two edits that belong together) of `src/*.rs`, applied to a private copy in a private workspace (`crates/nata-scan/mutate.py`; the match ignores whitespace so rustfmt cannot silently invalidate an edit).
**CAUGHT 53, SURVIVED 0, INVALID 0.** The unmutated copy passes first, and the real tree is hashed before and after.
All of the card's seven are present and caught: `<`/`<=` and `>`/`>=` conflated (4 mutants, both directions), NaN min/max trusted, -0.0 narrowed, truncated bounds treated as exact, a missing statistic treated as zero (bound and null count), off-by-one in the range comparison of `=` (3 mutants) and of the exclusive bound, the null count ignored (8 mutants for `IS NULL`, `IS NOT NULL`, all-NULL groups).
"Kind": *unsound* mutants can skip a group that matches; *weak* mutants keep more than the rules require (the soundness property cannot see them, the tightness test and the boundary tests do: 12 of them); *policy* mutants remove a validation that the contract asks for but that sound statistics never trigger (13; caught by unit tests, not by the random soundness run, which is the honest place for them). 33 of 53 are caught by the random soundness property.
One mutant history, kept for honesty: the first run had 49 of 50 caught. `nan-below-numbers` (NaN ordered below numbers in the comparator) survived because the NaN branches of the comparator were dead code (NaN literals and NaN bounds are handled before any comparison). The fix was in the code, not the mutant: the comparator now gives NaN no order at all, and the "NaN trusted" mutant became the pair of edits that really trust NaN (no guard, NaN ordered as the largest value).

| # | Mutant | Card mutant | Kind | Result | Caught by (first 3 of N tests) | by the soundness property |
|---|---|---|---|---|---|---|
| 1 | `le-treated-as-lt` | 1 (`<=` as `<`) | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `float32_columns_compare_as_the_double_they_widen_to`, `float_group_with_unknown_nan_rows` (+10) | yes |
| 2 | `lt-treated-as-le` | 1 (`<` as `<=`) | weak (keeps more than needed) | CAUGHT | `decimal_extremes_and_validation`, `float32_columns_compare_as_the_double_they_widen_to`, `float_group_with_unknown_nan_rows` (+17) | no |
| 3 | `ge-treated-as-gt` | 1 (`>=` as `>`) | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `clustered_random_data_skips_the_disjoint_groups`, `decimal_literal_with_more_fraction_digits_than_the_column` (+13) | yes |
| 4 | `gt-treated-as-ge` | 1 (`>` as `>=`) | weak (keeps more than needed) | CAUGHT | `binary_increment_skips_trailing_ff`, `decimal_extremes_and_validation`, `decimal_literal_with_more_fraction_digits_than_the_column` (+19) | no |
| 5 | `eq-prunes-at-the-maximum` | 6 (off by one) | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `decimal_extremes_and_validation`, `decimal_literal_with_more_fraction_digits_than_the_column` (+11) | yes |
| 6 | `eq-prunes-at-the-minimum` | 6 (off by one) | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `decimal_literal_with_more_fraction_digits_than_the_column`, `float32_columns_compare_as_the_double_they_widen_to` (+7) | yes |
| 7 | `eq-never-prunes-above-the-maximum` | 6 (off by one) | weak (keeps more than needed) | CAUGHT | `clustered_random_data_skips_the_disjoint_groups`, `decimal_literal_with_more_fraction_digits_than_the_column`, `decimal_rescaling_does_not_overflow` (+17) | no |
| 8 | `ne-skips-when-only-the-minimum-equals` |  | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `integer_boundaries_of_each_operator`, `tightness_floats` (+2) | yes |
| 9 | `ne-needs-no-exact-bounds` |  | policy (accepts what the contract rejects; invisible on sound statistics) | CAUGHT | `truncated_bounds_are_bounds_only` | no |
| 10 | `nan-bound-guard-removed` | 2 (guard alone) | policy (accepts what the contract rejects; invisible on sound statistics) | CAUGHT | `nan_in_the_bounds_is_not_trusted` | no |
| 11 | `nan-bounds-trusted` | 2 (NaN min/max trusted) | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `nan_in_the_bounds_is_not_trusted` | yes |
| 12 | `nan-rows-ignored-for-all-three` |  | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `float_group_with_unknown_nan_rows`, `float_group_without_nan_rows` (+1) | yes |
| 13 | `nan-rows-ignored-by-gt` |  | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `float_group_with_unknown_nan_rows`, `float_group_without_nan_rows` (+1) | yes |
| 14 | `nan-rows-ignored-by-ge` |  | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `float_group_with_unknown_nan_rows`, `tightness_floats` | yes |
| 15 | `nan-rows-ignored-by-ne` |  | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `float_group_with_unknown_nan_rows`, `tightness_floats` | yes |
| 16 | `unknown-nan-count-means-no-nan` |  | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `float_group_with_unknown_nan_rows`, `nan_literal` (+1) | yes |
| 17 | `nan-count-zero-ignored` |  | weak (keeps more than needed) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `control_nan_blind_reader_is_rejected`, `float32_columns_compare_as_the_double_they_widen_to` (+5) | yes |
| 18 | `nan-count-above-rows-trusted` |  | policy (accepts what the contract rejects; invisible on sound statistics) | CAUGHT | `float_nan_count_above_the_row_count_is_contradictory` | no |
| 19 | `nan-literal-gt-kept` |  | weak (keeps more than needed) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `nan_literal`, `tightness_floats` | yes |
| 20 | `nan-literal-le-skipped-when-nan-free` |  | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `nan_literal`, `tightness_floats` | yes |
| 21 | `nan-literal-eq-ignores-the-nan-count` |  | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `nan_literal`, `tightness_floats` | yes |
| 22 | `zero-signs-distinct` | 3 (-0.0 narrowed) | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `tightness_floats`, `zeros_are_equal_whatever_their_sign` | yes |
| 23 | `float32-column-literal-narrowed-to-f32` |  | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `float32_columns_compare_as_the_double_they_widen_to` | yes |
| 24 | `truncated-bounds-treated-as-exact` | 4 (truncated as exact) | unsound (can skip a group that matches) | CAUGHT | `binary_increment_skips_trailing_ff`, `can_skip_is_sound_against_a_row_by_row_oracle`, `text_truncated_at_a_character_boundary_is_usable` (+2) | yes |
| 25 | `constant-ignores-exactness` |  | policy (accepts what the contract rejects; invisible on sound statistics) | CAUGHT | `truncated_bounds_are_bounds_only` | no |
| 26 | `increment-wraps-0xff` |  | unsound (can skip a group that matches) | CAUGHT | `binary_increment_skips_trailing_ff`, `can_skip_is_sound_against_a_row_by_row_oracle`, `truncated_maximum_that_cannot_be_used` | yes |
| 27 | `increment-gives-up-on-0xff` |  | weak (keeps more than needed) | CAUGHT | `binary_increment_skips_trailing_ff` | no |
| 28 | `truncated-maximum-used-without-increment-but-exclusive` |  | unsound (can skip a group that matches) | CAUGHT | `binary_increment_skips_trailing_ff`, `can_skip_is_sound_against_a_row_by_row_oracle`, `text_truncated_at_a_character_boundary_is_usable` (+2) | yes |
| 29 | `utf8-bound-accepted-when-cut` |  | policy (accepts what the contract rejects; invisible on sound statistics) | CAUGHT | `utf8_bound_from_bytes_refuses_a_cut_character` | no |
| 30 | `bytes-compared-as-signed` |  | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `text_order_is_by_bytes_for_non_ascii`, `tightness_text` (+1) | yes |
| 31 | `missing-bound-treated-as-zero` | 5 (missing as zero) | unsound (can skip a group that matches) | CAUGHT | `all_null_row_group_with_each_operator`, `can_skip_is_sound_against_a_row_by_row_oracle`, `missing_statistics_never_skip` (+1) | yes |
| 32 | `missing-null-count-treated-as-zero` | 5 (null count) | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `is_null_and_is_not_null_use_the_null_count` | yes |
| 33 | `null-count-ignored-all-null-kept` | 7 (null count ignored) | weak (keeps more than needed) | CAUGHT | `all_null_row_group_with_each_operator`, `one_provably_empty_term_skips_the_conjunction`, `one_row_group` | no |
| 34 | `is-not-null-keeps-all-null` | 7 | weak (keeps more than needed) | CAUGHT | `all_null_row_group_with_each_operator`, `is_null_and_is_not_null_use_the_null_count` | no |
| 35 | `is-null-never-skipped` | 7 | weak (keeps more than needed) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `control_missing_null_count_as_zero_is_rejected`, `is_null_and_is_not_null_use_the_null_count` | yes |
| 36 | `is-null-skipped-unless-all-null` | 7 | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `is_null_and_is_not_null_use_the_null_count` | yes |
| 37 | `all-null-skipped-for-is-null` | 7 | unsound (can skip a group that matches) | CAUGHT | `all_null_row_group_with_each_operator`, `all_null_with_bounds_is_a_contradiction_and_kept`, `can_skip_is_sound_against_a_row_by_row_oracle` (+3) | yes |
| 38 | `null-count-above-rows-trusted` | 7 | policy (accepts what the contract rejects; invisible on sound statistics) | CAUGHT | `contradictory_statistics_are_kept` | no |
| 39 | `all-null-with-bounds-trusted` | 7 | policy (accepts what the contract rejects; invisible on sound statistics) | CAUGHT | `all_null_with_bounds_is_a_contradiction_and_kept` | no |
| 40 | `is-not-null-skipped-when-some-null` | 7 | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `is_null_and_is_not_null_use_the_null_count` | yes |
| 41 | `exclusive-upper-bound-off-by-one` | 6 (off by one) | weak (keeps more than needed) | CAUGHT | `binary_increment_skips_trailing_ff`, `truncated_bounds_are_bounds_only` | no |
| 42 | `empty-row-group-kept` |  | weak (keeps more than needed) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `empty_row_group_is_skipped_for_every_predicate` | yes |
| 43 | `min-above-max-trusted` |  | policy (accepts what the contract rejects; invisible on sound statistics) | CAUGHT | `contradictory_statistics_are_kept` | no |
| 44 | `bound-of-wrong-type-ignored` |  | policy (accepts what the contract rejects; invisible on sound statistics) | CAUGHT | `contradictory_statistics_are_kept`, `decimal_extremes_and_validation` | no |
| 45 | `int-literal-truncated-to-32-bits` |  | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `int32_column_with_wide_literals_is_compared_exactly`, `integer_extremes_do_not_overflow` | yes |
| 46 | `decimal-literal-precision-unchecked` |  | policy (accepts what the contract rejects; invisible on sound statistics) | CAUGHT | `decimal_extremes_and_validation` | no |
| 47 | `decimal-literal-rounded-to-the-column-scale` |  | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `decimal_literal_with_more_fraction_digits_than_the_column`, `tightness_integers_dates_booleans_and_decimals` | yes |
| 48 | `decimal-rescale-wraps` |  | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `decimal_rescaling_does_not_overflow` | yes |
| 49 | `decimal-overflow-sign-flipped` |  | unsound (can skip a group that matches) | CAUGHT | `can_skip_is_sound_against_a_row_by_row_oracle`, `decimal_rescaling_does_not_overflow` | yes |
| 50 | `decimal-bound-scale-unchecked` |  | policy (accepts what the contract rejects; invisible on sound statistics) | CAUGHT | `decimal_extremes_and_validation` | no |
| 51 | `float-literal-accepted-for-decimal-column` |  | policy (accepts what the contract rejects; invisible on sound statistics) | CAUGHT | `decimal_extremes_and_validation` | no |
| 52 | `unsupported-column-pruned` |  | policy (accepts what the contract rejects; invisible on sound statistics) | CAUGHT | `unknown_column_unsupported_type_and_mismatched_literal_are_kept` | no |
| 53 | `conjunction-never-skips` |  | weak (keeps more than needed) | CAUGHT | `all_null_row_group_with_each_operator`, `binary_increment_skips_trailing_ff`, `can_skip_is_sound_against_a_row_by_row_oracle` (+36) | yes |

## Commands (all offline, no cloud)

```sh
cargo fmt --check
cargo clippy -p nata-scan --all-targets -- -D warnings
cargo test -p nata-scan
cargo test --release -p nata-scan
python3 crates/nata-scan/mutate.py --jobs 6 --out /tmp/mutants.txt
NATA_SCAN_SEED=<seed> NATA_SCAN_ONLY=<case> cargo test -p nata-scan --test soundness -- --nocapture   # replay one case
```

## What this does NOT show

- **Real Parquet statistics.** The statistics in the tests are produced by a generator that imitates writers (exact bounds, NaN left out, widened bounds, both zero signs, NaN contamination, truncation with and without increment, missing statistics). No footer of a file written by DuckDB, pyarrow or parquet-rs was decoded into these types (the decoder is a later step), and a Java-written file is not available. A writer with a fault the generator does not model, for example text bounds compared as signed bytes (PARQUET-251 style), would be trusted. The reader does not decode `is_min_value_exact` or the legacy `min`/`max` fields.
- **Page-level pruning** (the column index and offset index), **bloom filters**, **dictionary-page filtering**: not covered.
- **Nested types, lists, maps, `INT96`, unsigned integers, time-of-day, timestamps in other units, collations other than bytewise**: `Unsupported` (kept) or not modelled.
- **`OR`, `NOT`, `IN`, `LIKE`**: only an AND of simple terms is expressible.
- **The row-level filter** that runs on the kept row groups is not part of this crate: a Skip is sound only if that filter has DuckDB's semantics (NaN above numbers and equal to itself, zeros equal). Arrow's compare kernels use IEEE rules for NaN and would disagree with these rules on NaN rows (732 of 19,925 random cases differ from DuckDB).
- **R2 on the real file.** R2 (at least 60% of the row groups skipped on SF1 `lineitem` sorted by `l_shipdate`) is shown here on an analytic model of that file (49 row groups, exact bounds), 83.7% for a one-year range; the card's check on the real file needs the Parquet footer decoder and is a later step. The fraction of BYTES read (R2's 40% limit) is not measured.
- **Performance.** The cost of a decision was not measured; it is a few comparisons, and 400,000 decisions with data generation and oracle take about 2 s. Nothing here is a speed claim.
- **Overflow in other crates.** `nata-scan` uses `checked_*`, `saturating_*` and `i128::from`; the workspace builds release with overflow checks on (all tests ran with them) and the 128-bit decimal limits are tested. The stricter arithmetic lints of `nata-kernels` (`arithmetic_side_effects`, `cast_possible_truncation`) are not enabled for this crate; the library code has no `as` casts (checked with grep; casts exist only in the tests).
- DuckDB's pruning on parquet files, and Iceberg metrics (bounds truncated to 16 characters, NaN counts), are other steps; the crate takes a NaN count where a source has one.
