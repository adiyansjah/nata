| mutant | suite | result | first failing tests |
|---|---|---|---|
| error-raised-for-an-unselected-row | exec | CAUGHT | a_projection_evaluates_the_operands_of_and_or_and_between_only_where_they_matter, case_evaluates_only_the_branch_a_row_takes, errors_are_raised_only_for_selected_valid_rows |
| aggregate-sum-null-bitmap-ignored | exec | CAUGHT | aggregate::tests::group_vector_keeps_selected_null_arguments_and_distinct_groups |
| aggregate-group-index-off-by-one | exec | CAUGHT | a_hot_string_group_coalesces_without_recursive_repartition, aggregate::tests::group_vector_keeps_selected_null_arguments_and_distinct_groups, aggregate::tests::partial_polls_at_batch_entry_and_bounded_intervals_in_large_batches ... |
| decimal-result-digits-unchecked | exec | CAUGHT | eval::tests::decimal_overflow_is_checked_against_the_digits_of_the_result |
| aggregate-cancel-poll-per-row | exec | CAUGHT | aggregate::tests::partial_polls_at_batch_entry_and_bounded_intervals_in_large_batches |

5 mutants: 5 CAUGHT, 0 SURVIVED, 0 INVALID
