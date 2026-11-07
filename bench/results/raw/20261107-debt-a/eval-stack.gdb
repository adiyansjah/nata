set pagination off
set confirm off
set debuginfod enabled off
set environment ASAN_OPTIONS detect_stack_use_after_return=0:detect_leaks=0
set $count = 0
break _RNvMNtCs1dAlT12grQL_9nata_exec4evalNtB2_9Evaluator4eval
commands
silent
set $count = $count + 1
if $count == 100
set $base = $rsp
end
if $count == 1000
printf "eval levels=900 delta=%ld bytes_per_level=%ld\n", $base-$rsp, ($base-$rsp)/900
quit
end
continue
end
run --exact an_operator_chain_of_10000_terms_is_evaluated_on_a_2_mb_thread_for_each_operator
