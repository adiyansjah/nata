#!/bin/bash
# Tests orchestrate.sh against a fake gcloud/ssh that simulate Spot behaviour. Never touches the real gcloud:
# a stub named gcloud (and ssh) sits first in PATH and fails the test if anything calls it by name.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
ORCH=$HERE/../orchestrate.sh
T=$(mktemp -d)
mkdir -p "$T/bin"
for tool in gcloud ssh; do
  printf '#!/bin/bash\necho "REAL %s CALLED: $*" >> "%s/real-calls.log"\nexit 99\n' "$tool" "$T" > "$T/bin/$tool"
  chmod +x "$T/bin/$tool"
done
export PATH="$T/bin:$PATH"
fails=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fails=$((fails + 1)); fi; }

# new_world NAME -> sets F (fake state dir) and O (local output dir)
new_world() { F=$T/$1/fake; O=$T/$1/out; mkdir -p "$F" "$O"; }
orch() { # orch ARGS...  (environment: FAKE_* knobs may be set by the caller)
  env FAKE="$F" GCLOUD="$HERE/fake-gcloud" SSH="$HERE/fake-ssh" OUT="$O" POLL_SECONDS=0.05 PULL_SECONDS=0 \
    CREATE_RETRY_SECONDS=0.05 CLEANUP_RETRY_SECONDS=0.05 MIN_REMAINING_SECONDS=1 timeout 60 "$ORCH" "$@"
}
left() { echo $(( $(ls -d "$F"/vms/*/ 2>/dev/null | wc -l) + $(ls -d "$F"/disks/*/ 2>/dev/null | wc -l) )); }
calls() { local n; n=$(grep -c -- "$1" "$F/calls.log" 2>/dev/null); echo "${n:-0}"; }

# ---------------------------------------------------------------- guards: nothing is called
new_world plan
orch plan --job ci > "$T/plan.out" 2>&1; rc=$?
check "plan exits 0 and prints the worst-case cost" "[ $rc -eq 0 ] && grep -q 'Worst-case cost' $T/plan.out"
check "plan calls nothing" "[ ! -e $F/calls.log ]"
orch plan --job genops > "$T/plan2.out" 2>&1; rc=$?
orch plan --job verify > "$T/plan3.out" 2>&1; rc=$?
for j in hashops kani kani-mut decimal parmut parmut-hashops crates fairness-bench bridge perf cpack; do orch plan --job $j > "$T/plan-$j.out" 2>&1; rc=$?
check "the $j job is a known job" "[ $rc -eq 0 ] && grep -q \"job: $j\" $T/plan-$j.out && [ ! -e $F/calls.log ]"; done
check "the verify job is a known job" "[ $rc -eq 0 ] && grep -q 'job: verify' $T/plan3.out && [ ! -e $F/calls.log ]"
check "the genops job is a known job" "[ $rc -eq 0 ] && grep -q 'job: genops' $T/plan2.out && [ ! -e $F/calls.log ]"
new_world noapproval
orch run --job ci > "$T/o" 2>&1; rc=$?
check "a real run without --approved-usd is refused" "[ $rc -eq 2 ] && grep -q 'approved-usd' $T/o && [ ! -e $F/calls.log ]"
new_world lowapproval
orch run --job ci --approved-usd 0.01 > "$T/o" 2>&1; rc=$?
check "an approval below the worst case is refused" "[ $rc -eq 2 ] && grep -q 'worst case' $T/o && [ ! -e $F/calls.log ]"
new_world mixed
orch run --job ci --approved-usd 5 --machines "n4a-highcpu-8 t2a-standard-8" > "$T/o" 2>&1; rc=$?
check "a chain mixing Hyperdisk and persistent-disk machines is refused" "[ $rc -eq 2 ] && grep -q 'disk class' $T/o && [ ! -e $F/calls.log ]"
new_world oslogin
FAKE_OSLOGIN=TRUE orch run --job ci --approved-usd 5 > "$T/o" 2>&1; rc=$?
check "OS Login enabled: stops before creating anything" "[ $rc -eq 2 ] && [ \$(calls 'disks create') -eq 0 ] && [ \$(left) -eq 0 ]"

