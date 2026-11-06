# Slice 4 SQL benchmark

Settled all blocks: True. Planning corpus: 289 queries (including rejection paths).
Warm OS cache; process timing includes startup, metadata, output. Nata planning includes parser thread startup and footer lookup; DuckDB EXPLAIN in-process includes plan rendering. Both example arms stop at 10 and render CSV; SQL adds the net expression. Startup probes are separate, not subtracted.

| metric | min s | median s | max s | verdict |
|---|---:|---:|---:|---|
| files10000/scan/8 | 0.108569042 | 0.108569042 | 0.108569042 |  |
| files10000/sql/8 | 0.140727894 | 0.140727894 | 0.140727894 |  |
| plan-card-example/duckdb/1 | 0.002235184 | 0.002360224 | 0.002398139 |  |
| plan-card-example/nata/1 | 0.000430865 | 0.000440345 | 0.000455434 | MET |
| plan-card-example/twin/1 | 0.000423589 | 0.000444244 | 0.000448180 |  |
| plan-sql-differences-d3_decimal_literal_below_one_is_decimal_p_p/duckdb/1 | 0.000555795 | 0.000581555 | 0.000610349 |  |
| plan-sql-differences-d3_decimal_literal_below_one_is_decimal_p_p/nata/1 | 0.000157780 | 0.000169224 | 0.000175295 | MET |
| plan-sql-differences-d3_decimal_literal_below_one_is_decimal_p_p/twin/1 | 0.000162080 | 0.000164045 | 0.000166175 |  |
| plan-sql-differences-d9_case_decimals_beyond_38/duckdb/1 | 0.000630860 | 0.000659090 | 0.000664050 |  |
| plan-sql-differences-d9_case_decimals_beyond_38/nata/1 | 0.000166225 | 0.000171715 | 0.000185730 | MET |
| plan-sql-differences-d9_case_decimals_beyond_38/twin/1 | 0.000167145 | 0.000172975 | 0.000178920 |  |
| plan-sql-differences-d9_cast_boolean_to_integer/duckdb/1 | 0.000539520 | 0.000548270 | 0.000562505 |  |
| plan-sql-differences-d9_cast_boolean_to_integer/nata/1 | 0.000149685 | 0.000155310 | 0.000157870 | MET |
| plan-sql-differences-d9_cast_boolean_to_integer/twin/1 | 0.000145920 | 0.000151795 | 0.000159890 |  |
| plan-sql-differences-d9_cast_double_to_varchar/duckdb/1 | 0.000510665 | 0.000525640 | 0.000544150 |  |
| plan-sql-differences-d9_cast_double_to_varchar/nata/1 | 0.000147085 | 0.000149820 | 0.000152460 | MET |
| plan-sql-differences-d9_cast_double_to_varchar/twin/1 | 0.000141915 | 0.000149705 | 0.000152395 |  |
| plan-sql-differences-d9_cast_short_date/duckdb/1 | 0.000471785 | 0.000497245 | 0.000518589 |  |
| plan-sql-differences-d9_cast_short_date/nata/1 | 0.000141660 | 0.000145825 | 0.000154070 | MET |
| plan-sql-differences-d9_cast_short_date/twin/1 | 0.000147945 | 0.000149150 | 0.000228855 |  |
| plan-sql-differences-d9_cast_slash_date/duckdb/1 | 0.000479224 | 0.000498250 | 0.000524934 |  |
| plan-sql-differences-d9_cast_slash_date/nata/1 | 0.000147400 | 0.000152790 | 0.000156900 | MET |
| plan-sql-differences-d9_cast_slash_date/twin/1 | 0.000140695 | 0.000151310 | 0.000163160 |  |
| plan-sql-differences-d9_cast_string_with_exponent_to_integer/duckdb/1 | 0.000464650 | 0.000491465 | 0.000520010 |  |
| plan-sql-differences-d9_cast_string_with_exponent_to_integer/nata/1 | 0.000142140 | 0.000155940 | 0.000245715 | MET |
| plan-sql-differences-d9_cast_string_with_exponent_to_integer/twin/1 | 0.000144980 | 0.000151285 | 0.000156455 |  |
| plan-sql-differences-d9_cast_string_with_fraction_to_integer/duckdb/1 | 0.000475715 | 0.000483905 | 0.000500175 |  |
| plan-sql-differences-d9_cast_string_with_fraction_to_integer/nata/1 | 0.000143400 | 0.000147850 | 0.000162550 | MET |
| plan-sql-differences-d9_cast_string_with_fraction_to_integer/twin/1 | 0.000148665 | 0.000150755 | 0.000158130 |  |
| plan-sql-differences-d9_cast_string_with_underscore/duckdb/1 | 0.000480294 | 0.000488565 | 0.000495810 |  |
| plan-sql-differences-d9_cast_string_with_underscore/nata/1 | 0.000148225 | 0.000150635 | 0.000155905 | MET |
| plan-sql-differences-d9_cast_string_with_underscore/twin/1 | 0.000145810 | 0.000149505 | 0.000153455 |  |
| plan-sql-differences-d9_cast_t_to_boolean/duckdb/1 | 0.000455870 | 0.000463635 | 0.000469835 |  |
| plan-sql-differences-d9_cast_t_to_boolean/nata/1 | 0.000141520 | 0.000144240 | 0.000148835 | MET |
| plan-sql-differences-d9_cast_t_to_boolean/twin/1 | 0.000139865 | 0.000143680 | 0.000144235 |  |
| plan-sql-differences-d9_cast_yes_to_boolean/duckdb/1 | 0.000459654 | 0.000488930 | 0.000507580 |  |
| plan-sql-differences-d9_cast_yes_to_boolean/nata/1 | 0.000141755 | 0.000149580 | 0.000157655 | MET |
| plan-sql-differences-d9_cast_yes_to_boolean/twin/1 | 0.000144405 | 0.000149870 | 0.000153985 |  |
| plan-sql-differences-d9_date_minus_date/duckdb/1 | 0.000503340 | 0.000510205 | 0.000570919 |  |
| plan-sql-differences-d9_date_minus_date/nata/1 | 0.000137000 | 0.000144380 | 0.000151490 | MET |
| plan-sql-differences-d9_date_minus_date/twin/1 | 0.000137125 | 0.000138685 | 0.000151875 |  |
| plan-sql-differences-d9_date_plus_integer/duckdb/1 | 0.000504185 | 0.000526270 | 0.000533365 |  |
| plan-sql-differences-d9_date_plus_integer/nata/1 | 0.000139025 | 0.000140850 | 0.000154095 | MET |
| plan-sql-differences-d9_date_plus_integer/twin/1 | 0.000138770 | 0.000142645 | 0.000146395 |  |
| plan-sql-differences-d9_dec_mod_tight_result_type/duckdb/1 | 0.000972720 | 0.000991595 | 0.001028905 |  |
| plan-sql-differences-d9_dec_mod_tight_result_type/nata/1 | 0.000171105 | 0.000174650 | 0.000179215 | MET |
| plan-sql-differences-d9_dec_mod_tight_result_type/twin/1 | 0.000167055 | 0.000183420 | 0.000185180 |  |
| plan-sql-differences-d9_decimal_compared_with_double/duckdb/1 | 0.000677175 | 0.000704000 | 0.000728634 |  |
| plan-sql-differences-d9_decimal_compared_with_double/nata/1 | 0.000159195 | 0.000166940 | 0.000238050 | MET |
| plan-sql-differences-d9_decimal_compared_with_double/twin/1 | 0.000157814 | 0.000164715 | 0.000178470 |  |
| plan-sql-differences-d9_decimal_division_too_wide/duckdb/1 | 0.000651940 | 0.000657635 | 0.000668145 |  |
| plan-sql-differences-d9_decimal_division_too_wide/nata/1 | 0.000153435 | 0.000157745 | 0.000160185 | MET |
| plan-sql-differences-d9_decimal_division_too_wide/twin/1 | 0.000154574 | 0.000156415 | 0.000161800 |  |
| plan-sql-differences-d9_decimal_mul_scale_too_large/duckdb/1 | 0.000495485 | 0.000512204 | 0.000526719 |  |
| plan-sql-differences-d9_decimal_mul_scale_too_large/nata/1 | 0.000163875 | 0.000169700 | 0.000173190 | MET |
| plan-sql-differences-d9_decimal_mul_scale_too_large/twin/1 | 0.000163855 | 0.000168709 | 0.000177690 |  |
| plan-sql-differences-d9_decimal_plus_double/duckdb/1 | 0.000547505 | 0.000589980 | 0.000611420 |  |
| plan-sql-differences-d9_decimal_plus_double/nata/1 | 0.000147340 | 0.000157235 | 0.000162525 | MET |
| plan-sql-differences-d9_decimal_plus_double/twin/1 | 0.000149525 | 0.000155929 | 0.000262085 |  |
| plan-sql-differences-d9_divide_decimals/duckdb/1 | 0.000895265 | 0.000905085 | 0.000913549 |  |
| plan-sql-differences-d9_divide_decimals/nata/1 | 0.000176920 | 0.000186585 | 0.000194355 | MET |
| plan-sql-differences-d9_divide_decimals/twin/1 | 0.000176905 | 0.000191340 | 0.000195265 |  |
| plan-sql-differences-d9_divide_integers/duckdb/1 | 0.000879930 | 0.000891669 | 0.000922960 |  |
| plan-sql-differences-d9_divide_integers/nata/1 | 0.000172195 | 0.000183530 | 0.000190550 | MET |
| plan-sql-differences-d9_divide_integers/twin/1 | 0.000173605 | 0.000181035 | 0.000192850 |  |
| plan-sql-differences-d9_division_by_zero_is_an_error/duckdb/1 | 0.000561579 | 0.000571790 | 0.000593500 |  |
| plan-sql-differences-d9_division_by_zero_is_an_error/nata/1 | 0.000155470 | 0.000159795 | 0.000161620 | MET |
| plan-sql-differences-d9_division_by_zero_is_an_error/twin/1 | 0.000158295 | 0.000160690 | 0.000165200 |  |
| plan-sql-differences-d9_double_to_decimal_exact_binary_value/duckdb/1 | 0.000606595 | 0.000611750 | 0.000627540 |  |
| plan-sql-differences-d9_double_to_decimal_exact_binary_value/nata/1 | 0.000160115 | 0.000164445 | 0.000168230 | MET |
| plan-sql-differences-d9_double_to_decimal_exact_binary_value/twin/1 | 0.000163035 | 0.000163710 | 0.000223080 |  |
| plan-sql-differences-d9_float_arithmetic/duckdb/1 | 0.000506470 | 0.000546264 | 0.000551465 |  |
| plan-sql-differences-d9_float_arithmetic/nata/1 | 0.000139410 | 0.000150595 | 0.000154025 | MET |
| plan-sql-differences-d9_float_arithmetic/twin/1 | 0.000142010 | 0.000154015 | 0.000154965 |  |
| plan-sql-differences-d9_float_compared_with_integer/duckdb/1 | 0.000581645 | 0.000609875 | 0.000630360 |  |
| plan-sql-differences-d9_float_compared_with_integer/nata/1 | 0.000146395 | 0.000155290 | 0.000161425 | MET |
| plan-sql-differences-d9_float_compared_with_integer/twin/1 | 0.000150045 | 0.000153615 | 0.000157350 |  |
| plan-sql-differences-d9_float_literal_is_exact_decimal/duckdb/1 | 0.000558105 | 0.000597954 | 0.000609045 |  |
| plan-sql-differences-d9_float_literal_is_exact_decimal/nata/1 | 0.000161990 | 0.000166420 | 0.000240399 | MET |
| plan-sql-differences-d9_float_literal_is_exact_decimal/twin/1 | 0.000156990 | 0.000166495 | 0.000257024 |  |
| plan-sql-differences-d9_in_list_with_a_cast_of_a_literal/duckdb/1 | 0.000574720 | 0.000598925 | 0.000606890 |  |
| plan-sql-differences-d9_in_list_with_a_cast_of_a_literal/nata/1 | 0.000152515 | 0.000153760 | 0.000161230 | MET |
| plan-sql-differences-d9_in_list_with_a_cast_of_a_literal/twin/1 | 0.000152415 | 0.000159115 | 0.000162810 |  |
| plan-sql-differences-d9_in_list_with_an_expression/duckdb/1 | 0.000613400 | 0.000643045 | 0.000655275 |  |
| plan-sql-differences-d9_in_list_with_an_expression/nata/1 | 0.000146265 | 0.000152865 | 0.000154530 | MET |
| plan-sql-differences-d9_in_list_with_an_expression/twin/1 | 0.000142385 | 0.000150695 | 0.000153279 |  |
| plan-sql-differences-d9_integer_literal_does_not_narrow_to_the_column/duckdb/1 | 0.000686540 | 0.000718780 | 0.000726334 |  |
| plan-sql-differences-d9_integer_literal_does_not_narrow_to_the_column/nata/1 | 0.000161515 | 0.000164485 | 0.000169410 | MET |
| plan-sql-differences-d9_integer_literal_does_not_narrow_to_the_column/twin/1 | 0.000154745 | 0.000168750 | 0.000170650 |  |
| plan-sql-differences-d9_quoted_identifier_is_case_sensitive/duckdb/1 | 0.000452060 | 0.000468809 | 0.000484725 |  |
| plan-sql-differences-d9_quoted_identifier_is_case_sensitive/nata/1 | 0.000139760 | 0.000140960 | 0.000223375 | MET |
| plan-sql-differences-d9_quoted_identifier_is_case_sensitive/twin/1 | 0.000140650 | 0.000143105 | 0.000230860 |  |
| plan-sql-differences-d9_string_compared_with_date/duckdb/1 | 0.000597410 | 0.000616319 | 0.000630780 |  |
| plan-sql-differences-d9_string_compared_with_date/nata/1 | 0.000148745 | 0.000154120 | 0.000157095 | MET |
| plan-sql-differences-d9_string_compared_with_date/twin/1 | 0.000147040 | 0.000151710 | 0.000157375 |  |
| plan-sql-differences-d9_string_compared_with_number_column/duckdb/1 | 0.000575320 | 0.000602299 | 0.000620164 |  |
| plan-sql-differences-d9_string_compared_with_number_column/nata/1 | 0.000145300 | 0.000153345 | 0.000157345 | MET |
| plan-sql-differences-d9_string_compared_with_number_column/twin/1 | 0.000149550 | 0.000150785 | 0.000158985 |  |
| plan-sql-differences-d9_string_equals_number/duckdb/1 | 0.000538820 | 0.000556409 | 0.000570675 |  |
| plan-sql-differences-d9_string_equals_number/nata/1 | 0.000151830 | 0.000153165 | 0.000155070 | MET |
| plan-sql-differences-d9_string_equals_number/twin/1 | 0.000144140 | 0.000153885 | 0.000155710 |  |
| plan-sql-differences-d9_where_integer_is_not_boolean/duckdb/1 | 0.000519590 | 0.000528079 | 0.000548380 |  |
| plan-sql-differences-d9_where_integer_is_not_boolean/nata/1 | 0.000135644 | 0.000138090 | 0.000140365 | MET |
| plan-sql-differences-d9_where_integer_is_not_boolean/twin/1 | 0.000136235 | 0.000142975 | 0.000149100 |  |
| plan-sql-differences-d9b_a_case_condition_is_evaluated_only_for_the_rows_that_reach_it/duckdb/1 | 0.000648744 | 0.000671389 | 0.000684960 |  |
| plan-sql-differences-d9b_a_case_condition_is_evaluated_only_for_the_rows_that_reach_it/nata/1 | 0.000185910 | 0.000188645 | 0.000195060 | MET |
| plan-sql-differences-d9b_a_case_condition_is_evaluated_only_for_the_rows_that_reach_it/twin/1 | 0.000180925 | 0.000186800 | 0.000202890 |  |
| plan-sql-differences-d9b_an_operand_next_to_a_null_is_still_evaluated/duckdb/1 | 0.000667379 | 0.000692330 | 0.000723460 |  |
| plan-sql-differences-d9b_an_operand_next_to_a_null_is_still_evaluated/nata/1 | 0.000162315 | 0.000168900 | 0.000189905 | MET |
| plan-sql-differences-d9b_an_operand_next_to_a_null_is_still_evaluated/twin/1 | 0.000160350 | 0.000184815 | 0.000206245 |  |
| plan-sql-differences-d9b_and_skips_the_second_operand_for_decided_rows/duckdb/1 | 0.000748430 | 0.000777304 | 0.000861789 |  |
| plan-sql-differences-d9b_and_skips_the_second_operand_for_decided_rows/nata/1 | 0.000175135 | 0.000195170 | 0.000237275 | MET |
| plan-sql-differences-d9b_and_skips_the_second_operand_for_decided_rows/twin/1 | 0.000180570 | 0.000183965 | 0.000220745 |  |
| plan-sql-differences-d9b_or_skips_the_second_operand_for_decided_rows/duckdb/1 | 0.000763950 | 0.000788590 | 0.000805335 |  |
| plan-sql-differences-d9b_or_skips_the_second_operand_for_decided_rows/nata/1 | 0.000179695 | 0.000187215 | 0.000260839 | MET |
| plan-sql-differences-d9b_or_skips_the_second_operand_for_decided_rows/twin/1 | 0.000183535 | 0.000197585 | 0.000226495 |  |
| plan-sql-edge_all_null_cmp_everything/duckdb/1 | 0.000767714 | 0.000799524 | 0.000826200 |  |
| plan-sql-edge_all_null_cmp_everything/nata/1 | 0.000189219 | 0.000198914 | 0.000211760 | MET |
| plan-sql-edge_all_null_cmp_everything/twin/1 | 0.000187585 | 0.000197575 | 0.000204650 |  |
| plan-sql-edge_all_null_expressions/duckdb/1 | 0.000805450 | 0.000809464 | 0.000866665 |  |
| plan-sql-edge_all_null_expressions/nata/1 | 0.000190295 | 0.000196930 | 0.000203270 | MET |
| plan-sql-edge_all_null_expressions/twin/1 | 0.000189830 | 0.000202760 | 0.000207174 |  |
| plan-sql-edge_all_null_filter_all/duckdb/1 | 0.000650034 | 0.000674380 | 0.000732435 |  |
| plan-sql-edge_all_null_filter_all/nata/1 | 0.000167455 | 0.000168965 | 0.000170095 | MET |
| plan-sql-edge_all_null_filter_all/twin/1 | 0.000160965 | 0.000166570 | 0.000180125 |  |
| plan-sql-edge_all_null_filter_none/duckdb/1 | 0.000616560 | 0.000623390 | 0.000640445 |  |
| plan-sql-edge_all_null_filter_none/nata/1 | 0.000161435 | 0.000164430 | 0.000171430 | MET |
| plan-sql-edge_all_null_filter_none/twin/1 | 0.000162935 | 0.000169805 | 0.000177010 |  |
| plan-sql-edge_all_null_limit_offset/duckdb/1 | 0.000847369 | 0.000852799 | 0.000867509 |  |
| plan-sql-edge_all_null_limit_offset/nata/1 | 0.000151220 | 0.000154430 | 0.000161040 | MET |
| plan-sql-edge_all_null_limit_offset/twin/1 | 0.000158685 | 0.000161995 | 0.000163610 |  |
| plan-sql-edge_all_null_star/duckdb/1 | 0.000595460 | 0.000616645 | 0.000641094 |  |
| plan-sql-edge_all_null_star/nata/1 | 0.000137570 | 0.000142680 | 0.000150834 | MET |
| plan-sql-edge_all_null_star/twin/1 | 0.000137055 | 0.000144605 | 0.000152595 |  |
| plan-sql-edge_duplicate_rows_kept/duckdb/1 | 0.000539415 | 0.000567755 | 0.000576480 |  |
| plan-sql-edge_duplicate_rows_kept/nata/1 | 0.000153025 | 0.000156810 | 0.000241635 | MET |
| plan-sql-edge_duplicate_rows_kept/twin/1 | 0.000156080 | 0.000156445 | 0.000163070 |  |
| plan-sql-edge_dups_cmp_everything/duckdb/1 | 0.000821480 | 0.000841625 | 0.000856080 |  |
| plan-sql-edge_dups_cmp_everything/nata/1 | 0.000192155 | 0.000195740 | 0.000197645 | MET |
| plan-sql-edge_dups_cmp_everything/twin/1 | 0.000193470 | 0.000196215 | 0.000203635 |  |
| plan-sql-edge_dups_expressions/duckdb/1 | 0.000868105 | 0.000883990 | 0.000935365 |  |
| plan-sql-edge_dups_expressions/nata/1 | 0.000197665 | 0.000210025 | 0.000213140 | MET |
| plan-sql-edge_dups_expressions/twin/1 | 0.000205290 | 0.000214125 | 0.000229830 |  |
| plan-sql-edge_dups_filter_all/duckdb/1 | 0.000719134 | 0.000739915 | 0.000759715 |  |
| plan-sql-edge_dups_filter_all/nata/1 | 0.000170534 | 0.000173970 | 0.000182675 | MET |
| plan-sql-edge_dups_filter_all/twin/1 | 0.000171005 | 0.000172360 | 0.000175705 |  |
| plan-sql-edge_dups_filter_none/duckdb/1 | 0.000605225 | 0.000634644 | 0.000655120 |  |
| plan-sql-edge_dups_filter_none/nata/1 | 0.000160080 | 0.000163275 | 0.000174795 | MET |
| plan-sql-edge_dups_filter_none/twin/1 | 0.000161725 | 0.000167780 | 0.000172080 |  |
| plan-sql-edge_dups_limit_offset/duckdb/1 | 0.000843084 | 0.000872239 | 0.000917945 |  |
| plan-sql-edge_dups_limit_offset/nata/1 | 0.000155825 | 0.000163585 | 0.000166830 | MET |
| plan-sql-edge_dups_limit_offset/twin/1 | 0.000152145 | 0.000163605 | 0.000176160 |  |
| plan-sql-edge_dups_star/duckdb/1 | 0.000674835 | 0.000711009 | 0.000720724 |  |
| plan-sql-edge_dups_star/nata/1 | 0.000147265 | 0.000150910 | 0.000152755 | MET |
| plan-sql-edge_dups_star/twin/1 | 0.000150365 | 0.000152390 | 0.000157930 |  |
| plan-sql-edge_empty_cmp_everything/duckdb/1 | 0.000721690 | 0.000743010 | 0.000767555 |  |
| plan-sql-edge_empty_cmp_everything/nata/1 | 0.000163750 | 0.000167405 | 0.000173190 | MET |
| plan-sql-edge_empty_cmp_everything/twin/1 | 0.000159785 | 0.000172165 | 0.000185070 |  |
| plan-sql-edge_empty_expressions/duckdb/1 | 0.000756844 | 0.000786305 | 0.000829450 |  |
| plan-sql-edge_empty_expressions/nata/1 | 0.000176564 | 0.000180370 | 0.000192015 | MET |
| plan-sql-edge_empty_expressions/twin/1 | 0.000177590 | 0.000191480 | 0.000208320 |  |
| plan-sql-edge_empty_filter_all/duckdb/1 | 0.000648705 | 0.000668545 | 0.000719659 |  |
| plan-sql-edge_empty_filter_all/nata/1 | 0.000161220 | 0.000163145 | 0.000165765 | MET |
| plan-sql-edge_empty_filter_all/twin/1 | 0.000154245 | 0.000167325 | 0.000177055 |  |
| plan-sql-edge_empty_filter_none/duckdb/1 | 0.000596745 | 0.000630805 | 0.000636440 |  |
| plan-sql-edge_empty_filter_none/nata/1 | 0.000147135 | 0.000155345 | 0.000167335 | MET |
| plan-sql-edge_empty_filter_none/twin/1 | 0.000149150 | 0.000158460 | 0.000158880 |  |
| plan-sql-edge_empty_limit_offset/duckdb/1 | 0.000762220 | 0.000785970 | 0.000814934 |  |
| plan-sql-edge_empty_limit_offset/nata/1 | 0.000136545 | 0.000140275 | 0.000148775 | MET |
| plan-sql-edge_empty_limit_offset/twin/1 | 0.000135715 | 0.000138580 | 0.000147075 |  |
| plan-sql-edge_empty_star/duckdb/1 | 0.000576055 | 0.000601650 | 0.000620465 |  |
| plan-sql-edge_empty_star/nata/1 | 0.000131950 | 0.000138595 | 0.000145620 | MET |
| plan-sql-edge_empty_star/twin/1 | 0.000131050 | 0.000141135 | 0.000142270 |  |
| plan-sql-edge_filter_false_literal/duckdb/1 | 0.000457055 | 0.000476650 | 0.000488425 |  |
| plan-sql-edge_filter_false_literal/nata/1 | 0.000149880 | 0.000154749 | 0.000158765 | MET |
| plan-sql-edge_filter_false_literal/twin/1 | 0.000149845 | 0.000151225 | 0.000164185 |  |
| plan-sql-edge_filter_keeps_everything_pushed/duckdb/1 | 0.000583345 | 0.000620315 | 0.000640090 |  |
| plan-sql-edge_filter_keeps_everything_pushed/nata/1 | 0.000154435 | 0.000156914 | 0.000165765 | MET |
| plan-sql-edge_filter_keeps_everything_pushed/twin/1 | 0.000152770 | 0.000157520 | 0.000161725 |  |
| plan-sql-edge_filter_keeps_everything_remaining/duckdb/1 | 0.000602190 | 0.000629394 | 0.000657600 |  |
| plan-sql-edge_filter_keeps_everything_remaining/nata/1 | 0.000154150 | 0.000160245 | 0.000165780 | MET |
| plan-sql-edge_filter_keeps_everything_remaining/twin/1 | 0.000154215 | 0.000158145 | 0.000161510 |  |
| plan-sql-edge_filter_keeps_nothing_pushed/duckdb/1 | 0.000582795 | 0.000599835 | 0.000621429 |  |
| plan-sql-edge_filter_keeps_nothing_pushed/nata/1 | 0.000154715 | 0.000161335 | 0.000165440 | MET |
| plan-sql-edge_filter_keeps_nothing_pushed/twin/1 | 0.000155105 | 0.000159395 | 0.000169399 |  |
| plan-sql-edge_filter_keeps_nothing_remaining/duckdb/1 | 0.000587989 | 0.000607484 | 0.000629509 |  |
| plan-sql-edge_filter_keeps_nothing_remaining/nata/1 | 0.000164360 | 0.000170115 | 0.000174850 | MET |
| plan-sql-edge_filter_keeps_nothing_remaining/twin/1 | 0.000166890 | 0.000172260 | 0.000181135 |  |
| plan-sql-edge_filter_null_literal/duckdb/1 | 0.000513029 | 0.000524634 | 0.000541500 |  |
| plan-sql-edge_filter_null_literal/nata/1 | 0.000162475 | 0.000165515 | 0.000167360 | MET |
| plan-sql-edge_filter_null_literal/twin/1 | 0.000154480 | 0.000167045 | 0.000169140 |  |
| plan-sql-edge_filter_true_literal/duckdb/1 | 0.000500010 | 0.000525060 | 0.000548175 |  |
| plan-sql-edge_filter_true_literal/nata/1 | 0.000144375 | 0.000157150 | 0.000161765 | MET |
| plan-sql-edge_filter_true_literal/twin/1 | 0.000139554 | 0.000152090 | 0.000167595 |  |
| plan-sql-edge_one_row_cmp_everything/duckdb/1 | 0.000774805 | 0.000812240 | 0.000821605 |  |
| plan-sql-edge_one_row_cmp_everything/nata/1 | 0.000180625 | 0.000187755 | 0.000189520 | MET |
| plan-sql-edge_one_row_cmp_everything/twin/1 | 0.000179820 | 0.000185495 | 0.000194515 |  |
| plan-sql-edge_one_row_expressions/duckdb/1 | 0.000779624 | 0.000839724 | 0.000894775 |  |
| plan-sql-edge_one_row_expressions/nata/1 | 0.000178905 | 0.000199375 | 0.000211980 | MET |
| plan-sql-edge_one_row_expressions/twin/1 | 0.000192575 | 0.000196395 | 0.000205110 |  |
| plan-sql-edge_one_row_filter_all/duckdb/1 | 0.000632715 | 0.000663849 | 0.000669465 |  |
| plan-sql-edge_one_row_filter_all/nata/1 | 0.000153365 | 0.000164215 | 0.000172385 | MET |
| plan-sql-edge_one_row_filter_all/twin/1 | 0.000155495 | 0.000165715 | 0.000172640 |  |
| plan-sql-edge_one_row_filter_none/duckdb/1 | 0.000583414 | 0.000600135 | 0.000625104 |  |
| plan-sql-edge_one_row_filter_none/nata/1 | 0.000159720 | 0.000165505 | 0.000171320 | MET |
| plan-sql-edge_one_row_filter_none/twin/1 | 0.000158425 | 0.000163005 | 0.000231095 |  |
| plan-sql-edge_one_row_limit_offset/duckdb/1 | 0.000813059 | 0.000829830 | 0.000859130 |  |
| plan-sql-edge_one_row_limit_offset/nata/1 | 0.000152210 | 0.000155130 | 0.000162260 | MET |
| plan-sql-edge_one_row_limit_offset/twin/1 | 0.000147470 | 0.000154550 | 0.000167875 |  |
| plan-sql-edge_one_row_star/duckdb/1 | 0.000615540 | 0.000656625 | 0.000658435 |  |
| plan-sql-edge_one_row_star/nata/1 | 0.000138985 | 0.000148305 | 0.000148900 | MET |
| plan-sql-edge_one_row_star/twin/1 | 0.000145960 | 0.000146410 | 0.000151435 |  |
| plan-sql-edge_pushed_and_remaining/duckdb/1 | 0.000824504 | 0.000877330 | 0.000889449 |  |
| plan-sql-edge_pushed_and_remaining/nata/1 | 0.000165215 | 0.000174290 | 0.000181425 | MET |
| plan-sql-edge_pushed_and_remaining/twin/1 | 0.000169075 | 0.000174085 | 0.000184125 |  |
| plan-sql-edge_pushed_bool/duckdb/1 | 0.000584430 | 0.000610055 | 0.000620585 |  |
| plan-sql-edge_pushed_bool/nata/1 | 0.000152510 | 0.000153800 | 0.000158280 | MET |
| plan-sql-edge_pushed_bool/twin/1 | 0.000149100 | 0.000158705 | 0.000162890 |  |
| plan-sql-edge_pushed_decimal_literal_scales/duckdb/1 | 0.000741925 | 0.000783170 | 0.000808465 |  |
| plan-sql-edge_pushed_decimal_literal_scales/nata/1 | 0.000159210 | 0.000166375 | 0.000170760 | MET |
| plan-sql-edge_pushed_decimal_literal_scales/twin/1 | 0.000158774 | 0.000169360 | 0.000175500 |  |
| plan-sql-edge_pushed_float/duckdb/1 | 0.000605000 | 0.000652695 | 0.000654770 |  |
| plan-sql-edge_pushed_float/nata/1 | 0.000157980 | 0.000160635 | 0.000166110 | MET |
| plan-sql-edge_pushed_float/twin/1 | 0.000153300 | 0.000162330 | 0.000166160 |  |
| plan-sql-edge_pushed_is_null/duckdb/1 | 0.000589300 | 0.000595814 | 0.000607605 |  |
| plan-sql-edge_pushed_is_null/nata/1 | 0.000153425 | 0.000163675 | 0.000167735 | MET |
| plan-sql-edge_pushed_is_null/twin/1 | 0.000159154 | 0.000163460 | 0.000166315 |  |
| plan-sql-edge_pushed_literal_on_left/duckdb/1 | 0.000687025 | 0.000692380 | 0.000714939 |  |
| plan-sql-edge_pushed_literal_on_left/nata/1 | 0.000155465 | 0.000160535 | 0.000175915 | MET |
| plan-sql-edge_pushed_literal_on_left/twin/1 | 0.000158255 | 0.000159920 | 0.000162920 |  |
| plan-sql-edge_pushed_string/duckdb/1 | 0.000754329 | 0.000772820 | 0.000787184 |  |
| plan-sql-edge_pushed_string/nata/1 | 0.000151000 | 0.000159420 | 0.000168580 | MET |
| plan-sql-edge_pushed_string/twin/1 | 0.000151510 | 0.000162360 | 0.000177520 |  |
| plan-sql-edge_pushed_string_and/duckdb/1 | 0.000684550 | 0.000691905 | 0.000698820 |  |
| plan-sql-edge_pushed_string_and/nata/1 | 0.000159279 | 0.000161655 | 0.000166555 | MET |
| plan-sql-edge_pushed_string_and/twin/1 | 0.000157504 | 0.000163049 | 0.000167880 |  |
| plan-sql-edge_select_literal_only/duckdb/1 | 0.000627444 | 0.000651885 | 0.000689234 |  |
| plan-sql-edge_select_literal_only/nata/1 | 0.000159060 | 0.000163120 | 0.000169390 | MET |
| plan-sql-edge_select_literal_only/twin/1 | 0.000159280 | 0.000167884 | 0.000169670 |  |
| plan-sql-edge_select_literal_only_empty/duckdb/1 | 0.000442039 | 0.000452645 | 0.000461985 |  |
| plan-sql-edge_select_literal_only_empty/nata/1 | 0.000132645 | 0.000138640 | 0.000142660 | MET |
| plan-sql-edge_select_literal_only_empty/twin/1 | 0.000130420 | 0.000135330 | 0.000144835 |  |
| plan-sql-s1_columns_in_order/duckdb/1 | 0.000488840 | 0.000514934 | 0.000560609 |  |
| plan-sql-s1_columns_in_order/nata/1 | 0.000136415 | 0.000145810 | 0.000150990 | MET |
| plan-sql-s1_columns_in_order/twin/1 | 0.000146544 | 0.000147084 | 0.000159880 |  |
| plan-sql-s1_comments_whitespace/duckdb/1 | 0.000593635 | 0.000639715 | 0.000642675 |  |
| plan-sql-s1_comments_whitespace/nata/1 | 0.000151225 | 0.000161850 | 0.000165045 | MET |
| plan-sql-s1_comments_whitespace/twin/1 | 0.000154815 | 0.000167040 | 0.000173565 |  |
| plan-sql-s1_glob/duckdb/1 | 0.000529400 | 0.000552260 | 0.000556919 |  |
| plan-sql-s1_glob/nata/1 | 0.000193115 | 0.000194490 | 0.000202780 | MET |
| plan-sql-s1_glob/twin/1 | 0.000190065 | 0.000200990 | 0.000203575 |  |
| plan-sql-s1_glob_limit/duckdb/1 | 0.000553185 | 0.000574789 | 0.000598910 |  |
| plan-sql-s1_glob_limit/nata/1 | 0.000183685 | 0.000197460 | 0.000201325 | MET |
| plan-sql-s1_glob_limit/twin/1 | 0.000185435 | 0.000194075 | 0.000203415 |  |
| plan-sql-s1_glob_limit_offset/duckdb/1 | 0.000824889 | 0.000843315 | 0.000864490 |  |
| plan-sql-s1_glob_limit_offset/nata/1 | 0.000188245 | 0.000197625 | 0.000202600 | MET |
| plan-sql-s1_glob_limit_offset/twin/1 | 0.000180584 | 0.000200755 | 0.000207535 |  |
| plan-sql-s1_glob_where/duckdb/1 | 0.000665219 | 0.000673304 | 0.000692314 |  |
| plan-sql-s1_glob_where/nata/1 | 0.000197875 | 0.000203945 | 0.000293950 | MET |
| plan-sql-s1_glob_where/twin/1 | 0.000196559 | 0.000200760 | 0.000210520 |  |
| plan-sql-s1_keywords_any_case/duckdb/1 | 0.000685995 | 0.000720950 | 0.000741050 |  |
| plan-sql-s1_keywords_any_case/nata/1 | 0.000154110 | 0.000157090 | 0.000160685 | MET |
| plan-sql-s1_keywords_any_case/twin/1 | 0.000154959 | 0.000158065 | 0.000159625 |  |
| plan-sql-s1_star/duckdb/1 | 0.000581815 | 0.000605390 | 0.000660435 |  |
| plan-sql-s1_star/nata/1 | 0.000132174 | 0.000133430 | 0.000149890 | MET |
| plan-sql-s1_star/twin/1 | 0.000129245 | 0.000133885 | 0.000142750 |  |
| plan-sql-s1_trailing_semicolon/duckdb/1 | 0.000485579 | 0.000489410 | 0.000501774 |  |
| plan-sql-s1_trailing_semicolon/nata/1 | 0.000141930 | 0.000145995 | 0.000148840 | MET |
| plan-sql-s1_trailing_semicolon/twin/1 | 0.000135710 | 0.000142699 | 0.000146805 |  |
| plan-sql-s1_where_limit_offset/duckdb/1 | 0.000647089 | 0.000664980 | 0.000686060 |  |
| plan-sql-s1_where_limit_offset/nata/1 | 0.000153890 | 0.000157370 | 0.000162495 | MET |
| plan-sql-s1_where_limit_offset/twin/1 | 0.000147460 | 0.000157735 | 0.000159785 |  |
| plan-sql-s2_alias/duckdb/1 | 0.000567044 | 0.000599155 | 0.000602269 |  |
| plan-sql-s2_alias/nata/1 | 0.000149115 | 0.000153575 | 0.000167400 | MET |
| plan-sql-s2_alias/twin/1 | 0.000148165 | 0.000155880 | 0.000157430 |  |
| plan-sql-s2_alias_quoted_case/duckdb/1 | 0.000505579 | 0.000515655 | 0.000524839 |  |
| plan-sql-s2_alias_quoted_case/nata/1 | 0.000143789 | 0.000146805 | 0.000163020 | MET |
| plan-sql-s2_alias_quoted_case/twin/1 | 0.000139600 | 0.000143990 | 0.000152100 |  |
| plan-sql-s2_duplicate_output_names/duckdb/1 | 0.000475904 | 0.000508589 | 0.000514840 |  |
| plan-sql-s2_duplicate_output_names/nata/1 | 0.000140699 | 0.000145080 | 0.000156600 | MET |
| plan-sql-s2_duplicate_output_names/twin/1 | 0.000136960 | 0.000147294 | 0.000154605 |  |
| plan-sql-s2_filter_column_not_projected/duckdb/1 | 0.000588910 | 0.000596250 | 0.000615400 |  |
| plan-sql-s2_filter_column_not_projected/nata/1 | 0.000142340 | 0.000149764 | 0.000153665 | MET |
| plan-sql-s2_filter_column_not_projected/twin/1 | 0.000145650 | 0.000148305 | 0.000153605 |  |
| plan-sql-s2_quoted_exact/duckdb/1 | 0.000495180 | 0.000517165 | 0.000539285 |  |
| plan-sql-s2_quoted_exact/nata/1 | 0.000142615 | 0.000145600 | 0.000149510 | MET |
| plan-sql-s2_quoted_exact/twin/1 | 0.000140110 | 0.000143555 | 0.000150100 |  |
| plan-sql-s2_same_column_twice/duckdb/1 | 0.000517510 | 0.000542820 | 0.000556765 |  |
| plan-sql-s2_same_column_twice/nata/1 | 0.000142915 | 0.000148460 | 0.000154920 | MET |
| plan-sql-s2_same_column_twice/twin/1 | 0.000147405 | 0.000151795 | 0.000154550 |  |
| plan-sql-s2_unquoted_folded/duckdb/1 | 0.000544790 | 0.000554900 | 0.000569655 |  |
| plan-sql-s2_unquoted_folded/nata/1 | 0.000146135 | 0.000155780 | 0.000161840 | MET |
| plan-sql-s2_unquoted_folded/twin/1 | 0.000147790 | 0.000151960 | 0.000160225 |  |
| plan-sql-s3_big_integer_literal/duckdb/1 | 0.000542605 | 0.000556170 | 0.000571074 |  |
| plan-sql-s3_big_integer_literal/nata/1 | 0.000150755 | 0.000158905 | 0.000159255 | MET |
| plan-sql-s3_big_integer_literal/twin/1 | 0.000148395 | 0.000159480 | 0.000162550 |  |
| plan-sql-s3_boolean_literals/duckdb/1 | 0.000538865 | 0.000545085 | 0.000558465 |  |
| plan-sql-s3_boolean_literals/nata/1 | 0.000148870 | 0.000154950 | 0.000156440 | MET |
| plan-sql-s3_boolean_literals/twin/1 | 0.000145670 | 0.000152215 | 0.000157435 |  |
| plan-sql-s3_date_literal/duckdb/1 | 0.000558220 | 0.000584480 | 0.000602090 |  |
| plan-sql-s3_date_literal/nata/1 | 0.000146020 | 0.000148845 | 0.000154375 | MET |
| plan-sql-s3_date_literal/twin/1 | 0.000141275 | 0.000149555 | 0.000154070 |  |
| plan-sql-s3_decimal_literals/duckdb/1 | 0.000539989 | 0.000577270 | 0.000601679 |  |
| plan-sql-s3_decimal_literals/nata/1 | 0.000149715 | 0.000160980 | 0.000164750 | MET |
| plan-sql-s3_decimal_literals/twin/1 | 0.000148230 | 0.000156320 | 0.000164895 |  |
| plan-sql-s3_impossible_date/duckdb/1 | 0.000542475 | 0.000562945 | 0.000577780 |  |
| plan-sql-s3_impossible_date/nata/1 | 0.000144870 | 0.000144970 | 0.000231860 | MET |
| plan-sql-s3_impossible_date/twin/1 | 0.000143200 | 0.000145064 | 0.000149500 |  |
| plan-sql-s3_integer_literals/duckdb/1 | 0.000562199 | 0.000577704 | 0.000587540 |  |
| plan-sql-s3_integer_literals/nata/1 | 0.000158155 | 0.000166410 | 0.000245775 | MET |
| plan-sql-s3_integer_literals/twin/1 | 0.000156970 | 0.000166830 | 0.000171950 |  |
| plan-sql-s3_null_in_case/duckdb/1 | 0.000604275 | 0.000614675 | 0.000638719 |  |
| plan-sql-s3_null_in_case/nata/1 | 0.000164410 | 0.000168645 | 0.000175095 | MET |
| plan-sql-s3_null_in_case/twin/1 | 0.000164335 | 0.000167885 | 0.000169375 |  |
| plan-sql-s3_null_typed_by_context/duckdb/1 | 0.000629649 | 0.000652379 | 0.000678680 |  |
| plan-sql-s3_null_typed_by_context/nata/1 | 0.000169180 | 0.000176755 | 0.000182880 | MET |
| plan-sql-s3_null_typed_by_context/twin/1 | 0.000171420 | 0.000186540 | 0.000268380 |  |
| plan-sql-s3_string_literals/duckdb/1 | 0.000578480 | 0.000598975 | 0.000608400 |  |
| plan-sql-s3_string_literals/nata/1 | 0.000161425 | 0.000167180 | 0.000171735 | MET |
| plan-sql-s3_string_literals/twin/1 | 0.000160670 | 0.000167665 | 0.000171840 |  |
| plan-sql-s4_bigint_mul_overflow/duckdb/1 | 0.000568520 | 0.000599575 | 0.000602940 |  |
| plan-sql-s4_bigint_mul_overflow/nata/1 | 0.000156074 | 0.000161750 | 0.000166195 | MET |
| plan-sql-s4_bigint_mul_overflow/twin/1 | 0.000155820 | 0.000164500 | 0.000171240 |  |
| plan-sql-s4_dec_add_operand_does_not_fit_after_rescale/duckdb/1 | 0.000700699 | 0.000733980 | 0.000741434 |  |
| plan-sql-s4_dec_add_operand_does_not_fit_after_rescale/nata/1 | 0.000167615 | 0.000170115 | 0.000179355 | MET |
| plan-sql-s4_dec_add_operand_does_not_fit_after_rescale/twin/1 | 0.000168710 | 0.000174359 | 0.000255394 |  |
| plan-sql-s4_dec_add_sub_mul/duckdb/1 | 0.001300625 | 0.001352899 | 0.001362849 |  |
| plan-sql-s4_dec_add_sub_mul/nata/1 | 0.000193930 | 0.000197305 | 0.000203665 | MET |
| plan-sql-s4_dec_add_sub_mul/twin/1 | 0.000194810 | 0.000201155 | 0.000213675 |  |
| plan-sql-s4_dec_int_mix/duckdb/1 | 0.001312845 | 0.001370205 | 0.001397665 |  |
| plan-sql-s4_dec_int_mix/nata/1 | 0.000199610 | 0.000205894 | 0.000233495 | MET |
| plan-sql-s4_dec_int_mix/twin/1 | 0.000188120 | 0.000208000 | 0.000253490 |  |
| plan-sql-s4_dec_mod/duckdb/1 | 0.001131289 | 0.001162359 | 0.001240444 |  |
| plan-sql-s4_dec_mod/nata/1 | 0.000182180 | 0.000198410 | 0.000241525 | MET |
| plan-sql-s4_dec_mod/twin/1 | 0.000187915 | 0.000196870 | 0.000216860 |  |
| plan-sql-s4_dec_mul_overflow/duckdb/1 | 0.000662754 | 0.000707859 | 0.000712984 |  |
| plan-sql-s4_dec_mul_overflow/nata/1 | 0.000158750 | 0.000168020 | 0.000173455 | MET |
| plan-sql-s4_dec_mul_overflow/twin/1 | 0.000161725 | 0.000169635 | 0.000175430 |  |
| plan-sql-s4_dec_q1_chain/duckdb/1 | 0.001301990 | 0.001320984 | 0.001348869 |  |
| plan-sql-s4_dec_q1_chain/nata/1 | 0.000193145 | 0.000196225 | 0.000207580 | MET |
| plan-sql-s4_dec_q1_chain/twin/1 | 0.000189800 | 0.000197255 | 0.000208434 |  |
| plan-sql-s4_dec_wide_add/duckdb/1 | 0.001052059 | 0.001069349 | 0.001092370 |  |
| plan-sql-s4_dec_wide_add/nata/1 | 0.000172750 | 0.000181195 | 0.000188845 | MET |
| plan-sql-s4_dec_wide_add/twin/1 | 0.000177100 | 0.000188785 | 0.000255995 |  |
| plan-sql-s4_div_zero_guarded/duckdb/1 | 0.000726824 | 0.000745180 | 0.000770040 |  |
| plan-sql-s4_div_zero_guarded/nata/1 | 0.000163240 | 0.000166545 | 0.000176595 | MET |
| plan-sql-s4_div_zero_guarded/twin/1 | 0.000165885 | 0.000167250 | 0.000173280 |  |
| plan-sql-s4_div_zero_unselected/duckdb/1 | 0.000644680 | 0.000673154 | 0.000695694 |  |
| plan-sql-s4_div_zero_unselected/nata/1 | 0.000157865 | 0.000162245 | 0.000166905 | MET |
| plan-sql-s4_div_zero_unselected/twin/1 | 0.000155555 | 0.000160270 | 0.000165010 |  |
| plan-sql-s4_int_add_bigint/duckdb/1 | 0.001016624 | 0.001033314 | 0.001069680 |  |
| plan-sql-s4_int_add_bigint/nata/1 | 0.000175190 | 0.000177634 | 0.000190325 | MET |
| plan-sql-s4_int_add_bigint/twin/1 | 0.000174920 | 0.000180485 | 0.000188570 |  |
| plan-sql-s4_int_add_integer/duckdb/1 | 0.001028945 | 0.001063414 | 0.001099150 |  |
| plan-sql-s4_int_add_integer/nata/1 | 0.000177850 | 0.000186004 | 0.000251730 | MET |
| plan-sql-s4_int_add_integer/twin/1 | 0.000168659 | 0.000184925 | 0.000216080 |  |
| plan-sql-s4_int_add_overflow/duckdb/1 | 0.000540669 | 0.000571079 | 0.000579954 |  |
| plan-sql-s4_int_add_overflow/nata/1 | 0.000149445 | 0.000157860 | 0.000164690 | MET |
| plan-sql-s4_int_add_overflow/twin/1 | 0.000147505 | 0.000159380 | 0.000161699 |  |
| plan-sql-s4_int_add_smallint/duckdb/1 | 0.001015370 | 0.001030995 | 0.001048780 |  |
| plan-sql-s4_int_add_smallint/nata/1 | 0.000171950 | 0.000179635 | 0.000181235 | MET |
| plan-sql-s4_int_add_smallint/twin/1 | 0.000177575 | 0.000181725 | 0.000187429 |  |
| plan-sql-s4_int_add_tinyint/duckdb/1 | 0.001018274 | 0.001048919 | 0.001056525 |  |
| plan-sql-s4_int_add_tinyint/nata/1 | 0.000169240 | 0.000185700 | 0.000188174 | MET |
| plan-sql-s4_int_add_tinyint/twin/1 | 0.000177060 | 0.000186395 | 0.000194400 |  |
| plan-sql-s4_int_div_trunc/duckdb/1 | 0.001069169 | 0.001104020 | 0.001120840 |  |
| plan-sql-s4_int_div_trunc/nata/1 | 0.000182405 | 0.000186059 | 0.000193545 | MET |
| plan-sql-s4_int_div_trunc/twin/1 | 0.000179680 | 0.000187390 | 0.000196330 |  |
| plan-sql-s4_int_intdiv_min_minus_one/duckdb/1 | 0.000545050 | 0.000568600 | 0.000599430 |  |
| plan-sql-s4_int_intdiv_min_minus_one/nata/1 | 0.000155505 | 0.000160450 | 0.000171730 | MET |
| plan-sql-s4_int_intdiv_min_minus_one/twin/1 | 0.000152605 | 0.000165319 | 0.000175080 |  |
| plan-sql-s4_int_intdiv_zero/duckdb/1 | 0.000555975 | 0.000571199 | 0.000590150 |  |
| plan-sql-s4_int_intdiv_zero/nata/1 | 0.000150730 | 0.000157950 | 0.000163775 | MET |
| plan-sql-s4_int_intdiv_zero/twin/1 | 0.000152020 | 0.000161740 | 0.000166255 |  |
| plan-sql-s4_int_mixed_ops_bigint/duckdb/1 | 0.001178134 | 0.001215764 | 0.001228734 |  |
| plan-sql-s4_int_mixed_ops_bigint/nata/1 | 0.000195220 | 0.000200650 | 0.000209760 | MET |
| plan-sql-s4_int_mixed_ops_bigint/twin/1 | 0.000192735 | 0.000198959 | 0.000216150 |  |
| plan-sql-s4_int_mixed_ops_integer/duckdb/1 | 0.001162809 | 0.001183324 | 0.001230430 |  |
| plan-sql-s4_int_mixed_ops_integer/nata/1 | 0.000187475 | 0.000194395 | 0.000277545 | MET |
| plan-sql-s4_int_mixed_ops_integer/twin/1 | 0.000192465 | 0.000209185 | 0.000217065 |  |
| plan-sql-s4_int_mixed_ops_smallint/duckdb/1 | 0.001176415 | 0.001200970 | 0.001252174 |  |
| plan-sql-s4_int_mixed_ops_smallint/nata/1 | 0.000191340 | 0.000204380 | 0.000211735 | MET |
| plan-sql-s4_int_mixed_ops_smallint/twin/1 | 0.000196065 | 0.000202360 | 0.000206550 |  |
| plan-sql-s4_int_mixed_ops_tinyint/duckdb/1 | 0.001171405 | 0.001195010 | 0.001211230 |  |
| plan-sql-s4_int_mixed_ops_tinyint/nata/1 | 0.000199504 | 0.000200580 | 0.000203130 | MET |
| plan-sql-s4_int_mixed_ops_tinyint/twin/1 | 0.000191730 | 0.000204575 | 0.000289265 |  |
| plan-sql-s4_int_mod_min_minus_one/duckdb/1 | 0.000547945 | 0.000565365 | 0.000574890 |  |
| plan-sql-s4_int_mod_min_minus_one/nata/1 | 0.000152029 | 0.000156870 | 0.000158620 | MET |
| plan-sql-s4_int_mod_min_minus_one/twin/1 | 0.000155465 | 0.000161549 | 0.000164900 |  |
| plan-sql-s4_int_mod_sign/duckdb/1 | 0.001078705 | 0.001109525 | 0.001133814 |  |
| plan-sql-s4_int_mod_sign/nata/1 | 0.000191430 | 0.000193210 | 0.000205830 | MET |
| plan-sql-s4_int_mod_sign/twin/1 | 0.000189940 | 0.000194015 | 0.000201970 |  |
| plan-sql-s4_int_mod_zero/duckdb/1 | 0.000540559 | 0.000545930 | 0.000584054 |  |
| plan-sql-s4_int_mod_zero/nata/1 | 0.000153660 | 0.000156560 | 0.000161480 | MET |
| plan-sql-s4_int_mod_zero/twin/1 | 0.000147195 | 0.000152450 | 0.000161459 |  |
| plan-sql-s4_int_mul_overflow/duckdb/1 | 0.000536145 | 0.000569670 | 0.000584164 |  |
| plan-sql-s4_int_mul_overflow/nata/1 | 0.000146505 | 0.000153320 | 0.000160285 | MET |
| plan-sql-s4_int_mul_overflow/twin/1 | 0.000149010 | 0.000160260 | 0.000231070 |  |
| plan-sql-s4_int_sub_overflow/duckdb/1 | 0.000565264 | 0.000567834 | 0.000575755 |  |
| plan-sql-s4_int_sub_overflow/nata/1 | 0.000153235 | 0.000164335 | 0.000172785 | MET |
| plan-sql-s4_int_sub_overflow/twin/1 | 0.000153700 | 0.000165310 | 0.000235685 |  |
| plan-sql-s4_int_widening_i_plus_b/duckdb/1 | 0.000757825 | 0.000761635 | 0.000772560 |  |
| plan-sql-s4_int_widening_i_plus_b/nata/1 | 0.000173975 | 0.000175405 | 0.000186815 | MET |
| plan-sql-s4_int_widening_i_plus_b/twin/1 | 0.000172500 | 0.000179080 | 0.000183730 |  |
| plan-sql-s4_mod_zero_null_row/duckdb/1 | 0.000679430 | 0.000692270 | 0.000705685 |  |
| plan-sql-s4_mod_zero_null_row/nata/1 | 0.000157990 | 0.000160640 | 0.000166645 | MET |
| plan-sql-s4_mod_zero_null_row/twin/1 | 0.000154295 | 0.000167060 | 0.000169945 |  |
| plan-sql-s4_overflow_guarded_by_and/duckdb/1 | 0.000680740 | 0.000712879 | 0.000730080 |  |
| plan-sql-s4_overflow_guarded_by_and/nata/1 | 0.000158225 | 0.000162180 | 0.000241855 | MET |
| plan-sql-s4_overflow_guarded_by_and/twin/1 | 0.000158654 | 0.000169034 | 0.000173935 |  |
| plan-sql-s4_overflow_guarded_by_case/duckdb/1 | 0.000701825 | 0.000724289 | 0.000730879 |  |
| plan-sql-s4_overflow_guarded_by_case/nata/1 | 0.000172804 | 0.000178895 | 0.000185349 | MET |
| plan-sql-s4_overflow_guarded_by_case/twin/1 | 0.000172565 | 0.000181885 | 0.000191730 |  |
| plan-sql-s4_overflow_null_row_is_not_an_error/duckdb/1 | 0.000655855 | 0.000682530 | 0.000693700 |  |
| plan-sql-s4_overflow_null_row_is_not_an_error/nata/1 | 0.000152755 | 0.000162110 | 0.000166470 | MET |
| plan-sql-s4_overflow_null_row_is_not_an_error/twin/1 | 0.000156755 | 0.000160910 | 0.000167175 |  |
| plan-sql-s4_overflow_unselected_rows/duckdb/1 | 0.000648310 | 0.000664565 | 0.000671595 |  |
| plan-sql-s4_overflow_unselected_rows/nata/1 | 0.000154435 | 0.000160370 | 0.000163405 | MET |
| plan-sql-s4_overflow_unselected_rows/twin/1 | 0.000157509 | 0.000159430 | 0.000160790 |  |
| plan-sql-s4_precedence/duckdb/1 | 0.000779454 | 0.000814084 | 0.000847760 |  |
| plan-sql-s4_precedence/nata/1 | 0.000180335 | 0.000197094 | 0.000202875 | MET |
| plan-sql-s4_precedence/twin/1 | 0.000181740 | 0.000192975 | 0.000215835 |  |
| plan-sql-s4_precedence_logic/duckdb/1 | 0.000818915 | 0.000922180 | 0.001010124 |  |
| plan-sql-s4_precedence_logic/nata/1 | 0.000163855 | 0.000178755 | 0.000200014 | MET |
| plan-sql-s4_precedence_logic/twin/1 | 0.000165030 | 0.000167865 | 0.000202110 |  |
| plan-sql-s4_tinyint_overflow/duckdb/1 | 0.000555535 | 0.000560295 | 0.000572520 |  |
| plan-sql-s4_tinyint_overflow/nata/1 | 0.000143805 | 0.000148865 | 0.000155680 | MET |
| plan-sql-s4_tinyint_overflow/twin/1 | 0.000149380 | 0.000152870 | 0.000156905 |  |
| plan-sql-s4_unary_minus/duckdb/1 | 0.000808820 | 0.000823114 | 0.000848619 |  |
| plan-sql-s4_unary_minus/nata/1 | 0.000177850 | 0.000180670 | 0.000190545 | MET |
| plan-sql-s4_unary_minus/twin/1 | 0.000175630 | 0.000190200 | 0.000194660 |  |
| plan-sql-s4_unary_minus_min/duckdb/1 | 0.000555534 | 0.000583129 | 0.000613085 |  |
| plan-sql-s4_unary_minus_min/nata/1 | 0.000149375 | 0.000151135 | 0.000163075 | MET |
| plan-sql-s4_unary_minus_min/twin/1 | 0.000145985 | 0.000158005 | 0.000161020 |  |
| plan-sql-s5_and_or_not_truth_table/duckdb/1 | 0.000919569 | 0.000949855 | 0.000985000 |  |
| plan-sql-s5_and_or_not_truth_table/nata/1 | 0.000214875 | 0.000219595 | 0.000239005 | MET |
| plan-sql-s5_and_or_not_truth_table/twin/1 | 0.000217210 | 0.000219650 | 0.000242724 |  |
| plan-sql-s5_between/duckdb/1 | 0.000919660 | 0.000940740 | 0.001017344 |  |
| plan-sql-s5_between/nata/1 | 0.000185074 | 0.000200685 | 0.000207670 | MET |
| plan-sql-s5_between/twin/1 | 0.000183440 | 0.000202870 | 0.000206590 |  |
| plan-sql-s5_between_doubles/duckdb/1 | 0.000649960 | 0.000676370 | 0.000685620 |  |
| plan-sql-s5_between_doubles/nata/1 | 0.000168135 | 0.000170009 | 0.000189595 | MET |
| plan-sql-s5_between_doubles/twin/1 | 0.000168190 | 0.000174235 | 0.000187064 |  |
| plan-sql-s5_between_expressions/duckdb/1 | 0.000640399 | 0.000673889 | 0.000686815 |  |
| plan-sql-s5_between_expressions/nata/1 | 0.000166530 | 0.000169490 | 0.000177369 | MET |
| plan-sql-s5_between_expressions/twin/1 | 0.000167665 | 0.000170960 | 0.000177050 |  |
| plan-sql-s5_between_null_bounds/duckdb/1 | 0.000731875 | 0.000755730 | 0.000777514 |  |
| plan-sql-s5_between_null_bounds/nata/1 | 0.000174470 | 0.000181890 | 0.000196925 | MET |
| plan-sql-s5_between_null_bounds/twin/1 | 0.000177505 | 0.000185495 | 0.000193320 |  |
| plan-sql-s5_between_reversed/duckdb/1 | 0.000678935 | 0.000707919 | 0.000721140 |  |
| plan-sql-s5_between_reversed/nata/1 | 0.000169950 | 0.000172535 | 0.000182030 | MET |
| plan-sql-s5_between_reversed/twin/1 | 0.000169050 | 0.000172360 | 0.000178945 |  |
| plan-sql-s5_between_where/duckdb/1 | 0.000772429 | 0.000782915 | 0.000811399 |  |
| plan-sql-s5_between_where/nata/1 | 0.000156000 | 0.000167720 | 0.000234285 | MET |
| plan-sql-s5_between_where/twin/1 | 0.000160990 | 0.000165615 | 0.000172810 |  |
| plan-sql-s5_cmp_across_scales_exact/duckdb/1 | 0.000727240 | 0.000765225 | 0.000778684 |  |
| plan-sql-s5_cmp_across_scales_exact/nata/1 | 0.000171064 | 0.000176505 | 0.000194670 | MET |
| plan-sql-s5_cmp_across_scales_exact/twin/1 | 0.000166974 | 0.000179160 | 0.000182720 |  |
| plan-sql-s5_cmp_bool_bang/duckdb/1 | 0.000552495 | 0.000576500 | 0.000594779 |  |
| plan-sql-s5_cmp_bool_bang/nata/1 | 0.000144050 | 0.000157775 | 0.000160350 | MET |
| plan-sql-s5_cmp_bool_bang/twin/1 | 0.000149730 | 0.000153920 | 0.000225900 |  |
| plan-sql-s5_cmp_bool_eq/duckdb/1 | 0.000550864 | 0.000569395 | 0.000611479 |  |
| plan-sql-s5_cmp_bool_eq/nata/1 | 0.000153450 | 0.000157434 | 0.000161700 | MET |
| plan-sql-s5_cmp_bool_eq/twin/1 | 0.000142450 | 0.000157565 | 0.000163120 |  |
| plan-sql-s5_cmp_bool_ge/duckdb/1 | 0.000583195 | 0.000596620 | 0.000600330 |  |
| plan-sql-s5_cmp_bool_ge/nata/1 | 0.000152755 | 0.000157400 | 0.000161375 | MET |
| plan-sql-s5_cmp_bool_ge/twin/1 | 0.000158439 | 0.000163800 | 0.000169365 |  |
| plan-sql-s5_cmp_bool_gt/duckdb/1 | 0.000593880 | 0.000639185 | 0.000659489 |  |
| plan-sql-s5_cmp_bool_gt/nata/1 | 0.000147320 | 0.000160305 | 0.000168965 | MET |
| plan-sql-s5_cmp_bool_gt/twin/1 | 0.000150925 | 0.000162974 | 0.000173235 |  |
| plan-sql-s5_cmp_bool_le/duckdb/1 | 0.000604209 | 0.000609680 | 0.000624265 |  |
| plan-sql-s5_cmp_bool_le/nata/1 | 0.000154269 | 0.000157920 | 0.000161110 | MET |
| plan-sql-s5_cmp_bool_le/twin/1 | 0.000154870 | 0.000157885 | 0.000159350 |  |
| plan-sql-s5_cmp_bool_lt/duckdb/1 | 0.000554330 | 0.000575374 | 0.000580450 |  |
| plan-sql-s5_cmp_bool_lt/nata/1 | 0.000149215 | 0.000160485 | 0.000165230 | MET |
| plan-sql-s5_cmp_bool_lt/twin/1 | 0.000151315 | 0.000158020 | 0.000164985 |  |
| plan-sql-s5_cmp_bool_ne/duckdb/1 | 0.000554409 | 0.000571285 | 0.000596660 |  |
| plan-sql-s5_cmp_bool_ne/nata/1 | 0.000151255 | 0.000162885 | 0.000165815 | MET |
| plan-sql-s5_cmp_bool_ne/twin/1 | 0.000148555 | 0.000162125 | 0.000174775 |  |
| plan-sql-s5_cmp_columns/duckdb/1 | 0.000772050 | 0.000801325 | 0.000823445 |  |
| plan-sql-s5_cmp_columns/nata/1 | 0.000166070 | 0.000186470 | 0.000191555 | MET |
| plan-sql-s5_cmp_columns/twin/1 | 0.000168175 | 0.000181625 | 0.000193875 |  |
| plan-sql-s5_cmp_date_bang/duckdb/1 | 0.000567124 | 0.000598480 | 0.000632025 |  |
| plan-sql-s5_cmp_date_bang/nata/1 | 0.000153055 | 0.000162180 | 0.000163119 | MET |
| plan-sql-s5_cmp_date_bang/twin/1 | 0.000151320 | 0.000153605 | 0.000163910 |  |
| plan-sql-s5_cmp_date_eq/duckdb/1 | 0.000560715 | 0.000582400 | 0.000607360 |  |
| plan-sql-s5_cmp_date_eq/nata/1 | 0.000153085 | 0.000158875 | 0.000163305 | MET |
| plan-sql-s5_cmp_date_eq/twin/1 | 0.000150610 | 0.000155695 | 0.000162350 |  |
| plan-sql-s5_cmp_date_ge/duckdb/1 | 0.000574120 | 0.000593530 | 0.000613715 |  |
| plan-sql-s5_cmp_date_ge/nata/1 | 0.000147375 | 0.000159035 | 0.000165675 | MET |
| plan-sql-s5_cmp_date_ge/twin/1 | 0.000149420 | 0.000157235 | 0.000171535 |  |
| plan-sql-s5_cmp_date_gt/duckdb/1 | 0.000575040 | 0.000586730 | 0.000628554 |  |
| plan-sql-s5_cmp_date_gt/nata/1 | 0.000153625 | 0.000155895 | 0.000167615 | MET |
| plan-sql-s5_cmp_date_gt/twin/1 | 0.000155075 | 0.000161260 | 0.000167100 |  |
| plan-sql-s5_cmp_date_le/duckdb/1 | 0.000563530 | 0.000578604 | 0.000587405 |  |
| plan-sql-s5_cmp_date_le/nata/1 | 0.000152665 | 0.000154020 | 0.000158100 | MET |
| plan-sql-s5_cmp_date_le/twin/1 | 0.000152565 | 0.000156450 | 0.000157085 |  |
| plan-sql-s5_cmp_date_lt/duckdb/1 | 0.000533640 | 0.000586960 | 0.000628410 |  |
| plan-sql-s5_cmp_date_lt/nata/1 | 0.000140565 | 0.000161085 | 0.000166585 | MET |
| plan-sql-s5_cmp_date_lt/twin/1 | 0.000142760 | 0.000154360 | 0.000164970 |  |
| plan-sql-s5_cmp_date_ne/duckdb/1 | 0.000565015 | 0.000577360 | 0.000593985 |  |
| plan-sql-s5_cmp_date_ne/nata/1 | 0.000148060 | 0.000154245 | 0.000158940 | MET |
| plan-sql-s5_cmp_date_ne/twin/1 | 0.000151255 | 0.000156005 | 0.000236700 |  |
| plan-sql-s5_cmp_dec_bang/duckdb/1 | 0.000591269 | 0.000622400 | 0.000635414 |  |
| plan-sql-s5_cmp_dec_bang/nata/1 | 0.000155320 | 0.000162815 | 0.000230465 | MET |
| plan-sql-s5_cmp_dec_bang/twin/1 | 0.000156525 | 0.000163210 | 0.000229915 |  |
| plan-sql-s5_cmp_dec_eq/duckdb/1 | 0.000601700 | 0.000632640 | 0.000639460 |  |
| plan-sql-s5_cmp_dec_eq/nata/1 | 0.000156115 | 0.000161640 | 0.000170705 | MET |
| plan-sql-s5_cmp_dec_eq/twin/1 | 0.000156815 | 0.000164020 | 0.000247960 |  |
| plan-sql-s5_cmp_dec_ge/duckdb/1 | 0.000610399 | 0.000635984 | 0.000642344 |  |
| plan-sql-s5_cmp_dec_ge/nata/1 | 0.000152600 | 0.000161025 | 0.000163720 | MET |
| plan-sql-s5_cmp_dec_ge/twin/1 | 0.000157755 | 0.000163820 | 0.000166325 |  |
| plan-sql-s5_cmp_dec_gt/duckdb/1 | 0.000618414 | 0.000664335 | 0.000681075 |  |
| plan-sql-s5_cmp_dec_gt/nata/1 | 0.000159695 | 0.000168795 | 0.000170535 | MET |
| plan-sql-s5_cmp_dec_gt/twin/1 | 0.000156150 | 0.000169075 | 0.000173219 |  |
| plan-sql-s5_cmp_dec_le/duckdb/1 | 0.000624274 | 0.000652490 | 0.000694170 |  |
| plan-sql-s5_cmp_dec_le/nata/1 | 0.000161745 | 0.000170665 | 0.000177165 | MET |
| plan-sql-s5_cmp_dec_le/twin/1 | 0.000161445 | 0.000165405 | 0.000181610 |  |
| plan-sql-s5_cmp_dec_lt/duckdb/1 | 0.000614415 | 0.000652740 | 0.000658895 |  |
| plan-sql-s5_cmp_dec_lt/nata/1 | 0.000153860 | 0.000163515 | 0.000179810 | MET |
| plan-sql-s5_cmp_dec_lt/twin/1 | 0.000155395 | 0.000163285 | 0.000173510 |  |
| plan-sql-s5_cmp_dec_ne/duckdb/1 | 0.000609090 | 0.000626480 | 0.000677120 |  |
| plan-sql-s5_cmp_dec_ne/nata/1 | 0.000151555 | 0.000164269 | 0.000167860 | MET |
| plan-sql-s5_cmp_dec_ne/twin/1 | 0.000159165 | 0.000164225 | 0.000171930 |  |
| plan-sql-s5_cmp_decimal_with_wide_and_narrow/duckdb/1 | 0.000682455 | 0.000701684 | 0.000736284 |  |
| plan-sql-s5_cmp_decimal_with_wide_and_narrow/nata/1 | 0.000158984 | 0.000169470 | 0.000172060 | MET |
| plan-sql-s5_cmp_decimal_with_wide_and_narrow/twin/1 | 0.000163934 | 0.000165840 | 0.000173040 |  |
| plan-sql-s5_cmp_double_bang/duckdb/1 | 0.000613355 | 0.000625189 | 0.000667444 |  |
| plan-sql-s5_cmp_double_bang/nata/1 | 0.000156350 | 0.000165900 | 0.000240440 | MET |
| plan-sql-s5_cmp_double_bang/twin/1 | 0.000157530 | 0.000167595 | 0.000171270 |  |
| plan-sql-s5_cmp_double_eq/duckdb/1 | 0.000604909 | 0.000636015 | 0.000676469 |  |
| plan-sql-s5_cmp_double_eq/nata/1 | 0.000162650 | 0.000166580 | 0.000170670 | MET |
| plan-sql-s5_cmp_double_eq/twin/1 | 0.000153085 | 0.000164165 | 0.000171710 |  |
| plan-sql-s5_cmp_double_ge/duckdb/1 | 0.000642345 | 0.000670335 | 0.000677434 |  |
| plan-sql-s5_cmp_double_ge/nata/1 | 0.000166975 | 0.000167990 | 0.000175780 | MET |
| plan-sql-s5_cmp_double_ge/twin/1 | 0.000164085 | 0.000170485 | 0.000173790 |  |
| plan-sql-s5_cmp_double_gt/duckdb/1 | 0.000651700 | 0.000662205 | 0.000677750 |  |
| plan-sql-s5_cmp_double_gt/nata/1 | 0.000164319 | 0.000170470 | 0.000255320 | MET |
| plan-sql-s5_cmp_double_gt/twin/1 | 0.000165535 | 0.000171115 | 0.000172955 |  |
| plan-sql-s5_cmp_double_le/duckdb/1 | 0.000642920 | 0.000662429 | 0.000662685 |  |
| plan-sql-s5_cmp_double_le/nata/1 | 0.000165099 | 0.000172410 | 0.000179875 | MET |
| plan-sql-s5_cmp_double_le/twin/1 | 0.000165525 | 0.000171715 | 0.000178885 |  |
| plan-sql-s5_cmp_double_lt/duckdb/1 | 0.000639774 | 0.000658810 | 0.000669635 |  |
| plan-sql-s5_cmp_double_lt/nata/1 | 0.000164150 | 0.000167045 | 0.000175404 | MET |
| plan-sql-s5_cmp_double_lt/twin/1 | 0.000162980 | 0.000168880 | 0.000174120 |  |
| plan-sql-s5_cmp_double_ne/duckdb/1 | 0.000638299 | 0.000654965 | 0.000670510 |  |
| plan-sql-s5_cmp_double_ne/nata/1 | 0.000168975 | 0.000169385 | 0.000175685 | MET |
| plan-sql-s5_cmp_double_ne/twin/1 | 0.000165144 | 0.000169225 | 0.000175145 |  |
| plan-sql-s5_cmp_int_bang/duckdb/1 | 0.000546810 | 0.000559985 | 0.000585029 |  |
| plan-sql-s5_cmp_int_bang/nata/1 | 0.000151130 | 0.000155465 | 0.000169045 | MET |
| plan-sql-s5_cmp_int_bang/twin/1 | 0.000147190 | 0.000155805 | 0.000159700 |  |
| plan-sql-s5_cmp_int_eq/duckdb/1 | 0.000549204 | 0.000568649 | 0.000579480 |  |
| plan-sql-s5_cmp_int_eq/nata/1 | 0.000151835 | 0.000154825 | 0.000156595 | MET |
| plan-sql-s5_cmp_int_eq/twin/1 | 0.000155485 | 0.000156995 | 0.000161220 |  |
| plan-sql-s5_cmp_int_ge/duckdb/1 | 0.000533375 | 0.000545210 | 0.000548260 |  |
| plan-sql-s5_cmp_int_ge/nata/1 | 0.000142565 | 0.000146405 | 0.000151959 | MET |
| plan-sql-s5_cmp_int_ge/twin/1 | 0.000148005 | 0.000149490 | 0.000154569 |  |
| plan-sql-s5_cmp_int_gt/duckdb/1 | 0.000529550 | 0.000539165 | 0.000577265 |  |
| plan-sql-s5_cmp_int_gt/nata/1 | 0.000148905 | 0.000149809 | 0.000167320 | MET |
| plan-sql-s5_cmp_int_gt/twin/1 | 0.000142910 | 0.000151300 | 0.000156550 |  |
| plan-sql-s5_cmp_int_le/duckdb/1 | 0.000530469 | 0.000538735 | 0.000544960 |  |
| plan-sql-s5_cmp_int_le/nata/1 | 0.000147150 | 0.000148795 | 0.000156425 | MET |
| plan-sql-s5_cmp_int_le/twin/1 | 0.000148155 | 0.000148975 | 0.000162530 |  |
| plan-sql-s5_cmp_int_lt/duckdb/1 | 0.000539160 | 0.000580170 | 0.000587395 |  |
| plan-sql-s5_cmp_int_lt/nata/1 | 0.000150730 | 0.000159140 | 0.000166089 | MET |
| plan-sql-s5_cmp_int_lt/twin/1 | 0.000150545 | 0.000155800 | 0.000169045 |  |
| plan-sql-s5_cmp_int_ne/duckdb/1 | 0.000548314 | 0.000581190 | 0.000607300 |  |
| plan-sql-s5_cmp_int_ne/nata/1 | 0.000149990 | 0.000164169 | 0.000165140 | MET |
| plan-sql-s5_cmp_int_ne/twin/1 | 0.000154590 | 0.000157405 | 0.000166175 |  |
| plan-sql-s5_cmp_real_bang/duckdb/1 | 0.000523759 | 0.000544740 | 0.000556719 |  |
| plan-sql-s5_cmp_real_bang/nata/1 | 0.000144544 | 0.000150920 | 0.000159300 | MET |
| plan-sql-s5_cmp_real_bang/twin/1 | 0.000141125 | 0.000150895 | 0.000153695 |  |
| plan-sql-s5_cmp_real_eq/duckdb/1 | 0.000537495 | 0.000563695 | 0.000576650 |  |
| plan-sql-s5_cmp_real_eq/nata/1 | 0.000151974 | 0.000153295 | 0.000166910 | MET |
| plan-sql-s5_cmp_real_eq/twin/1 | 0.000147445 | 0.000152580 | 0.000154870 |  |
| plan-sql-s5_cmp_real_ge/duckdb/1 | 0.000533700 | 0.000539645 | 0.000564530 |  |
| plan-sql-s5_cmp_real_ge/nata/1 | 0.000138939 | 0.000149450 | 0.000159494 | MET |
| plan-sql-s5_cmp_real_ge/twin/1 | 0.000148785 | 0.000150335 | 0.000154385 |  |
| plan-sql-s5_cmp_real_gt/duckdb/1 | 0.000523595 | 0.000540450 | 0.000552820 |  |
| plan-sql-s5_cmp_real_gt/nata/1 | 0.000142015 | 0.000147750 | 0.000227520 | MET |
| plan-sql-s5_cmp_real_gt/twin/1 | 0.000146710 | 0.000149170 | 0.000163005 |  |
| plan-sql-s5_cmp_real_le/duckdb/1 | 0.000525859 | 0.000558974 | 0.000590454 |  |
| plan-sql-s5_cmp_real_le/nata/1 | 0.000147380 | 0.000155480 | 0.000164300 | MET |
| plan-sql-s5_cmp_real_le/twin/1 | 0.000142639 | 0.000152050 | 0.000158915 |  |
| plan-sql-s5_cmp_real_lt/duckdb/1 | 0.000508910 | 0.000510715 | 0.000537944 |  |
| plan-sql-s5_cmp_real_lt/nata/1 | 0.000140660 | 0.000145585 | 0.000150870 | MET |
| plan-sql-s5_cmp_real_lt/twin/1 | 0.000141330 | 0.000146565 | 0.000151205 |  |
| plan-sql-s5_cmp_real_ne/duckdb/1 | 0.000544660 | 0.000551789 | 0.000583105 |  |
| plan-sql-s5_cmp_real_ne/nata/1 | 0.000155330 | 0.000156290 | 0.000156645 | MET |
| plan-sql-s5_cmp_real_ne/twin/1 | 0.000149485 | 0.000155805 | 0.000167590 |  |
| plan-sql-s5_cmp_str_bang/duckdb/1 | 0.000549895 | 0.000562745 | 0.000567905 |  |
| plan-sql-s5_cmp_str_bang/nata/1 | 0.000148930 | 0.000151505 | 0.000154895 | MET |
| plan-sql-s5_cmp_str_bang/twin/1 | 0.000145655 | 0.000152435 | 0.000226805 |  |
| plan-sql-s5_cmp_str_eq/duckdb/1 | 0.000545805 | 0.000562005 | 0.000566379 |  |
| plan-sql-s5_cmp_str_eq/nata/1 | 0.000145810 | 0.000152310 | 0.000160130 | MET |
| plan-sql-s5_cmp_str_eq/twin/1 | 0.000149830 | 0.000158485 | 0.000162950 |  |
| plan-sql-s5_cmp_str_ge/duckdb/1 | 0.000537085 | 0.000547010 | 0.000563250 |  |
| plan-sql-s5_cmp_str_ge/nata/1 | 0.000145440 | 0.000153310 | 0.000237579 | MET |
| plan-sql-s5_cmp_str_ge/twin/1 | 0.000142885 | 0.000154080 | 0.000158845 |  |
| plan-sql-s5_cmp_str_gt/duckdb/1 | 0.000527880 | 0.000543494 | 0.000581480 |  |
| plan-sql-s5_cmp_str_gt/nata/1 | 0.000141415 | 0.000149935 | 0.000151710 | MET |
| plan-sql-s5_cmp_str_gt/twin/1 | 0.000146565 | 0.000154560 | 0.000223740 |  |
| plan-sql-s5_cmp_str_le/duckdb/1 | 0.000536829 | 0.000548740 | 0.000579945 |  |
| plan-sql-s5_cmp_str_le/nata/1 | 0.000142590 | 0.000150420 | 0.000160415 | MET |
| plan-sql-s5_cmp_str_le/twin/1 | 0.000146500 | 0.000147775 | 0.000149725 |  |
| plan-sql-s5_cmp_str_lt/duckdb/1 | 0.000522155 | 0.000538060 | 0.000555570 |  |
| plan-sql-s5_cmp_str_lt/nata/1 | 0.000146285 | 0.000148575 | 0.000157800 | MET |
| plan-sql-s5_cmp_str_lt/twin/1 | 0.000141815 | 0.000149760 | 0.000159470 |  |
| plan-sql-s5_cmp_str_ne/duckdb/1 | 0.000545510 | 0.000567764 | 0.000575395 |  |
| plan-sql-s5_cmp_str_ne/nata/1 | 0.000147645 | 0.000151145 | 0.000155520 | MET |
| plan-sql-s5_cmp_str_ne/twin/1 | 0.000149525 | 0.000152270 | 0.000156480 |  |
| plan-sql-s5_in_list/duckdb/1 | 0.001107729 | 0.001168209 | 0.001193054 |  |
| plan-sql-s5_in_list/nata/1 | 0.000193600 | 0.000206775 | 0.000217045 | MET |
| plan-sql-s5_in_list/twin/1 | 0.000188510 | 0.000204169 | 0.000212155 |  |
| plan-sql-s5_in_list_mixed_numeric/duckdb/1 | 0.000896120 | 0.000934624 | 0.000955064 |  |
| plan-sql-s5_in_list_mixed_numeric/nata/1 | 0.000164974 | 0.000175240 | 0.000186930 | MET |
| plan-sql-s5_in_list_mixed_numeric/twin/1 | 0.000165670 | 0.000173780 | 0.000179455 |  |
| plan-sql-s5_in_list_null/duckdb/1 | 0.000792600 | 0.000837729 | 0.000855225 |  |
| plan-sql-s5_in_list_null/nata/1 | 0.000180710 | 0.000195385 | 0.000270505 | MET |
| plan-sql-s5_in_list_null/twin/1 | 0.000190405 | 0.000195089 | 0.000206215 |  |
| plan-sql-s5_in_list_where/duckdb/1 | 0.000838670 | 0.000857070 | 0.000889825 |  |
| plan-sql-s5_in_list_where/nata/1 | 0.000165635 | 0.000181955 | 0.000242015 | MET |
| plan-sql-s5_in_list_where/twin/1 | 0.000163595 | 0.000184080 | 0.000224730 |  |
| plan-sql-s5_is_null/duckdb/1 | 0.000825710 | 0.000841055 | 0.000861829 |  |
| plan-sql-s5_is_null/nata/1 | 0.000176425 | 0.000186315 | 0.000210530 | MET |
| plan-sql-s5_is_null/twin/1 | 0.000187815 | 0.000196430 | 0.000204270 |  |
| plan-sql-s5_is_null_where/duckdb/1 | 0.000679215 | 0.000716570 | 0.000734584 |  |
| plan-sql-s5_is_null_where/nata/1 | 0.000158095 | 0.000160230 | 0.000167375 | MET |
| plan-sql-s5_is_null_where/twin/1 | 0.000151270 | 0.000159355 | 0.000168430 |  |
| plan-sql-s5_nan_semantics/duckdb/1 | 0.000719575 | 0.000732875 | 0.000753509 |  |
| plan-sql-s5_nan_semantics/nata/1 | 0.000173890 | 0.000178979 | 0.000181185 | MET |
| plan-sql-s5_nan_semantics/twin/1 | 0.000177705 | 0.000181070 | 0.000184660 |  |
| plan-sql-s5_negative_zero/duckdb/1 | 0.000870790 | 0.000890230 | 0.000930730 |  |
| plan-sql-s5_negative_zero/nata/1 | 0.000173400 | 0.000202030 | 0.000204309 | MET |
| plan-sql-s5_negative_zero/twin/1 | 0.000177435 | 0.000186635 | 0.000201175 |  |
| plan-sql-s5_not_null/duckdb/1 | 0.000619895 | 0.000641345 | 0.000696260 |  |
| plan-sql-s5_not_null/nata/1 | 0.000171675 | 0.000180920 | 0.000209534 | MET |
| plan-sql-s5_not_null/twin/1 | 0.000173815 | 0.000180460 | 0.000205745 |  |
| plan-sql-s5_where_nan/duckdb/1 | 0.000613265 | 0.000629574 | 0.000647009 |  |
| plan-sql-s5_where_nan/nata/1 | 0.000155315 | 0.000158730 | 0.000169404 | MET |
| plan-sql-s5_where_nan/twin/1 | 0.000153775 | 0.000154895 | 0.000167255 |  |
| plan-sql-s5_where_nan_greater/duckdb/1 | 0.000609765 | 0.000620110 | 0.000632310 |  |
| plan-sql-s5_where_nan_greater/nata/1 | 0.000150950 | 0.000155585 | 0.000162040 | MET |
| plan-sql-s5_where_nan_greater/twin/1 | 0.000152350 | 0.000156735 | 0.000163374 |  |
| plan-sql-s5_where_null_drops/duckdb/1 | 0.000587184 | 0.000602769 | 0.000614665 |  |
| plan-sql-s5_where_null_drops/nata/1 | 0.000149750 | 0.000153435 | 0.000159570 | MET |
| plan-sql-s5_where_null_drops/twin/1 | 0.000146945 | 0.000152620 | 0.000160389 |  |
| plan-sql-s5_where_or_null/duckdb/1 | 0.000628834 | 0.000641220 | 0.000673085 |  |
| plan-sql-s5_where_or_null/nata/1 | 0.000149020 | 0.000157400 | 0.000226610 | MET |
| plan-sql-s5_where_or_null/twin/1 | 0.000145939 | 0.000153960 | 0.000161835 |  |
| plan-sql-s5_where_x_eq_null/duckdb/1 | 0.000458774 | 0.000469695 | 0.000495870 |  |
| plan-sql-s5_where_x_eq_null/nata/1 | 0.000139800 | 0.000141970 | 0.000235245 | MET |
| plan-sql-s5_where_x_eq_null/twin/1 | 0.000140945 | 0.000149019 | 0.000158024 |  |
| plan-sql-s5_where_x_ne_null/duckdb/1 | 0.000632430 | 0.000654529 | 0.000696220 |  |
| plan-sql-s5_where_x_ne_null/nata/1 | 0.000157680 | 0.000159795 | 0.000165190 | MET |
| plan-sql-s5_where_x_ne_null/twin/1 | 0.000160570 | 0.000162265 | 0.000166975 |  |
| plan-sql-s6_case_dates_strings/duckdb/1 | 0.000607675 | 0.000639720 | 0.000658124 |  |
| plan-sql-s6_case_dates_strings/nata/1 | 0.000158115 | 0.000168735 | 0.000176905 | MET |
| plan-sql-s6_case_dates_strings/twin/1 | 0.000157405 | 0.000165435 | 0.000166960 |  |
| plan-sql-s6_case_double/duckdb/1 | 0.000618385 | 0.000645990 | 0.000677440 |  |
| plan-sql-s6_case_double/nata/1 | 0.000165410 | 0.000175395 | 0.000181340 | MET |
| plan-sql-s6_case_double/twin/1 | 0.000163590 | 0.000166975 | 0.000183040 |  |
| plan-sql-s6_case_else/duckdb/1 | 0.000580800 | 0.000593159 | 0.000616675 |  |
| plan-sql-s6_case_else/nata/1 | 0.000159475 | 0.000162250 | 0.000173900 | MET |
| plan-sql-s6_case_else/twin/1 | 0.000157035 | 0.000157334 | 0.000163639 |  |
| plan-sql-s6_case_guard_overflow/duckdb/1 | 0.000687915 | 0.000725130 | 0.000735195 |  |
| plan-sql-s6_case_guard_overflow/nata/1 | 0.000174320 | 0.000175845 | 0.000183905 | MET |
| plan-sql-s6_case_guard_overflow/twin/1 | 0.000160435 | 0.000176955 | 0.000186080 |  |
| plan-sql-s6_case_in_where/duckdb/1 | 0.000716234 | 0.000720269 | 0.000771340 |  |
| plan-sql-s6_case_in_where/nata/1 | 0.000157800 | 0.000161125 | 0.000240699 | MET |
| plan-sql-s6_case_in_where/twin/1 | 0.000158445 | 0.000163045 | 0.000169175 |  |
| plan-sql-s6_case_nested/duckdb/1 | 0.000617635 | 0.000621935 | 0.000672710 |  |
| plan-sql-s6_case_nested/nata/1 | 0.000167545 | 0.000170135 | 0.000177930 | MET |
| plan-sql-s6_case_nested/twin/1 | 0.000166670 | 0.000170815 | 0.000173215 |  |
| plan-sql-s6_case_null_condition_falls_through/duckdb/1 | 0.000582864 | 0.000588494 | 0.000624285 |  |
| plan-sql-s6_case_null_condition_falls_through/nata/1 | 0.000159210 | 0.000162925 | 0.000172205 | MET |
| plan-sql-s6_case_null_condition_falls_through/twin/1 | 0.000159889 | 0.000162300 | 0.000163219 |  |
| plan-sql-s6_case_only_taken_branch_evaluated/duckdb/1 | 0.000651619 | 0.000657410 | 0.000721984 |  |
| plan-sql-s6_case_only_taken_branch_evaluated/nata/1 | 0.000160190 | 0.000165685 | 0.000174965 | MET |
| plan-sql-s6_case_only_taken_branch_evaluated/twin/1 | 0.000159490 | 0.000165995 | 0.000176460 |  |
| plan-sql-s6_case_searched/duckdb/1 | 0.000637910 | 0.000677509 | 0.000685539 |  |
| plan-sql-s6_case_searched/nata/1 | 0.000162470 | 0.000175695 | 0.000180275 | MET |
| plan-sql-s6_case_searched/twin/1 | 0.000165040 | 0.000178720 | 0.000181715 |  |
| plan-sql-s6_case_simple/duckdb/1 | 0.000600285 | 0.000621720 | 0.000658225 |  |
| plan-sql-s6_case_simple/nata/1 | 0.000160630 | 0.000163055 | 0.000172160 | MET |
| plan-sql-s6_case_simple/twin/1 | 0.000163164 | 0.000168180 | 0.000170425 |  |
| plan-sql-s6_case_simple_null_operand/duckdb/1 | 0.000609890 | 0.000643610 | 0.000676795 |  |
| plan-sql-s6_case_simple_null_operand/nata/1 | 0.000165625 | 0.000176970 | 0.000181484 | MET |
| plan-sql-s6_case_simple_null_operand/twin/1 | 0.000167055 | 0.000176940 | 0.000200025 |  |
| plan-sql-s6_case_untaken_branch_but_taken_row_errors/duckdb/1 | 0.000662690 | 0.000682289 | 0.000692174 |  |
| plan-sql-s6_case_untaken_branch_but_taken_row_errors/nata/1 | 0.000168745 | 0.000174655 | 0.000179215 | MET |
| plan-sql-s6_case_untaken_branch_but_taken_row_errors/twin/1 | 0.000165015 | 0.000175830 | 0.000178320 |  |
| plan-sql-s6_case_widens_decimals/duckdb/1 | 0.000651325 | 0.000663355 | 0.000684569 |  |
| plan-sql-s6_case_widens_decimals/nata/1 | 0.000167670 | 0.000171450 | 0.000179195 | MET |
| plan-sql-s6_case_widens_decimals/twin/1 | 0.000166780 | 0.000171434 | 0.000173565 |  |
| plan-sql-s6_case_widens_ints/duckdb/1 | 0.000579439 | 0.000595720 | 0.000611335 |  |
| plan-sql-s6_case_widens_ints/nata/1 | 0.000151620 | 0.000158875 | 0.000162420 | MET |
| plan-sql-s6_case_widens_ints/twin/1 | 0.000151615 | 0.000159360 | 0.000163970 |  |
| plan-sql-s6_cast_bad_date/duckdb/1 | 0.000502490 | 0.000540950 | 0.000557565 |  |
| plan-sql-s6_cast_bad_date/nata/1 | 0.000139735 | 0.000145445 | 0.000155750 | MET |
| plan-sql-s6_cast_bad_date/twin/1 | 0.000143100 | 0.000151640 | 0.000155254 |  |
| plan-sql-s6_cast_decimal_default_scale/duckdb/1 | 0.000581405 | 0.000606959 | 0.000621475 |  |
| plan-sql-s6_cast_decimal_default_scale/nata/1 | 0.000165725 | 0.000166610 | 0.000173275 | MET |
| plan-sql-s6_cast_decimal_default_scale/twin/1 | 0.000162125 | 0.000164585 | 0.000172250 |  |
| plan-sql-s6_cast_decimal_does_not_fit/duckdb/1 | 0.000646255 | 0.000667415 | 0.000682730 |  |
| plan-sql-s6_cast_decimal_does_not_fit/nata/1 | 0.000159025 | 0.000165354 | 0.000169115 | MET |
| plan-sql-s6_cast_decimal_does_not_fit/twin/1 | 0.000160180 | 0.000161580 | 0.000168580 |  |
| plan-sql-s6_cast_decimal_to_decimal/duckdb/1 | 0.001024059 | 0.001076410 | 0.001135290 |  |
| plan-sql-s6_cast_decimal_to_decimal/nata/1 | 0.000173634 | 0.000180260 | 0.000192450 | MET |
| plan-sql-s6_cast_decimal_to_decimal/twin/1 | 0.000175460 | 0.000187040 | 0.000195355 |  |
| plan-sql-s6_cast_decimal_to_double/duckdb/1 | 0.000618909 | 0.000638405 | 0.000655015 |  |
| plan-sql-s6_cast_decimal_to_double/nata/1 | 0.000160505 | 0.000166490 | 0.000171065 | MET |
| plan-sql-s6_cast_decimal_to_double/twin/1 | 0.000163640 | 0.000167925 | 0.000173715 |  |
| plan-sql-s6_cast_decimal_to_int_rounds/duckdb/1 | 0.001135960 | 0.001138164 | 0.001176320 |  |
| plan-sql-s6_cast_decimal_to_int_rounds/nata/1 | 0.000184455 | 0.000189935 | 0.000203255 | MET |
| plan-sql-s6_cast_decimal_to_int_rounds/twin/1 | 0.000174615 | 0.000187745 | 0.000194849 |  |
| plan-sql-s6_cast_double_to_decimal_exact_values/duckdb/1 | 0.001019649 | 0.001058345 | 0.001083735 |  |
| plan-sql-s6_cast_double_to_decimal_exact_values/nata/1 | 0.000170555 | 0.000176500 | 0.000180935 | MET |
| plan-sql-s6_cast_double_to_decimal_exact_values/twin/1 | 0.000172555 | 0.000178205 | 0.000181560 |  |
| plan-sql-s6_cast_double_to_int_exact_values/duckdb/1 | 0.000928689 | 0.000937099 | 0.001082720 |  |
| plan-sql-s6_cast_double_to_int_exact_values/nata/1 | 0.000161235 | 0.000165775 | 0.000177600 | MET |
| plan-sql-s6_cast_double_to_int_exact_values/twin/1 | 0.000163970 | 0.000165645 | 0.000175460 |  |
| plan-sql-s6_cast_double_too_big_for_decimal/duckdb/1 | 0.000635090 | 0.000662540 | 0.000687135 |  |
| plan-sql-s6_cast_double_too_big_for_decimal/nata/1 | 0.000157190 | 0.000161945 | 0.000170110 | MET |
| plan-sql-s6_cast_double_too_big_for_decimal/twin/1 | 0.000155620 | 0.000166930 | 0.000247045 |  |
| plan-sql-s6_cast_from_varchar/duckdb/1 | 0.000756729 | 0.000784975 | 0.000785579 |  |
| plan-sql-s6_cast_from_varchar/nata/1 | 0.000185715 | 0.000198280 | 0.000201264 | MET |
| plan-sql-s6_cast_from_varchar/twin/1 | 0.000190290 | 0.000195330 | 0.000205215 |  |
| plan-sql-s6_cast_half_away_from_zero/duckdb/1 | 0.001025369 | 0.001052899 | 0.001075205 |  |
| plan-sql-s6_cast_half_away_from_zero/nata/1 | 0.000193755 | 0.000203210 | 0.000212560 | MET |
| plan-sql-s6_cast_half_away_from_zero/twin/1 | 0.000192395 | 0.000198544 | 0.000210934 |  |
| plan-sql-s6_cast_in_where/duckdb/1 | 0.000710659 | 0.000761764 | 0.000784344 |  |
| plan-sql-s6_cast_in_where/nata/1 | 0.000162069 | 0.000168555 | 0.000176560 | MET |
| plan-sql-s6_cast_in_where/twin/1 | 0.000161900 | 0.000170230 | 0.000181310 |  |
| plan-sql-s6_cast_inf_to_integer/duckdb/1 | 0.000623224 | 0.000651560 | 0.000674180 |  |
| plan-sql-s6_cast_inf_to_integer/nata/1 | 0.000157755 | 0.000168180 | 0.000212305 | MET |
| plan-sql-s6_cast_inf_to_integer/twin/1 | 0.000156315 | 0.000161765 | 0.000175890 |  |
| plan-sql-s6_cast_int_narrow_fails/duckdb/1 | 0.000508295 | 0.000540310 | 0.000550560 |  |
| plan-sql-s6_cast_int_narrow_fails/nata/1 | 0.000146575 | 0.000154560 | 0.000168030 | MET |
| plan-sql-s6_cast_int_narrow_fails/twin/1 | 0.000146660 | 0.000156084 | 0.000159920 |  |
| plan-sql-s6_cast_int_narrow_ok/duckdb/1 | 0.000808579 | 0.000836129 | 0.000837699 |  |
| plan-sql-s6_cast_int_narrow_ok/nata/1 | 0.000167814 | 0.000172090 | 0.000181590 | MET |
| plan-sql-s6_cast_int_narrow_ok/twin/1 | 0.000169245 | 0.000170724 | 0.000180350 |  |
| plan-sql-s6_cast_int_to_decimal/duckdb/1 | 0.001033104 | 0.001085139 | 0.001120679 |  |
| plan-sql-s6_cast_int_to_decimal/nata/1 | 0.000180830 | 0.000188350 | 0.000198360 | MET |
| plan-sql-s6_cast_int_to_decimal/twin/1 | 0.000179120 | 0.000186950 | 0.000200450 |  |
| plan-sql-s6_cast_int_to_decimal_overflow/duckdb/1 | 0.000646925 | 0.000698245 | 0.000728965 |  |
| plan-sql-s6_cast_int_to_decimal_overflow/nata/1 | 0.000159435 | 0.000170975 | 0.000175505 | MET |
| plan-sql-s6_cast_int_to_decimal_overflow/twin/1 | 0.000154040 | 0.000169515 | 0.000173049 |  |
| plan-sql-s6_cast_int_to_double/duckdb/1 | 0.000594045 | 0.000628670 | 0.000648204 |  |
| plan-sql-s6_cast_int_to_double/nata/1 | 0.000150700 | 0.000163860 | 0.000166235 | MET |
| plan-sql-s6_cast_int_to_double/twin/1 | 0.000156940 | 0.000160975 | 0.000224400 |  |
| plan-sql-s6_cast_int_widen/duckdb/1 | 0.000625380 | 0.000654240 | 0.000675534 |  |
| plan-sql-s6_cast_int_widen/nata/1 | 0.000156275 | 0.000165195 | 0.000169785 | MET |
| plan-sql-s6_cast_int_widen/twin/1 | 0.000158775 | 0.000162595 | 0.000166780 |  |
| plan-sql-s6_cast_nan_to_decimal/duckdb/1 | 0.000694605 | 0.000709900 | 0.000725099 |  |
| plan-sql-s6_cast_nan_to_decimal/nata/1 | 0.000161595 | 0.000168395 | 0.000254680 | MET |
| plan-sql-s6_cast_nan_to_decimal/twin/1 | 0.000162540 | 0.000167865 | 0.000176205 |  |
| plan-sql-s6_cast_not_a_number/duckdb/1 | 0.000511665 | 0.000524850 | 0.000535925 |  |
| plan-sql-s6_cast_not_a_number/nata/1 | 0.000146485 | 0.000157290 | 0.000157775 | MET |
| plan-sql-s6_cast_not_a_number/twin/1 | 0.000145305 | 0.000150245 | 0.000161290 |  |
| plan-sql-s6_cast_null/duckdb/1 | 0.000541615 | 0.000544940 | 0.000554895 |  |
| plan-sql-s6_cast_null/nata/1 | 0.000148545 | 0.000152755 | 0.000154870 | MET |
| plan-sql-s6_cast_null/twin/1 | 0.000142860 | 0.000146545 | 0.000165550 |  |
| plan-sql-s6_cast_real_to_double/duckdb/1 | 0.001017445 | 0.001037895 | 0.001088210 |  |
| plan-sql-s6_cast_real_to_double/nata/1 | 0.000167030 | 0.000168575 | 0.000181914 | MET |
| plan-sql-s6_cast_real_to_double/twin/1 | 0.000162625 | 0.000165220 | 0.000179915 |  |
| plan-sql-s6_cast_string_overflows_int/duckdb/1 | 0.000507900 | 0.000527194 | 0.000543070 |  |
| plan-sql-s6_cast_string_overflows_int/nata/1 | 0.000140995 | 0.000145120 | 0.000153830 | MET |
| plan-sql-s6_cast_string_overflows_int/twin/1 | 0.000141050 | 0.000145815 | 0.000149150 |  |
| plan-sql-s6_cast_string_to_decimal_does_not_fit/duckdb/1 | 0.000530085 | 0.000531050 | 0.000547565 |  |
| plan-sql-s6_cast_string_to_decimal_does_not_fit/nata/1 | 0.000138720 | 0.000140200 | 0.000146350 | MET |
| plan-sql-s6_cast_string_to_decimal_does_not_fit/twin/1 | 0.000138920 | 0.000140800 | 0.000150180 |  |
| plan-sql-s6_cast_to_varchar/duckdb/1 | 0.000785210 | 0.000793075 | 0.000817695 |  |
| plan-sql-s6_cast_to_varchar/nata/1 | 0.000178150 | 0.000185630 | 0.000194455 | MET |
| plan-sql-s6_cast_to_varchar/twin/1 | 0.000188555 | 0.000191880 | 0.000194795 |  |
| plan-sql-s6_cast_varchar_roundtrip/duckdb/1 | 0.000646980 | 0.000664309 | 0.000686455 |  |
| plan-sql-s6_cast_varchar_roundtrip/nata/1 | 0.000164960 | 0.000169075 | 0.000171530 | MET |
| plan-sql-s6_cast_varchar_roundtrip/twin/1 | 0.000166250 | 0.000169220 | 0.000182960 |  |
| plan-sql-s6_double_colon/duckdb/1 | 0.000670880 | 0.000706799 | 0.000723655 |  |
| plan-sql-s6_double_colon/nata/1 | 0.000157890 | 0.000164355 | 0.000173205 | MET |
| plan-sql-s6_double_colon/twin/1 | 0.000167025 | 0.000171270 | 0.000173479 |  |
| plan-sql-s7_big_limit_filter_selective/duckdb/1 | 0.000874795 | 0.000902260 | 0.000914479 |  |
| plan-sql-s7_big_limit_filter_selective/nata/1 | 0.000181020 | 0.000187790 | 0.000201125 | MET |
| plan-sql-s7_big_limit_filter_selective/twin/1 | 0.000190675 | 0.000192530 | 0.000199550 |  |
| plan-sql-s7_big_limit_offset_across_row_groups/duckdb/1 | 0.000913735 | 0.000921180 | 0.000976400 |  |
| plan-sql-s7_big_limit_offset_across_row_groups/nata/1 | 0.000167920 | 0.000173410 | 0.000181215 | MET |
| plan-sql-s7_big_limit_offset_across_row_groups/twin/1 | 0.000170355 | 0.000174684 | 0.000190365 |  |
| plan-sql-s7_big_limit_row_groups/duckdb/1 | 0.000620669 | 0.000631985 | 0.000651870 |  |
| plan-sql-s7_big_limit_row_groups/nata/1 | 0.000160975 | 0.000166775 | 0.000172870 | MET |
| plan-sql-s7_big_limit_row_groups/twin/1 | 0.000156820 | 0.000168525 | 0.000179765 |  |
| plan-sql-s7_limit_0/duckdb/1 | 0.000441180 | 0.000460150 | 0.000477080 |  |
| plan-sql-s7_limit_0/nata/1 | 0.000146130 | 0.000148925 | 0.000156035 | MET |
| plan-sql-s7_limit_0/twin/1 | 0.000147190 | 0.000149215 | 0.000154175 |  |
| plan-sql-s7_limit_1/duckdb/1 | 0.000498379 | 0.000533675 | 0.000560595 |  |
| plan-sql-s7_limit_1/nata/1 | 0.000137845 | 0.000151135 | 0.000155230 | MET |
| plan-sql-s7_limit_1/twin/1 | 0.000137225 | 0.000155035 | 0.000177915 |  |
| plan-sql-s7_limit_100/duckdb/1 | 0.000724850 | 0.000750310 | 0.000782985 |  |
| plan-sql-s7_limit_100/nata/1 | 0.000137945 | 0.000140665 | 0.000205695 | MET |
| plan-sql-s7_limit_100/twin/1 | 0.000138800 | 0.000140640 | 0.000218170 |  |
| plan-sql-s7_limit_3/duckdb/1 | 0.000484160 | 0.000487270 | 0.000525465 |  |
| plan-sql-s7_limit_3/nata/1 | 0.000134995 | 0.000137134 | 0.000145975 | MET |
| plan-sql-s7_limit_3/twin/1 | 0.000133680 | 0.000134474 | 0.000154154 |  |
| plan-sql-s7_limit_after_filter_not_before/duckdb/1 | 0.000629075 | 0.000653650 | 0.000683710 |  |
| plan-sql-s7_limit_after_filter_not_before/nata/1 | 0.000157880 | 0.000161109 | 0.000236530 | MET |
| plan-sql-s7_limit_after_filter_not_before/twin/1 | 0.000157175 | 0.000167915 | 0.000239960 |  |
| plan-sql-s7_limit_expression_projection/duckdb/1 | 0.000696355 | 0.000730140 | 0.000734364 |  |
| plan-sql-s7_limit_expression_projection/nata/1 | 0.000161885 | 0.000168680 | 0.000171000 | MET |
| plan-sql-s7_limit_expression_projection/twin/1 | 0.000162735 | 0.000167255 | 0.000171105 |  |
| plan-sql-s7_limit_filter/duckdb/1 | 0.000627775 | 0.000640220 | 0.000643525 |  |
| plan-sql-s7_limit_filter/nata/1 | 0.000150585 | 0.000152950 | 0.000166095 | MET |
| plan-sql-s7_limit_filter/twin/1 | 0.000149245 | 0.000152820 | 0.000172710 |  |
| plan-sql-s7_limit_offset_beyond/duckdb/1 | 0.000746515 | 0.000795200 | 0.000803195 |  |
| plan-sql-s7_limit_offset_beyond/nata/1 | 0.000141800 | 0.000143375 | 0.000158660 | MET |
| plan-sql-s7_limit_offset_beyond/twin/1 | 0.000137265 | 0.000148570 | 0.000150625 |  |
| plan-sql-s7_limit_offset_inside/duckdb/1 | 0.000757465 | 0.000779490 | 0.000808094 |  |
| plan-sql-s7_limit_offset_inside/nata/1 | 0.000143990 | 0.000148850 | 0.000151770 | MET |
| plan-sql-s7_limit_offset_inside/twin/1 | 0.000142490 | 0.000149655 | 0.000151390 |  |
| plan-sql-s7_limit_offset_to_the_end/duckdb/1 | 0.000762550 | 0.000777955 | 0.000832764 |  |
| plan-sql-s7_limit_offset_to_the_end/nata/1 | 0.000144744 | 0.000146885 | 0.000149190 | MET |
| plan-sql-s7_limit_offset_to_the_end/twin/1 | 0.000143720 | 0.000148815 | 0.000158395 |  |
| plan-sql-s7_limit_zero_offset/duckdb/1 | 0.000433785 | 0.000447359 | 0.000479370 |  |
| plan-sql-s7_limit_zero_offset/nata/1 | 0.000147880 | 0.000149530 | 0.000180675 | MET |
| plan-sql-s7_limit_zero_offset/twin/1 | 0.000139835 | 0.000149445 | 0.000154830 |  |
| plan-sql-s8_case_mixed_types/duckdb/1 | 0.000621365 | 0.000627489 | 0.000635235 |  |
| plan-sql-s8_case_mixed_types/nata/1 | 0.000161265 | 0.000163385 | 0.000166285 | MET |
| plan-sql-s8_case_mixed_types/twin/1 | 0.000154375 | 0.000161130 | 0.000170445 |  |
| plan-sql-s8_missing_file/duckdb/1 | 0.000275340 | 0.000283770 | 0.000289440 |  |
| plan-sql-s8_missing_file/nata/1 | 0.000088120 | 0.000090010 | 0.000176465 | MET |
| plan-sql-s8_missing_file/twin/1 | 0.000085170 | 0.000090705 | 0.000093900 |  |
| plan-sql-s8_negative_limit/duckdb/1 | 0.000339365 | 0.000347950 | 0.000354145 |  |
| plan-sql-s8_negative_limit/nata/1 | 0.000078410 | 0.000080390 | 0.000083229 | MET |
| plan-sql-s8_negative_limit/twin/1 | 0.000079879 | 0.000080685 | 0.000087185 |  |
| plan-sql-s8_syntax/duckdb/1 | 0.000046725 | 0.000047300 | 0.000048460 |  |
| plan-sql-s8_syntax/nata/1 | 0.000045855 | 0.000048330 | 0.000049415 | MET |
| plan-sql-s8_syntax/twin/1 | 0.000045690 | 0.000047225 | 0.000048970 |  |
| plan-sql-s8_type_mismatch_bool_plus_int/duckdb/1 | 0.000445555 | 0.000460265 | 0.000478829 |  |
| plan-sql-s8_type_mismatch_bool_plus_int/nata/1 | 0.000145590 | 0.000149935 | 0.000155470 | MET |
| plan-sql-s8_type_mismatch_bool_plus_int/twin/1 | 0.000146070 | 0.000151995 | 0.000160745 |  |
| plan-sql-s8_unknown_column/duckdb/1 | 0.000314455 | 0.000329315 | 0.000342284 |  |
| plan-sql-s8_unknown_column/nata/1 | 0.000136020 | 0.000138515 | 0.000157925 | MET |
| plan-sql-s8_unknown_column/twin/1 | 0.000135120 | 0.000136640 | 0.000152305 |  |
| plan-sql-tpch_between_dates/duckdb/1 | 0.002132469 | 0.002194099 | 0.002221504 |  |
| plan-sql-tpch_between_dates/nata/1 | 0.000385289 | 0.000400615 | 0.000442514 | MET |
| plan-sql-tpch_between_dates/twin/1 | 0.000379080 | 0.000397044 | 0.000427515 |  |
| plan-sql-tpch_case/duckdb/1 | 0.002013314 | 0.002124460 | 0.002143194 |  |
| plan-sql-tpch_case/nata/1 | 0.000423370 | 0.000453160 | 0.000455460 | MET |
| plan-sql-tpch_case/twin/1 | 0.000428395 | 0.000438435 | 0.000451909 |  |
| plan-sql-tpch_dates_strings/duckdb/1 | 0.001805089 | 0.001848594 | 0.002002264 |  |
| plan-sql-tpch_dates_strings/nata/1 | 0.000402714 | 0.000413440 | 0.000460845 | MET |
| plan-sql-tpch_dates_strings/twin/1 | 0.000407975 | 0.000415380 | 0.000424605 |  |
| plan-sql-tpch_example/duckdb/1 | 0.002117379 | 0.002173269 | 0.002292654 |  |
| plan-sql-tpch_example/nata/1 | 0.000393755 | 0.000403785 | 0.000452250 | MET |
| plan-sql-tpch_example/twin/1 | 0.000390284 | 0.000404039 | 0.000432575 |  |
| plan-sql-tpch_lineitem_all_columns_limit/duckdb/1 | 0.002198544 | 0.002286409 | 0.002385224 |  |
| plan-sql-tpch_lineitem_all_columns_limit/nata/1 | 0.000383429 | 0.000403670 | 0.000420045 | MET |
| plan-sql-tpch_lineitem_all_columns_limit/twin/1 | 0.000378605 | 0.000404610 | 0.000482615 |  |
| plan-sql-tpch_nation_region/duckdb/1 | 0.000634599 | 0.000644574 | 0.000685405 |  |
| plan-sql-tpch_nation_region/nata/1 | 0.000139845 | 0.000147195 | 0.000153095 | MET |
| plan-sql-tpch_nation_region/twin/1 | 0.000143885 | 0.000149280 | 0.000151475 |  |
| plan-sql-tpch_orders_filter/duckdb/1 | 0.000915124 | 0.000976345 | 0.000984770 |  |
| plan-sql-tpch_orders_filter/nata/1 | 0.000195765 | 0.000205930 | 0.000207950 | MET |
| plan-sql-tpch_orders_filter/twin/1 | 0.000189635 | 0.000212935 | 0.000219305 |  |
| plan-sql-tpch_q1_rows/duckdb/1 | 0.001979809 | 0.002050679 | 0.002202184 |  |
| plan-sql-tpch_q1_rows/nata/1 | 0.000388305 | 0.000406229 | 0.000430105 | MET |
| plan-sql-tpch_q1_rows/twin/1 | 0.000396490 | 0.000399205 | 0.000436259 |  |
| plan-sql-tpch_q6_rows/duckdb/1 | 0.002056434 | 0.002221504 | 0.002254439 |  |
| plan-sql-tpch_q6_rows/nata/1 | 0.000400899 | 0.000409600 | 0.000433675 | MET |
| plan-sql-tpch_q6_rows/twin/1 | 0.000392210 | 0.000403225 | 0.000452895 |  |
| resource-sf10/scan/8 | 2.622433632 | 2.622433632 | 2.622433632 |  |
| resource-sf10/sql/8 | 2.695569253 | 2.695569253 | 2.695569253 |  |
| scale-rendered-sf10/sql/1 | 2.456472240 | 2.650251994 | 3.040536803 |  |
| scale-rendered-sf10/sql/8 | 2.664000803 | 2.703043112 | 2.751737187 |  |
| scale-sf10/sql/1 | 0.367706024 | 0.375028890 | 0.381195346 |  |
| scale-sf10/sql/8 | 0.085059700 | 0.096226971 | 0.098927529 |  |
| scale-sf10/twin/1 | 0.364453036 | 0.378028789 | 0.389567059 |  |
| scale-sf10/twin/8 | 0.090799911 | 0.094618898 | 0.098957010 |  |
| sf1/duckdb/1 | 0.048051678 | 0.050345058 | 0.052557407 |  |
| sf1/duckdb/8 | 0.060863815 | 0.066032312 | 0.067980303 |  |
| sf1/scan/1 | 0.010764168 | 0.011069987 | 0.011460997 |  |
| sf1/scan/8 | 0.019225846 | 0.019955845 | 0.020209094 |  |
| sf1/sql/1 | 0.011665728 | 0.012503307 | 0.012738856 |  |
| sf1/sql/8 | 0.020358746 | 0.020887835 | 0.021598094 |  |
| sf1/twin/1 | 0.010607017 | 0.011125448 | 0.011323817 |  |
| sf1/twin/8 | 0.020176655 | 0.020701055 | 0.022146575 |  |
| sf10/duckdb/1 | 0.063636194 | 0.066303214 | 0.068061803 |  |
| sf10/duckdb/8 | 0.075796441 | 0.081588280 | 0.083510000 |  |
| sf10/scan/1 | 0.014868496 | 0.015467936 | 0.015896766 |  |
| sf10/scan/8 | 0.022085014 | 0.022595485 | 0.024017705 |  |
| sf10/sql/1 | 0.019844255 | 0.020068055 | 0.020768965 |  |
| sf10/sql/8 | 0.026949153 | 0.027371343 | 0.029489282 |  |
| sf10/twin/1 | 0.015054166 | 0.015395467 | 0.015731447 |  |
| sf10/twin/8 | 0.021364395 | 0.022628164 | 0.023639065 |  |
| sorted-r2/sql/8 | 0.018817571 | 0.018817571 | 0.018817571 |  |
| startup/duckdb/1 | 0.041214471 | 0.042124491 | 0.043183961 |  |
| startup/nata/1 | 0.001785110 | 0.001991190 | 0.002161650 |  |

sf1 threads 1: SQL/scan 1.1295 (threshold <=1.2): MET; twin/scan 1.0050.

sf1 threads 8: SQL/scan 1.0467 (threshold <=1.2): MET; twin/scan 1.0373.

sf10 threads 1: SQL/scan 1.2974 (threshold <=1.2): NOT MET; twin/scan 0.9953.

sf10 threads 8: SQL/scan 1.2114 (threshold <=1.2): NOT MET; twin/scan 1.0014.

SF10 full SQL speedup 3.8973 (threshold >=5): NOT MET. EXPLAIN ANALYZE executes every row without rendering; rendered SQL is also measured.

files10000: {'block': 'files10000', 'arm': 'sql', 'threads': 8, 'round': 0, 'secs': 0.14072789400006513, 'peak_rss_kib': '12404', 'peak_open_files': '8', 'peak_footers': '12'}

resource-sf10: {'block': 'resource-sf10', 'arm': 'sql', 'threads': 8, 'round': 0, 'secs': 2.695569252999803, 'peak_rss_kib': '114964', 'peak_open_files': '8', 'peak_footers': '1'}
