# Cumulative executor/CLI mutation verification

Both private unmutated exec/CLI controls passed. The 15 completed catches in
`mutants-first-segment.txt` and 48 in `mutants-remaining.md` are disjoint.
The interrupted exponential CASE attempt is not a catch. The CASE mutant
was rerun against its original explicit selected-branch guard and caught.
Compile failures and global timeouts remain INVALID. All other mutants retain
their complete original suite. No production source changed during either segment.

| mutant | suite | result | first failing tests |
|---|---|---|---|
| scratch-reservation-not-released | exec | CAUGHT | aggregate::pipeline::tests::competing_scratch_requests_remain_retriable_pressure, aggregate::pipeline::tests::reusable_scratch_charges_capacity_once_and_releases_every_chunk |
| borrowed-key-equality-skipped | exec | CAUGHT | aggregate::tests::borrowed_lookup_resolves_collisions_and_never_reads_null_junk |
| aggregate-cancel-poll-removed | exec | CAUGHT | aggregate::tests::partial_polls_at_batch_entry_and_bounded_intervals_in_large_batches |
| aggregate-cancel-poll-per-row | exec | CAUGHT | aggregate::tests::partial_polls_at_batch_entry_and_bounded_intervals_in_large_batches |
| canonical-hash-recursion-seed-lost | exec | CAUGHT | aggregate::hashing::tests::canonical_hash_vectors_and_recursive_seeds, pressure_releases_reservations_and_recursive_merge_reads_every_partition |
| owner-initialization-cleanup-skipped | exec | CAUGHT | owner_lock_creation_failure_leaves_no_directory |
| restart-cleanup-skipped | exec | CAUGHT | restart_cleanup_skips_live_and_removes_killed_owner |
| spill-recursion-bound-removed | exec | CAUGHT | recursion_bound_is_enforced_under_a_process_deadline |
| spill-partition-not-read-back | exec | CAUGHT | a_hot_string_group_coalesces_without_recursive_repartition, altered_inner_offsets_rejected_even_with_repaired_checksum, decoded_buffers_retain_reservation_and_cannot_multiply_workspace ... |
| spill-reservation-not-released | exec | CAUGHT | cancel_during_spill_joins_io_and_removes_every_file, pressure_releases_reservations_and_recursive_merge_reads_every_partition |
| spill-descriptor-cap-removed | exec | CAUGHT | descriptors_are_capped_and_release_on_drop |
| spill-cleanup-skipped-on-error | exec | CAUGHT | injected_write_failure_returns_error_and_removes_every_file, quota_short_write_error_cancel_and_panic_leave_no_files |
| aggregate-partition-double-counted | exec | CAUGHT | aggregate::tests::final_partition_merges_every_contribution_exactly_once, deep_aggregate_having_and_projection_use_sized_stacks, nulls_empty_input_and_selected_arguments ... |
| aggregate-having-before-merge | exec | CAUGHT | pressure_releases_reservations_and_recursive_merge_reads_every_partition, typed_sql_replay_matches_resident_for_every_key_and_state_type |
| bare-projection-allocates-selection | exec | CAUGHT | exec::projection_memory_tests::bare_projection_allocates_no_row_selection |
| case-evaluates-every-branch | exec-case | CAUGHT | case_evaluates_only_the_branch_a_row_takes |
| error-raised-for-an-unselected-row | exec | CAUGHT | a_projection_evaluates_the_operands_of_and_or_and_between_only_where_they_matter, case_evaluates_only_the_branch_a_row_takes, errors_are_raised_only_for_selected_valid_rows |
| null-treated-as-false-inside-not | exec | CAUGHT | three_valued_logic_keeps_a_row_only_when_the_condition_is_true |
| offset-off-by-one | exec | CAUGHT | a_limit_shields_the_rows_beyond_it_from_projection_errors_but_not_the_filter, a_limit_stops_the_scan_and_the_report_says_how_far_it_got, a_query_without_a_column_to_read_still_counts_rows ... |
| limit-not-stopping-the-scan | exec | CAUGHT | a_limit_shields_the_rows_beyond_it_from_projection_errors_but_not_the_filter, a_limit_stops_the_scan_and_the_report_says_how_far_it_got |
| projection-reads-the-wrong-column | exec | CAUGHT | bare_columns_preserve_values_order_duplicates_and_nulls, random_cancel_error_and_delay_scenarios_of_nata_sql_leave_nothing_behind |
| csv-writer-prints-null-as-an-empty-string | exec | CAUGHT | a_projection_evaluates_the_operands_of_and_or_and_between_only_where_they_matter, bare_columns_preserve_values_order_duplicates_and_nulls, between_and_in_use_three_valued_logic ... |
| decimal-division-truncates | exec | CAUGHT | cast::tests::decimals_round_half_away_and_report_what_does_not_fit, decimals_are_exact_and_follow_the_result_types_of_the_rules_page, eval::tests::decimal_arithmetic_follows_the_rules_page ... |
| decimal-division-by-zero-unchecked-as-zero | exec | CAUGHT | eval::tests::decimal_arithmetic_follows_the_rules_page |
| decimal-addition-operand-fit-unchecked | exec | CAUGHT | eval::tests::decimal_overflow_is_checked_against_the_digits_of_the_result |
| decimal-result-digits-unchecked | exec | CAUGHT | decimals_are_exact_and_follow_the_result_types_of_the_rules_page, errors_quote_the_query_and_never_a_cell_value, eval::tests::decimal_overflow_is_checked_against_the_digits_of_the_result |
| integer-overflow-wraps | exec | CAUGHT | integer_arithmetic_is_checked_against_the_type |
| integer-range-of-the-type-unchecked | exec | CAUGHT | integer_arithmetic_is_checked_against_the_type |
| min-by-minus-one-allowed | exec | CAUGHT | min_by_minus_one_is_an_error_for_a_remainder_and_an_integer_division |
| remainder-takes-the-sign-of-the-divisor | exec | CAUGHT | decimals_are_exact_and_follow_the_result_types_of_the_rules_page, eval::tests::decimal_arithmetic_follows_the_rules_page |
| unary-minus-of-the-minimum-allowed | exec | CAUGHT | integer_arithmetic_is_checked_against_the_type, nested_negations_overflow_at_the_original_integer_width_only_for_reached_rows |
| decimal-compared-by-unscaled-value | exec | CAUGHT | decimals_are_exact_and_follow_the_result_types_of_the_rules_page |
| decimal-to-integer-truncates | exec | CAUGHT | decimals_are_exact_and_follow_the_result_types_of_the_rules_page |
| text-to-decimal-truncates | exec | CAUGHT | casts_between_text_and_numbers, num::tests::text_becomes_a_decimal_rounded_half_away_from_zero |
| float-to-decimal-tie-rounds-to-even | exec | CAUGHT | num::tests::floats_become_decimals_by_their_exact_binary_value |
| decimal-to-double-scale-off | exec | CAUGHT | floats_compare_with_nan_above_everything_and_negative_zero_equal_to_zero, num::tests::decimals_become_the_nearest_double |
| cast-int-to-narrower-int-unchecked | exec | CAUGHT | between_in_a_case_condition_evaluates_upper_bounds_for_null_but_not_false_lower_bounds, cast::tests::integers_narrow_with_a_range_check_and_a_null_stays_null, errors_quote_the_query_and_never_a_cell_value |
| cast-of-a-bad-date-is-null | exec | CAUGHT | cast::tests::text_to_numbers_dates_and_booleans, casts_between_text_and_numbers |
| null-and-false-is-null | exec | CAUGHT | a_projection_evaluates_the_operands_of_and_or_and_between_only_where_they_matter, between_and_in_use_three_valued_logic, eval::tests::three_valued_and_or ... |
| where-keeps-null-rows | exec | CAUGHT | errors_are_raised_only_for_selected_valid_rows, strings_dates_and_booleans, three_valued_logic_keeps_a_row_only_when_the_condition_is_true |
| where-or-evaluates-every-operand-on-all-rows | exec | CAUGHT | errors_are_raised_only_for_selected_valid_rows |
| where-and-evaluates-every-operand-on-all-rows | exec | CAUGHT | errors_are_raised_only_for_selected_valid_rows |
| and-or-in-a-projection-evaluate-every-operand | exec | CAUGHT | a_projection_evaluates_the_operands_of_and_or_and_between_only_where_they_matter |
| between-null-lower-bound-skips-upper-bound | exec | CAUGHT | a_projection_evaluates_the_operands_of_and_or_and_between_only_where_they_matter, between_in_a_case_condition_evaluates_upper_bounds_for_null_but_not_false_lower_bounds |
| between-upper-bound-evaluated-for-every-row | exec | CAUGHT | a_projection_evaluates_the_operands_of_and_or_and_between_only_where_they_matter, between_in_a_case_condition_evaluates_upper_bounds_for_null_but_not_false_lower_bounds |
| where-between-evaluates-the-upper-bound-for-every-row | exec | CAUGHT | a_projection_evaluates_the_operands_of_and_or_and_between_only_where_they_matter, between_in_a_case_condition_evaluates_upper_bounds_for_null_but_not_false_lower_bounds |
| in-list-null-element-ignored | exec | CAUGHT | between_and_in_use_three_valued_logic |
| nan-below-everything | exec | CAUGHT | floats_compare_with_nan_above_everything_and_negative_zero_equal_to_zero |
| string-compare-case-insensitive | exec | CAUGHT | strings_dates_and_booleans |
| case-without-else-takes-the-first-branch | exec | CAUGHT | between_in_a_case_condition_evaluates_upper_bounds_for_null_but_not_false_lower_bounds, case_evaluates_only_the_branch_a_row_takes, deep_aggregate_having_and_projection_use_sized_stacks ... |
| project-runs-before-the-limit | exec | CAUGHT | a_limit_shields_the_rows_beyond_it_from_projection_errors_but_not_the_filter |
| filter-mask-inverted | exec | CAUGHT | a_chain_of_10000_terms_is_evaluated_on_a_big_stack, a_projection_evaluates_the_operands_of_and_or_and_between_only_where_they_matter, an_operator_chain_of_10000_terms_is_evaluated_on_a_2_mb_thread_for_each_operator ... |
| deep-expressions-on-the-small-stack | exec | CAUGHT | the test process crashed |
| dictionary-key-off-by-one | exec | CAUGHT | a_hot_string_group_coalesces_without_recursive_repartition, a_query_without_a_column_to_read_still_counts_rows, aggregate::hashing::tests::columns_match_owned_keys_for_selected_null_dictionary_and_float_values ... |
| scan-columns-of-a-zero-column-query-skipped | exec | CAUGHT | a_query_without_a_column_to_read_still_counts_rows, between_and_in_use_three_valued_logic, casts_between_text_and_numbers ... |
| table-numbers-left-aligned | cli | CAUGHT | csv_and_the_aligned_table, rows_printed_before_an_error_make_the_exit_code_nonzero_and_the_error_says_so |
| data-error-exits-with-2 | cli | CAUGHT | missing_truncated_and_damaged_files_are_errors_naming_the_file |
| partial-result-not-announced | cli | CAUGHT | a_later_file_with_another_schema_is_an_error_naming_the_file_found_when_the_scan_reaches_it, random_sigint_closed_stdout_error_and_limit_scenarios_of_the_process, rows_printed_before_an_error_make_the_exit_code_nonzero_and_the_error_says_so |
| closed-stdout-is-an-error | cli | CAUGHT | a_closed_stdout_ends_the_query_quietly_with_exit_code_zero, random_sigint_closed_stdout_error_and_limit_scenarios_of_the_process |
| sigint-exits-with-1 | cli | CAUGHT | ctrl_c_while_the_footers_of_a_glob_are_still_being_read_stops_the_query_with_exit_code_130, random_sigint_closed_stdout_error_and_limit_scenarios_of_the_process |
| caret-under-the-wrong-column | cli | CAUGHT | exit_codes_and_the_rendering_of_errors |
| explain-analyze-counts-the-wrong-rows | cli | CAUGHT | explain_and_explain_analyze_have_golden_output |
| explain-analyze-project-rows-are-the-filters-rows | cli | CAUGHT | explain_and_explain_analyze_have_golden_output |

63 mutants: 63 CAUGHT, 0 SURVIVED, 0 INVALID.