# ---------------------------------------------------------------- happy path, and other people's resources
new_world happy
mkdir -p "$F/disks/unrelated-disk" "$F/vms/unrelated-vm"
echo "us-central1-a nata-run=otherrun" > "$F/disks/unrelated-disk.meta"
echo us-central1-a > "$F/vms/unrelated-vm/zone"; echo unrelated-disk > "$F/vms/unrelated-vm/disk"; echo 99 > "$F/vms/unrelated-vm/idx"; echo "nata-run=otherrun" > "$F/vms/unrelated-vm/labels"
orch run --job ci --approved-usd 1 --run-id happy > "$T/o" 2>&1; rc=$?
check "happy path: exit 0 and the job finished" "[ $rc -eq 0 ] && [ -f $O/state/job.finished ] && grep -q 'result: finished' $O/report.txt"
check "happy path: one data disk, one VM" "[ \$(calls 'disks create') -eq 1 ] && [ \$(calls 'instances create') -eq 1 ]"
check "happy path: its own resources are gone" "[ \$(ls -d $F/vms/happy-*/ 2>/dev/null | wc -l) -eq 0 ] && [ ! -d $F/disks/happy-work ]"
check "another run's resources (different label) are untouched" "[ -d $F/disks/unrelated-disk ] && [ -d $F/vms/unrelated-vm ]"
check "create calls carry the safety flags (Spot, DELETE, max-run-duration in seconds, no service account)" \
  "grep 'instances create' $F/calls.log | grep -q -- '--provisioning-model SPOT' && grep 'instances create' $F/calls.log | grep -qE -- '--max-run-duration [0-9]+s' && grep 'instances create' $F/calls.log | grep -q -- '--no-service-account'"
check "the machine used is recorded" "grep -q 'n4a-highcpu-8' $O/report.txt"

# ---------------------------------------------------------------- preemptions: resume on the same data disk
new_world preempt
FAKE_PREEMPT_AFTER="2 2 0" orch run --job ci --approved-usd 1 --run-id pre > "$T/o" 2>&1; rc=$?
check "two preemptions: still finishes" "[ $rc -eq 0 ] && [ -f $O/state/job.finished ]"
check "two preemptions: three VMs on ONE data disk" "[ \$(calls 'instances create') -eq 3 ] && [ \$(calls 'disks create') -eq 1 ]"
check "two preemptions: the source was uploaded once, not after each restart" "[ \$(wc -l < $F/uploads.log) -eq 1 ]"
check "two preemptions: every step is done exactly once" "[ \$(ls $O/state | grep -c '^step-.*\\.done\$') -eq 6 ]"
check "two preemptions: counted in the report, nothing left" "grep -q 'preemptions: 2' $O/report.txt && [ \$(left) -eq 0 ]"
new_world toomany
FAKE_PREEMPT_AFTER="2 2 2 2 2" orch run --job ci --approved-usd 1 --run-id many --max-preemptions 1 > "$T/o" 2>&1; rc=$?
check "too many preemptions: gives up with exit 3" "[ $rc -eq 3 ] && grep -q 'too many preemptions' $O/report.txt"
check "too many preemptions: nothing left behind" "[ \$(left) -eq 0 ]"

# ---------------------------------------------------------------- stock-outs: machine chain and zones
new_world chain
FAKE_FAIL_MACHINES="n4a-highcpu-8" orch run --job ci --approved-usd 1 --run-id chain > "$T/o" 2>&1; rc=$?
check "first machine unavailable: falls back to the second" "[ $rc -eq 0 ] && grep -q 'machines used, in order: c4a-highcpu-8' $O/report.txt && [ \$(calls 'instances create') -eq 2 ]"
new_world zone
FAKE_FAIL_VM_ZONES="us-central1-a" orch run --job ci --approved-usd 1 --run-id zone > "$T/o" 2>&1; rc=$?
check "zone out of capacity: the empty data disk moves to the next zone" "[ $rc -eq 0 ] && [ \$(calls 'disks create') -eq 2 ] && grep -q 'disks delete' $F/calls.log && [ \$(left) -eq 0 ]"
new_world nocreate
FAKE_CREATE_ALWAYS_FAIL=1 orch run --job ci --approved-usd 1 --run-id nocreate --max-vm-hours 1 > "$T/o" 2>&1; rc=$?
check "nothing can be created: exits 3 and leaves nothing (the empty disk is removed)" "[ $rc -eq 3 ] && [ \$(left) -eq 0 ]"

