# The SQL front end of slice 4: parse, bind, plan (card 0002, step 2)

Date: 2026-11-06. Machine: the dev box (8 vCPU x86-64, Linux 6.12, Rust 1.99.1, DuckDB not needed), local only, no GCP. Code: `crates/nata-sql` (new), one public function added to `crates/nata-parquet` (`read_leaves`, the schema of a file from its footer, with its test). Card: `docs/slices/0002-select-where-limit.md`; rules: `docs/dialect.md`, ADR 0007, `docs/decimal-rules.md`. Step 3 (evaluation, `nata sql`, `EXPLAIN ANALYZE`, the CLI) is not part of this step.

## What was built

| piece | where | what it does |
|---|---|---|
| parse | `src/parse.rs` | `sqlparser` 0.63.0 (`default-features = false, features = ["std"]`, PostgreSQL dialect) behind our own AST (`src/ast.rs`). A token guard runs first: text at most 1 MiB, nesting (parentheses, `CASE`, runs of prefix operators) at most 1,000, at most 10,000 terms in a chain of one operator; it also finds what the crate's tree hides (`LIMIT ALL`, a stray `;`) and reads `//` as integer division. The parse runs on a thread with a 16 MB stack. The conversion destructures every field of the crate's `Query` and `Select` and answers `not supported yet: <what>` at the construct for the rest. Chains and `a::t::u`, `IS NULL IS NULL` chains are walked with a loop. |
| bind | `src/bind.rs`, `src/types.rs`, `src/source.rs`, `src/glob.rs` | names (unquoted folded, quoted exact), `*`, aliases, exact literal typing (integers by value, decimals from their digits, dates checked), the decimal result types of `decimal-rules.md` sections 3 and 4 with the 38-digit bind errors, integer widening, no string/number/date conversion, CASE and CAST typing, NULL typed by context, "did you mean". The source is a path or glob; every file's footer is read through `nata_parquet::read_leaves` and all must have one schema. |
| typed tree | `src/texpr.rs` | `Expr { kind, ty, pos }` with resolved column indices and literal values, n-ary `And`/`Or`, `Between` and `InList` as nodes (not lowered: lowering would copy the operand and a nest of BETWEENs would grow as 2^depth); the evaluator contract is in the module doc. A deep tree is dropped, visited and printed with loops. |
| plan | `src/plan.rs` | `Scan { files, columns, pushed } -> Filter -> Project -> Limit`, rule R2: split at the top-level ANDs, push `column <op> literal` (either side), `IS [NOT] NULL` and a non-negated `column BETWEEN literal AND literal` when the scan's own typing takes the literal for the column's kind without conversion; pushed conjuncts are enforced by the scan's exact row filter and are not repeated in the `Filter`. Pushed terms are `nata_parquet::filter::Term` values; `Scan::scan_options()` gives the `ScanOptions` (filter texts that parse back to the same terms: tested). `LogicalPlan::explain_text()` is the text for `EXPLAIN`. |

API for step 3: `nata_sql::plan_sql(sql, &FsSource) -> Result<Planned, SqlError>`; `Planned { mode (No/Plan/Analyze), plan: LogicalPlan, output: Vec<OutputColumn> }`; `SqlError { class (Syntax/Bind/NotSupported/Execution), message, pos { line, col }, suggestion }`; `MemSource` for tests.

## Checks (all run: `cargo test -p nata-sql`, 69 tests; release too)

