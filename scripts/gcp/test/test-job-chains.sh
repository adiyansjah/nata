#!/usr/bin/env bash
# Which steps does each job select? job.sh is run with a stub lib.sh whose `step` only prints the step name, so the
# `case` structure (fall-through blocks ;;& and the no-op patterns before the `*)` catch-all) is tested without a VM.
# Catches: a registered job that ends in "unknown job", a block that is skipped or run twice, a chained bundle that
# contains the wrong steps.
set -u
HERE=$(cd "$(dirname "$0")/.." && pwd); T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
cp "$HERE/job.sh" "$T/job.sh"
cat > "$T/lib.sh" <<'LIB'
: "${WORK:?}"; RESULTS="$WORK/results"; mkdir -p "$RESULTS"
now() { date -u +%H:%M:%S; }; boot_id() { echo stubboot0; }
step() { echo "STEP $1"; [ "$1" = ac-all ] && echo "ARMTASKS=${ARMCHECK_TASKS:-<all>}"; [ "$1" = ip-all ] && echo "IPONLY=${IP_ONLY:-<all phases>}"; return 0; }
fail_job() { echo "FAILJOB $1"; }
finish_job() { echo "FINISHED"; }
LIB
fail=0; ok() { echo "ok   $1"; }; bad() { echo "FAIL $1"; fail=1; }
steps() { mkdir -p "$T/w"; echo "$1" > "$T/w/JOBNAME"; WORK="$T/w" bash "$T/job.sh" 2>&1; }
# every job the orchestrator accepts must reach FINISHED and never FAILJOB
jobs=$(sed -n 's/.*case "\$JOB" in \(.*\)) ;; \*) die.*/\1/p' "$HERE/orchestrate.sh" | tr '|' ' ')
[ -n "$jobs" ] || bad "could not read the job list from orchestrate.sh"
for j in $jobs; do
  out=$(steps "$j")
  echo "$out" | grep -q '^FINISHED$' && ! echo "$out" | grep -q FAILJOB && ok "job $j finishes" || bad "job $j: $(echo "$out" | grep -E 'FAILJOB|FINISHED' | tr '\n' ' ')"
  dup=$(echo "$out" | grep '^STEP' | sort | uniq -d | head -3 | tr '\n' ' ')
  [ -z "$dup" ] || bad "job $j runs a step twice: $dup"
done
has() { echo "$2" | grep -q "^STEP $1\$"; }
t=$(steps timing); h=$(steps hashops)
has h-bench-ov-on "$t" && has np-all "$t" && has p-timing "$t" && has t-compaction "$t" && has t-pushcost "$t" && has t-ap-bench "$t" && ok "timing chains hashops, nullpath, perf and its own steps" || bad "timing is missing a block"
has h-mutate "$t" && bad "timing must not run the hash-ops mutation step" || ok "timing skips the hash-ops mutation step"
has h-mutate "$h" && ok "hashops alone still runs the mutation step" || bad "hashops lost its mutation step"
[ "$(echo "$t" | grep -n '^STEP h-nullcheck$' | cut -d: -f1)" -lt "$(echo "$t" | grep -n '^STEP h-bench-ov-on$' | cut -d: -f1)" ] && ok "in timing, correctness steps come before the benchmarks" || bad "order"
n=$(steps nullpath); has np-all "$n" && ! has h-build "$n" && ok "nullpath alone runs only its own steps" || bad "nullpath alone: $(echo "$n" | grep STEP | tr '\n' ' ')"
p=$(steps perf); has p-timing "$p" && ! has np-all "$p" && ok "perf alone runs only its own steps" || bad "perf alone"
a=$(steps armcheck); has ac-all "$a" && ! has np-all "$a" && ok "armcheck runs only its own steps" || bad "armcheck"
r=$(steps armcheck-retry); echo "$r" | grep -q '^ARMTASKS=exloom-mutants hash-layers nullmut data-shapes arrow-mutants$' && ok "armcheck-retry runs only the five tasks that did not finish" || bad "armcheck-retry tasks: $(echo "$r" | grep ARMTASKS)"
echo "$a" | grep -q '^ARMTASKS=<all>$' && ok "armcheck runs all tasks" || bad "armcheck tasks: $(echo "$a" | grep ARMTASKS)"
pm=$(steps icebergplan-mut); echo "$pm" | grep -q '^IPONLY=mutants$' && ok "icebergplan-mut runs only the mutation phase" || bad "icebergplan-mut: $(echo "$pm" | grep IPONLY)"
pf=$(steps icebergplan); echo "$pf" | grep -q '^IPONLY=<all phases>$' && ok "icebergplan runs all phases" || bad "icebergplan: $(echo "$pf" | grep IPONLY)"
u=$(steps nosuchjob); echo "$u" | grep -q 'FAILJOB unknown job' && ok "an unknown job is refused" || bad "unknown job accepted: $u"
[ "$fail" = 0 ] && echo "all job-chain tests passed" || { echo "SOME TESTS FAILED"; exit 1; }
