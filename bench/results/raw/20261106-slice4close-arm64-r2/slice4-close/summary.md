# Slice 4 SQL benchmark

Settled all blocks: True. Planning corpus: 289 queries (including rejection paths).
Warm OS cache; process timing includes startup, metadata, output. Nata planning includes parser thread startup and footer lookup; DuckDB EXPLAIN in-process includes plan rendering. Both example arms stop at 10 and render CSV; SQL adds the net expression. Startup probes are separate, not subtracted.

| metric | min s | median s | max s | verdict |
|---|---:|---:|---:|---|
| files10000/scan/8 | 0.068897816 | 0.068897816 | 0.068897816 |  |
| files10000/sql/8 | 0.096404792 | 0.096404792 | 0.096404792 |  |
| plan-card-example/duckdb/1 | 0.002665376 | 0.002733509 | 0.002785751 |  |
| plan-card-example/nata/1 | 0.000341418 | 0.000357247 | 0.000425765 | MET |
| plan-card-example/twin/1 | 0.000344293 | 0.000390119 | 0.000409595 |  |
| plan-sql-differences-d3_decimal_literal_below_one_is_decimal_p_p/duckdb/1 | 0.000644774 | 0.000662880 | 0.000669074 |  |
| plan-sql-differences-d3_decimal_literal_below_one_is_decimal_p_p/nata/1 | 0.000079948 | 0.000081838 | 0.000084533 | MET |
| plan-sql-differences-d3_decimal_literal_below_one_is_decimal_p_p/twin/1 | 0.000085823 | 0.000087204 | 0.000089853 |  |
| plan-sql-differences-d9_case_decimals_beyond_38/duckdb/1 | 0.000748787 | 0.000755949 | 0.000781749 |  |
| plan-sql-differences-d9_case_decimals_beyond_38/nata/1 | 0.000084313 | 0.000085899 | 0.000090233 | MET |
| plan-sql-differences-d9_case_decimals_beyond_38/twin/1 | 0.000083113 | 0.000091443 | 0.000106969 |  |
| plan-sql-differences-d9_cast_boolean_to_integer/duckdb/1 | 0.000626163 | 0.000645834 | 0.000649899 |  |
| plan-sql-differences-d9_cast_boolean_to_integer/nata/1 | 0.000073017 | 0.000073848 | 0.000080743 | MET |
| plan-sql-differences-d9_cast_boolean_to_integer/twin/1 | 0.000076768 | 0.000082063 | 0.000098649 |  |
| plan-sql-differences-d9_cast_double_to_varchar/duckdb/1 | 0.000639419 | 0.000647859 | 0.000663659 |  |
| plan-sql-differences-d9_cast_double_to_varchar/nata/1 | 0.000073823 | 0.000074693 | 0.000078743 | MET |
| plan-sql-differences-d9_cast_double_to_varchar/twin/1 | 0.000074723 | 0.000077108 | 0.000081954 |  |
| plan-sql-differences-d9_cast_short_date/duckdb/1 | 0.000562371 | 0.000564226 | 0.000576821 |  |
| plan-sql-differences-d9_cast_short_date/nata/1 | 0.000074647 | 0.000075983 | 0.000078192 | MET |
| plan-sql-differences-d9_cast_short_date/twin/1 | 0.000074093 | 0.000076573 | 0.000081208 |  |
| plan-sql-differences-d9_cast_slash_date/duckdb/1 | 0.000577426 | 0.000582556 | 0.000590727 |  |
| plan-sql-differences-d9_cast_slash_date/nata/1 | 0.000075493 | 0.000078128 | 0.000080843 | MET |
| plan-sql-differences-d9_cast_slash_date/twin/1 | 0.000074578 | 0.000077778 | 0.000098953 |  |
| plan-sql-differences-d9_cast_string_with_exponent_to_integer/duckdb/1 | 0.000567666 | 0.000576371 | 0.000587857 |  |
| plan-sql-differences-d9_cast_string_with_exponent_to_integer/nata/1 | 0.000074828 | 0.000077873 | 0.000110244 | MET |
| plan-sql-differences-d9_cast_string_with_exponent_to_integer/twin/1 | 0.000075658 | 0.000078767 | 0.000079853 |  |
| plan-sql-differences-d9_cast_string_with_fraction_to_integer/duckdb/1 | 0.000578507 | 0.000593766 | 0.000614108 |  |
| plan-sql-differences-d9_cast_string_with_fraction_to_integer/nata/1 | 0.000074472 | 0.000076698 | 0.000081033 | MET |
| plan-sql-differences-d9_cast_string_with_fraction_to_integer/twin/1 | 0.000076548 | 0.000078843 | 0.000095043 |  |
| plan-sql-differences-d9_cast_string_with_underscore/duckdb/1 | 0.000548455 | 0.000580696 | 0.000587867 |  |
| plan-sql-differences-d9_cast_string_with_underscore/nata/1 | 0.000074737 | 0.000075168 | 0.000076023 | MET |
| plan-sql-differences-d9_cast_string_with_underscore/twin/1 | 0.000074893 | 0.000079388 | 0.000091123 |  |
| plan-sql-differences-d9_cast_t_to_boolean/duckdb/1 | 0.000560050 | 0.000567381 | 0.000579832 |  |
| plan-sql-differences-d9_cast_t_to_boolean/nata/1 | 0.000073547 | 0.000075003 | 0.000075883 | MET |
| plan-sql-differences-d9_cast_t_to_boolean/twin/1 | 0.000075258 | 0.000077128 | 0.000082023 |  |
| plan-sql-differences-d9_cast_yes_to_boolean/duckdb/1 | 0.000558671 | 0.000572471 | 0.000579831 |  |
| plan-sql-differences-d9_cast_yes_to_boolean/nata/1 | 0.000073057 | 0.000075922 | 0.000076968 | MET |
| plan-sql-differences-d9_cast_yes_to_boolean/twin/1 | 0.000073098 | 0.000075933 | 0.000080738 |  |
| plan-sql-differences-d9_date_minus_date/duckdb/1 | 0.000613203 | 0.000622448 | 0.000635114 |  |
| plan-sql-differences-d9_date_minus_date/nata/1 | 0.000072358 | 0.000072908 | 0.000074428 | MET |
| plan-sql-differences-d9_date_minus_date/twin/1 | 0.000073958 | 0.000083538 | 0.000085313 |  |
| plan-sql-differences-d9_date_plus_integer/duckdb/1 | 0.000621528 | 0.000670820 | 0.000706271 |  |
| plan-sql-differences-d9_date_plus_integer/nata/1 | 0.000073182 | 0.000076333 | 0.000078318 | MET |
| plan-sql-differences-d9_date_plus_integer/twin/1 | 0.000073568 | 0.000076283 | 0.000078003 |  |
| plan-sql-differences-d9_dec_mod_tight_result_type/duckdb/1 | 0.001152277 | 0.001187889 | 0.001196424 |  |
| plan-sql-differences-d9_dec_mod_tight_result_type/nata/1 | 0.000085178 | 0.000086143 | 0.000090513 | MET |
| plan-sql-differences-d9_dec_mod_tight_result_type/twin/1 | 0.000088308 | 0.000092804 | 0.000103088 |  |
| plan-sql-differences-d9_decimal_compared_with_double/duckdb/1 | 0.000765063 | 0.000795845 | 0.000810364 |  |
| plan-sql-differences-d9_decimal_compared_with_double/nata/1 | 0.000072158 | 0.000073122 | 0.000080778 | MET |
| plan-sql-differences-d9_decimal_compared_with_double/twin/1 | 0.000073257 | 0.000075168 | 0.000077978 |  |
| plan-sql-differences-d9_decimal_division_too_wide/duckdb/1 | 0.000740008 | 0.000753723 | 0.000761668 |  |
| plan-sql-differences-d9_decimal_division_too_wide/nata/1 | 0.000074277 | 0.000076863 | 0.000083923 | MET |
| plan-sql-differences-d9_decimal_division_too_wide/twin/1 | 0.000072272 | 0.000075578 | 0.000117435 |  |
| plan-sql-differences-d9_decimal_mul_scale_too_large/duckdb/1 | 0.000527890 | 0.000531709 | 0.000540819 |  |
| plan-sql-differences-d9_decimal_mul_scale_too_large/nata/1 | 0.000078958 | 0.000080298 | 0.000082968 | MET |
| plan-sql-differences-d9_decimal_mul_scale_too_large/twin/1 | 0.000079603 | 0.000081593 | 0.000083658 |  |
| plan-sql-differences-d9_decimal_plus_double/duckdb/1 | 0.000607277 | 0.000618722 | 0.000717841 |  |
| plan-sql-differences-d9_decimal_plus_double/nata/1 | 0.000072553 | 0.000073948 | 0.000074828 | MET |
| plan-sql-differences-d9_decimal_plus_double/twin/1 | 0.000073518 | 0.000074152 | 0.000087293 |  |
| plan-sql-differences-d9_divide_decimals/duckdb/1 | 0.000953829 | 0.000972416 | 0.001037518 |  |
| plan-sql-differences-d9_divide_decimals/nata/1 | 0.000083963 | 0.000084899 | 0.000090578 | MET |
| plan-sql-differences-d9_divide_decimals/twin/1 | 0.000085744 | 0.000087473 | 0.000093733 |  |
| plan-sql-differences-d9_divide_integers/duckdb/1 | 0.001022457 | 0.001045453 | 0.001105680 |  |
| plan-sql-differences-d9_divide_integers/nata/1 | 0.000086138 | 0.000088708 | 0.000089878 | MET |
| plan-sql-differences-d9_divide_integers/twin/1 | 0.000092358 | 0.000096478 | 0.000116865 |  |
| plan-sql-differences-d9_division_by_zero_is_an_error/duckdb/1 | 0.000647028 | 0.000649304 | 0.000658894 |  |
| plan-sql-differences-d9_division_by_zero_is_an_error/nata/1 | 0.000073878 | 0.000076183 | 0.000079238 | MET |
| plan-sql-differences-d9_division_by_zero_is_an_error/twin/1 | 0.000075878 | 0.000079812 | 0.000083383 |  |
| plan-sql-differences-d9_double_to_decimal_exact_binary_value/duckdb/1 | 0.000671359 | 0.000676234 | 0.000713101 |  |
| plan-sql-differences-d9_double_to_decimal_exact_binary_value/nata/1 | 0.000080458 | 0.000081263 | 0.000083118 | MET |
| plan-sql-differences-d9_double_to_decimal_exact_binary_value/twin/1 | 0.000079148 | 0.000083503 | 0.000086708 |  |
| plan-sql-differences-d9_float_arithmetic/duckdb/1 | 0.000597777 | 0.000619573 | 0.000636903 |  |
| plan-sql-differences-d9_float_arithmetic/nata/1 | 0.000071513 | 0.000072153 | 0.000073703 | MET |
| plan-sql-differences-d9_float_arithmetic/twin/1 | 0.000072683 | 0.000073938 | 0.000083618 |  |
| plan-sql-differences-d9_float_compared_with_integer/duckdb/1 | 0.000713707 | 0.000734777 | 0.000738757 |  |
| plan-sql-differences-d9_float_compared_with_integer/nata/1 | 0.000073468 | 0.000075442 | 0.000079718 | MET |
| plan-sql-differences-d9_float_compared_with_integer/twin/1 | 0.000075573 | 0.000077093 | 0.000093539 |  |
| plan-sql-differences-d9_float_literal_is_exact_decimal/duckdb/1 | 0.000703816 | 0.000720036 | 0.000728441 |  |
| plan-sql-differences-d9_float_literal_is_exact_decimal/nata/1 | 0.000080263 | 0.000082473 | 0.000086633 | MET |
| plan-sql-differences-d9_float_literal_is_exact_decimal/twin/1 | 0.000084438 | 0.000091328 | 0.000101678 |  |
| plan-sql-differences-d9_in_list_with_a_cast_of_a_literal/duckdb/1 | 0.000753373 | 0.000763048 | 0.000779779 |  |
| plan-sql-differences-d9_in_list_with_a_cast_of_a_literal/nata/1 | 0.000076528 | 0.000077448 | 0.000079408 | MET |
| plan-sql-differences-d9_in_list_with_a_cast_of_a_literal/twin/1 | 0.000079133 | 0.000080218 | 0.000087843 |  |
| plan-sql-differences-d9_in_list_with_an_expression/duckdb/1 | 0.000802650 | 0.000812809 | 0.000820229 |  |
| plan-sql-differences-d9_in_list_with_an_expression/nata/1 | 0.000073193 | 0.000074748 | 0.000081253 | MET |
| plan-sql-differences-d9_in_list_with_an_expression/twin/1 | 0.000075498 | 0.000077553 | 0.000098163 |  |
| plan-sql-differences-d9_integer_literal_does_not_narrow_to_the_column/duckdb/1 | 0.000800784 | 0.000860316 | 0.000903358 |  |
| plan-sql-differences-d9_integer_literal_does_not_narrow_to_the_column/nata/1 | 0.000086958 | 0.000087863 | 0.000094234 | MET |
| plan-sql-differences-d9_integer_literal_does_not_narrow_to_the_column/twin/1 | 0.000086269 | 0.000091768 | 0.000101843 |  |
| plan-sql-differences-d9_quoted_identifier_is_case_sensitive/duckdb/1 | 0.000529594 | 0.000532415 | 0.000534834 |  |
| plan-sql-differences-d9_quoted_identifier_is_case_sensitive/nata/1 | 0.000070332 | 0.000071207 | 0.000073568 | MET |
| plan-sql-differences-d9_quoted_identifier_is_case_sensitive/twin/1 | 0.000070153 | 0.000071192 | 0.000086823 |  |
| plan-sql-differences-d9_string_compared_with_date/duckdb/1 | 0.000700535 | 0.000760832 | 0.000805454 |  |
| plan-sql-differences-d9_string_compared_with_date/nata/1 | 0.000073382 | 0.000074942 | 0.000076727 | MET |
| plan-sql-differences-d9_string_compared_with_date/twin/1 | 0.000075028 | 0.000078653 | 0.000081938 |  |
| plan-sql-differences-d9_string_compared_with_number_column/duckdb/1 | 0.000675834 | 0.000688070 | 0.000696975 |  |
| plan-sql-differences-d9_string_compared_with_number_column/nata/1 | 0.000073812 | 0.000077392 | 0.000080252 | MET |
| plan-sql-differences-d9_string_compared_with_number_column/twin/1 | 0.000074028 | 0.000074868 | 0.000082162 |  |
| plan-sql-differences-d9_string_equals_number/duckdb/1 | 0.000619542 | 0.000685274 | 0.000701081 |  |
| plan-sql-differences-d9_string_equals_number/nata/1 | 0.000072167 | 0.000075197 | 0.000077147 | MET |
| plan-sql-differences-d9_string_equals_number/twin/1 | 0.000074752 | 0.000077108 | 0.000082388 |  |
| plan-sql-differences-d9_where_integer_is_not_boolean/duckdb/1 | 0.000650378 | 0.000691455 | 0.000707936 |  |
| plan-sql-differences-d9_where_integer_is_not_boolean/nata/1 | 0.000071423 | 0.000072553 | 0.000074543 | MET |
| plan-sql-differences-d9_where_integer_is_not_boolean/twin/1 | 0.000072363 | 0.000073758 | 0.000076298 |  |
| plan-sql-differences-d9b_a_case_condition_is_evaluated_only_for_the_rows_that_reach_it/duckdb/1 | 0.000733137 | 0.000757353 | 0.000772853 |  |
| plan-sql-differences-d9b_a_case_condition_is_evaluated_only_for_the_rows_that_reach_it/nata/1 | 0.000095159 | 0.000099899 | 0.000102314 | MET |
| plan-sql-differences-d9b_a_case_condition_is_evaluated_only_for_the_rows_that_reach_it/twin/1 | 0.000096884 | 0.000104719 | 0.000119859 |  |
| plan-sql-differences-d9b_an_operand_next_to_a_null_is_still_evaluated/duckdb/1 | 0.000840750 | 0.000865847 | 0.000881757 |  |
| plan-sql-differences-d9b_an_operand_next_to_a_null_is_still_evaluated/nata/1 | 0.000084088 | 0.000087488 | 0.000091308 | MET |
| plan-sql-differences-d9b_an_operand_next_to_a_null_is_still_evaluated/twin/1 | 0.000082928 | 0.000088333 | 0.000120985 |  |
| plan-sql-differences-d9b_and_skips_the_second_operand_for_decided_rows/duckdb/1 | 0.000897007 | 0.000913513 | 0.000946689 |  |
| plan-sql-differences-d9b_and_skips_the_second_operand_for_decided_rows/nata/1 | 0.000091549 | 0.000092394 | 0.000095869 | MET |
| plan-sql-differences-d9b_and_skips_the_second_operand_for_decided_rows/twin/1 | 0.000095298 | 0.000108704 | 0.000133395 |  |
| plan-sql-differences-d9b_or_skips_the_second_operand_for_decided_rows/duckdb/1 | 0.000904163 | 0.000925878 | 0.000944939 |  |
| plan-sql-differences-d9b_or_skips_the_second_operand_for_decided_rows/nata/1 | 0.000090113 | 0.000091983 | 0.000092869 | MET |
| plan-sql-differences-d9b_or_skips_the_second_operand_for_decided_rows/twin/1 | 0.000094538 | 0.000098629 | 0.000102104 |  |
| plan-sql-edge_all_null_cmp_everything/duckdb/1 | 0.000864665 | 0.000908152 | 0.000952930 |  |
| plan-sql-edge_all_null_cmp_everything/nata/1 | 0.000098295 | 0.000102310 | 0.000117411 | MET |
| plan-sql-edge_all_null_cmp_everything/twin/1 | 0.000098089 | 0.000115235 | 0.000140871 |  |
| plan-sql-edge_all_null_expressions/duckdb/1 | 0.000934729 | 0.000953480 | 0.001012717 |  |
| plan-sql-edge_all_null_expressions/nata/1 | 0.000097839 | 0.000100360 | 0.000103385 | MET |
| plan-sql-edge_all_null_expressions/twin/1 | 0.000103594 | 0.000107585 | 0.000132161 |  |
| plan-sql-edge_all_null_filter_all/duckdb/1 | 0.000793222 | 0.000819989 | 0.000840424 |  |
| plan-sql-edge_all_null_filter_all/nata/1 | 0.000081849 | 0.000082604 | 0.000084199 | MET |
| plan-sql-edge_all_null_filter_all/twin/1 | 0.000084449 | 0.000086840 | 0.000109640 |  |
| plan-sql-edge_all_null_filter_none/duckdb/1 | 0.000674146 | 0.000696237 | 0.000705252 |  |
| plan-sql-edge_all_null_filter_none/nata/1 | 0.000078399 | 0.000080079 | 0.000083934 | MET |
| plan-sql-edge_all_null_filter_none/twin/1 | 0.000076324 | 0.000078664 | 0.000099595 |  |
| plan-sql-edge_all_null_limit_offset/duckdb/1 | 0.000899191 | 0.000940044 | 0.000972915 |  |
| plan-sql-edge_all_null_limit_offset/nata/1 | 0.000071914 | 0.000074254 | 0.000075053 | MET |
| plan-sql-edge_all_null_limit_offset/twin/1 | 0.000072534 | 0.000077764 | 0.000085069 |  |
| plan-sql-edge_all_null_star/duckdb/1 | 0.000724214 | 0.000753400 | 0.000754986 |  |
| plan-sql-edge_all_null_star/nata/1 | 0.000072764 | 0.000074209 | 0.000075279 | MET |
| plan-sql-edge_all_null_star/twin/1 | 0.000073978 | 0.000076878 | 0.000088169 |  |
| plan-sql-edge_duplicate_rows_kept/duckdb/1 | 0.000659250 | 0.000668546 | 0.000687407 |  |
| plan-sql-edge_duplicate_rows_kept/nata/1 | 0.000078673 | 0.000079069 | 0.000086489 | MET |
| plan-sql-edge_duplicate_rows_kept/twin/1 | 0.000075899 | 0.000079539 | 0.000080449 |  |
| plan-sql-edge_dups_cmp_everything/duckdb/1 | 0.000945183 | 0.000986286 | 0.001046544 |  |
| plan-sql-edge_dups_cmp_everything/nata/1 | 0.000104495 | 0.000105805 | 0.000110695 | MET |
| plan-sql-edge_dups_cmp_everything/twin/1 | 0.000104805 | 0.000107700 | 0.000128896 |  |
| plan-sql-edge_dups_expressions/duckdb/1 | 0.001006981 | 0.001049244 | 0.001070565 |  |
| plan-sql-edge_dups_expressions/nata/1 | 0.000105120 | 0.000105295 | 0.000110155 | MET |
| plan-sql-edge_dups_expressions/twin/1 | 0.000112550 | 0.000126931 | 0.000169502 |  |
| plan-sql-edge_dups_filter_all/duckdb/1 | 0.000854560 | 0.000883675 | 0.000906901 |  |
| plan-sql-edge_dups_filter_all/nata/1 | 0.000088600 | 0.000089609 | 0.000090344 | MET |
| plan-sql-edge_dups_filter_all/twin/1 | 0.000094129 | 0.000098945 | 0.000124801 |  |
| plan-sql-edge_dups_filter_none/duckdb/1 | 0.000715578 | 0.000723763 | 0.000752865 |  |
| plan-sql-edge_dups_filter_none/nata/1 | 0.000082859 | 0.000084524 | 0.000092055 | MET |
| plan-sql-edge_dups_filter_none/twin/1 | 0.000082384 | 0.000084193 | 0.000095469 |  |
| plan-sql-edge_dups_limit_offset/duckdb/1 | 0.000945389 | 0.000970525 | 0.000979571 |  |
| plan-sql-edge_dups_limit_offset/nata/1 | 0.000079093 | 0.000079889 | 0.000080864 | MET |
| plan-sql-edge_dups_limit_offset/twin/1 | 0.000080374 | 0.000082168 | 0.000086694 |  |
| plan-sql-edge_dups_star/duckdb/1 | 0.000778726 | 0.000792922 | 0.000813517 |  |
| plan-sql-edge_dups_star/nata/1 | 0.000078099 | 0.000080044 | 0.000083429 | MET |
| plan-sql-edge_dups_star/twin/1 | 0.000078348 | 0.000081068 | 0.000092904 |  |
| plan-sql-edge_empty_cmp_everything/duckdb/1 | 0.000842259 | 0.000844854 | 0.000902747 |  |
| plan-sql-edge_empty_cmp_everything/nata/1 | 0.000094015 | 0.000094694 | 0.000100444 | MET |
| plan-sql-edge_empty_cmp_everything/twin/1 | 0.000096209 | 0.000097035 | 0.000106115 |  |
| plan-sql-edge_empty_expressions/duckdb/1 | 0.000926743 | 0.000962920 | 0.000978425 |  |
| plan-sql-edge_empty_expressions/nata/1 | 0.000092639 | 0.000095099 | 0.000099705 | MET |
| plan-sql-edge_empty_expressions/twin/1 | 0.000098015 | 0.000100615 | 0.000125481 |  |
| plan-sql-edge_empty_filter_all/duckdb/1 | 0.000820057 | 0.000833059 | 0.000840959 |  |
| plan-sql-edge_empty_filter_all/nata/1 | 0.000080469 | 0.000082244 | 0.000082659 | MET |
| plan-sql-edge_empty_filter_all/twin/1 | 0.000080728 | 0.000083899 | 0.000095250 |  |
| plan-sql-edge_empty_filter_none/duckdb/1 | 0.000677881 | 0.000745175 | 0.000759174 |  |
| plan-sql-edge_empty_filter_none/nata/1 | 0.000073179 | 0.000074943 | 0.000076009 | MET |
| plan-sql-edge_empty_filter_none/twin/1 | 0.000073688 | 0.000078094 | 0.000083254 |  |
| plan-sql-edge_empty_limit_offset/duckdb/1 | 0.000892091 | 0.000914573 | 0.000972795 |  |
| plan-sql-edge_empty_limit_offset/nata/1 | 0.000068723 | 0.000070463 | 0.000072018 | MET |
| plan-sql-edge_empty_limit_offset/twin/1 | 0.000071843 | 0.000074818 | 0.000105109 |  |
| plan-sql-edge_empty_star/duckdb/1 | 0.000664435 | 0.000666776 | 0.000695962 |  |
| plan-sql-edge_empty_star/nata/1 | 0.000068063 | 0.000070749 | 0.000083334 | MET |
| plan-sql-edge_empty_star/twin/1 | 0.000071278 | 0.000072718 | 0.000092234 |  |
| plan-sql-edge_filter_false_literal/duckdb/1 | 0.000464136 | 0.000475447 | 0.000480967 |  |
| plan-sql-edge_filter_false_literal/nata/1 | 0.000072428 | 0.000073784 | 0.000076044 | MET |
| plan-sql-edge_filter_false_literal/twin/1 | 0.000073088 | 0.000077339 | 0.000088034 |  |
| plan-sql-edge_filter_keeps_everything_pushed/duckdb/1 | 0.000649955 | 0.000672776 | 0.000688417 |  |
| plan-sql-edge_filter_keeps_everything_pushed/nata/1 | 0.000073949 | 0.000075703 | 0.000076844 | MET |
| plan-sql-edge_filter_keeps_everything_pushed/twin/1 | 0.000076173 | 0.000077133 | 0.000088044 |  |
| plan-sql-edge_filter_keeps_everything_remaining/duckdb/1 | 0.000704088 | 0.000735049 | 0.000795932 |  |
| plan-sql-edge_filter_keeps_everything_remaining/nata/1 | 0.000077319 | 0.000077803 | 0.000083313 | MET |
| plan-sql-edge_filter_keeps_everything_remaining/twin/1 | 0.000078118 | 0.000079414 | 0.000083389 |  |
| plan-sql-edge_filter_keeps_nothing_pushed/duckdb/1 | 0.000736624 | 0.000745579 | 0.000775400 |  |
| plan-sql-edge_filter_keeps_nothing_pushed/nata/1 | 0.000074909 | 0.000075298 | 0.000078533 | MET |
| plan-sql-edge_filter_keeps_nothing_pushed/twin/1 | 0.000076608 | 0.000079779 | 0.000090369 |  |
| plan-sql-edge_filter_keeps_nothing_remaining/duckdb/1 | 0.000697837 | 0.000715558 | 0.000728353 |  |
| plan-sql-edge_filter_keeps_nothing_remaining/nata/1 | 0.000082169 | 0.000082834 | 0.000085399 | MET |
| plan-sql-edge_filter_keeps_nothing_remaining/twin/1 | 0.000080829 | 0.000085154 | 0.000091404 |  |
| plan-sql-edge_filter_null_literal/duckdb/1 | 0.000555311 | 0.000584242 | 0.000614323 |  |
| plan-sql-edge_filter_null_literal/nata/1 | 0.000075863 | 0.000079049 | 0.000092274 | MET |
| plan-sql-edge_filter_null_literal/twin/1 | 0.000077944 | 0.000079949 | 0.000091609 |  |
| plan-sql-edge_filter_true_literal/duckdb/1 | 0.000562546 | 0.000573096 | 0.000591762 |  |
| plan-sql-edge_filter_true_literal/nata/1 | 0.000072529 | 0.000074363 | 0.000076364 | MET |
| plan-sql-edge_filter_true_literal/twin/1 | 0.000074573 | 0.000076018 | 0.000079318 |  |
| plan-sql-edge_one_row_cmp_everything/duckdb/1 | 0.000884570 | 0.000932218 | 0.000953843 |  |
| plan-sql-edge_one_row_cmp_everything/nata/1 | 0.000097069 | 0.000100769 | 0.000103685 | MET |
| plan-sql-edge_one_row_cmp_everything/twin/1 | 0.000098995 | 0.000104005 | 0.000108210 |  |
| plan-sql-edge_one_row_expressions/duckdb/1 | 0.000983725 | 0.001008986 | 0.001041668 |  |
| plan-sql-edge_one_row_expressions/nata/1 | 0.000099070 | 0.000101739 | 0.000107730 | MET |
| plan-sql-edge_one_row_expressions/twin/1 | 0.000100924 | 0.000103794 | 0.000106229 |  |
| plan-sql-edge_one_row_filter_all/duckdb/1 | 0.000776731 | 0.000790236 | 0.000824868 |  |
| plan-sql-edge_one_row_filter_all/nata/1 | 0.000082409 | 0.000084969 | 0.000088934 | MET |
| plan-sql-edge_one_row_filter_all/twin/1 | 0.000086509 | 0.000089899 | 0.000106660 |  |
| plan-sql-edge_one_row_filter_none/duckdb/1 | 0.000673036 | 0.000709587 | 0.000755850 |  |
| plan-sql-edge_one_row_filter_none/nata/1 | 0.000077223 | 0.000078358 | 0.000081523 | MET |
| plan-sql-edge_one_row_filter_none/twin/1 | 0.000077953 | 0.000085014 | 0.000095235 |  |
| plan-sql-edge_one_row_limit_offset/duckdb/1 | 0.000886381 | 0.000923062 | 0.000940658 |  |
| plan-sql-edge_one_row_limit_offset/nata/1 | 0.000073073 | 0.000073529 | 0.000077083 | MET |
| plan-sql-edge_one_row_limit_offset/twin/1 | 0.000076089 | 0.000079369 | 0.000085359 |  |
| plan-sql-edge_one_row_star/duckdb/1 | 0.000702407 | 0.000742769 | 0.000763340 |  |
| plan-sql-edge_one_row_star/nata/1 | 0.000071633 | 0.000074558 | 0.000079489 | MET |
| plan-sql-edge_one_row_star/twin/1 | 0.000075368 | 0.000082919 | 0.000092854 |  |
| plan-sql-edge_pushed_and_remaining/duckdb/1 | 0.000980279 | 0.001013451 | 0.001032007 |  |
| plan-sql-edge_pushed_and_remaining/nata/1 | 0.000085194 | 0.000087634 | 0.000088689 | MET |
| plan-sql-edge_pushed_and_remaining/twin/1 | 0.000089214 | 0.000092229 | 0.000123170 |  |
| plan-sql-edge_pushed_bool/duckdb/1 | 0.000706217 | 0.000710082 | 0.000738239 |  |
| plan-sql-edge_pushed_bool/nata/1 | 0.000074298 | 0.000074598 | 0.000077729 | MET |
| plan-sql-edge_pushed_bool/twin/1 | 0.000077113 | 0.000078818 | 0.000084464 |  |
| plan-sql-edge_pushed_decimal_literal_scales/duckdb/1 | 0.000944672 | 0.000961054 | 0.000968734 |  |
| plan-sql-edge_pushed_decimal_literal_scales/nata/1 | 0.000082549 | 0.000083389 | 0.000086083 | MET |
| plan-sql-edge_pushed_decimal_literal_scales/twin/1 | 0.000085209 | 0.000088134 | 0.000088849 |  |
| plan-sql-edge_pushed_float/duckdb/1 | 0.000825397 | 0.000838943 | 0.000846388 |  |
| plan-sql-edge_pushed_float/nata/1 | 0.000077719 | 0.000079958 | 0.000084599 | MET |
| plan-sql-edge_pushed_float/twin/1 | 0.000079643 | 0.000083363 | 0.000083819 |  |
| plan-sql-edge_pushed_is_null/duckdb/1 | 0.000675795 | 0.000709377 | 0.000721203 |  |
| plan-sql-edge_pushed_is_null/nata/1 | 0.000076584 | 0.000078353 | 0.000081204 | MET |
| plan-sql-edge_pushed_is_null/twin/1 | 0.000081814 | 0.000086489 | 0.000090924 |  |
| plan-sql-edge_pushed_literal_on_left/duckdb/1 | 0.000802736 | 0.000843383 | 0.000851974 |  |
| plan-sql-edge_pushed_literal_on_left/nata/1 | 0.000080048 | 0.000082959 | 0.000085424 | MET |
| plan-sql-edge_pushed_literal_on_left/twin/1 | 0.000083159 | 0.000088469 | 0.000094839 |  |
| plan-sql-edge_pushed_string/duckdb/1 | 0.000890680 | 0.000904882 | 0.000925592 |  |
| plan-sql-edge_pushed_string/nata/1 | 0.000079548 | 0.000079738 | 0.000083249 | MET |
| plan-sql-edge_pushed_string/twin/1 | 0.000080184 | 0.000082169 | 0.000086729 |  |
| plan-sql-edge_pushed_string_and/duckdb/1 | 0.000802696 | 0.000824602 | 0.000825577 |  |
| plan-sql-edge_pushed_string_and/nata/1 | 0.000078779 | 0.000081473 | 0.000082623 | MET |
| plan-sql-edge_pushed_string_and/twin/1 | 0.000081333 | 0.000083639 | 0.000088274 |  |
| plan-sql-edge_select_literal_only/duckdb/1 | 0.000740594 | 0.000784265 | 0.000820977 |  |
| plan-sql-edge_select_literal_only/nata/1 | 0.000079124 | 0.000079939 | 0.000081773 | MET |
| plan-sql-edge_select_literal_only/twin/1 | 0.000080589 | 0.000082524 | 0.000091674 |  |
| plan-sql-edge_select_literal_only_empty/duckdb/1 | 0.000513953 | 0.000527203 | 0.000543979 |  |
| plan-sql-edge_select_literal_only_empty/nata/1 | 0.000067608 | 0.000069393 | 0.000074678 | MET |
| plan-sql-edge_select_literal_only_empty/twin/1 | 0.000068368 | 0.000069248 | 0.000084440 |  |
| plan-sql-s1_columns_in_order/duckdb/1 | 0.000594171 | 0.000599817 | 0.000650814 |  |
| plan-sql-s1_columns_in_order/nata/1 | 0.000072303 | 0.000074278 | 0.000075753 | MET |
| plan-sql-s1_columns_in_order/twin/1 | 0.000074729 | 0.000076983 | 0.000082584 |  |
| plan-sql-s1_comments_whitespace/duckdb/1 | 0.000710342 | 0.000756279 | 0.000772105 |  |
| plan-sql-s1_comments_whitespace/nata/1 | 0.000076304 | 0.000076909 | 0.000083609 | MET |
| plan-sql-s1_comments_whitespace/twin/1 | 0.000076803 | 0.000081849 | 0.000102615 |  |
| plan-sql-s1_glob/duckdb/1 | 0.000571261 | 0.000581831 | 0.000614487 |  |
| plan-sql-s1_glob/nata/1 | 0.000089604 | 0.000091044 | 0.000093009 | MET |
| plan-sql-s1_glob/twin/1 | 0.000092999 | 0.000094944 | 0.000105265 |  |
| plan-sql-s1_glob_limit/duckdb/1 | 0.000642149 | 0.000662605 | 0.000681990 |  |
| plan-sql-s1_glob_limit/nata/1 | 0.000088154 | 0.000089934 | 0.000091234 | MET |
| plan-sql-s1_glob_limit/twin/1 | 0.000091764 | 0.000093329 | 0.000106779 |  |
| plan-sql-s1_glob_limit_offset/duckdb/1 | 0.000965104 | 0.000972009 | 0.000994865 |  |
| plan-sql-s1_glob_limit_offset/nata/1 | 0.000091719 | 0.000092045 | 0.000096044 | MET |
| plan-sql-s1_glob_limit_offset/twin/1 | 0.000093224 | 0.000100709 | 0.000118921 |  |
| plan-sql-s1_glob_where/duckdb/1 | 0.000763909 | 0.000810612 | 0.000830647 |  |
| plan-sql-s1_glob_where/nata/1 | 0.000094089 | 0.000097429 | 0.000099900 | MET |
| plan-sql-s1_glob_where/twin/1 | 0.000099524 | 0.000114135 | 0.000135396 |  |
| plan-sql-s1_keywords_any_case/duckdb/1 | 0.000827427 | 0.000853268 | 0.000867364 |  |
| plan-sql-s1_keywords_any_case/nata/1 | 0.000079354 | 0.000081104 | 0.000082714 | MET |
| plan-sql-s1_keywords_any_case/twin/1 | 0.000084564 | 0.000089009 | 0.000095004 |  |
| plan-sql-s1_star/duckdb/1 | 0.000692286 | 0.000720357 | 0.000739323 |  |
| plan-sql-s1_star/nata/1 | 0.000071499 | 0.000073098 | 0.000075568 | MET |
| plan-sql-s1_star/twin/1 | 0.000072813 | 0.000074258 | 0.000081848 |  |
| plan-sql-s1_trailing_semicolon/duckdb/1 | 0.000556645 | 0.000578451 | 0.000593217 |  |
| plan-sql-s1_trailing_semicolon/nata/1 | 0.000069733 | 0.000071344 | 0.000072163 | MET |
| plan-sql-s1_trailing_semicolon/twin/1 | 0.000069008 | 0.000072054 | 0.000074988 |  |
| plan-sql-s1_where_limit_offset/duckdb/1 | 0.000800101 | 0.000811707 | 0.000832027 |  |
| plan-sql-s1_where_limit_offset/nata/1 | 0.000077899 | 0.000079083 | 0.000080969 | MET |
| plan-sql-s1_where_limit_offset/twin/1 | 0.000082184 | 0.000084809 | 0.000098730 |  |
| plan-sql-s2_alias/duckdb/1 | 0.000706296 | 0.000722373 | 0.000728112 |  |
| plan-sql-s2_alias/nata/1 | 0.000075503 | 0.000079418 | 0.000082724 | MET |
| plan-sql-s2_alias/twin/1 | 0.000079109 | 0.000081203 | 0.000086109 |  |
| plan-sql-s2_alias_quoted_case/duckdb/1 | 0.000609697 | 0.000641289 | 0.000653245 |  |
| plan-sql-s2_alias_quoted_case/nata/1 | 0.000074114 | 0.000074928 | 0.000076054 | MET |
| plan-sql-s2_alias_quoted_case/twin/1 | 0.000072798 | 0.000077743 | 0.000078519 |  |
| plan-sql-s2_duplicate_output_names/duckdb/1 | 0.000593487 | 0.000602078 | 0.000611198 |  |
| plan-sql-s2_duplicate_output_names/nata/1 | 0.000073894 | 0.000075174 | 0.000076949 | MET |
| plan-sql-s2_duplicate_output_names/twin/1 | 0.000074064 | 0.000075884 | 0.000077164 |  |
| plan-sql-s2_filter_column_not_projected/duckdb/1 | 0.000744659 | 0.000758829 | 0.000759434 |  |
| plan-sql-s2_filter_column_not_projected/nata/1 | 0.000075169 | 0.000076574 | 0.000077658 | MET |
| plan-sql-s2_filter_column_not_projected/twin/1 | 0.000075673 | 0.000078953 | 0.000081614 |  |
| plan-sql-s2_quoted_exact/duckdb/1 | 0.000544229 | 0.000567440 | 0.000603347 |  |
| plan-sql-s2_quoted_exact/nata/1 | 0.000071133 | 0.000072233 | 0.000075173 | MET |
| plan-sql-s2_quoted_exact/twin/1 | 0.000072583 | 0.000074873 | 0.000078444 |  |
| plan-sql-s2_same_column_twice/duckdb/1 | 0.000587377 | 0.000592872 | 0.000609967 |  |
| plan-sql-s2_same_column_twice/nata/1 | 0.000072848 | 0.000075448 | 0.000087489 | MET |
| plan-sql-s2_same_column_twice/twin/1 | 0.000072774 | 0.000074064 | 0.000074739 |  |
| plan-sql-s2_unquoted_folded/duckdb/1 | 0.000616277 | 0.000626848 | 0.000635043 |  |
| plan-sql-s2_unquoted_folded/nata/1 | 0.000074434 | 0.000077023 | 0.000079149 | MET |
| plan-sql-s2_unquoted_folded/twin/1 | 0.000074519 | 0.000076538 | 0.000084923 |  |
| plan-sql-s3_big_integer_literal/duckdb/1 | 0.000651384 | 0.000677045 | 0.000683780 |  |
| plan-sql-s3_big_integer_literal/nata/1 | 0.000073983 | 0.000076744 | 0.000080363 | MET |
| plan-sql-s3_big_integer_literal/twin/1 | 0.000077038 | 0.000085614 | 0.000092609 |  |
| plan-sql-s3_boolean_literals/duckdb/1 | 0.000615053 | 0.000643213 | 0.000657559 |  |
| plan-sql-s3_boolean_literals/nata/1 | 0.000072653 | 0.000075758 | 0.000082129 | MET |
| plan-sql-s3_boolean_literals/twin/1 | 0.000074788 | 0.000076354 | 0.000080528 |  |
| plan-sql-s3_date_literal/duckdb/1 | 0.000638528 | 0.000661199 | 0.000673710 |  |
| plan-sql-s3_date_literal/nata/1 | 0.000072458 | 0.000074379 | 0.000076194 | MET |
| plan-sql-s3_date_literal/twin/1 | 0.000073239 | 0.000086624 | 0.000091699 |  |
| plan-sql-s3_decimal_literals/duckdb/1 | 0.000631544 | 0.000646939 | 0.000659004 |  |
| plan-sql-s3_decimal_literals/nata/1 | 0.000079929 | 0.000080879 | 0.000083143 | MET |
| plan-sql-s3_decimal_literals/twin/1 | 0.000080294 | 0.000081753 | 0.000091459 |  |
| plan-sql-s3_impossible_date/duckdb/1 | 0.000648389 | 0.000669320 | 0.000700676 |  |
| plan-sql-s3_impossible_date/nata/1 | 0.000071108 | 0.000072539 | 0.000074124 | MET |
| plan-sql-s3_impossible_date/twin/1 | 0.000071248 | 0.000072193 | 0.000098214 |  |
| plan-sql-s3_integer_literals/duckdb/1 | 0.000675445 | 0.000683411 | 0.000696326 |  |
| plan-sql-s3_integer_literals/nata/1 | 0.000083019 | 0.000084739 | 0.000089289 | MET |
| plan-sql-s3_integer_literals/twin/1 | 0.000085189 | 0.000087174 | 0.000116400 |  |
| plan-sql-s3_null_in_case/duckdb/1 | 0.000737648 | 0.000756709 | 0.000769714 |  |
| plan-sql-s3_null_in_case/nata/1 | 0.000080358 | 0.000082413 | 0.000083114 | MET |
| plan-sql-s3_null_in_case/twin/1 | 0.000079929 | 0.000086944 | 0.000099429 |  |
| plan-sql-s3_null_typed_by_context/duckdb/1 | 0.000734717 | 0.000748324 | 0.000772864 |  |
| plan-sql-s3_null_typed_by_context/nata/1 | 0.000083174 | 0.000086054 | 0.000089039 | MET |
| plan-sql-s3_null_typed_by_context/twin/1 | 0.000084398 | 0.000087703 | 0.000092814 |  |
| plan-sql-s3_string_literals/duckdb/1 | 0.000624462 | 0.000661229 | 0.000688065 |  |
| plan-sql-s3_string_literals/nata/1 | 0.000077523 | 0.000080413 | 0.000083129 | MET |
| plan-sql-s3_string_literals/twin/1 | 0.000079039 | 0.000082568 | 0.000088714 |  |
| plan-sql-s4_bigint_mul_overflow/duckdb/1 | 0.000623448 | 0.000631333 | 0.000659020 |  |
| plan-sql-s4_bigint_mul_overflow/nata/1 | 0.000073544 | 0.000075783 | 0.000079533 | MET |
| plan-sql-s4_bigint_mul_overflow/twin/1 | 0.000073423 | 0.000077314 | 0.000094009 |  |
| plan-sql-s4_dec_add_operand_does_not_fit_after_rescale/duckdb/1 | 0.000765148 | 0.000785090 | 0.000806531 |  |
| plan-sql-s4_dec_add_operand_does_not_fit_after_rescale/nata/1 | 0.000078104 | 0.000079483 | 0.000081179 | MET |
| plan-sql-s4_dec_add_operand_does_not_fit_after_rescale/twin/1 | 0.000078638 | 0.000080979 | 0.000082993 |  |
| plan-sql-s4_dec_add_sub_mul/duckdb/1 | 0.001443859 | 0.001447324 | 0.001448129 |  |
| plan-sql-s4_dec_add_sub_mul/nata/1 | 0.000099645 | 0.000100845 | 0.000102880 | MET |
| plan-sql-s4_dec_add_sub_mul/twin/1 | 0.000106110 | 0.000108235 | 0.000144916 |  |
| plan-sql-s4_dec_int_mix/duckdb/1 | 0.001461130 | 0.001535438 | 0.001551168 |  |
| plan-sql-s4_dec_int_mix/nata/1 | 0.000100259 | 0.000103075 | 0.000104460 | MET |
| plan-sql-s4_dec_int_mix/twin/1 | 0.000106710 | 0.000111475 | 0.000118775 |  |
| plan-sql-s4_dec_mod/duckdb/1 | 0.001337289 | 0.001352380 | 0.001396037 |  |
| plan-sql-s4_dec_mod/nata/1 | 0.000094615 | 0.000096329 | 0.000098479 | MET |
| plan-sql-s4_dec_mod/twin/1 | 0.000099030 | 0.000101059 | 0.000123516 |  |
| plan-sql-s4_dec_mul_overflow/duckdb/1 | 0.000855802 | 0.000859903 | 0.000901860 |  |
| plan-sql-s4_dec_mul_overflow/nata/1 | 0.000079593 | 0.000081019 | 0.000082374 | MET |
| plan-sql-s4_dec_mul_overflow/twin/1 | 0.000081409 | 0.000094810 | 0.000099435 |  |
| plan-sql-s4_dec_q1_chain/duckdb/1 | 0.001468409 | 0.001472765 | 0.001544923 |  |
| plan-sql-s4_dec_q1_chain/nata/1 | 0.000093529 | 0.000094839 | 0.000097674 | MET |
| plan-sql-s4_dec_q1_chain/twin/1 | 0.000097820 | 0.000104255 | 0.000138556 |  |
| plan-sql-s4_dec_wide_add/duckdb/1 | 0.001217294 | 0.001286006 | 0.001295951 |  |
| plan-sql-s4_dec_wide_add/nata/1 | 0.000085544 | 0.000087254 | 0.000091369 | MET |
| plan-sql-s4_dec_wide_add/twin/1 | 0.000089984 | 0.000090599 | 0.000098869 |  |
| plan-sql-s4_div_zero_guarded/duckdb/1 | 0.000888059 | 0.000907564 | 0.000939182 |  |
| plan-sql-s4_div_zero_guarded/nata/1 | 0.000080459 | 0.000081979 | 0.000090929 | MET |
| plan-sql-s4_div_zero_guarded/twin/1 | 0.000081954 | 0.000084048 | 0.000099784 |  |
| plan-sql-s4_div_zero_unselected/duckdb/1 | 0.000832336 | 0.000883118 | 0.000901455 |  |
| plan-sql-s4_div_zero_unselected/nata/1 | 0.000077083 | 0.000079223 | 0.000079919 | MET |
| plan-sql-s4_div_zero_unselected/twin/1 | 0.000077249 | 0.000079138 | 0.000083694 |  |
| plan-sql-s4_int_add_bigint/duckdb/1 | 0.001244045 | 0.001269645 | 0.001289876 |  |
| plan-sql-s4_int_add_bigint/nata/1 | 0.000083734 | 0.000088134 | 0.000089984 | MET |
| plan-sql-s4_int_add_bigint/twin/1 | 0.000088084 | 0.000095564 | 0.000097870 |  |
| plan-sql-s4_int_add_integer/duckdb/1 | 0.001137859 | 0.001180497 | 0.001232364 |  |
| plan-sql-s4_int_add_integer/nata/1 | 0.000085074 | 0.000085149 | 0.000089414 | MET |
| plan-sql-s4_int_add_integer/twin/1 | 0.000085519 | 0.000087624 | 0.000108680 |  |
| plan-sql-s4_int_add_overflow/duckdb/1 | 0.000639123 | 0.000648893 | 0.000661109 |  |
| plan-sql-s4_int_add_overflow/nata/1 | 0.000075154 | 0.000075909 | 0.000077099 | MET |
| plan-sql-s4_int_add_overflow/twin/1 | 0.000075413 | 0.000076168 | 0.000077553 |  |
| plan-sql-s4_int_add_smallint/duckdb/1 | 0.001161381 | 0.001176257 | 0.001188756 |  |
| plan-sql-s4_int_add_smallint/nata/1 | 0.000085373 | 0.000086033 | 0.000089084 | MET |
| plan-sql-s4_int_add_smallint/twin/1 | 0.000086159 | 0.000091729 | 0.000098429 |  |
| plan-sql-s4_int_add_tinyint/duckdb/1 | 0.001161115 | 0.001181391 | 0.001202897 |  |
| plan-sql-s4_int_add_tinyint/nata/1 | 0.000084974 | 0.000086254 | 0.000088899 | MET |
| plan-sql-s4_int_add_tinyint/twin/1 | 0.000087959 | 0.000089889 | 0.000090564 |  |
| plan-sql-s4_int_div_trunc/duckdb/1 | 0.001271650 | 0.001281741 | 0.001324973 |  |
| plan-sql-s4_int_div_trunc/nata/1 | 0.000092994 | 0.000094814 | 0.000095779 | MET |
| plan-sql-s4_int_div_trunc/twin/1 | 0.000096754 | 0.000099879 | 0.000111289 |  |
| plan-sql-s4_int_intdiv_min_minus_one/duckdb/1 | 0.000668934 | 0.000699400 | 0.000701011 |  |
| plan-sql-s4_int_intdiv_min_minus_one/nata/1 | 0.000074464 | 0.000075719 | 0.000081344 | MET |
| plan-sql-s4_int_intdiv_min_minus_one/twin/1 | 0.000078323 | 0.000079603 | 0.000082294 |  |
| plan-sql-s4_int_intdiv_zero/duckdb/1 | 0.000658823 | 0.000683984 | 0.000702865 |  |
| plan-sql-s4_int_intdiv_zero/nata/1 | 0.000073533 | 0.000077068 | 0.000081314 | MET |
| plan-sql-s4_int_intdiv_zero/twin/1 | 0.000074799 | 0.000075963 | 0.000102045 |  |
| plan-sql-s4_int_mixed_ops_bigint/duckdb/1 | 0.001375755 | 0.001454448 | 0.001479034 |  |
| plan-sql-s4_int_mixed_ops_bigint/nata/1 | 0.000099549 | 0.000100460 | 0.000104200 | MET |
| plan-sql-s4_int_mixed_ops_bigint/twin/1 | 0.000102215 | 0.000104625 | 0.000129020 |  |
| plan-sql-s4_int_mixed_ops_integer/duckdb/1 | 0.001325533 | 0.001365980 | 0.001400021 |  |
| plan-sql-s4_int_mixed_ops_integer/nata/1 | 0.000097834 | 0.000101780 | 0.000103490 | MET |
| plan-sql-s4_int_mixed_ops_integer/twin/1 | 0.000102539 | 0.000103539 | 0.000117635 |  |
| plan-sql-s4_int_mixed_ops_smallint/duckdb/1 | 0.001324837 | 0.001387105 | 0.001406181 |  |
| plan-sql-s4_int_mixed_ops_smallint/nata/1 | 0.000100410 | 0.000101334 | 0.000104000 | MET |
| plan-sql-s4_int_mixed_ops_smallint/twin/1 | 0.000101914 | 0.000107630 | 0.000119419 |  |
| plan-sql-s4_int_mixed_ops_tinyint/duckdb/1 | 0.001361659 | 0.001393221 | 0.001451013 |  |
| plan-sql-s4_int_mixed_ops_tinyint/nata/1 | 0.000097514 | 0.000099554 | 0.000101184 | MET |
| plan-sql-s4_int_mixed_ops_tinyint/twin/1 | 0.000100694 | 0.000106165 | 0.000143087 |  |
| plan-sql-s4_int_mod_min_minus_one/duckdb/1 | 0.000649138 | 0.000667714 | 0.000686464 |  |
| plan-sql-s4_int_mod_min_minus_one/nata/1 | 0.000073413 | 0.000074943 | 0.000078853 | MET |
| plan-sql-s4_int_mod_min_minus_one/twin/1 | 0.000078749 | 0.000081444 | 0.000101104 |  |
| plan-sql-s4_int_mod_sign/duckdb/1 | 0.001279095 | 0.001292106 | 0.001345523 |  |
| plan-sql-s4_int_mod_sign/nata/1 | 0.000093724 | 0.000095769 | 0.000099010 | MET |
| plan-sql-s4_int_mod_sign/twin/1 | 0.000101244 | 0.000103820 | 0.000124170 |  |
| plan-sql-s4_int_mod_zero/duckdb/1 | 0.000652978 | 0.000665429 | 0.000679524 |  |
| plan-sql-s4_int_mod_zero/nata/1 | 0.000072974 | 0.000075063 | 0.000077638 | MET |
| plan-sql-s4_int_mod_zero/twin/1 | 0.000075644 | 0.000077863 | 0.000108600 |  |
| plan-sql-s4_int_mul_overflow/duckdb/1 | 0.000683955 | 0.000711461 | 0.000719336 |  |
| plan-sql-s4_int_mul_overflow/nata/1 | 0.000075758 | 0.000079968 | 0.000082859 | MET |
| plan-sql-s4_int_mul_overflow/twin/1 | 0.000073208 | 0.000077674 | 0.000082749 |  |
| plan-sql-s4_int_sub_overflow/duckdb/1 | 0.000662478 | 0.000677834 | 0.000703710 |  |
| plan-sql-s4_int_sub_overflow/nata/1 | 0.000074788 | 0.000076538 | 0.000082068 | MET |
| plan-sql-s4_int_sub_overflow/twin/1 | 0.000075673 | 0.000076583 | 0.000084993 |  |
| plan-sql-s4_int_widening_i_plus_b/duckdb/1 | 0.000915205 | 0.000956931 | 0.000984223 |  |
| plan-sql-s4_int_widening_i_plus_b/nata/1 | 0.000083129 | 0.000085663 | 0.000087219 | MET |
| plan-sql-s4_int_widening_i_plus_b/twin/1 | 0.000085149 | 0.000086123 | 0.000097729 |  |
| plan-sql-s4_mod_zero_null_row/duckdb/1 | 0.000850561 | 0.000867913 | 0.000901569 |  |
| plan-sql-s4_mod_zero_null_row/nata/1 | 0.000078184 | 0.000078904 | 0.000090174 | MET |
| plan-sql-s4_mod_zero_null_row/twin/1 | 0.000080583 | 0.000081194 | 0.000084299 |  |
| plan-sql-s4_overflow_guarded_by_and/duckdb/1 | 0.000805349 | 0.000873193 | 0.000965921 |  |
| plan-sql-s4_overflow_guarded_by_and/nata/1 | 0.000080693 | 0.000081668 | 0.000083773 | MET |
| plan-sql-s4_overflow_guarded_by_and/twin/1 | 0.000083139 | 0.000084603 | 0.000095719 |  |
| plan-sql-s4_overflow_guarded_by_case/duckdb/1 | 0.000817771 | 0.000829000 | 0.000848111 |  |
| plan-sql-s4_overflow_guarded_by_case/nata/1 | 0.000083699 | 0.000087929 | 0.000088894 | MET |
| plan-sql-s4_overflow_guarded_by_case/twin/1 | 0.000087304 | 0.000093830 | 0.000100275 |  |
| plan-sql-s4_overflow_null_row_is_not_an_error/duckdb/1 | 0.000805320 | 0.000840171 | 0.000852896 |  |
| plan-sql-s4_overflow_null_row_is_not_an_error/nata/1 | 0.000076789 | 0.000079414 | 0.000080138 | MET |
| plan-sql-s4_overflow_null_row_is_not_an_error/twin/1 | 0.000077504 | 0.000081323 | 0.000082919 |  |
| plan-sql-s4_overflow_unselected_rows/duckdb/1 | 0.000797659 | 0.000824930 | 0.000826625 |  |
| plan-sql-s4_overflow_unselected_rows/nata/1 | 0.000078548 | 0.000078899 | 0.000083589 | MET |
| plan-sql-s4_overflow_unselected_rows/twin/1 | 0.000077663 | 0.000083559 | 0.000086684 |  |
| plan-sql-s4_precedence/duckdb/1 | 0.001007898 | 0.001033094 | 0.001059810 |  |
| plan-sql-s4_precedence/nata/1 | 0.000097349 | 0.000098334 | 0.000101944 | MET |
| plan-sql-s4_precedence/twin/1 | 0.000100624 | 0.000113335 | 0.000119555 |  |
| plan-sql-s4_precedence_logic/duckdb/1 | 0.001067880 | 0.001090191 | 0.001101547 |  |
| plan-sql-s4_precedence_logic/nata/1 | 0.000093039 | 0.000094784 | 0.000097769 | MET |
| plan-sql-s4_precedence_logic/twin/1 | 0.000095034 | 0.000102620 | 0.000116900 |  |
| plan-sql-s4_tinyint_overflow/duckdb/1 | 0.000680595 | 0.000691639 | 0.000711995 |  |
| plan-sql-s4_tinyint_overflow/nata/1 | 0.000075203 | 0.000077868 | 0.000081774 | MET |
| plan-sql-s4_tinyint_overflow/twin/1 | 0.000075238 | 0.000078039 | 0.000082053 |  |
| plan-sql-s4_unary_minus/duckdb/1 | 0.000957541 | 0.000975402 | 0.000993298 |  |
| plan-sql-s4_unary_minus/nata/1 | 0.000086353 | 0.000089334 | 0.000092604 | MET |
| plan-sql-s4_unary_minus/twin/1 | 0.000092084 | 0.000092729 | 0.000117030 |  |
| plan-sql-s4_unary_minus_min/duckdb/1 | 0.000654248 | 0.000661519 | 0.000685669 |  |
| plan-sql-s4_unary_minus_min/nata/1 | 0.000071573 | 0.000072883 | 0.000074738 | MET |
| plan-sql-s4_unary_minus_min/twin/1 | 0.000072793 | 0.000074483 | 0.000102240 |  |
| plan-sql-s5_and_or_not_truth_table/duckdb/1 | 0.001081256 | 0.001098502 | 0.001115658 |  |
| plan-sql-s5_and_or_not_truth_table/nata/1 | 0.000110339 | 0.000111569 | 0.000113070 | MET |
| plan-sql-s5_and_or_not_truth_table/twin/1 | 0.000111905 | 0.000116280 | 0.000127726 |  |
| plan-sql-s5_between/duckdb/1 | 0.001139004 | 0.001153809 | 0.001164394 |  |
| plan-sql-s5_between/nata/1 | 0.000103279 | 0.000104620 | 0.000107625 | MET |
| plan-sql-s5_between/twin/1 | 0.000108375 | 0.000109840 | 0.000117100 |  |
| plan-sql-s5_between_doubles/duckdb/1 | 0.000793044 | 0.000822380 | 0.000860561 |  |
| plan-sql-s5_between_doubles/nata/1 | 0.000082174 | 0.000085719 | 0.000088179 | MET |
| plan-sql-s5_between_doubles/twin/1 | 0.000082754 | 0.000088123 | 0.000129105 |  |
| plan-sql-s5_between_expressions/duckdb/1 | 0.000799699 | 0.000816214 | 0.000864926 |  |
| plan-sql-s5_between_expressions/nata/1 | 0.000080228 | 0.000081513 | 0.000084119 | MET |
| plan-sql-s5_between_expressions/twin/1 | 0.000086054 | 0.000088498 | 0.000102359 |  |
| plan-sql-s5_between_null_bounds/duckdb/1 | 0.000865877 | 0.000883738 | 0.000910684 |  |
| plan-sql-s5_between_null_bounds/nata/1 | 0.000089874 | 0.000092174 | 0.000100175 | MET |
| plan-sql-s5_between_null_bounds/twin/1 | 0.000094399 | 0.000095209 | 0.000102534 |  |
| plan-sql-s5_between_reversed/duckdb/1 | 0.000763772 | 0.000777328 | 0.000833356 |  |
| plan-sql-s5_between_reversed/nata/1 | 0.000082884 | 0.000083373 | 0.000084348 | MET |
| plan-sql-s5_between_reversed/twin/1 | 0.000083403 | 0.000085229 | 0.000085908 |  |
| plan-sql-s5_between_where/duckdb/1 | 0.000972377 | 0.000999732 | 0.001033609 |  |
| plan-sql-s5_between_where/nata/1 | 0.000083754 | 0.000084899 | 0.000089168 | MET |
| plan-sql-s5_between_where/twin/1 | 0.000085619 | 0.000087359 | 0.000089999 |  |
| plan-sql-s5_cmp_across_scales_exact/duckdb/1 | 0.000897688 | 0.000919374 | 0.000923024 |  |
| plan-sql-s5_cmp_across_scales_exact/nata/1 | 0.000087394 | 0.000090294 | 0.000090894 | MET |
| plan-sql-s5_cmp_across_scales_exact/twin/1 | 0.000091179 | 0.000097314 | 0.000103719 |  |
| plan-sql-s5_cmp_bool_bang/duckdb/1 | 0.000687364 | 0.000698780 | 0.000731261 |  |
| plan-sql-s5_cmp_bool_bang/nata/1 | 0.000073948 | 0.000076678 | 0.000077178 | MET |
| plan-sql-s5_cmp_bool_bang/twin/1 | 0.000076803 | 0.000078729 | 0.000088489 |  |
| plan-sql-s5_cmp_bool_eq/duckdb/1 | 0.000717390 | 0.000721430 | 0.000735086 |  |
| plan-sql-s5_cmp_bool_eq/nata/1 | 0.000075019 | 0.000075443 | 0.000079788 | MET |
| plan-sql-s5_cmp_bool_eq/twin/1 | 0.000078078 | 0.000081224 | 0.000099050 |  |
| plan-sql-s5_cmp_bool_ge/duckdb/1 | 0.000685278 | 0.000714365 | 0.000736586 |  |
| plan-sql-s5_cmp_bool_ge/nata/1 | 0.000075218 | 0.000077679 | 0.000078958 | MET |
| plan-sql-s5_cmp_bool_ge/twin/1 | 0.000075978 | 0.000076958 | 0.000079044 |  |
| plan-sql-s5_cmp_bool_gt/duckdb/1 | 0.000712445 | 0.000730596 | 0.000739146 |  |
| plan-sql-s5_cmp_bool_gt/nata/1 | 0.000076028 | 0.000076978 | 0.000084854 | MET |
| plan-sql-s5_cmp_bool_gt/twin/1 | 0.000077693 | 0.000080718 | 0.000086274 |  |
| plan-sql-s5_cmp_bool_le/duckdb/1 | 0.000718980 | 0.000729105 | 0.000761907 |  |
| plan-sql-s5_cmp_bool_le/nata/1 | 0.000073418 | 0.000076889 | 0.000083249 | MET |
| plan-sql-s5_cmp_bool_le/twin/1 | 0.000078688 | 0.000088743 | 0.000119876 |  |
| plan-sql-s5_cmp_bool_lt/duckdb/1 | 0.000658258 | 0.000671939 | 0.000710340 |  |
| plan-sql-s5_cmp_bool_lt/nata/1 | 0.000076244 | 0.000077648 | 0.000078793 | MET |
| plan-sql-s5_cmp_bool_lt/twin/1 | 0.000076554 | 0.000087149 | 0.000105400 |  |
| plan-sql-s5_cmp_bool_ne/duckdb/1 | 0.000661493 | 0.000704055 | 0.000709845 |  |
| plan-sql-s5_cmp_bool_ne/nata/1 | 0.000075188 | 0.000076553 | 0.000112680 | MET |
| plan-sql-s5_cmp_bool_ne/twin/1 | 0.000078213 | 0.000078908 | 0.000089328 |  |
| plan-sql-s5_cmp_columns/duckdb/1 | 0.000934744 | 0.000942714 | 0.000965726 |  |
| plan-sql-s5_cmp_columns/nata/1 | 0.000094084 | 0.000095899 | 0.000098795 | MET |
| plan-sql-s5_cmp_columns/twin/1 | 0.000098184 | 0.000100719 | 0.000110045 |  |
| plan-sql-s5_cmp_date_bang/duckdb/1 | 0.000622701 | 0.000677579 | 0.000719270 |  |
| plan-sql-s5_cmp_date_bang/nata/1 | 0.000075508 | 0.000076739 | 0.000081824 | MET |
| plan-sql-s5_cmp_date_bang/twin/1 | 0.000077698 | 0.000081743 | 0.000082694 |  |
| plan-sql-s5_cmp_date_eq/duckdb/1 | 0.000648387 | 0.000676383 | 0.000754672 |  |
| plan-sql-s5_cmp_date_eq/nata/1 | 0.000075453 | 0.000077999 | 0.000085639 | MET |
| plan-sql-s5_cmp_date_eq/twin/1 | 0.000076834 | 0.000078038 | 0.000080768 |  |
| plan-sql-s5_cmp_date_ge/duckdb/1 | 0.000673178 | 0.000692104 | 0.000712855 |  |
| plan-sql-s5_cmp_date_ge/nata/1 | 0.000075648 | 0.000078524 | 0.000080818 | MET |
| plan-sql-s5_cmp_date_ge/twin/1 | 0.000076483 | 0.000080594 | 0.000083629 |  |
| plan-sql-s5_cmp_date_gt/duckdb/1 | 0.000691189 | 0.000711409 | 0.000727540 |  |
| plan-sql-s5_cmp_date_gt/nata/1 | 0.000077754 | 0.000078443 | 0.000080688 | MET |
| plan-sql-s5_cmp_date_gt/twin/1 | 0.000077518 | 0.000078053 | 0.000080668 |  |
| plan-sql-s5_cmp_date_le/duckdb/1 | 0.000672438 | 0.000697709 | 0.000721090 |  |
| plan-sql-s5_cmp_date_le/nata/1 | 0.000076078 | 0.000076528 | 0.000080749 | MET |
| plan-sql-s5_cmp_date_le/twin/1 | 0.000075353 | 0.000078519 | 0.000084403 |  |
| plan-sql-s5_cmp_date_lt/duckdb/1 | 0.000673573 | 0.000708980 | 0.000765487 |  |
| plan-sql-s5_cmp_date_lt/nata/1 | 0.000076154 | 0.000076784 | 0.000079293 | MET |
| plan-sql-s5_cmp_date_lt/twin/1 | 0.000079823 | 0.000087178 | 0.000103199 |  |
| plan-sql-s5_cmp_date_ne/duckdb/1 | 0.000682784 | 0.000686699 | 0.000696464 |  |
| plan-sql-s5_cmp_date_ne/nata/1 | 0.000075083 | 0.000076139 | 0.000077163 | MET |
| plan-sql-s5_cmp_date_ne/twin/1 | 0.000076023 | 0.000077243 | 0.000084733 |  |
| plan-sql-s5_cmp_dec_bang/duckdb/1 | 0.000735886 | 0.000762536 | 0.000769387 |  |
| plan-sql-s5_cmp_dec_bang/nata/1 | 0.000080354 | 0.000082714 | 0.000083184 | MET |
| plan-sql-s5_cmp_dec_bang/twin/1 | 0.000082893 | 0.000087484 | 0.000099114 |  |
| plan-sql-s5_cmp_dec_eq/duckdb/1 | 0.000730165 | 0.000764667 | 0.000769947 |  |
| plan-sql-s5_cmp_dec_eq/nata/1 | 0.000080168 | 0.000081788 | 0.000082458 | MET |
| plan-sql-s5_cmp_dec_eq/twin/1 | 0.000081478 | 0.000086188 | 0.000112165 |  |
| plan-sql-s5_cmp_dec_ge/duckdb/1 | 0.000740691 | 0.000770597 | 0.000783962 |  |
| plan-sql-s5_cmp_dec_ge/nata/1 | 0.000081678 | 0.000082894 | 0.000084054 | MET |
| plan-sql-s5_cmp_dec_ge/twin/1 | 0.000085169 | 0.000094004 | 0.000095799 |  |
| plan-sql-s5_cmp_dec_gt/duckdb/1 | 0.000735926 | 0.000744226 | 0.000774047 |  |
| plan-sql-s5_cmp_dec_gt/nata/1 | 0.000081083 | 0.000082919 | 0.000084548 | MET |
| plan-sql-s5_cmp_dec_gt/twin/1 | 0.000081203 | 0.000084559 | 0.000087243 |  |
| plan-sql-s5_cmp_dec_le/duckdb/1 | 0.000718239 | 0.000745391 | 0.000754896 |  |
| plan-sql-s5_cmp_dec_le/nata/1 | 0.000080184 | 0.000084009 | 0.000088689 | MET |
| plan-sql-s5_cmp_dec_le/twin/1 | 0.000084989 | 0.000099079 | 0.000120910 |  |
| plan-sql-s5_cmp_dec_lt/duckdb/1 | 0.000739526 | 0.000749221 | 0.000757961 |  |
| plan-sql-s5_cmp_dec_lt/nata/1 | 0.000081243 | 0.000081654 | 0.000084988 | MET |
| plan-sql-s5_cmp_dec_lt/twin/1 | 0.000084653 | 0.000085568 | 0.000110814 |  |
| plan-sql-s5_cmp_dec_ne/duckdb/1 | 0.000723000 | 0.000728561 | 0.000736831 |  |
| plan-sql-s5_cmp_dec_ne/nata/1 | 0.000079783 | 0.000081408 | 0.000082704 | MET |
| plan-sql-s5_cmp_dec_ne/twin/1 | 0.000080433 | 0.000097784 | 0.000129485 |  |
| plan-sql-s5_cmp_decimal_with_wide_and_narrow/duckdb/1 | 0.000838120 | 0.000843765 | 0.000873136 |  |
| plan-sql-s5_cmp_decimal_with_wide_and_narrow/nata/1 | 0.000084799 | 0.000085184 | 0.000085849 | MET |
| plan-sql-s5_cmp_decimal_with_wide_and_narrow/twin/1 | 0.000086954 | 0.000094759 | 0.000119445 |  |
| plan-sql-s5_cmp_double_bang/duckdb/1 | 0.000769942 | 0.000777617 | 0.000821675 |  |
| plan-sql-s5_cmp_double_bang/nata/1 | 0.000083058 | 0.000083988 | 0.000086184 | MET |
| plan-sql-s5_cmp_double_bang/twin/1 | 0.000082783 | 0.000085614 | 0.000109485 |  |
| plan-sql-s5_cmp_double_eq/duckdb/1 | 0.000765577 | 0.000785358 | 0.000798218 |  |
| plan-sql-s5_cmp_double_eq/nata/1 | 0.000083163 | 0.000084174 | 0.000086134 | MET |
| plan-sql-s5_cmp_double_eq/twin/1 | 0.000084673 | 0.000090854 | 0.000099924 |  |
| plan-sql-s5_cmp_double_ge/duckdb/1 | 0.000768467 | 0.000808338 | 0.000813078 |  |
| plan-sql-s5_cmp_double_ge/nata/1 | 0.000085554 | 0.000085769 | 0.000093599 | MET |
| plan-sql-s5_cmp_double_ge/twin/1 | 0.000086219 | 0.000088239 | 0.000093670 |  |
| plan-sql-s5_cmp_double_gt/duckdb/1 | 0.000778397 | 0.000797758 | 0.000828594 |  |
| plan-sql-s5_cmp_double_gt/nata/1 | 0.000081608 | 0.000082088 | 0.000088659 | MET |
| plan-sql-s5_cmp_double_gt/twin/1 | 0.000082723 | 0.000090544 | 0.000098994 |  |
| plan-sql-s5_cmp_double_le/duckdb/1 | 0.000760286 | 0.000785162 | 0.000789743 |  |
| plan-sql-s5_cmp_double_le/nata/1 | 0.000081363 | 0.000085238 | 0.000085474 | MET |
| plan-sql-s5_cmp_double_le/twin/1 | 0.000083353 | 0.000083699 | 0.000088824 |  |
| plan-sql-s5_cmp_double_lt/duckdb/1 | 0.000768281 | 0.000786522 | 0.000797928 |  |
| plan-sql-s5_cmp_double_lt/nata/1 | 0.000082223 | 0.000083123 | 0.000088458 | MET |
| plan-sql-s5_cmp_double_lt/twin/1 | 0.000083598 | 0.000086963 | 0.000114020 |  |
| plan-sql-s5_cmp_double_ne/duckdb/1 | 0.000716195 | 0.000758206 | 0.000768271 |  |
| plan-sql-s5_cmp_double_ne/nata/1 | 0.000082009 | 0.000083684 | 0.000091374 | MET |
| plan-sql-s5_cmp_double_ne/twin/1 | 0.000085438 | 0.000086244 | 0.000090313 |  |
| plan-sql-s5_cmp_int_bang/duckdb/1 | 0.000645672 | 0.000665528 | 0.000674538 |  |
| plan-sql-s5_cmp_int_bang/nata/1 | 0.000074378 | 0.000075713 | 0.000080788 | MET |
| plan-sql-s5_cmp_int_bang/twin/1 | 0.000079249 | 0.000079934 | 0.000095639 |  |
| plan-sql-s5_cmp_int_eq/duckdb/1 | 0.000654872 | 0.000666132 | 0.000678098 |  |
| plan-sql-s5_cmp_int_eq/nata/1 | 0.000075934 | 0.000076983 | 0.000078599 | MET |
| plan-sql-s5_cmp_int_eq/twin/1 | 0.000076513 | 0.000080459 | 0.000093954 |  |
| plan-sql-s5_cmp_int_ge/duckdb/1 | 0.000651692 | 0.000662433 | 0.000672428 |  |
| plan-sql-s5_cmp_int_ge/nata/1 | 0.000075043 | 0.000078444 | 0.000083673 | MET |
| plan-sql-s5_cmp_int_ge/twin/1 | 0.000076353 | 0.000080628 | 0.000107324 |  |
| plan-sql-s5_cmp_int_gt/duckdb/1 | 0.000621050 | 0.000648192 | 0.000653202 |  |
| plan-sql-s5_cmp_int_gt/nata/1 | 0.000075718 | 0.000077388 | 0.000078053 | MET |
| plan-sql-s5_cmp_int_gt/twin/1 | 0.000079624 | 0.000080308 | 0.000091364 |  |
| plan-sql-s5_cmp_int_le/duckdb/1 | 0.000629651 | 0.000633516 | 0.000715954 |  |
| plan-sql-s5_cmp_int_le/nata/1 | 0.000074633 | 0.000075353 | 0.000077968 | MET |
| plan-sql-s5_cmp_int_le/twin/1 | 0.000077908 | 0.000078853 | 0.000084644 |  |
| plan-sql-s5_cmp_int_lt/duckdb/1 | 0.000648021 | 0.000670358 | 0.000711504 |  |
| plan-sql-s5_cmp_int_lt/nata/1 | 0.000076458 | 0.000078318 | 0.000082663 | MET |
| plan-sql-s5_cmp_int_lt/twin/1 | 0.000075583 | 0.000080508 | 0.000091929 |  |
| plan-sql-s5_cmp_int_ne/duckdb/1 | 0.000645517 | 0.000662357 | 0.000677272 |  |
| plan-sql-s5_cmp_int_ne/nata/1 | 0.000074458 | 0.000078863 | 0.000081079 | MET |
| plan-sql-s5_cmp_int_ne/twin/1 | 0.000077208 | 0.000080353 | 0.000099224 |  |
| plan-sql-s5_cmp_real_bang/duckdb/1 | 0.000633561 | 0.000644191 | 0.000663717 |  |
| plan-sql-s5_cmp_real_bang/nata/1 | 0.000076398 | 0.000076788 | 0.000080669 | MET |
| plan-sql-s5_cmp_real_bang/twin/1 | 0.000076338 | 0.000078558 | 0.000081158 |  |
| plan-sql-s5_cmp_real_eq/duckdb/1 | 0.000616460 | 0.000642832 | 0.000657137 |  |
| plan-sql-s5_cmp_real_eq/nata/1 | 0.000075973 | 0.000077973 | 0.000078528 | MET |
| plan-sql-s5_cmp_real_eq/twin/1 | 0.000079368 | 0.000082289 | 0.000100189 |  |
| plan-sql-s5_cmp_real_ge/duckdb/1 | 0.000612460 | 0.000621291 | 0.000638987 |  |
| plan-sql-s5_cmp_real_ge/nata/1 | 0.000075423 | 0.000079023 | 0.000081299 | MET |
| plan-sql-s5_cmp_real_ge/twin/1 | 0.000078889 | 0.000081203 | 0.000091673 |  |
| plan-sql-s5_cmp_real_gt/duckdb/1 | 0.000637241 | 0.000670913 | 0.000685983 |  |
| plan-sql-s5_cmp_real_gt/nata/1 | 0.000073998 | 0.000074843 | 0.000077828 | MET |
| plan-sql-s5_cmp_real_gt/twin/1 | 0.000075768 | 0.000076883 | 0.000120370 |  |
| plan-sql-s5_cmp_real_le/duckdb/1 | 0.000632541 | 0.000663222 | 0.000665312 |  |
| plan-sql-s5_cmp_real_le/nata/1 | 0.000075103 | 0.000075723 | 0.000076373 | MET |
| plan-sql-s5_cmp_real_le/twin/1 | 0.000077093 | 0.000079553 | 0.000080203 |  |
| plan-sql-s5_cmp_real_lt/duckdb/1 | 0.000649606 | 0.000651082 | 0.000670442 |  |
| plan-sql-s5_cmp_real_lt/nata/1 | 0.000075243 | 0.000075578 | 0.000079858 | MET |
| plan-sql-s5_cmp_real_lt/twin/1 | 0.000075763 | 0.000079778 | 0.000080698 |  |
| plan-sql-s5_cmp_real_ne/duckdb/1 | 0.000648936 | 0.000701449 | 0.000718850 |  |
| plan-sql-s5_cmp_real_ne/nata/1 | 0.000074578 | 0.000077968 | 0.000078524 | MET |
| plan-sql-s5_cmp_real_ne/twin/1 | 0.000077288 | 0.000079388 | 0.000116720 |  |
| plan-sql-s5_cmp_str_bang/duckdb/1 | 0.000671113 | 0.000689283 | 0.000711489 |  |
| plan-sql-s5_cmp_str_bang/nata/1 | 0.000075763 | 0.000077853 | 0.000078133 | MET |
| plan-sql-s5_cmp_str_bang/twin/1 | 0.000076519 | 0.000079363 | 0.000083359 |  |
| plan-sql-s5_cmp_str_eq/duckdb/1 | 0.000659612 | 0.000671233 | 0.000713154 |  |
| plan-sql-s5_cmp_str_eq/nata/1 | 0.000075153 | 0.000078254 | 0.000081719 | MET |
| plan-sql-s5_cmp_str_eq/twin/1 | 0.000077273 | 0.000080084 | 0.000086909 |  |
| plan-sql-s5_cmp_str_ge/duckdb/1 | 0.000627561 | 0.000646532 | 0.000664848 |  |
| plan-sql-s5_cmp_str_ge/nata/1 | 0.000075453 | 0.000077158 | 0.000078813 | MET |
| plan-sql-s5_cmp_str_ge/twin/1 | 0.000076248 | 0.000078518 | 0.000090073 |  |
| plan-sql-s5_cmp_str_gt/duckdb/1 | 0.000612350 | 0.000650027 | 0.000682418 |  |
| plan-sql-s5_cmp_str_gt/nata/1 | 0.000076623 | 0.000077898 | 0.000081059 | MET |
| plan-sql-s5_cmp_str_gt/twin/1 | 0.000077209 | 0.000078908 | 0.000094694 |  |
| plan-sql-s5_cmp_str_le/duckdb/1 | 0.000667102 | 0.000678232 | 0.000686828 |  |
| plan-sql-s5_cmp_str_le/nata/1 | 0.000073108 | 0.000075063 | 0.000080069 | MET |
| plan-sql-s5_cmp_str_le/twin/1 | 0.000075559 | 0.000078553 | 0.000092814 |  |
| plan-sql-s5_cmp_str_lt/duckdb/1 | 0.000671998 | 0.000684468 | 0.000716544 |  |
| plan-sql-s5_cmp_str_lt/nata/1 | 0.000074763 | 0.000076913 | 0.000080633 | MET |
| plan-sql-s5_cmp_str_lt/twin/1 | 0.000075058 | 0.000076698 | 0.000080188 |  |
| plan-sql-s5_cmp_str_ne/duckdb/1 | 0.000668497 | 0.000715934 | 0.000755536 |  |
| plan-sql-s5_cmp_str_ne/nata/1 | 0.000074928 | 0.000075223 | 0.000076593 | MET |
| plan-sql-s5_cmp_str_ne/twin/1 | 0.000075303 | 0.000077593 | 0.000093444 |  |
| plan-sql-s5_in_list/duckdb/1 | 0.001306338 | 0.001325874 | 0.001365976 |  |
| plan-sql-s5_in_list/nata/1 | 0.000106635 | 0.000109629 | 0.000115310 | MET |
| plan-sql-s5_in_list/twin/1 | 0.000112105 | 0.000121770 | 0.000135520 |  |
| plan-sql-s5_in_list_mixed_numeric/duckdb/1 | 0.001041998 | 0.001074429 | 0.001093729 |  |
| plan-sql-s5_in_list_mixed_numeric/nata/1 | 0.000086048 | 0.000087004 | 0.000089658 | MET |
| plan-sql-s5_in_list_mixed_numeric/twin/1 | 0.000088999 | 0.000090874 | 0.000095134 |  |
| plan-sql-s5_in_list_null/duckdb/1 | 0.000901346 | 0.000914697 | 0.000956183 |  |
| plan-sql-s5_in_list_null/nata/1 | 0.000093663 | 0.000097079 | 0.000101029 | MET |
| plan-sql-s5_in_list_null/twin/1 | 0.000097939 | 0.000111304 | 0.000136870 |  |
| plan-sql-s5_in_list_where/duckdb/1 | 0.000931468 | 0.000982880 | 0.001013646 |  |
| plan-sql-s5_in_list_where/nata/1 | 0.000087458 | 0.000087704 | 0.000090213 | MET |
| plan-sql-s5_in_list_where/twin/1 | 0.000089159 | 0.000090974 | 0.000117404 |  |
| plan-sql-s5_is_null/duckdb/1 | 0.000960729 | 0.000969830 | 0.001017977 |  |
| plan-sql-s5_is_null/nata/1 | 0.000102284 | 0.000102589 | 0.000113875 | MET |
| plan-sql-s5_is_null/twin/1 | 0.000103084 | 0.000110994 | 0.000125049 |  |
| plan-sql-s5_is_null_where/duckdb/1 | 0.000771996 | 0.000772877 | 0.000802248 |  |
| plan-sql-s5_is_null_where/nata/1 | 0.000081114 | 0.000083298 | 0.000086018 | MET |
| plan-sql-s5_is_null_where/twin/1 | 0.000084083 | 0.000086858 | 0.000104649 |  |
| plan-sql-s5_nan_semantics/duckdb/1 | 0.000832858 | 0.000864485 | 0.000871905 |  |
| plan-sql-s5_nan_semantics/nata/1 | 0.000097063 | 0.000098934 | 0.000100034 | MET |
| plan-sql-s5_nan_semantics/twin/1 | 0.000099339 | 0.000100749 | 0.000102269 |  |
| plan-sql-s5_negative_zero/duckdb/1 | 0.001004156 | 0.001046533 | 0.001079663 |  |
| plan-sql-s5_negative_zero/nata/1 | 0.000095804 | 0.000097849 | 0.000103454 | MET |
| plan-sql-s5_negative_zero/twin/1 | 0.000097319 | 0.000100594 | 0.000103819 |  |
| plan-sql-s5_not_null/duckdb/1 | 0.000710899 | 0.000726415 | 0.000735050 |  |
| plan-sql-s5_not_null/nata/1 | 0.000085119 | 0.000088093 | 0.000093883 | MET |
| plan-sql-s5_not_null/twin/1 | 0.000090734 | 0.000094309 | 0.000123920 |  |
| plan-sql-s5_where_nan/duckdb/1 | 0.000695958 | 0.000773346 | 0.000816828 |  |
| plan-sql-s5_where_nan/nata/1 | 0.000077553 | 0.000080858 | 0.000083443 | MET |
| plan-sql-s5_where_nan/twin/1 | 0.000083019 | 0.000090628 | 0.000098534 |  |
| plan-sql-s5_where_nan_greater/duckdb/1 | 0.000701913 | 0.000737724 | 0.000750055 |  |
| plan-sql-s5_where_nan_greater/nata/1 | 0.000078203 | 0.000080418 | 0.000081739 | MET |
| plan-sql-s5_where_nan_greater/twin/1 | 0.000078958 | 0.000096294 | 0.000110275 |  |
| plan-sql-s5_where_null_drops/duckdb/1 | 0.000754730 | 0.000771291 | 0.000782757 |  |
| plan-sql-s5_where_null_drops/nata/1 | 0.000076833 | 0.000079948 | 0.000102389 | MET |
| plan-sql-s5_where_null_drops/twin/1 | 0.000080268 | 0.000082533 | 0.000089208 |  |
| plan-sql-s5_where_or_null/duckdb/1 | 0.000713419 | 0.000763211 | 0.000806572 |  |
| plan-sql-s5_where_or_null/nata/1 | 0.000077248 | 0.000079453 | 0.000080893 | MET |
| plan-sql-s5_where_or_null/twin/1 | 0.000077888 | 0.000080403 | 0.000098979 |  |
| plan-sql-s5_where_x_eq_null/duckdb/1 | 0.000548147 | 0.000551458 | 0.000564123 |  |
| plan-sql-s5_where_x_eq_null/nata/1 | 0.000075988 | 0.000078158 | 0.000083629 | MET |
| plan-sql-s5_where_x_eq_null/twin/1 | 0.000075513 | 0.000076103 | 0.000089489 |  |
| plan-sql-s5_where_x_ne_null/duckdb/1 | 0.000725965 | 0.000746505 | 0.000776482 |  |
| plan-sql-s5_where_x_ne_null/nata/1 | 0.000078289 | 0.000082318 | 0.000083179 | MET |
| plan-sql-s5_where_x_ne_null/twin/1 | 0.000078918 | 0.000081134 | 0.000104509 |  |
| plan-sql-s6_case_dates_strings/duckdb/1 | 0.000739860 | 0.000754620 | 0.000766626 |  |
| plan-sql-s6_case_dates_strings/nata/1 | 0.000085259 | 0.000086673 | 0.000092424 | MET |
| plan-sql-s6_case_dates_strings/twin/1 | 0.000088328 | 0.000088754 | 0.000093014 |  |
| plan-sql-s6_case_double/duckdb/1 | 0.000726654 | 0.000765531 | 0.000813937 |  |
| plan-sql-s6_case_double/nata/1 | 0.000084494 | 0.000088483 | 0.000090884 | MET |
| plan-sql-s6_case_double/twin/1 | 0.000087858 | 0.000093679 | 0.000113730 |  |
| plan-sql-s6_case_else/duckdb/1 | 0.000704668 | 0.000729624 | 0.000737845 |  |
| plan-sql-s6_case_else/nata/1 | 0.000081023 | 0.000085539 | 0.000086458 | MET |
| plan-sql-s6_case_else/twin/1 | 0.000084453 | 0.000086238 | 0.000094808 |  |
| plan-sql-s6_case_guard_overflow/duckdb/1 | 0.000839523 | 0.000858079 | 0.000871025 |  |
| plan-sql-s6_case_guard_overflow/nata/1 | 0.000087049 | 0.000088683 | 0.000093899 | MET |
| plan-sql-s6_case_guard_overflow/twin/1 | 0.000091399 | 0.000094533 | 0.000129346 |  |
| plan-sql-s6_case_in_where/duckdb/1 | 0.000837464 | 0.000881005 | 0.000892420 |  |
| plan-sql-s6_case_in_where/nata/1 | 0.000083978 | 0.000086239 | 0.000092673 | MET |
| plan-sql-s6_case_in_where/twin/1 | 0.000086519 | 0.000091024 | 0.000103030 |  |
| plan-sql-s6_case_nested/duckdb/1 | 0.000768966 | 0.000788191 | 0.000799067 |  |
| plan-sql-s6_case_nested/nata/1 | 0.000087289 | 0.000089818 | 0.000091459 | MET |
| plan-sql-s6_case_nested/twin/1 | 0.000091178 | 0.000093844 | 0.000100579 |  |
| plan-sql-s6_case_null_condition_falls_through/duckdb/1 | 0.000676517 | 0.000689238 | 0.000708628 |  |
| plan-sql-s6_case_null_condition_falls_through/nata/1 | 0.000084694 | 0.000085568 | 0.000090793 | MET |
| plan-sql-s6_case_null_condition_falls_through/twin/1 | 0.000084009 | 0.000088488 | 0.000095399 |  |
| plan-sql-s6_case_only_taken_branch_evaluated/duckdb/1 | 0.000807817 | 0.000830808 | 0.000864704 |  |
| plan-sql-s6_case_only_taken_branch_evaluated/nata/1 | 0.000087659 | 0.000089368 | 0.000092954 | MET |
| plan-sql-s6_case_only_taken_branch_evaluated/twin/1 | 0.000088069 | 0.000095669 | 0.000107924 |  |
| plan-sql-s6_case_searched/duckdb/1 | 0.000771321 | 0.000797602 | 0.000829833 |  |
| plan-sql-s6_case_searched/nata/1 | 0.000087753 | 0.000090009 | 0.000098044 | MET |
| plan-sql-s6_case_searched/twin/1 | 0.000091089 | 0.000093374 | 0.000104014 |  |
| plan-sql-s6_case_simple/duckdb/1 | 0.000716533 | 0.000746619 | 0.000771906 |  |
| plan-sql-s6_case_simple/nata/1 | 0.000085999 | 0.000088014 | 0.000091253 | MET |
| plan-sql-s6_case_simple/twin/1 | 0.000085634 | 0.000089318 | 0.000107199 |  |
| plan-sql-s6_case_simple_null_operand/duckdb/1 | 0.000725859 | 0.000760771 | 0.000783586 |  |
| plan-sql-s6_case_simple_null_operand/nata/1 | 0.000088873 | 0.000091769 | 0.000098134 | MET |
| plan-sql-s6_case_simple_null_operand/twin/1 | 0.000094484 | 0.000098184 | 0.000121465 |  |
| plan-sql-s6_case_untaken_branch_but_taken_row_errors/duckdb/1 | 0.000769171 | 0.000816437 | 0.000836318 |  |
| plan-sql-s6_case_untaken_branch_but_taken_row_errors/nata/1 | 0.000081843 | 0.000083893 | 0.000087889 | MET |
| plan-sql-s6_case_untaken_branch_but_taken_row_errors/twin/1 | 0.000085003 | 0.000088044 | 0.000117030 |  |
| plan-sql-s6_case_widens_decimals/duckdb/1 | 0.000824857 | 0.000835093 | 0.000849059 |  |
| plan-sql-s6_case_widens_decimals/nata/1 | 0.000087789 | 0.000090809 | 0.000095189 | MET |
| plan-sql-s6_case_widens_decimals/twin/1 | 0.000093479 | 0.000096514 | 0.000111420 |  |
| plan-sql-s6_case_widens_ints/duckdb/1 | 0.000779041 | 0.000782121 | 0.000792771 |  |
| plan-sql-s6_case_widens_ints/nata/1 | 0.000081738 | 0.000083243 | 0.000090049 | MET |
| plan-sql-s6_case_widens_ints/twin/1 | 0.000083019 | 0.000092363 | 0.000107765 |  |
| plan-sql-s6_cast_bad_date/duckdb/1 | 0.000646426 | 0.000678877 | 0.000688953 |  |
| plan-sql-s6_cast_bad_date/nata/1 | 0.000073563 | 0.000076738 | 0.000077548 | MET |
| plan-sql-s6_cast_bad_date/twin/1 | 0.000076628 | 0.000081048 | 0.000085849 |  |
| plan-sql-s6_cast_decimal_default_scale/duckdb/1 | 0.000696672 | 0.000728934 | 0.000739469 |  |
| plan-sql-s6_cast_decimal_default_scale/nata/1 | 0.000081073 | 0.000082223 | 0.000084323 | MET |
| plan-sql-s6_cast_decimal_default_scale/twin/1 | 0.000084133 | 0.000084948 | 0.000086913 |  |
| plan-sql-s6_cast_decimal_does_not_fit/duckdb/1 | 0.000818033 | 0.000823978 | 0.000832583 |  |
| plan-sql-s6_cast_decimal_does_not_fit/nata/1 | 0.000081964 | 0.000083228 | 0.000086713 | MET |
| plan-sql-s6_cast_decimal_does_not_fit/twin/1 | 0.000082773 | 0.000085154 | 0.000101274 |  |
| plan-sql-s6_cast_decimal_to_decimal/duckdb/1 | 0.001258975 | 0.001276580 | 0.001287406 |  |
| plan-sql-s6_cast_decimal_to_decimal/nata/1 | 0.000091524 | 0.000092933 | 0.000095009 | MET |
| plan-sql-s6_cast_decimal_to_decimal/twin/1 | 0.000094694 | 0.000095699 | 0.000160997 |  |
| plan-sql-s6_cast_decimal_to_double/duckdb/1 | 0.000710333 | 0.000716938 | 0.000754030 |  |
| plan-sql-s6_cast_decimal_to_double/nata/1 | 0.000080088 | 0.000082833 | 0.000083558 | MET |
| plan-sql-s6_cast_decimal_to_double/twin/1 | 0.000080094 | 0.000081133 | 0.000088899 |  |
| plan-sql-s6_cast_decimal_to_int_rounds/duckdb/1 | 0.001275076 | 0.001277045 | 0.001327632 |  |
| plan-sql-s6_cast_decimal_to_int_rounds/nata/1 | 0.000090824 | 0.000093284 | 0.000096714 | MET |
| plan-sql-s6_cast_decimal_to_int_rounds/twin/1 | 0.000097074 | 0.000098594 | 0.000101844 |  |
| plan-sql-s6_cast_double_to_decimal_exact_values/duckdb/1 | 0.001247319 | 0.001280590 | 0.001291725 |  |
| plan-sql-s6_cast_double_to_decimal_exact_values/nata/1 | 0.000085793 | 0.000088958 | 0.000101068 | MET |
| plan-sql-s6_cast_double_to_decimal_exact_values/twin/1 | 0.000089823 | 0.000092834 | 0.000106564 |  |
| plan-sql-s6_cast_double_to_int_exact_values/duckdb/1 | 0.001174521 | 0.001183126 | 0.001194142 |  |
| plan-sql-s6_cast_double_to_int_exact_values/nata/1 | 0.000085863 | 0.000087764 | 0.000089938 | MET |
| plan-sql-s6_cast_double_to_int_exact_values/twin/1 | 0.000089619 | 0.000090138 | 0.000092339 |  |
| plan-sql-s6_cast_double_too_big_for_decimal/duckdb/1 | 0.000806447 | 0.000824152 | 0.000833263 |  |
| plan-sql-s6_cast_double_too_big_for_decimal/nata/1 | 0.000080658 | 0.000082153 | 0.000086374 | MET |
| plan-sql-s6_cast_double_too_big_for_decimal/twin/1 | 0.000082189 | 0.000086013 | 0.000120914 |  |
| plan-sql-s6_cast_from_varchar/duckdb/1 | 0.000884715 | 0.000895301 | 0.000949478 |  |
| plan-sql-s6_cast_from_varchar/nata/1 | 0.000105589 | 0.000107854 | 0.000110345 | MET |
| plan-sql-s6_cast_from_varchar/twin/1 | 0.000112534 | 0.000115880 | 0.000121500 |  |
| plan-sql-s6_cast_half_away_from_zero/duckdb/1 | 0.001201907 | 0.001252834 | 0.001280446 |  |
| plan-sql-s6_cast_half_away_from_zero/nata/1 | 0.000109554 | 0.000111014 | 0.000118550 | MET |
| plan-sql-s6_cast_half_away_from_zero/twin/1 | 0.000111569 | 0.000113019 | 0.000125174 |  |
| plan-sql-s6_cast_in_where/duckdb/1 | 0.000881244 | 0.000890809 | 0.000906136 |  |
| plan-sql-s6_cast_in_where/nata/1 | 0.000084949 | 0.000086359 | 0.000093129 | MET |
| plan-sql-s6_cast_in_where/twin/1 | 0.000083898 | 0.000088753 | 0.000118435 |  |
| plan-sql-s6_cast_inf_to_integer/duckdb/1 | 0.000736908 | 0.000759685 | 0.000790701 |  |
| plan-sql-s6_cast_inf_to_integer/nata/1 | 0.000078228 | 0.000080673 | 0.000081808 | MET |
| plan-sql-s6_cast_inf_to_integer/twin/1 | 0.000081463 | 0.000082563 | 0.000114729 |  |
| plan-sql-s6_cast_int_narrow_fails/duckdb/1 | 0.000600619 | 0.000618144 | 0.000640455 |  |
| plan-sql-s6_cast_int_narrow_fails/nata/1 | 0.000074113 | 0.000075508 | 0.000077533 | MET |
| plan-sql-s6_cast_int_narrow_fails/twin/1 | 0.000075728 | 0.000079928 | 0.000082864 |  |
| plan-sql-s6_cast_int_narrow_ok/duckdb/1 | 0.000952817 | 0.000966523 | 0.000987474 |  |
| plan-sql-s6_cast_int_narrow_ok/nata/1 | 0.000085089 | 0.000085778 | 0.000088443 | MET |
| plan-sql-s6_cast_int_narrow_ok/twin/1 | 0.000085843 | 0.000093573 | 0.000117370 |  |
| plan-sql-s6_cast_int_to_decimal/duckdb/1 | 0.001222898 | 0.001267509 | 0.001294341 |  |
| plan-sql-s6_cast_int_to_decimal/nata/1 | 0.000091628 | 0.000094274 | 0.000095629 | MET |
| plan-sql-s6_cast_int_to_decimal/twin/1 | 0.000091819 | 0.000098068 | 0.000113280 |  |
| plan-sql-s6_cast_int_to_decimal_overflow/duckdb/1 | 0.000830823 | 0.000836658 | 0.000856673 |  |
| plan-sql-s6_cast_int_to_decimal_overflow/nata/1 | 0.000080333 | 0.000081518 | 0.000085858 | MET |
| plan-sql-s6_cast_int_to_decimal_overflow/twin/1 | 0.000083083 | 0.000086413 | 0.000104495 |  |
| plan-sql-s6_cast_int_to_double/duckdb/1 | 0.000738034 | 0.000748619 | 0.000760480 |  |
| plan-sql-s6_cast_int_to_double/nata/1 | 0.000079248 | 0.000083063 | 0.000084188 | MET |
| plan-sql-s6_cast_int_to_double/twin/1 | 0.000080308 | 0.000081353 | 0.000087163 |  |
| plan-sql-s6_cast_int_widen/duckdb/1 | 0.000792831 | 0.000805311 | 0.000819572 |  |
| plan-sql-s6_cast_int_widen/nata/1 | 0.000085743 | 0.000086183 | 0.000087388 | MET |
| plan-sql-s6_cast_int_widen/twin/1 | 0.000088108 | 0.000093024 | 0.000099279 |  |
| plan-sql-s6_cast_nan_to_decimal/duckdb/1 | 0.000796265 | 0.000817531 | 0.000848838 |  |
| plan-sql-s6_cast_nan_to_decimal/nata/1 | 0.000079713 | 0.000081543 | 0.000086898 | MET |
| plan-sql-s6_cast_nan_to_decimal/twin/1 | 0.000079533 | 0.000083918 | 0.000088273 |  |
| plan-sql-s6_cast_not_a_number/duckdb/1 | 0.000604849 | 0.000625639 | 0.000640364 |  |
| plan-sql-s6_cast_not_a_number/nata/1 | 0.000075448 | 0.000075882 | 0.000083268 | MET |
| plan-sql-s6_cast_not_a_number/twin/1 | 0.000073573 | 0.000076693 | 0.000078923 |  |
| plan-sql-s6_cast_null/duckdb/1 | 0.000647560 | 0.000658006 | 0.000669806 |  |
| plan-sql-s6_cast_null/nata/1 | 0.000078143 | 0.000079163 | 0.000083843 | MET |
| plan-sql-s6_cast_null/twin/1 | 0.000078218 | 0.000080773 | 0.000084733 |  |
| plan-sql-s6_cast_real_to_double/duckdb/1 | 0.001231297 | 0.001270424 | 0.001286580 |  |
| plan-sql-s6_cast_real_to_double/nata/1 | 0.000087228 | 0.000088153 | 0.000096189 | MET |
| plan-sql-s6_cast_real_to_double/twin/1 | 0.000090649 | 0.000092463 | 0.000119850 |  |
| plan-sql-s6_cast_string_overflows_int/duckdb/1 | 0.000595353 | 0.000601223 | 0.000616129 |  |
| plan-sql-s6_cast_string_overflows_int/nata/1 | 0.000073302 | 0.000074893 | 0.000077438 | MET |
| plan-sql-s6_cast_string_overflows_int/twin/1 | 0.000075773 | 0.000076483 | 0.000078588 |  |
| plan-sql-s6_cast_string_to_decimal_does_not_fit/duckdb/1 | 0.000666031 | 0.000671696 | 0.000711017 |  |
| plan-sql-s6_cast_string_to_decimal_does_not_fit/nata/1 | 0.000073088 | 0.000075418 | 0.000078188 | MET |
| plan-sql-s6_cast_string_to_decimal_does_not_fit/twin/1 | 0.000076283 | 0.000077038 | 0.000090384 |  |
| plan-sql-s6_cast_to_varchar/duckdb/1 | 0.000940242 | 0.000951591 | 0.000963667 |  |
| plan-sql-s6_cast_to_varchar/nata/1 | 0.000102209 | 0.000103754 | 0.000104634 | MET |
| plan-sql-s6_cast_to_varchar/twin/1 | 0.000105044 | 0.000107624 | 0.000112249 |  |
| plan-sql-s6_cast_varchar_roundtrip/duckdb/1 | 0.000810916 | 0.000826797 | 0.000835017 |  |
| plan-sql-s6_cast_varchar_roundtrip/nata/1 | 0.000086553 | 0.000088213 | 0.000093093 | MET |
| plan-sql-s6_cast_varchar_roundtrip/twin/1 | 0.000085009 | 0.000088804 | 0.000101189 |  |
| plan-sql-s6_double_colon/duckdb/1 | 0.000832847 | 0.000854213 | 0.000891035 |  |
| plan-sql-s6_double_colon/nata/1 | 0.000086219 | 0.000087323 | 0.000089113 | MET |
| plan-sql-s6_double_colon/twin/1 | 0.000086024 | 0.000091653 | 0.000108039 |  |
| plan-sql-s7_big_limit_filter_selective/duckdb/1 | 0.000998703 | 0.001014834 | 0.001036965 |  |
| plan-sql-s7_big_limit_filter_selective/nata/1 | 0.000102369 | 0.000103659 | 0.000104969 | MET |
| plan-sql-s7_big_limit_filter_selective/twin/1 | 0.000100779 | 0.000104069 | 0.000111799 |  |
| plan-sql-s7_big_limit_offset_across_row_groups/duckdb/1 | 0.001050545 | 0.001082267 | 0.001121218 |  |
| plan-sql-s7_big_limit_offset_across_row_groups/nata/1 | 0.000091449 | 0.000093523 | 0.000095689 | MET |
| plan-sql-s7_big_limit_offset_across_row_groups/twin/1 | 0.000095869 | 0.000099663 | 0.000109629 |  |
| plan-sql-s7_big_limit_row_groups/duckdb/1 | 0.000760800 | 0.000784775 | 0.000791285 |  |
| plan-sql-s7_big_limit_row_groups/nata/1 | 0.000088948 | 0.000091333 | 0.000097123 | MET |
| plan-sql-s7_big_limit_row_groups/twin/1 | 0.000090963 | 0.000092153 | 0.000094944 |  |
| plan-sql-s7_limit_0/duckdb/1 | 0.000481114 | 0.000494049 | 0.000513465 |  |
| plan-sql-s7_limit_0/nata/1 | 0.000070508 | 0.000072608 | 0.000075253 | MET |
| plan-sql-s7_limit_0/twin/1 | 0.000071998 | 0.000072918 | 0.000075723 |  |
| plan-sql-s7_limit_1/duckdb/1 | 0.000592933 | 0.000604624 | 0.000616159 |  |
| plan-sql-s7_limit_1/nata/1 | 0.000071378 | 0.000075038 | 0.000077418 | MET |
| plan-sql-s7_limit_1/twin/1 | 0.000072603 | 0.000072783 | 0.000081343 |  |
| plan-sql-s7_limit_100/duckdb/1 | 0.000863977 | 0.000873123 | 0.000946232 |  |
| plan-sql-s7_limit_100/nata/1 | 0.000070452 | 0.000072082 | 0.000073633 | MET |
| plan-sql-s7_limit_100/twin/1 | 0.000073278 | 0.000074693 | 0.000077013 |  |
| plan-sql-s7_limit_3/duckdb/1 | 0.000592237 | 0.000607428 | 0.000612128 |  |
| plan-sql-s7_limit_3/nata/1 | 0.000069857 | 0.000072867 | 0.000073798 | MET |
| plan-sql-s7_limit_3/twin/1 | 0.000076168 | 0.000078278 | 0.000098084 |  |
| plan-sql-s7_limit_after_filter_not_before/duckdb/1 | 0.000744868 | 0.000789880 | 0.000828146 |  |
| plan-sql-s7_limit_after_filter_not_before/nata/1 | 0.000076598 | 0.000079343 | 0.000083248 | MET |
| plan-sql-s7_limit_after_filter_not_before/twin/1 | 0.000079563 | 0.000081538 | 0.000084703 |  |
| plan-sql-s7_limit_expression_projection/duckdb/1 | 0.000893589 | 0.000906235 | 0.000920371 |  |
| plan-sql-s7_limit_expression_projection/nata/1 | 0.000081183 | 0.000081273 | 0.000085758 | MET |
| plan-sql-s7_limit_expression_projection/twin/1 | 0.000085203 | 0.000086738 | 0.000090324 |  |
| plan-sql-s7_limit_filter/duckdb/1 | 0.000756139 | 0.000780240 | 0.000804781 |  |
| plan-sql-s7_limit_filter/nata/1 | 0.000076768 | 0.000078298 | 0.000088978 | MET |
| plan-sql-s7_limit_filter/twin/1 | 0.000078728 | 0.000081433 | 0.000092563 |  |
| plan-sql-s7_limit_offset_beyond/duckdb/1 | 0.000881974 | 0.000943431 | 0.000990203 |  |
| plan-sql-s7_limit_offset_beyond/nata/1 | 0.000072833 | 0.000073128 | 0.000074503 | MET |
| plan-sql-s7_limit_offset_beyond/twin/1 | 0.000073818 | 0.000075663 | 0.000078308 |  |
| plan-sql-s7_limit_offset_inside/duckdb/1 | 0.000902294 | 0.000921056 | 0.000949447 |  |
| plan-sql-s7_limit_offset_inside/nata/1 | 0.000071302 | 0.000072918 | 0.000076248 | MET |
| plan-sql-s7_limit_offset_inside/twin/1 | 0.000073132 | 0.000078848 | 0.000085904 |  |
| plan-sql-s7_limit_offset_to_the_end/duckdb/1 | 0.000902690 | 0.000927630 | 0.000948946 |  |
| plan-sql-s7_limit_offset_to_the_end/nata/1 | 0.000071163 | 0.000073818 | 0.000075243 | MET |
| plan-sql-s7_limit_offset_to_the_end/twin/1 | 0.000073618 | 0.000075038 | 0.000080633 |  |
| plan-sql-s7_limit_zero_offset/duckdb/1 | 0.000485543 | 0.000502679 | 0.000517329 |  |
| plan-sql-s7_limit_zero_offset/nata/1 | 0.000072553 | 0.000075278 | 0.000076638 | MET |
| plan-sql-s7_limit_zero_offset/twin/1 | 0.000073543 | 0.000074888 | 0.000081463 |  |
| plan-sql-s8_case_mixed_types/duckdb/1 | 0.000708882 | 0.000747288 | 0.000772535 |  |
| plan-sql-s8_case_mixed_types/nata/1 | 0.000073908 | 0.000077397 | 0.000079853 | MET |
| plan-sql-s8_case_mixed_types/twin/1 | 0.000079653 | 0.000096504 | 0.000099998 |  |
| plan-sql-s8_missing_file/duckdb/1 | 0.000224399 | 0.000231944 | 0.000243704 |  |
| plan-sql-s8_missing_file/nata/1 | 0.000044902 | 0.000047282 | 0.000048052 | MET |
| plan-sql-s8_missing_file/twin/1 | 0.000045332 | 0.000047806 | 0.000051247 |  |
| plan-sql-s8_negative_limit/duckdb/1 | 0.000322797 | 0.000326288 | 0.000331802 |  |
| plan-sql-s8_negative_limit/nata/1 | 0.000042802 | 0.000044112 | 0.000048307 | MET |
| plan-sql-s8_negative_limit/twin/1 | 0.000042256 | 0.000043337 | 0.000052902 |  |
| plan-sql-s8_syntax/duckdb/1 | 0.000052177 | 0.000052427 | 0.000052972 |  |
| plan-sql-s8_syntax/nata/1 | 0.000037082 | 0.000037606 | 0.000038957 | MET |
| plan-sql-s8_syntax/twin/1 | 0.000035902 | 0.000037627 | 0.000038867 |  |
| plan-sql-s8_type_mismatch_bool_plus_int/duckdb/1 | 0.000448442 | 0.000451282 | 0.000463298 |  |
| plan-sql-s8_type_mismatch_bool_plus_int/nata/1 | 0.000073093 | 0.000074943 | 0.000077947 | MET |
| plan-sql-s8_type_mismatch_bool_plus_int/twin/1 | 0.000074913 | 0.000079523 | 0.000094708 |  |
| plan-sql-s8_unknown_column/duckdb/1 | 0.000302906 | 0.000307136 | 0.000313002 |  |
| plan-sql-s8_unknown_column/nata/1 | 0.000071088 | 0.000073583 | 0.000074588 | MET |
| plan-sql-s8_unknown_column/twin/1 | 0.000072688 | 0.000073973 | 0.000080148 |  |
| plan-sql-tpch_between_dates/duckdb/1 | 0.002485184 | 0.002558377 | 0.002639996 |  |
| plan-sql-tpch_between_dates/nata/1 | 0.000310532 | 0.000326453 | 0.000370574 | MET |
| plan-sql-tpch_between_dates/twin/1 | 0.000339618 | 0.000377215 | 0.000391325 |  |
| plan-sql-tpch_case/duckdb/1 | 0.002461529 | 0.002498845 | 0.002539451 |  |
| plan-sql-tpch_case/nata/1 | 0.000320127 | 0.000329142 | 0.000347503 | MET |
| plan-sql-tpch_case/twin/1 | 0.000329712 | 0.000335863 | 0.000362789 |  |
| plan-sql-tpch_dates_strings/duckdb/1 | 0.002183287 | 0.002280606 | 0.002355794 |  |
| plan-sql-tpch_dates_strings/nata/1 | 0.000320112 | 0.000325747 | 0.000391895 | MET |
| plan-sql-tpch_dates_strings/twin/1 | 0.000311892 | 0.000343963 | 0.000351708 |  |
| plan-sql-tpch_example/duckdb/1 | 0.002650904 | 0.002688382 | 0.002770315 |  |
| plan-sql-tpch_example/nata/1 | 0.000317932 | 0.000321537 | 0.000329257 | MET |
| plan-sql-tpch_example/twin/1 | 0.000329158 | 0.000339073 | 0.000394340 |  |
| plan-sql-tpch_lineitem_all_columns_limit/duckdb/1 | 0.002655464 | 0.002685085 | 0.002725797 |  |
| plan-sql-tpch_lineitem_all_columns_limit/nata/1 | 0.000298966 | 0.000308566 | 0.000363159 | MET |
| plan-sql-tpch_lineitem_all_columns_limit/twin/1 | 0.000303696 | 0.000311967 | 0.000356863 |  |
| plan-sql-tpch_nation_region/duckdb/1 | 0.000748788 | 0.000759078 | 0.000815071 |  |
| plan-sql-tpch_nation_region/nata/1 | 0.000066507 | 0.000069913 | 0.000072563 | MET |
| plan-sql-tpch_nation_region/twin/1 | 0.000069548 | 0.000072598 | 0.000091414 |  |
| plan-sql-tpch_orders_filter/duckdb/1 | 0.001109801 | 0.001182939 | 0.001198495 |  |
| plan-sql-tpch_orders_filter/nata/1 | 0.000109679 | 0.000110899 | 0.000113504 | MET |
| plan-sql-tpch_orders_filter/twin/1 | 0.000111715 | 0.000114499 | 0.000125210 |  |
| plan-sql-tpch_q1_rows/duckdb/1 | 0.002486399 | 0.002559826 | 0.002616953 |  |
| plan-sql-tpch_q1_rows/nata/1 | 0.000311886 | 0.000328422 | 0.000364628 | MET |
| plan-sql-tpch_q1_rows/twin/1 | 0.000319337 | 0.000335893 | 0.000416746 |  |
| plan-sql-tpch_q6_rows/duckdb/1 | 0.002670510 | 0.002699660 | 0.002734822 |  |
| plan-sql-tpch_q6_rows/nata/1 | 0.000316632 | 0.000342203 | 0.000352653 | MET |
| plan-sql-tpch_q6_rows/twin/1 | 0.000327072 | 0.000334938 | 0.000407720 |  |
| resource-sf10/scan/8 | 2.214505441 | 2.214505441 | 2.214505441 |  |
| resource-sf10/sql/8 | 2.212910518 | 2.212910518 | 2.212910518 |  |
| scale-rendered-sf10/sql/1 | 2.232460645 | 2.234174576 | 2.238893166 |  |
| scale-rendered-sf10/sql/8 | 2.210401111 | 2.211430078 | 2.215538047 |  |
| scale-sf10/sql/1 | 0.340432294 | 0.343459852 | 0.348588878 |  |
| scale-sf10/sql/8 | 0.067876574 | 0.068653863 | 0.068844811 |  |
| scale-sf10/twin/1 | 0.339530349 | 0.343756984 | 0.344328302 |  |
| scale-sf10/twin/8 | 0.060235264 | 0.065911863 | 0.068834395 |  |
| sf1/duckdb/1 | 0.040161604 | 0.040332228 | 0.041345245 |  |
| sf1/duckdb/8 | 0.048762365 | 0.048925442 | 0.050572607 |  |
| sf1/scan/1 | 0.008058350 | 0.008112381 | 0.008186473 |  |
| sf1/scan/8 | 0.012508201 | 0.013301662 | 0.013819546 |  |
| sf1/sql/1 | 0.008887471 | 0.009148959 | 0.009429386 |  |
| sf1/sql/8 | 0.013184089 | 0.014009602 | 0.015160993 |  |
| sf1/twin/1 | 0.007741321 | 0.008092741 | 0.008259105 |  |
| sf1/twin/8 | 0.012929412 | 0.013242991 | 0.013658931 |  |
| sf10/duckdb/1 | 0.049663506 | 0.051300994 | 0.051875893 |  |
| sf10/duckdb/8 | 0.056845075 | 0.059707453 | 0.059989666 |  |
| sf10/scan/1 | 0.010640446 | 0.010965482 | 0.011367909 |  |
| sf10/scan/8 | 0.014463661 | 0.014859255 | 0.015228340 |  |
| sf10/sql/1 | 0.014482141 | 0.014925579 | 0.015208932 |  |
| sf10/sql/8 | 0.015487295 | 0.018896804 | 0.019195256 |  |
| sf10/twin/1 | 0.010651798 | 0.010881392 | 0.011172356 |  |
| sf10/twin/8 | 0.014615130 | 0.014949126 | 0.015215861 |  |
| sorted-r2/sql/8 | 0.011171782 | 0.011171782 | 0.011171782 |  |
| startup/duckdb/1 | 0.033205756 | 0.033878489 | 0.040905824 |  |
| startup/nata/1 | 0.001329897 | 0.001370927 | 0.001574178 |  |

sf1 threads 1: SQL/scan 1.1278 (threshold <=1.2): MET; twin/scan 0.9976.

sf1 threads 8: SQL/scan 1.0532 (threshold <=1.2): MET; twin/scan 0.9956.

sf10 threads 1: SQL/scan 1.3611 (threshold <=1.2): NOT MET; twin/scan 0.9923.

sf10 threads 8: SQL/scan 1.2717 (threshold <=1.2): NOT MET; twin/scan 1.0060.

SF10 full SQL speedup 5.0028 (threshold >=5): MET. EXPLAIN ANALYZE executes every row without rendering; rendered SQL is also measured.

files10000: {'block': 'files10000', 'arm': 'sql', 'threads': 8, 'round': 0, 'secs': 0.09640479199993024, 'peak_rss_kib': '11080', 'peak_open_files': '8', 'peak_footers': '13'}

resource-sf10: {'block': 'resource-sf10', 'arm': 'sql', 'threads': 8, 'round': 0, 'secs': 2.212910517999944, 'peak_rss_kib': '67352', 'peak_open_files': '8', 'peak_footers': '1'}