| check | what | result |
|---|---|---|
| rules of the dialect page | `tests/dialect.rs`, 37 tests, one or more per bullet of sections 1 to 9; expected values come from the documents' tables | pass; the table below |
| comparison with the oracle parser | `src/oracle` (the hand-written parser of the bake-off, extended with `::`, `//`, `TRUE`/`FALSE`): 255 accepted test-set queries give the same tree, 120 refused ones are refused with a position by both, 38 edge cases agree except two listed differences (`a = NOT b` taken by sqlparser; a no-break space is a letter for sqlparser), 3,000 generated expressions in two renderings give one tree | pass; the 3 known-bad oracles of the spike are still caught |
| golden plans | `tests/golden.rs` + `tests/golden/plans.txt`: 52 queries (pushdown decisions, columns read, errors with positions) | each block read against the rules when written |
| pushdown never changes a result | `tests/pushdown.rs`: 4,000 random WHERE clauses over 40 random rows each (160,000 rows, NULLs and extremes); rows kept by (pushed terms run by `nata_parquet::filter::evaluate` on an Arrow batch) AND (remaining filter run by a reference evaluator written for the test) equal the rows kept by the whole clause on the same evaluator; 2,706 queries had a pushed conjunct, 3,455 a remaining filter, 2,161 both; also the text form of every pushed term parses back to the same typed term | pass |
| parse fuzz | `src/parse_tests.rs`: 10,000 mangled queries (one edit of an accepted test-set query: delete, swap, insert; seed 20261106, query *i* depends on (seed, *i*), `NATA_SQL_FUZZ_SEED`, `NATA_SQL_FUZZ_N`): 3,677 parse, 6,323 refused, 0 panics, every error has a position inside the text (40 at its end) | pass |
| bind and plan fuzz | the same 10,000 plus the 255 accepted queries through `plan_sql` on one wide in-memory schema: 899 planned, 9,356 refused, 0 panics, every error a class other than `execution` and a position of at least 1:1 | pass |
| limits | `src/parse_tests.rs`, `tests/limits.rs`, on a 2 MB test-thread stack: 1,000 levels of parentheses / `CASE` / `NOT` / unary minus / `CAST(` and a mixture parse and plan, 1,001 is a positioned limit error; a chain of exactly 10,000 terms of each operator (`+ - * / % // AND OR ::`, `IS NULL`) parses, plans, prints and drops, 10,001 is refused; 1 MiB parses, 1 MiB + 1 byte is refused at its first extra character; IN list of 400,000 literals, select list of 200,000 items, a 1 MB string and a 1 MB comment plan in a few seconds | pass (debug and release) |
| real files | `tests/real_files.rs`: glob expanded on disk in byte order, the plan's `scan_options()` open the real slice 3 scan and return exactly the rows the query asks for (row groups skipped: reported), the first differing file is named, missing / corrupt / empty / remote sources are positioned errors, no cell value (`SECRET-...`) in any error | pass |
| time | `plan_sql` of the card's example, in-memory schema, median of 200 in this build: 97 us (thread start included). Card target: under 1 ms | met on this machine; one machine, x86-64 only |
| lint | `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (debug), `cargo test --release -p nata-sql`, `cargo deny check` (advisories, bans, licenses, sources ok; only `sqlparser` is new in the tree, 0 `unsafe`, `scripts/unsafe-inventory.py nata-sql`: 0 in the workspace crates) | pass |

### Rule-to-test table

| rule (docs/dialect.md) | tests in `tests/dialect.rs` (and others) |
|---|---|
| 1 statements; EXPLAIN modes; glob schema; not supported yet with position | `s1_statement_forms_and_the_explain_modes`, `s1_what_is_not_yet_supported_names_the_construct_at_its_position`, `s1_all_the_files_of_a_glob_must_have_the_same_schema_and_the_first_differing_file_is_named`, `real_files.rs` |
| 2 names: folding, quoted exact, `*`, alias, duplicates, reserved words | `s2_*` (6 tests) |
| 3 literals and their exact types; NULL typed by context; `SELECT NULL` | `s3_*` (6 tests) |
| 4 precedence, integer rules, decimal result tables, integer with decimal, float and date arithmetic | `s4_*` (7 tests), `golden.rs` |
| 5 comparison types, three-valued logic (typing), BETWEEN, IN, NaN/-0.0 are scan semantics (slice 3) | `s5_*` (7 tests), `pushdown.rs` |
| 6 CASE, CAST | `s6_*` (4 tests) |
| 7 LIMIT, OFFSET | `s7_limit_and_offset_take_non_negative_integer_literals` |
| 8 error classes, positions, "did you mean", limits | `s8_*` (3 tests), `parse_tests.rs`, `limits.rs`, fuzz |
| 9 the four differences from DuckDB | `s9_the_differences_from_duckdb_each_have_a_test` and the tests it points to |

## Mutants (`python3 crates/nata-sql/mutate.py`; each is one text edit of a source file, `cargo test --release --no-fail-fast -p nata-sql` on a private copy)

43 mutants: **43 CAUGHT, 0 SURVIVED, 0 INVALID** (the unmutated copy passes first; the real tree is hashed before and after). Asked for: a conjunct pushed through OR (`push-through-or`), a pushed predicate dropped from the remaining filter or kept in both (`conjunct-dropped-...`, `pushed-conjunct-also-left-in-the-filter`; the scan filter is always exact for what is pushed, so the "not exact" variants are the literal ones: `pushed-decimal-literal-loses-its-scale`, `pushed-date-literal-off-by-one`, `pushed-text-literal-lower-cased`), NOT lost (`not-lost`, `not-lost-in-pushdown`), an alias or name resolved to the wrong column, a literal typed wrongly (decimal scale, date, integer width, sign, exponent), LIMIT before FILTER (`limit-printed-below-the-filter`: the plan type fixes the order, so the mutant is the printed order and only the golden plans catch it), OFFSET off by one, a quoted name folded, implicit string-to-number and string-to-date accepted. Added: the operator flip of `5 < x`, NOT BETWEEN pushed, IS NULL swapped, BETWEEN bounds swapped, filter-only columns not read, float with decimal accepted, WHERE without a BOOLEAN, IN with expressions, the decimal result rules (addition, multiplication cap, division scale, remainder, CASE scale), the guard limits (off by one), `LIMIT ALL`, error position at the end, `//`, chained comparisons, reserved words.