# ---------------------------------------------------------------- failures and ways out
new_world jobfail
FAKE_JOB_FAILS_AT_STEP=3 orch run --job ci --approved-usd 1 --run-id jf > "$T/o" 2>&1; rc=$?
check "a failing job: exit 1, not retried, nothing left" "[ $rc -eq 1 ] && [ -f $O/state/job.failed ] && [ \$(calls 'instances create') -eq 1 ] && [ \$(left) -eq 0 ]"
new_world budget
FAKE_STEPS_PER_POLL=0 POLL_SECONDS=0.3 MAX_VM_HOURS=0.001 orch run --job ci --approved-usd 1 --run-id bud > "$T/o" 2>&1; rc=$?
check "VM-hour budget used up: exit 3, nothing left" "[ $rc -eq 3 ] && grep -q 'time budget' $O/report.txt && [ \$(left) -eq 0 ]"
new_world term
( FAKE_STEPS_PER_POLL=0 FAKE="$F" GCLOUD="$HERE/fake-gcloud" SSH="$HERE/fake-ssh" OUT="$O" POLL_SECONDS=0.3 PULL_SECONDS=0 \
  CREATE_RETRY_SECONDS=0.05 CLEANUP_RETRY_SECONDS=0.05 MIN_REMAINING_SECONDS=1 exec "$ORCH" run --job ci --approved-usd 1 --run-id term > "$T/o" 2>&1 ) &
pid=$!
for _ in $(seq 1 100); do [ "$(calls 'instances create')" -ge 1 ] && break; sleep 0.1; done
kill -TERM "$pid"; wait "$pid"; rc=$?
check "kill -TERM mid-run: cleans up and exits 143" "[ $rc -eq 143 ] && [ \$(left) -eq 0 ] && grep -q 'result: aborted' $O/report.txt"
new_world intr
( FAKE_STEPS_PER_POLL=0 FAKE="$F" GCLOUD="$HERE/fake-gcloud" SSH="$HERE/fake-ssh" OUT="$O" POLL_SECONDS=0.3 PULL_SECONDS=0 \
  CREATE_RETRY_SECONDS=0.05 CLEANUP_RETRY_SECONDS=0.05 MIN_REMAINING_SECONDS=1 exec "$ORCH" run --job ci --approved-usd 1 --run-id intr > "$T/o" 2>&1 ) &
pid=$!
for _ in $(seq 1 100); do [ "$(calls 'instances create')" -ge 1 ] && break; sleep 0.1; done
kill -INT "$pid"; wait "$pid"; rc=$?
check "Ctrl-C mid-run: cleans up and exits 130" "[ $rc -eq 130 ] && [ \$(left) -eq 0 ]"

# ---------------------------------------------------------------- run ids are unique
# (2026-11-04: four runs started in one second got the same id, the same run directory and the same resource label)
for i in 1 2 3 4 5 6; do ( orch plan --job ci 2>&1 | head -1 | awk '{print $2}' > "$T/rid-$i.txt" ) & done; wait
check "six plans started at once get six different run ids" "[ \$(cat $T/rid-*.txt | sort -u | wc -l) -eq 6 ]"

# ---------------------------------------------------------------- never the real gcloud
check "nothing called the real gcloud or ssh by name" "[ ! -e $T/real-calls.log ]"

rm -rf "$T"
echo; [ $fails -eq 0 ] && echo "all orchestrator tests passed" || { echo "$fails failed"; exit 1; }
