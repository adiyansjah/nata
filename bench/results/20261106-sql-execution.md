# The execution of slice 4: `nata sql`, the executor, `EXPLAIN ANALYZE`, cancellation, the gating differential job (card 0002, step 3)

Date: 2026-11-06. Machine: the dev box (8 vCPU x86-64, Linux 6.12, 16 GB, shared with other jobs), Rust 1.99.1, DuckDB 2.0.0 CLI, `parquet`/`arrow` 60.1.0. **Local only: no GCP, no push.** No timing is
claimed (the card's benchmark rows are step 4, on settled VMs of both CPU types); one unsettled indication is given under "What this does not show". Code: `crates/nata-exec` (new), `crates/nata` (`nata sql`), `crates/nata-parquet` (a per-batch map in the scan
tasks, an `Eval` error kind, the CSV text of `DECIMAL(p,p)`), `crates/nata-sql` (one change of error class), `crates/nata-check` (`expect-nata`, the `PENDING` list, one change of order), `spikes/sqlfuzz` (the subset fuzzer), `check/queries/`, `.github/workflows/ci.yml`.
Rules: `docs/dialect.md` (sections 1 to 9b), `docs/decimal-rules.md`, ADR 0003 decision 4 (errors only for selected valid rows). The front end it runs is `bench/results/20261106-sql-frontend.md`.

## What was built