| mutant | result | tests that failed (first three) |
|---|---|---|
| push-through-or | CAUGHT | `pushed_conjuncts_plus_the_remaining_filter_keep_exactly_the_rows_of_the_whole_where_clause`, `the_plans_match_the_golden_file` |
| not-lost-in-pushdown | CAUGHT | `pushed_conjuncts_plus_the_remaining_filter_keep_exactly_the_rows_of_the_whole_where_clause`, `the_plans_match_the_golden_file` |
| pushed-conjunct-also-left-in-the-filter | CAUGHT | `the_plans_match_the_golden_file` |
| conjunct-dropped-from-the-filter-when-not-pushed | CAUGHT | `a_glob_is_expanded_in_file_order_and_the_pushed_filter_drives_the_real_scan`, `a_long_in_list_and_a_long_select_list_are_planned`, `pushed_conjuncts_plus_the_remaining_filter_keep_exactly_the_rows_of_the_whole_where_clause` |
| operator-flip-wrong | CAUGHT | `pushed_conjuncts_plus_the_remaining_filter_keep_exactly_the_rows_of_the_whole_where_clause`, `the_plans_match_the_golden_file` |
| not-between-pushed-as-between | CAUGHT | `pushed_conjuncts_plus_the_remaining_filter_keep_exactly_the_rows_of_the_whole_where_clause`, `the_plans_match_the_golden_file` |
| is-null-and-is-not-null-swapped | CAUGHT | `pushed_conjuncts_plus_the_remaining_filter_keep_exactly_the_rows_of_the_whole_where_clause`, `the_plans_match_the_golden_file` |
| between-bounds-swapped | CAUGHT | `pushed_conjuncts_plus_the_remaining_filter_keep_exactly_the_rows_of_the_whole_where_clause`, `the_plans_match_the_golden_file` |
| pushed-decimal-literal-loses-its-scale | CAUGHT | `a_glob_is_expanded_in_file_order_and_the_pushed_filter_drives_the_real_scan`, `pushed_conjuncts_plus_the_remaining_filter_keep_exactly_the_rows_of_the_whole_where_clause`, `the_plans_match_the_golden_file` |
| pushed-date-literal-off-by-one | CAUGHT | `pushed_conjuncts_plus_the_remaining_filter_keep_exactly_the_rows_of_the_whole_where_clause`, `the_plans_match_the_golden_file` |
| pushed-text-literal-lower-cased | CAUGHT | `pushed_conjuncts_plus_the_remaining_filter_keep_exactly_the_rows_of_the_whole_where_clause`, `the_plans_match_the_golden_file` |
| filter-only-columns-not-read | CAUGHT | `pushed_conjuncts_plus_the_remaining_filter_keep_exactly_the_rows_of_the_whole_where_clause`, `the_plans_match_the_golden_file` |
| offset-off-by-one | CAUGHT | `s7_limit_and_offset_take_non_negative_integer_literals`, `the_plans_match_the_golden_file` |
| limit-printed-below-the-filter | CAUGHT | `the_plans_match_the_golden_file` |
| quoted-name-folded-to-lower-case | CAUGHT | `s2_a_quoted_identifier_is_exact_and_case_sensitive`, `s2_an_alias_needs_as_names_an_output_column_and_cannot_be_used_in_the_same_select_or_where`, `s9_the_differences_from_duckdb_each_have_a_test` |
| string-compared-with-number-accepted | CAUGHT | `a_file_with_another_schema_is_named_and_no_cell_value_appears_in_an_error`, `s5_between_is_two_comparisons_with_the_type_rules_of_both`, `s5_in_takes_literals_of_a_comparable_type` |
| string-compared-with-date-accepted | CAUGHT | `s5_no_implicit_conversion_between_strings_numbers_and_dates`, `the_plans_match_the_golden_file` |
| float-compared-with-decimal-accepted | CAUGHT | `s5_floats_are_not_compared_with_decimals_and_the_integer_case_is_not_specified` |
| not-lost | CAUGHT | `s4_precedence_from_tightest_to_loosest`, `the_plans_match_the_golden_file` |
| alias-ignored | CAUGHT | `s2_an_alias_needs_as_names_an_output_column_and_cannot_be_used_in_the_same_select_or_where`, `s2_two_output_columns_may_have_the_same_name`, `the_plans_match_the_golden_file` |
| name-resolved-to-the-wrong-column | CAUGHT | `a_glob_is_expanded_in_file_order_and_the_pushed_filter_drives_the_real_scan`, `a_query_of_1_mb_with_a_long_comment_or_string_is_planned`, `s2_a_quoted_identifier_is_exact_and_case_sensitive` |
| sign-of-a-negative-literal-lost | CAUGHT | `s3_decimal_literals_are_exact_from_their_digits`, `s3_integer_literals_take_the_smallest_of_integer_and_bigint_then_a_decimal_up_to_38_digits`, `s5_in_takes_literals_of_a_comparable_type` |
| exponent-sign-flipped | CAUGHT | `s3_decimal_literals_are_exact_from_their_digits`, `s9_the_differences_from_duckdb_each_have_a_test`, `the_plans_match_the_golden_file` |
| integer-literal-width | CAUGHT | `s3_integer_literals_take_the_smallest_of_integer_and_bigint_then_a_decimal_up_to_38_digits` |
| date-literal-off-by-one | CAUGHT | `s3_strings_dates_booleans`, `s5_in_takes_literals_of_a_comparable_type`, `the_plans_match_the_golden_file` |
| decimal-literal-scale-off-by-one | CAUGHT | `a_glob_is_expanded_in_file_order_and_the_pushed_filter_drives_the_real_scan`, `s3_decimal_literals_are_exact_from_their_digits`, `s3_there_is_no_float_literal_syntax` |
| null-never-gets-its-type | CAUGHT | `s3_null_takes_the_type_its_context_needs`, `the_plans_match_the_golden_file` |
| suggestion-distance-3 | CAUGHT | `s8_an_unknown_column_suggests_the_closest_name_within_an_edit_distance_of_2` |
| where-needs-no-boolean | CAUGHT | `s5_the_logic_operators_need_booleans` |
| and-or-operands-unchecked | CAUGHT | `s5_the_logic_operators_need_booleans` |
| in-list-takes-any-expression | CAUGHT | `s5_in_takes_literals_of_a_comparable_type` |
| case-scale-takes-the-minimum | CAUGHT | `s6_case_results_have_one_type_or_widen_integers_and_decimals`, `s6_case_results_that_cannot_be_one_type_are_bind_errors`, `the_plans_match_the_golden_file` |
| division-scale-5 | CAUGHT | `s4_decimal_result_types_follow_the_tables_of_decimal_rules`, `s4_only_division_of_integers_is_a_decimal`, `s9_the_differences_from_duckdb_each_have_a_test` |
| addition-precision-plus-one-dropped | CAUGHT | `s3_null_takes_the_type_its_context_needs`, `s4_an_integer_meets_a_decimal_as_an_exact_decimal`, `s4_decimal_result_types_follow_the_tables_of_decimal_rules` |
| multiplication-cap-at-18-dropped | CAUGHT | `s4_an_integer_meets_a_decimal_as_an_exact_decimal`, `s4_decimal_result_types_follow_the_tables_of_decimal_rules`, `the_plans_match_the_golden_file` |
| remainder-precision-takes-the-maximum | CAUGHT | `s4_decimal_result_types_follow_the_tables_of_decimal_rules` |
| nesting-limit-off-by-one | CAUGHT | `nesting_of_1000_levels_is_planned_printed_and_dropped`, `a_deep_tree_is_dropped_on_a_default_stack`, `nesting_of_1000_parses_and_1001_is_a_positioned_limit_error_for_every_construct` |
| chain-limit-off-by-one | CAUGHT | `s8_the_limits_are_positioned_errors_before_parsing` |
| limit-all-not-detected | CAUGHT | `queries_outside_the_subset_are_refused_by_both_parsers_with_a_position`, `constructs_the_crate_hides_are_refused`, `s7_limit_and_offset_take_non_negative_integer_literals` |
| error-at-the-end-reported-at-the-start | CAUGHT | `positions_are_one_based_lines_and_characters`, `s8_every_error_has_a_class_a_message_and_a_one_based_position` |
| integer-division-read-as-division | CAUGHT | `chains_of_10000_terms_are_planned_printed_and_dropped_for_each_kind_of_operator`, `accepted_queries_give_the_same_tree_in_both_parsers`, `generated_expressions_give_the_same_tree_in_both_parsers_and_in_both_renderings` |
| chained-comparison-accepted | CAUGHT | `edge_cases_have_the_same_outcome_except_the_listed_ones` |
| reserved-word-accepted-as-a-name | CAUGHT | `queries_outside_the_subset_are_refused_by_both_parsers_with_a_position`, `positions_are_one_based_lines_and_characters`, `s2_a_reserved_word_needs_quotes_to_be_a_name` |

