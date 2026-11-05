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
step() { echo "STEP $1"; [ "$1" = ac-all ] && echo "ARMTASKS=${ARMCHECK_TASKS:-<all>}"; [ "$1" = ip-all ] && echo "IPONLY=${IP_ONLY:-<all phases>}"; case "$1" in ic-all|ic-model|ic-spikes) echo "ICMODE[$1]=${IC_MODE:-<full>}";; esac; return 0; }
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
# icebergmodel: the targeted TLC re-run. It is ONE step (ic-model) that runs gcp-check.sh in model mode, which has only the tools, model-lint,
# model-parse-and-edits and model-full phases; it must not select the full check's step (ic-all), any other job's steps, or the full mode.
im=$(steps icebergmodel); ic=$(steps iceberg)
has ic-model "$im" && echo "$im" | grep -q '^ICMODE\[ic-model\]=model$' && ok "icebergmodel runs the one model-mode step" || bad "icebergmodel: $(echo "$im" | grep -E 'STEP ic|ICMODE' | tr '\n' ' ')"
has ic-all "$im" && bad "icebergmodel must not run the full iceberg step ic-all" || ok "icebergmodel does not run the full iceberg check"
extra=$(echo "$im" | grep '^STEP' | grep -v -E '^STEP (setup-rust|setup-duckdb|machine-stubboot|ic-model)$' | tr '\n' ' ')
[ -z "$extra" ] && ok "icebergmodel runs no step of any other job (only the common setup and ic-model)" || bad "icebergmodel runs extra steps: $extra"
has ic-all "$ic" && echo "$ic" | grep -q '^ICMODE\[ic-all\]=<full>$' && ! has ic-model "$ic" && ok "iceberg still runs the full check (IC_MODE unset), not the model-only step" || bad "iceberg: $(echo "$ic" | grep -E 'STEP ic|ICMODE' | tr '\n' ' ')"
# netexchsim: ONE step (nx-all) that runs spikes/net-exchange/gcp-check.sh; no settle steps, nothing of another job.
nx=$(steps netexchsim)
has nx-all "$nx" && ok "netexchsim runs the one step nx-all" || bad "netexchsim: $(echo "$nx" | grep '^STEP' | tr '\n' ' ')"
extra=$(echo "$nx" | grep '^STEP' | grep -v -E '^STEP (setup-rust|setup-duckdb|machine-stubboot|nx-all)$' | tr '\n' ' ')
[ -z "$extra" ] && ok "netexchsim runs no step of any other job" || bad "netexchsim runs extra steps: $extra"
# the VM must be prepared for it as root: the job runs as `nata`
VS="$HERE/vm-startup.sh"
grep -q 'netexchsim) ;; \*) false ;; esac' "$VS" && grep -q 'nata ALL=(ALL) NOPASSWD:ALL' "$VS" && grep -q 'modprobe sch_netem' "$VS" && grep -q 'iproute2 ethtool' "$VS" && ok "vm-startup.sh prepares the VM for netexchsim (iproute2, ethtool, sch_netem, sudo for nata)" || bad "vm-startup.sh does not prepare the VM for netexchsim"
# icebergspikes: ONE step (ic-spikes) in IC_MODE=spikes (no model); it must not select the full or the model step, nor any step of another job.
isp=$(steps icebergspikes)
has ic-spikes "$isp" && echo "$isp" | grep -q '^ICMODE\[ic-spikes\]=spikes$' && ok "icebergspikes runs the one spikes-mode step" || bad "icebergspikes: $(echo "$isp" | grep -E 'STEP ic|ICMODE' | tr '\n' ' ')"
has ic-all "$isp" && bad "icebergspikes must not run the full iceberg step" || ok "icebergspikes does not run the full check"
has ic-model "$isp" && bad "icebergspikes must not run the model step" || ok "icebergspikes does not run the model step"
extra=$(echo "$isp" | grep '^STEP' | grep -v -E '^STEP (setup-rust|setup-duckdb|machine-stubboot|ic-spikes)$' | tr '\n' ' ')
[ -z "$extra" ] && ok "icebergspikes runs no step of any other job" || bad "icebergspikes runs extra steps: $extra"
# the settle step: in the timing jobs every timed step is preceded by a wait for a quiet machine; the other jobs do not have it
idx() { echo "$2" | grep -n "^STEP $1\$" | head -1 | cut -d: -f1; }
t=$(steps timing)
for pair in "settle-h-bench-ov-on:h-bench-ov-on" "settle-h-membench-ov-off:h-membench-ov-off" "settle-h-duckdb:h-duckdb" "settle-np-all:np-all" "settle-p-timing:p-timing" "settle-t-compaction:t-compaction" "settle-t-pushcost:t-pushcost" "settle-t-ap-bench:t-ap-bench"; do
  s=${pair%%:*}; r=${pair##*:}; a=$(idx $s "$t"); b=$(idx $r "$t")
  [ -n "$a" ] && [ -n "$b" ] && [ "$a" -lt "$b" ] && ok "timing: $s comes before $r" || bad "timing: $s ($a) must come before $r ($b)"
done
for j in hashops armcheck simharness objscan iceberg icebergmodel icebergspikes netexchsim; do steps $j | grep -q '^STEP settle-' && bad "job $j must not have settle steps" || ok "job $j has no settle steps"; done
steps perf | grep -q '^STEP settle-p-timing$' && ok "perf alone settles before p-timing" || bad "perf alone has no settle step"
# the settle step of the job: it runs scripts/gcp/settle.sh (tested in test-settle.sh) and must NEVER fail the job, even when the machine does
# not settle (the other results are kept; the NOT SETTLED line is the record). The real command is cut out of job.sh and run with a short wait.
C=$(sed -n "s/^settle_before() {.*run \"settle-\$1\" bash -c '\([^']*\)' \"\$1\".*\$/\1/p" "$HERE/job.sh")
[ -n "$C" ] || bad "could not extract the settle command from job.sh"
echo "0.05 0.10 0.10 1/100 1" > "$T/quiet"; echo "9.00 5.00 2.00 1/100 1" > "$T/busy"
out=$(SRC="$(dirname "$(dirname "$HERE")")" SETTLE_LOADAVG="$T/quiet" bash -c "${C/ 600;/ 600;}" lbl 2>&1); echo "$out" | grep -q '^SETTLE lbl: settled, load 0.05 after 0 s' && ok "settle step: a quiet machine settles at once" || bad "settle quiet: $out"
out=$(SRC="$(dirname "$(dirname "$HERE")")" SETTLE_LOADAVG="$T/busy" SETTLE_POLL=1 bash -c "${C/ 600;/ 1;}" lbl 2>&1); rc=$?; echo "$out" | grep -q '^SETTLE lbl: NOT SETTLED, load 9.00' && [ $rc = 0 ] && ok "settle step: a busy machine is given up on cleanly (exit 0, says NOT SETTLED)" || bad "settle busy rc=$rc: $out"
u=$(steps nosuchjob); echo "$u" | grep -q 'FAILJOB unknown job' && ok "an unknown job is refused" || bad "unknown job accepted: $u"
[ "$fail" = 0 ] && echo "all job-chain tests passed" || { echo "SOME TESTS FAILED"; exit 1; }