| piece | where | what it does |
|---|---|---|
| executor | `crates/nata-exec/src/exec.rs` | `execute(plan, config, cancel, sink)`: opens the slice 3 scan with the plan's pushed conjuncts and columns (`ScanPlan::open_with_map`), runs `Filter` and `Project` on every batch **inside the scan's worker tasks** (so they parallelise and are bounded by the scan's window), applies `OFFSET`/`LIMIT` on the consuming thread in file order and stops the scan at `offset + n`. With a `LIMIT` the projection runs on the consuming thread for the first `offset + n` rows (`dialect.md` 9b). A deep expression (more than 64 levels; the dialect allows 10,000) runs on a thread whose stack is sized for it. A query with no column to read reads the first supported column for the row count. |
| evaluator | `src/eval.rs`, `src/cast.rs`, `src/num.rs`, `src/value.rs` | A typed expression over the rows of one Arrow batch for a **selection** of rows: checked integers (range of the SQL type; `MIN % -1` and `MIN // -1` are errors), exact `i128` decimals checked against the digits of the result type (`decimal-rules.md` 3: `/` at scale s+6 rounded half away from zero, `%` with the sign of the dividend, the rescaled-operand overflow), exact comparison across scales and widths, three-valued logic, `NaN` greatest and equal to itself, `-0.0 = 0.0`, `AND`/`OR`/`CASE`/`BETWEEN` evaluating an operand only for the rows that need it, a `WHERE` that only follows TRUE rows, `IN` lists with NULL, `CASE` simple and searched, every cast of the dialect (half away from zero, the exact binary value of a double, plain text forms only, an error that names the position and never the value). Dictionary strings are read through their dictionary; a bare column passes through untouched. |
| output | `src/output.rs`, `crates/nata/src/sql.rs` | CSV with the exact text `nata-check` compares (`nata_parquet::csv`), or an aligned table (widths from the header and the first 1,000 rows, then streaming); errors as `error: <class>: <message>` with `--> line L, column C`, the source line and a caret; exit codes 0 / 1 / 2 / 130 as in `docs/supported-sql.md`; a result cut short by an error is announced as INCOMPLETE; a closed stdout is a quiet exit 0; SIGINT (`signal-hook`, one watcher thread, joined at the end) cancels the scan, also while the footers of a glob are still being read. |
| `EXPLAIN` / `EXPLAIN ANALYZE` | `crates/nata-sql` (printer, step 2), `src/explain.rs` | `EXPLAIN` prints the step 2 plan; `EXPLAIN ANALYZE` runs the query without printing rows and annotates the plan: rows in and out per operator, CPU time per operator (summed over the workers), the scan's wall time, files opened, row groups total / skipped / read / abandoned, rows read, bytes read, columns read, pruning, threads, window, peak bytes and handles. |
| scan hooks | `crates/nata-parquet` | `ScanPlan::open_with_map` (`BatchMap`), `ErrorKind::Eval {message, line, column}` (an error of the query's expressions that the task names with file and row group), `csv::cell_text`, `csv::format_decimal_typed` (DuckDB prints a `DECIMAL(p,p)` without the leading zero: `.500`; the differential check found it, `nata scan` prints it now too). |
| source errors | `crates/nata-sql/src/source.rs` | A source file that is missing, unreadable or damaged is class `execution` (exit code 1), not `bind` (2): it is a failure of the data. Its tests were changed with it. |
| differential check | `crates/nata-check`, `check/queries/`, `scripts/gen-sql-check-data.sh`, `.github/workflows/ci.yml` | New directives `-- expect-nata:` (Nata's exact rows, for a difference where DuckDB errors or answers otherwise) and a `PENDING` file per directory (queries that need SQL beyond slice 4 must still answer `not supported yet`; one that starts to pass fails the run until it is removed: the list can only shrink); `expect-error-nata:` now looks for its own text before it treats `not supported yet` as a gap (a deliberate difference can BE `not supported yet`). 251 + 37 new queries, edge-case files written by DuckDB. The `differential` job is **gating**: its `continue-on-error` is gone. |
| subset fuzzer | `spikes/sqlfuzz/sqlfuzz/{subset,subset_cli}.py`, `adapters/nata_adapter.py` | `run-subset`, `replay-subset`, `prove-subset`: the generator of the card (restricted to the subset), DuckDB as the oracle, `nata sql` as the target through a command adapter; every difference is shrunk, classified from its shrunk query, printed with `seed`, `batch` and `query` and replays from those three numbers. |

Why a new crate, and why not `nata-kernels` for the arithmetic: the kernels crate holds the checked `i64` spike kernels and `strict_cast` of ADR 0003. The executor needs every integer width, decimals up to 38
digits, text, dates and floats, so it re-implements the same rules (errors only for selected valid rows, checked arithmetic, no lenient arrow entry point: `crates/nata-exec/clippy.toml` is a copy of the kernels' ban list,
`strict_cast` is the one arrow conversion it calls) as scalar loops over the selection. These loops are the reference that a SIMD kernel must match later (improvement register P5).

## Verification (a check counts only if it was shown to fail on a known-bad version: the mutant table, the planted fuzz failure, the control arms)

### A. `nata-check` against DuckDB 2.0.0 (`target/debug/nata-check --data data/tpch-sf0.01 --queries check/queries/<dir>`)

| directory | queries | pass | pending | what it holds |
|---|---|---|---|---|
| `scan` | 5 | 5 | 0 | the slice 3 queries (`-- scan:` runs `nata scan`) |
| `sql` | 251 | **251** | 0 | every construct of dialect sections 1 to 8b on the types file (15 rows, 4 row groups, NULLs, extremes, NaN, -0.0, empty / blank / quoted text, dates 0001 to 9999): statements, names, literals, the integer / decimal / date / text / float / boolean comparisons with all six operators, `AND`/`OR`/`NOT` truth tables, `BETWEEN`, `IN` with NULL, `IS NULL`, searched and simple `CASE`, every cast, `LIMIT`/`OFFSET` (also across row groups of a 100,000-row file), overflow and division-by-zero errors that must be errors on both sides, errors that a guard (`AND`, `CASE`, `WHERE`) must prevent, TPC-H SF 0.01 queries (the card's example among them); and the **edge files** (each with the same eight queries: star, filter keeping nothing, filter keeping everything, expressions, limit and offset, comparisons of every column with itself): the empty file, the one-row file, the all-NULL file, the file of duplicate rows, plus a filter that keeps nothing and one that keeps everything, pushed and not pushed |
| `sql-differences` | 37 | **37** | 0 | one query per difference of `dialect.md` section 9 and 9b (and `decimal-rules.md` 9 where `nata sql` can now run it): `expect-error-nata`, `expect-nata` (exact rows) or `tolerance` |
| `smoke` | 3 | 0 | 3 | ORDER BY and aggregates: `not supported yet`, listed in `PENDING` |
| `decimal` | 30 | 0 | 30 | `SELECT <expr>` without `FROM`: listed in `PENDING` |
| `decimal-differences` | 12 | 0 | 12 | the same. (`modulo_wide_operands` passed for a wrong reason when `expect-error-nata` was `not supported yet`, which `SELECT` without `FROM` also answers: its directive now names the real message, `aligning the operands`, and it is pending with the others) |

Counts are those of `nata-check`'s last line, run on the final build (`bench/results/raw/20261106-sql-execution-x86/differential.txt`). The first run of the first 253 queries passed 247 and found: decimal literals below one print `.1` where DuckDB
prints `0.1` (DuckDB types `0.1` as `DECIMAL(18,1)`; ours is `DECIMAL(1,1)` by `dialect.md` 3: a listed difference, and the CSV text of `DECIMAL(p,p)` was made DuckDB's), `i + 1` with a TINYINT column (DuckDB narrows the literal: a
listed difference), and a few queries of mine whose own overflow DuckDB's optimizer removes (`i * 2 > 0` is rewritten to `i > 0`). The `differential` job now runs exactly these directories, on both CPU types, and then the
fuzzer's own tests and 20,000 queries (seed 20261106); the unit tests of the new directives are in `crates/nata-check/tests/directives.rs`.

### B. The SQL fuzzer (`spikes/sqlfuzz`, `run-subset`; DuckDB oracle, `nata sql` target through `adapters/nata_adapter.py`)

The generator restricts `spikes/sqlfuzz`'s to the subset: one table (`t1`/`t2`, 0 to 25 rows, written as Parquet with row groups of 3 and 5 rows) of TINYINT, SMALLINT, INTEGER, BIGINT, REAL, DOUBLE, DECIMAL(10,2),
DECIMAL(25,5), VARCHAR, BOOLEAN and DATE columns with NULLs, extremes, NaN, -0.0, infinities, empty and non-ASCII text; select lists of 1 to 4 expressions of depth up to 3 over `+ - * % //`, unary minus,
comparisons, `AND`/`OR`/`NOT`, `IS [NOT] NULL`, `BETWEEN`, `IN`, `CASE`, `CAST`; `WHERE`, `LIMIT`, `OFFSET`. Rows are compared by position (both engines keep file order). A query DuckDB rejects must be rejected by Nata. Each query's
seed is `(seed, batch, query)`: `python3 -I fuzz.py replay-subset --seed S --batch B --query Q --target-cmd "python3 -I adapters/nata_adapter.py"`.

| seed | queries | oracle errors (DuckDB rejected: Nata must reject too) | rows compared | queries where DuckDB's two answers differ | `not supported yet` from Nata (a gap, not a difference) | differences | unexplained | time |
|---|---|---|---|---|---|---|---|---|
| 20261106 (the CI seed) | 20,000 | 3,387 (16%) | 47,495 | 481 | 22 | 20 | **0** | 146 s |
| 3 | 20,000 | 3,468 (17%) | 49,416 | 483 | 29 | 18 | **0** | 148 s |
| 4 | 20,000 | 3,366 (16%) | 48,053 | 464 | 24 | 24 | **0** | 146 s |
| total | **60,000** | 10,221 | 144,964 | 1,428 | 75 | 62 | **0** | |

By class, per seed (20261106 / 3 / 4): an operand next to a NULL 8 / 8 / 13; `AND`/`OR` in a projection 2 / 3 / 0; a constant in a `CASE` condition 1 / 0 / 2; a failing select list of literals with no row selected 7 / 4 / 7; `DECIMAL(p,p)` text 1 / 2 / 2; decimal `%` then overflow 0 / 1 / 0; `CASE` of decimals beyond 38 digits 1 / 0 / 0.

**Classes of difference, all consequences of rules in `dialect.md` 9 / 9b or `decimal-rules.md` 9; each was read in its shrunk form before it was listed** (a class is chosen from the SHRUNK query, so a real bug
cannot hide in one by sharing words with it; anything else is `BUG?` and fails the run):

| class | what happens | example (shrunk) |
|---|---|---|
| an operand next to a NULL | DuckDB does not evaluate the other operand of a binary operator when one is a constant NULL (the result is NULL); Nata evaluates it for the selected rows and raises its error | `SELECT (CAST(NULL AS BIGINT) - (- b)) FROM t` with `b = -9223372036854775808` |
| `AND`/`OR` in a projection | DuckDB evaluates every operand; Nata only the rows the earlier ones left undecided | `SELECT ((r >= r) OR ((12 % a) < id)) FROM t` with `a = 0` |
| a constant in a `CASE` condition | DuckDB evaluates `CAST('99999999.995' AS DECIMAL(10,2)) IS NULL` when it plans the query; Nata only for the rows that reach that condition | `SELECT CASE WHEN true THEN w WHEN CAST('99999999.995' AS DECIMAL(10,2)) IS NULL THEN w ELSE w END FROM t` |
| a failing select list of literals, no row selected | DuckDB over its in-memory tables folds it when planning and fails even when no row survives; over a Parquet file it does not (checked by hand: `WHERE id < 0`, `WHERE 1 = 2`); Nata fails only for a selected row | `SELECT (- (4294967296 * -4294967296)) FROM t WHERE NULL` |
| `DECIMAL(p,p)` text | a literal below one, or the tight result type of `%`, is a `DECIMAL(p,p)`: Nata and DuckDB print its value the same way but the types (and so `CAST(... AS VARCHAR)`) differ | `SELECT CAST(0.10 AS VARCHAR)` is `.10` here, `0.10` there |
| decimal `%` then `*` | the tight result type of `%` is narrower, so a later `*` can overflow where DuckDB's wider type does not | `d * (b % d)` with `d = 99999999.99` |
| `CASE` of decimals beyond 38 digits | bind error here, silent rounding there (`decimal-rules.md` 9) | `CASE WHEN .. THEN w * w WHEN .. THEN w - w ELSE .. END` |

What is **not** generated (each a listed difference or `not supported yet`): `/` (exact decimal here, DOUBLE there: `sql-differences/d9_divide_*` with a tolerance), float arithmetic, a float compared with an integer or a decimal, casts
from DOUBLE to an integer or a decimal (the exact binary value here), casts from strings that are not in the plain form, `date + integer`, `IN` over expressions, a non-boolean `WHERE`, integer literals next to TINYINT /
SMALLINT in arithmetic, typed NULL literals as an operand next to an expression that can fail (the class above), REAL literals and `CAST(x AS REAL / TINYINT / SMALLINT)` (not supported yet). The fuzz run counts the queries Nata
answered `not supported yet` to (about 20 to 30 in 20,000: `a CASE whose results are all NULL` is the commonest, then `// on decimals` and the remainder of two wide decimals): a coverage gap, never a difference.

**The oracle is run twice, with and without DuckDB's optimizer, and a target that matches either answer is accepted** (`optimizer-dependent-in-duckdb` above counts the queries where DuckDB disagrees with itself: it folds
constants, reorders conjuncts, rewrites `x * 2 > 0` to `x > 0` and pushes a `LIMIT` below the projection, which removes or adds errors). Without this rule the first runs of the fuzzer reported many differences that were only DuckDB's two behaviours (a `LIMIT` over a projection that fails, a constant folded or not).

**Findings of the fuzzer and the differential check, each fixed with a test** (the real ones): (1) a `WHERE` evaluated the second conjunct for rows whose first was NULL (DuckDB's filters do not): a top-level `AND`/`OR` in a `WHERE` now narrows
the rows (`tests/exec.rs`, mutants `where-and-...`, `where-or-...`); (2) `x BETWEEN lo AND hi` in a `WHERE` evaluated the upper bound for rows whose lower bound was NULL (seed 4, batch 471): now narrowed like the `AND` it is
(`where-between-evaluates-the-upper-bound-for-every-row`); (3) the projection ran on every row of a batch before the limit cut it: now after, for the rows up to `offset + n`, the rows `OFFSET` drops included, as DuckDB (`project-runs-before-the-limit`);
(4) `DECIMAL(p,p)` text (above); (5) an error message said "NaN or infinite": changed to "not a finite number" (no value, not even its class, in an error). Faults of the HARNESS found on the way: DuckDB's JSON writer leaves its opening
`[` on the line of the next marker when a query fails midway, which made the next query look failed (fixed in `engine.py`: the existing `prove` still catches 28 of 28 and 14 of 14 tests pass); `HUGEINT` and `REAL` come out of DuckDB's JSON as a string and as the double of a float; a one-column row holding an empty string is an empty line.

**The fuzzer itself**: `tests/test_subset.py` (6 tests: a seed replays exactly, the queries stay in the subset and cover its constructs, the control against DuckDB finds nothing, a **planted failure** (`limit-off-by-one` rendered for the target) is found, shrunk, printed with
its seed and replayed from the printed numbers, a replay without the fault is clean, and with `NATA_FUZZ_PROVE=1` the known-bad engines the subset can express are caught); `prove` (28 known-bad engines) still catches **28 of 28** with the control clean (2,000 queries) after the change of `engine.py`, 14 of 14 tests of the original fuzzer pass, and `prove-subset` (the six known-bad engines the subset can express: `nan-is-ieee`, `between-exclusive`, `limit-off-by-one`, `int-overflow-yields-null`, `decimal-via-double`, `string-compare-case-insensitive`) catches **6 of 6** with the control clean (caught after 33, 7, 2, 109, 32 and 3,238 queries; allowed 8,000).

### C. Mutants of the executor (`python3 crates/nata-exec/mutate.py`; one text edit each, on a private copy of `nata-exec`, `nata-parquet` and `nata`; the unmutated copy passes first; the real tree is hashed before and after)

The seven the card asks for come first (marked **card**); the others cover the exact-number, three-valued-logic, selection, cast, scan-hook and process rules. SURVIVED would be a gap in the tests, INVALID a mutant that does not apply or compile.

| mutant | suite | result | first failing tests |
|---|---|---|---|
| case-evaluates-every-branch (**card**: CASE evaluating every branch) | exec | CAUGHT | case_evaluates_only_the_branch_a_row_takes |
| error-raised-for-an-unselected-row (**card**: an error raised for an unselected row) | exec | CAUGHT | a_projection_evaluates_the_operands_of_and_or_and_between_only_where_they_matter, case_evaluates_only_the_branch_a_row_takes, errors_are_raised_only_for_selected_valid_rows |
| null-treated-as-false-inside-not (**card**: NULL treated as false inside NOT) | exec | CAUGHT | three_valued_logic_keeps_a_row_only_when_the_condition_is_true |
| offset-off-by-one (**card**: OFFSET off by one) | exec | CAUGHT | a_limit_shields_the_rows_beyond_it_from_projection_errors_but_not_the_filter, a_limit_stops_the_scan_and_the_report_says_how_far_it_got, a_query_without_a_column_to_read_still_counts_rows ... |
| limit-not-stopping-the-scan (**card**: LIMIT not stopping the scan (detected by the report: row groups read)) | exec | CAUGHT | a_limit_shields_the_rows_beyond_it_from_projection_errors_but_not_the_filter, a_limit_stops_the_scan_and_the_report_says_how_far_it_got |
| projection-reads-the-wrong-column (**card**: a projection reading the wrong column) | exec | CAUGHT | random_cancel_error_and_delay_scenarios_of_nata_sql_leave_nothing_behind |
| csv-writer-prints-null-as-an-empty-string (**card**: the CSV writer printing NULL as an empty string) | exec | CAUGHT | a_projection_evaluates_the_operands_of_and_or_and_between_only_where_they_matter, between_and_in_use_three_valued_logic, casts_between_text_and_numbers ... |
| decimal-division-truncates | exec | CAUGHT | cast::tests::decimals_round_half_away_and_report_what_does_not_fit, decimals_are_exact_and_follow_the_result_types_of_the_rules_page, eval::tests::decimal_arithmetic_follows_the_rules_page ... |
| decimal-division-by-zero-unchecked-as-zero | exec | CAUGHT | eval::tests::decimal_arithmetic_follows_the_rules_page |
| decimal-addition-operand-fit-unchecked | exec | CAUGHT | eval::tests::decimal_overflow_is_checked_against_the_digits_of_the_result |
| decimal-result-digits-unchecked | exec | CAUGHT | decimals_are_exact_and_follow_the_result_types_of_the_rules_page, errors_quote_the_query_and_never_a_cell_value, eval::tests::decimal_overflow_is_checked_against_the_digits_of_the_result |
| integer-overflow-wraps | exec | CAUGHT | integer_arithmetic_is_checked_against_the_type |
| integer-range-of-the-type-unchecked | exec | CAUGHT | integer_arithmetic_is_checked_against_the_type |
| min-by-minus-one-allowed | exec | CAUGHT | min_by_minus_one_is_an_error_for_a_remainder_and_an_integer_division |
| remainder-takes-the-sign-of-the-divisor | exec | CAUGHT | decimals_are_exact_and_follow_the_result_types_of_the_rules_page, eval::tests::decimal_arithmetic_follows_the_rules_page |
| unary-minus-of-the-minimum-allowed | exec | CAUGHT | integer_arithmetic_is_checked_against_the_type |
| decimal-compared-by-unscaled-value | exec | CAUGHT | decimals_are_exact_and_follow_the_result_types_of_the_rules_page |
| decimal-to-integer-truncates | exec | CAUGHT | decimals_are_exact_and_follow_the_result_types_of_the_rules_page |
| text-to-decimal-truncates | exec | CAUGHT | casts_between_text_and_numbers, num::tests::text_becomes_a_decimal_rounded_half_away_from_zero |
| float-to-decimal-tie-rounds-to-even | exec | CAUGHT | num::tests::floats_become_decimals_by_their_exact_binary_value |
| decimal-to-double-scale-off | exec | CAUGHT | floats_compare_with_nan_above_everything_and_negative_zero_equal_to_zero, num::tests::decimals_become_the_nearest_double |
| cast-int-to-narrower-int-unchecked | exec | CAUGHT | cast::tests::integers_narrow_with_a_range_check_and_a_null_stays_null, errors_quote_the_query_and_never_a_cell_value |
| cast-of-a-bad-date-is-null | exec | CAUGHT | cast::tests::text_to_numbers_dates_and_booleans, casts_between_text_and_numbers |
| null-and-false-is-null | exec | CAUGHT | a_projection_evaluates_the_operands_of_and_or_and_between_only_where_they_matter, between_and_in_use_three_valued_logic, eval::tests::three_valued_and_or ... |
| where-keeps-null-rows | exec | CAUGHT | errors_are_raised_only_for_selected_valid_rows, strings_dates_and_booleans, three_valued_logic_keeps_a_row_only_when_the_condition_is_true |
| where-or-evaluates-every-operand-on-all-rows | exec | CAUGHT | errors_are_raised_only_for_selected_valid_rows |
| where-and-evaluates-every-operand-on-all-rows | exec | CAUGHT | errors_are_raised_only_for_selected_valid_rows |
| and-or-in-a-projection-evaluate-every-operand | exec | CAUGHT | a_projection_evaluates_the_operands_of_and_or_and_between_only_where_they_matter |
| between-upper-bound-evaluated-for-every-row | exec | CAUGHT | a_projection_evaluates_the_operands_of_and_or_and_between_only_where_they_matter |
| where-between-evaluates-the-upper-bound-for-every-row | exec | CAUGHT | a_projection_evaluates_the_operands_of_and_or_and_between_only_where_they_matter |
| in-list-null-element-ignored | exec | CAUGHT | between_and_in_use_three_valued_logic |
| nan-below-everything | exec | CAUGHT | floats_compare_with_nan_above_everything_and_negative_zero_equal_to_zero |
| string-compare-case-insensitive | exec | CAUGHT | strings_dates_and_booleans |
| case-without-else-takes-the-first-branch | exec | CAUGHT | case_evaluates_only_the_branch_a_row_takes, strings_dates_and_booleans |
| project-runs-before-the-limit | exec | CAUGHT | a_limit_shields_the_rows_beyond_it_from_projection_errors_but_not_the_filter |
| filter-mask-inverted | exec | CAUGHT | a_chain_of_10000_terms_is_evaluated_on_a_big_stack, a_projection_evaluates_the_operands_of_and_or_and_between_only_where_they_matter, between_and_in_use_three_valued_logic ... |
| deep-expressions-on-the-small-stack | exec | CAUGHT | the test process crashed |
| dictionary-key-off-by-one | exec | CAUGHT | a_query_without_a_column_to_read_still_counts_rows, case_evaluates_only_the_branch_a_row_takes, casts_between_text_and_numbers ... |
| scan-columns-of-a-zero-column-query-skipped | exec | CAUGHT | a_query_without_a_column_to_read_still_counts_rows, between_and_in_use_three_valued_logic, casts_between_text_and_numbers ... |
| table-numbers-left-aligned | cli | CAUGHT | csv_and_the_aligned_table, rows_printed_before_an_error_make_the_exit_code_nonzero_and_the_error_says_so |
| data-error-exits-with-2 | cli | CAUGHT | missing_truncated_and_damaged_files_are_errors_naming_the_file |
| partial-result-not-announced | cli | CAUGHT | explain_and_explain_analyze_have_golden_output, random_sigint_closed_stdout_error_and_limit_scenarios_of_the_process, rows_printed_before_an_error_make_the_exit_code_nonzero_and_the_error_says_so |
| closed-stdout-is-an-error | cli | CAUGHT | a_closed_stdout_ends_the_query_quietly_with_exit_code_zero, random_sigint_closed_stdout_error_and_limit_scenarios_of_the_process |
| sigint-exits-with-1 | cli | CAUGHT | random_sigint_closed_stdout_error_and_limit_scenarios_of_the_process |
| caret-under-the-wrong-column | cli | CAUGHT | exit_codes_and_the_rendering_of_errors |
| explain-analyze-counts-the-wrong-rows | cli | CAUGHT | explain_and_explain_analyze_have_golden_output |
| explain-analyze-project-rows-are-the-filters-rows | cli | CAUGHT | explain_and_explain_analyze_have_golden_output |

**47 mutants: 47 CAUGHT, 0 SURVIVED, 0 INVALID**

Survivors found on the way and fixed with a test (each was shown CAUGHT afterwards): `case-evaluates-every-branch` (the failing expression was in the ELSE branch only, now also in THEN), `decimal-addition-operand-fit-unchecked` (the case where the exact sum fits but an operand does not fit the rescaled result type), `where-and-evaluates-every-operand-on-all-rows` and `string-compare-case-insensitive` (their conjuncts had been pushed into the scan: the tests now use expressions that stay in the executor), `explain-analyze-project-rows-are-the-filters-rows` (a golden query with a filter and a limit). One mutant of the first list was INVALID (it changed whitespace only) and was replaced.

### D. Reliability

| scenario family | count | result |
|---|---|---|
| **in-process** (`crates/nata-exec/tests/scenarios.rs`; the fault opener of slice 3; a random query of nine, with `LIMIT`/`OFFSET`, a filter that stays in the executor, a data error in a chosen row; threads 1 to 8, window 1 to 2 x threads, ordered or not; disturbance: nothing, a sink that fails after 1 to 6 batches (a closed stdout), an external cancel after 0 to 3 ms (what Ctrl-C does), 1 to 3 injected open failures / read failures / panics, a division by zero in a random row, random delays) | 3,000, seed 1 | **pass** in 25 s: `complete` 596, `cancelled` 385, `cancel-too-late` 238, `sink-failed` 489, `sink-not-reached` 110, `fault-error` 346, `fault-panic` 76, `data-error` 203, `not-reached` 423, `unordered-limit-with-fault` 134 |
| the same under a **busy loop on every core** (`NATA_COMPETITORS=1`: 8 spinning threads for the whole run) | 3,000, seed 2 | **pass** in 101 s: `complete` 627, `cancelled` 484, `cancel-too-late` 102, `sink-failed` 486, `sink-not-reached` 101, `fault-error` 374, `fault-panic` 78, `data-error` 185, `not-reached` 432, `unordered-limit-with-fault` 131 |
| the default of `cargo test` | 600, seed 20261106 | pass in 5 s |
| **process** (`crates/nata/tests/sql_reliability.rs`; the real `nata sql` on a 240,000-row file of 30 row groups; threads 1 to 8; CSV or table; a consumer that is sometimes slow; disturbance: SIGINT 0 to 250 ms after the spawn, stdout closed after 0 to 3 MB, a division by zero in a random row, a `LIMIT`, nothing) | 2,000, seed 1 | **pass** in 320 s: `sigint-stopped` 466 (exit 130, `interrupted`, INCOMPLETE if rows were printed, within 30 s), `sigint-too-late` 188 (finished first: the complete result), `sigint-before-the-handler` 5, `stdout-closed` 645 (exit 0, stderr empty), `data-error` 347 (exit 1, the message, INCOMPLETE if rows were printed), `limit` 167 (the right number of lines), `complete` 182 |
| the same under the busy loop | 2,000, seed 2 | **pass** in 793 s: `sigint-stopped` 640, `sigint-too-late` 5, `sigint-before-the-handler` 6, `stdout-closed` 685, `data-error` 316, `limit` 168, `complete` 180 |
| the default of `cargo test` | 60 | pass in 10 s |

In-process checks after every scenario: no handle (the opener counts them), no footer, no decoded byte counted alive, no thread of the query alive (the process's thread count is what it was before); the output is a prefix of / equal to the single-thread output (unordered: a sub-multiset); the error is the one of the LOWEST failing (file, row group) with the same text as the single-thread run; a 30-second watchdog aborts the process and names the scenario on a hang. Replay: `NATA_SCENARIO_SEED=<seed> NATA_SCENARIO_ONLY=<n>`, `NATA_PROC_SEED=<seed> NATA_PROC_ONLY=<n>`.

**Found by the process scenarios**: a SIGINT that arrives before the process has installed its handler (scenario 646 of seed 1: 0 ms after the spawn) kills it by the signal itself, which is what Ctrl-C does to any program and what a shell shows as 130; nothing had started, nothing is left. The test now has a class for it (allowed only for a signal sent in the first 30 ms); the handler is installed after the query is read from stdin on purpose (a Ctrl-C while `nata sql` waits for the query on stdin must end it). Also tested by hand-written tests: Ctrl-C while the footers of a 4,000-file glob are still being read (`sql_cli.rs`: exit 130 and nothing printed, or the finished result).

### E. Fault tolerance through `nata sql` (`crates/nata/tests/sql_cli.rs`, `crates/nata-exec/tests/scenarios.rs`)

- **Missing file**: exit 1, the file named, nothing on stdout. **Truncated file**: cut at 23 points down to the first byte: every one exit 1, the file named, nothing on stdout (no partial result presented as complete).
  **Damaged file**: the magic at both ends, the footer length and the first page-header bytes flipped: each is either found (exit 1, the file named, no panic) or changes nothing that was read (the same rows as the clean file); a damaged value that parquet-rs cannot detect (no page checksum on the file) is outside this test.
- **A row group that fails after earlier ones were printed**: the rows before it are printed, the exit code is 1, the message names the file and the row group and says that the output is INCOMPLETE and how many rows were printed (`rows_printed_before_an_error_...`; the table format too); an error before any row prints nothing, not even a header.
- **Injected I/O errors, panics in the opener, delays**: through the fault opener of slice 3 in the in-process scenarios (D): the error is that of the LOWEST failing (file, row group) with the same text as the single-thread run, the rows before it are delivered in ordered mode, no handle / footer / buffer / thread is left.
- **No cell value in an error**: `errors_quote_the_query_and_never_a_cell_value` (executor, six error kinds against eight values of the file) and `errors_never_carry_a_cell_value` (the process, seven queries over a file of `SECRET-n` strings).

### F. Resource bounds through `nata sql` (`crates/nata/tests/sql_resource_bounds.rs`, release build)

| test | result |
|---|---|
| **10,000 small files** (3 rows each, a glob) under `ulimit -n 256` (checked inside the shell that starts `nata`), `SELECT id + 1 AS next FROM '<glob>' WHERE id + 0 >= 0` (a filter that stays in the executor), CSV | 1 thread: all 30,000 rows right, `files_opened=10000`, `peak_open_files=1`, `peak_footers=1`, `peak_rss_kib=13196`; 8 threads: all rows right, `peak_open_files=5` (<= threads + 1), `peak_footers=6` (<= threads + 2), `peak_rss_kib=13640` |
| the same glob with `LIMIT 25` and 8 threads | 25 rows, `files_opened=20` of 10,000 (`row_groups=20`), `peak_open_files=2`: a limit does not read the footers it does not need |
| **memory**: a 100-row-group file (2 MB decoded each), a filter, a projection of three columns, a consumer of stdout that reads 64 KB per 0.5 ms | 4 threads: peak RSS 58,128 KiB <= bound 99,209 KiB (`base + 3 x window x M + 32 MiB`, base 10,388 KiB measured with `LIMIT 0`, M = 2,391,600 B measured by an independent parquet-rs read); 4 threads unordered: 64,632 KiB <= 99,209; 1 thread: 21,296 <= 57,169; the scan's own accounting stays within `window x largest task` |
| the control arm: the same query with `--window 100000` | peak RSS 254,304 KiB > the bound 71,182 KiB for window 4: the bound is a property of the window, and the test can fail |

Planning 10,000 files reads 10,000 footers before the query starts (one after the other, each opened and closed: that is why the handle bound holds); not timed here.

### G. Other checks

| check | result |
|---|---|
| golden `EXPLAIN` and `EXPLAIN ANALYZE` (`crates/nata/tests/golden/explain*.txt`, times and the memory line masked, thread count and window fixed so the counts are exact; the golden run 15 times in a row) | pass; the mutants `explain-analyze-counts-the-wrong-rows` and `...-project-rows-are-the-filters-rows` are caught only by it |
| `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (340 tests pass, 0 fail, 51 result lines; the binaries add up to 254 s, of which the slice 3 DuckDB differential and fuzz tests are 162 s; the new ones: in-process scenarios 5 s, process scenarios 9 s, resource bounds 8 s), `cargo deny check` (advisories, bans, licenses, sources ok) | pass |
| `scripts/check-leaks.sh --staged` before each commit | clean |
| `unsafe` in the workspace crates (`scripts/unsafe-inventory.py nata`) | 0. New dependencies: `signal-hook` 0.4.6 (95 `unsafe` keywords), `signal-hook-registry` 1.4.8 (35), `errno` 0.3.14 (12) (`libc` was already in the tree) |

## Readings decided while building (the strict reading is implemented; for the parent to confirm)

1. **A source file that cannot be read is class `execution`**, exit 1 (the card asks exit 1 for missing, truncated and damaged files; the front end had class `bind`).
2. **The order of evaluation** (`dialect.md` 9b, added): `AND`/`OR`/`CASE`/`BETWEEN` evaluate an operand only for the rows that need it; a `WHERE` follows TRUE rows only (a NULL first conjunct drops the row); with a `LIMIT` the select list is evaluated for the first `offset + n` rows the
   `WHERE` keeps and not later ones (the rows `OFFSET` drops are still evaluated). DuckDB agrees on the `WHERE` and the `LIMIT`, not on the projection's `AND`/`OR`. The alternative (evaluate every operand for every selected row) would fail `WHERE b <> 0 AND a // b > 1`, which ADR 0003 decision 4 and `decimal-rules.md` 1 promise to work.
3. **An operand next to a NULL is still evaluated** (DuckDB skips it when the NULL is constant): `NULL + (1 // 0)` is an error here. Strict, simple, listed.
4. **Integer literals in arithmetic are typed by value** (`dialect.md` 3): `ti + 1` is INTEGER, DuckDB's TINYINT. Implemented as written, listed in section 9 with a Nata-only expectation.
5. **Decimal literals below one are `DECIMAL(p,p)`** and print without the leading zero as DuckDB prints every `DECIMAL(p,p)` (the CSV writer changed, `nata scan` too). A literal `0.10` is `.10` as text and equal in value to DuckDB's.
6. **String casts take only the plain form** (an integer: sign and digits; `true`/`false`; `YYYY-MM-DD`; decimals and doubles with point and exponent, `inf`, `nan`); DuckDB also reads `'1.5'` as 2, `'yes'`, `'2024/02/29'`, `'1_000'`. Listed in section 9, one query each.
7. **A non-boolean `WHERE` is a bind error** (DuckDB converts an integer); an `IN` list takes literals only (a `CAST` of a literal is not one).
8. **Output**: the aligned table takes its widths from the header and the first 1,000 rows and prints a later wider cell in full (streaming forbids more); numbers are right-aligned, with their header; `EXPLAIN` and `EXPLAIN ANALYZE` print text with or without `--csv`; `EXPLAIN ANALYZE` times are CPU time summed over the workers (they can exceed the wall time), the scan line carries the wall time.
9. **Developer options added to `nata sql`**: `--threads`, `--window`, `--unordered`, `--report` (the scan's report line and the peak memory on stderr, for the resource tests).
10. **`SELECT 1 FROM 'f'`** reads the first supported column of the first file (the front end's open point 12): the cheapest correct way to count rows through the existing scan.

## What this does not show, and could not be verified

- **No timing is claimed.** The card's performance rows (planning under 1 ms: the front end measured 97 us; end-to-end within 1.2x of `nata scan`; scalability at 1 and 8 threads on SF10) and portability on ARM64 are step 4. One **indication**, from a shared box with other jobs and not a measurement (release build, SF1 `lineitem.parquet`, the card's example without its `LIMIT` so that 248,078 of 6,001,215 rows are returned, CSV to `/dev/null`, 7 runs, medians): `nata scan` with the same columns and filters 0.071 s, `nata sql` of the example 0.079 s (1.11x), `nata sql` of the plain three columns 0.093 s, DuckDB 2.0.0 CLI of the example 0.159 s; with one thread `nata scan` 0.192 s and `nata sql` 0.203 s (1.06x). The evaluator is scalar (a `match` per row); the card's 1.2x target is plausible, the benchmark decides.
- **ARM64 is unmeasured**: every number is from one x86-64 box. The CI job runs on both CPU types (the first run there is the first evidence).
- **Differences that only a real client would find**: DuckDB's answers for casts and typing were read for the cases above; any construct the generator does not reach (nested `CASE` operands of different decimal types beyond depth 3, glob queries over files with different row-group sizes, files with more than one batch per row group in the fuzzer) is covered only by the hand-written queries.
- **The fuzzer's data is small** (at most 25 rows, row groups of 3 and 5): effects that need several 8,192-row batches (a batch boundary inside an expression, a limit inside the second batch) are covered by `tests/exec.rs` and the 240,000-row process scenarios, not by the oracle.
- **A damaged value inside a page** that the file's own checks cannot detect is not an error (no page checksum is verified: carried-in item R4 of the card, not done here).
- **Footers of a glob are read at bind time**, one after the other (10,000 files: 10,000 footer reads before the query starts; `docs/improvements.md` has the item): the resource test shows it works under `ulimit -n 256`, not that it is fast.
- **SIGINT** is tested at random moments of a running process (4,060 scenarios) and once during the footer reads of a 4,000-file glob (exit 130 and nothing printed); that the footer reads stop promptly is by construction (the interrupt flag is checked between two footers), not timed. A Ctrl-C from a terminal is the same signal; the one case a test cannot show is the Ctrl-C of a terminal's whole process group (a `| head` pipeline), which delivers the signal to both processes.
- The leak counters (handles, footers, buffers, threads) exist in-process only; the process-level scenarios show that the process ends and what it prints, not that nothing leaked inside.
- Not done: `TRY`, `NULLIF`, any function, aggregates, `ORDER BY`, a parallel `Project` under a `LIMIT` (the consuming thread projects up to `offset + n` rows), the memory manager (register R1: the operators stream and hold nothing beyond the scan's window).

## Raw outputs

`bench/results/raw/20261106-sql-execution-x86/`: `differential.txt` (the six `nata-check` runs), `fuzz-3-seeds.log` (the three 20,000-query runs with their classes), `fuzzer-prove.log` (the original fuzzer's tests, `prove` 28 of 28, `prove-subset` 6 of 6), `mutants.md` and `mutants-cli-rerun.log` (the 47 mutants; the 8 process mutants rerun after the last edits of the process tests), `scenarios-in-process.log`, `scenarios-process.log`, `resource-bounds-release.log`, `workspace-test.log`.

## Reproduce

```bash
scripts/gen-tpch.sh 0.01 data/tpch-sf0.01 && scripts/gen-sql-check-data.sh data/tpch-sf0.01     # the data (DuckDB writes it; nothing is committed)
cargo build -p nata -p nata-check
for d in scan sql sql-differences smoke decimal decimal-differences; do target/debug/nata-check --data data/tpch-sf0.01 --queries check/queries/$d; done
cd spikes/sqlfuzz && python3 -I fuzz.py run-subset --seed 20261106 --cases 20000 --jobs 4 --target-cmd "python3 -I adapters/nata_adapter.py"   # any seed; ~3 minutes
python3 -I tests/test_subset.py; NATA_FUZZ_PROVE=1 python3 -I tests/test_subset.py; python3 -I fuzz.py prove --seed 1 --cases 6000
python3 crates/nata-exec/mutate.py                                                  # the mutant table (about 15 minutes)
cargo test -p nata-exec --test scenarios                                            # 600 scenarios; NATA_SCENARIOS=3000 NATA_SCENARIO_SEED=1 NATA_COMPETITORS=1 for the long run
cargo test -p nata --test sql_reliability                                           # 60 process scenarios; NATA_PROC_SCENARIOS=2000 NATA_COMPETITORS=1 for the long run
cargo test --release -p nata --test sql_resource_bounds -- --nocapture              # 10,000 files under ulimit -n 256, memory bound
NATA_UPDATE_GOLDEN=1 cargo test -p nata --test sql_cli golden                       # rewrite the EXPLAIN goldens after a deliberate change
```