Two faults of the harness were found and fixed during the work (a mutant whose text rustfmt had changed was INVALID, and a staged copy that kept old modification times let cargo reuse a mutated build: the unmutated check caught it).

## Found while building (each is a test now)

- `Pos::end_of` per node made a 500,000-item IN list quadratic (hang): computed once; covered by the 1 MB shapes in `parse_tests.rs`.
- A chain of `::` casts and of `IS NULL` made the conversion recurse to the length of the chain: the spine loop now covers `::`, `IS`, `BETWEEN`, `IN` as well as binary operators.
- sqlparser PostgreSQL dialect gives `||` the precedence of "other operators", so `//` cannot be mapped to it: the guard merges `//` into one `/` token and the conversion reads the operator before the right operand.
- The crate gives a quoted table name no position: the string is found as the token after `FROM`.
- Debug builds overflowed the 16 MB stack printing a 10,000-deep expression: the printer uses a work list.

## What this does not show

- Nothing runs: the typed tree has no evaluator here, so "same rows as DuckDB" is step 3's differential check. The reference evaluator of `pushdown.rs` is test code, written for integers, decimals, strings, dates and booleans (no floats; no `CAST` of strings).
- Pushdown exactness rests on the slice 3 row filter (verified there against DuckDB), not on DuckDB directly.
- Planning time is one machine, x86-64, an in-memory schema; the footer reads for a real glob (one per file, at bind time, one after the other) are not timed. 10,000 files means 10,000 footer reads before the query starts.
- Parse peak memory for the largest legal queries was seen only through the test timings (a 400,000-item IN list plans in a few seconds); no memory figure was recorded this time (the bake-off measured +275 MB for 500,000 items).
- ARM64 is unmeasured.
- The hand-written oracle and the test set are kept until the slice closes; `spikes/sql-parser` was copied, not deleted.

