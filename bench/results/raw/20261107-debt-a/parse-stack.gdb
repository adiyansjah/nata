set pagination off
set confirm off
set debuginfod enabled off
set environment ASAN_OPTIONS detect_stack_use_after_return=0:detect_leaks=0
set $count = 0
break _RNvMs5_NtCsel552EL8PfK_9sqlparser6parserNtB5_6Parser15parse_case_expr
commands
silent
set $count = $count + 1
if $count == 1
set $base = $rsp
end
if $count == 500
printf "parse levels=499 delta=%ld bytes_per_level=%ld\n", $base-$rsp, ($base-$rsp)/499
quit
end
continue
end
run --exact nesting_of_1000_levels_is_evaluated_on_a_2_mb_thread_for_each_construct