## Readings and open points for the owner (the dialect page is silent or ambiguous; the strict reading is implemented)

1. **Unaliased output names** follow PostgreSQL: a column keeps its name (also under a CAST), `CASE` is `case`, anything else `?column?`. DuckDB names an expression by its text; if `nata-check` compares headers this needs a decision.
2. **Bare `DECIMAL`** is DECIMAL(18,3) and `DECIMAL(p)` is DECIMAL(p,0) (DuckDB's defaults, as in the bake-off); the page lists only `DECIMAL(p,s)`.
3. **Float arithmetic** (`d + d`, `-d`, `d + 1`) and **comparing a float with an integer** are `not supported yet` (decimal-rules section 4 says so for integer with DOUBLE and is silent on the rest); float with decimal is a bind error. `FLOAT` and `DOUBLE` are two types: comparing them is a bind error (section 5: "the same type").
4. **CAST between booleans and numbers** and **DOUBLE to VARCHAR** are `not supported yet` (the page lists the target types but not which source types convert); meaningless casts (date to integer) are bind errors. `TINYINT` and `SMALLINT` are not CAST targets (not in the page's list) although they are column types.
5. **IN lists hold literals** (page: "the list holds literals"): a column or expression in the list is `not supported yet`. `x = NULL` is not folded to false (nata-scan's note says the binder folds it): it stays a comparison in the `Filter`.
6. **Class of the limit errors** (1 MiB, nesting 1,000, chains of 10,000): `syntax`, message starting "query too large or too deeply nested:". The chain limit counts each operator kind separately per nesting level, and `::` and `IS` as operators. A prefix operator after a value is binary and not counted as nesting.
7. **"Referring to an ambiguous name is an error naming both"** has no case yet: nothing can refer to an output name in this slice (aliases cannot be used inside the same SELECT or WHERE), and a source with two columns of one name is refused at the footer. Two output columns of one name are allowed and tested.
8. **Reserved words** are DuckDB's 112 (a superset of "the words of this page"): a Parquet column named `end` or `order` needs quotes.
9. **LIMIT** with a negative or fractional or too large number is a `bind` error; a non-literal (`LIMIT 1 + 1`, `LIMIT a`) and `OFFSET` without `LIMIT`, `OFFSET m ROWS` are `not supported yet`; `LIMIT 5, 10` is a syntax error (the PostgreSQL dialect does not take it).
10. **Negative literals are folded** (`-2147483648` is an INTEGER, `-9223372036854775808` a BIGINT): the page types a literal by value.
11. **"Did you mean"** also fires for a difference of case only (`"mixedcase"` finds `MixedCase`), which is more than the page's edit distance of 2 asks.
12. **Pushdown**: an integer column narrower than a literal that does not fit it (`l_linenumber < 3000000000`) is widened by a cast and so not pushed (correct, a missed chance); `x = NULL`, comparisons with casts of literals (`CAST('1994-01-01' AS DATE)`) and float columns are never pushed (no constant folding of casts). `SELECT 1 FROM 'f'` delivers no column: `ScanOptions` with an empty list is refused by nata-parquet today, step 3 must read a column or the footer row counts.
13. **Cost to step 3**: the typed tree of a 10,000-term chain is 10,000 deep; the evaluator must run on a large stack or iterate (the front end itself drops, visits and prints such trees with loops). All footers of a glob are read at bind time.
14. `a = NOT b` is accepted by sqlparser (a prefix NOT as the right operand of a comparison; PostgreSQL refuses it): harmless, one tree, listed as a known difference.
