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
check "default run: no service account and no scope is attached (the VM can reach nothing in the project)" "! grep 'instances create' $F/calls.log | grep -q -- '--service-account=' && ! grep 'instances create' $F/calls.log | grep -q -- '--scopes='"
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

# ---------------------------------------------------------------- the opt-in VM identity (--vm-service-account)
new_world sa
orch run --job ci --approved-usd 1 --run-id sa --vm-service-account nata-objscan-ro@some-project.iam.gserviceaccount.com > "$T/o" 2>&1; rc=$?
check "--vm-service-account: the VM gets that account with the read-only Cloud Storage scope, and not --no-service-account" "[ $rc -eq 0 ] && grep 'instances create' $F/calls.log | grep -q -- '--service-account=nata-objscan-ro@some-project.iam.gserviceaccount.com' && grep 'instances create' $F/calls.log | grep -q -- '--scopes=https://www.googleapis.com/auth/devstorage.read_only' && ! grep 'instances create' $F/calls.log | grep -q -- '--no-service-account'"
check "--vm-service-account: only the read-only storage scope is requested (no cloud-platform, no read-write)" "! grep 'instances create' $F/calls.log | grep -qE -- 'cloud-platform|devstorage.read_write|devstorage.full_control'"
check "--vm-service-account: its resources are gone afterwards" "[ \$(left) -eq 0 ]"
for bad in "default" "123456789-compute@developer.gserviceaccount.com" "x@y.com" "nata@proj.iam.gserviceaccount.com;rm" "NATA-X@proj.iam.gserviceaccount.com"; do
  new_world badsa
  orch run --job ci --approved-usd 1 --vm-service-account "$bad" > "$T/o" 2>&1; rc=$?
  check "--vm-service-account '$bad' is refused before anything is called" "[ $rc -eq 2 ] && grep -q 'vm-service-account' $T/o && [ ! -e $F/calls.log ]"
done
new_world saw
orch run --job ci --approved-usd 1 --run-id saw --vm-service-account nata-wtest@some-project.iam.gserviceaccount.com --vm-storage-scope read-write > "$T/o" 2>&1; rc=$?
check "--vm-storage-scope read-write: the VM gets the read-write storage scope and nothing broader" "[ $rc -eq 0 ] && grep 'instances create' $F/calls.log | grep -q -- '--scopes=https://www.googleapis.com/auth/devstorage.read_write' && ! grep 'instances create' $F/calls.log | grep -qE -- 'cloud-platform|full_control|devstorage.read_only'"
for badscope in "full-control" "cloud-platform" ""; do
  new_world badscope
  orch run --job ci --approved-usd 1 --vm-service-account nata-wtest@some-project.iam.gserviceaccount.com --vm-storage-scope "$badscope" > "$T/o" 2>&1; rc=$?
  check "--vm-storage-scope '$badscope' is refused before anything is called" "[ $rc -eq 2 ] && [ ! -e $F/calls.log ]"
done
new_world scopenosa
orch run --job ci --approved-usd 1 --vm-storage-scope read-write > "$T/o" 2>&1; rc=$?
check "--vm-storage-scope without --vm-service-account is refused before anything is called" "[ $rc -eq 2 ] && grep -q 'needs --vm-service-account' $T/o && [ ! -e $F/calls.log ]"
new_world kimg
orch plan --job kani-solvers --machines n4a-highmem-8 > "$T/o" 2>&1
check "a kani job defaults to Debian 13 (Kani's bundle needs glibc 2.39)" "grep -q 'image family: debian-13-arm64' $T/o"
orch plan --job ci --machines n4a-highcpu-8 > "$T/o" 2>&1
check "other jobs keep Debian 12" "grep -q 'image family: debian-12-arm64' $T/o"
# ---------------------------------------------------------------- recover: the orchestrator of a run died
# a "dead run": its VM and data disk exist (made by hand through the fake), its key is in the run directory
dead_run() { # name steps_done finished(0/1)
  new_world "$1"; mkdir -p "$F/disks/$1-work" "$F/vms/$1-vm1" "$F/disks/$1-work/state"
  echo "us-central1-a nata-run=$1" > "$F/disks/$1-work.meta"
  printf 'us-central1-a\n' > "$F/vms/$1-vm1/zone"; echo "$1-work" > "$F/vms/$1-vm1/disk"; echo n4a-highcpu-8 > "$F/vms/$1-vm1/machine"
  echo 7 > "$F/vms/$1-vm1/idx"; echo 0 > "$F/vms/$1-vm1/polls"; echo "nata-run=$1" > "$F/vms/$1-vm1/labels"
  date -u -d "-3600 seconds" +%FT%T.000-00:00 > "$F/vms/$1-vm1/created"
  for i in $(seq 1 "$2"); do touch "$F/disks/$1-work/state/step-$i.done"; done
  [ "$3" = 1 ] && touch "$F/disks/$1-work/state/job.finished"
  mkdir -p "$F/disks/$1-work/results"; echo "result-of-$1" > "$F/disks/$1-work/results/r.txt"
  touch "$O/key"
}
dead_run recdone 6 1
orch recover --run-id recdone > "$T/o" 2>&1; rc=$?
check "recover of a finished job: pulls the results before it deletes, then deletes everything" "[ $rc -eq 0 ] && grep -q result-of-recdone $O/results/r.txt && [ \$(left) -eq 0 ] && grep -q 'job state: finished   results pulled: yes' $O/report-recover.txt"
check "recover pulls before it deletes (order of the calls)" "[ \$(grep -n 'tar -czf' $F/calls.log | head -1 | cut -d: -f1) -lt \$(grep -n 'instances delete' $F/calls.log | head -1 | cut -d: -f1) ]"
check "recover writes a cost estimate from the VM's creation time (about 1 h of n4a-highcpu-8)" "grep -qE 'VM cost estimate .*: \\\$0\.(09|10)[0-9]' $O/report-recover.txt"
dead_run recrun 3 0
orch recover --run-id recrun > "$T/o" 2>&1; rc=$?
check "recover of a job that is still running: pulls, deletes NOTHING, exit 5" "[ $rc -eq 5 ] && grep -q result-of-recrun $O/results/r.txt && [ \$(left) -eq 2 ] && grep -q 'nothing deleted: the job is still running' $O/report-recover.txt"
orch recover --run-id recrun --force > "$T/o" 2>&1; rc=$?
check "recover --force of a running job deletes everything" "[ $rc -eq 0 ] && [ \$(left) -eq 0 ]"
dead_run recdisk 2 0; rm -rf "$F/vms/recdisk-vm1"
orch recover --run-id recdisk > "$T/o" 2>&1; rc=$?
check "recover of a data disk without a VM keeps the disk (it may be the only copy), exit 5" "[ $rc -eq 5 ] && [ -d $F/disks/recdisk-work ] && grep -q 'only copy' $O/report-recover.txt"
dead_run recnokey 6 1; rm -f "$O/key"
orch recover --run-id recnokey > "$T/o" 2>&1; rc=$?
check "recover without the run's key cannot pull, so it deletes nothing" "[ $rc -eq 5 ] && [ \$(left) -eq 2 ] && grep -q 'could not be pulled' $O/report-recover.txt"
new_world recnone
orch recover --run-id recnone > "$T/o" 2>&1; rc=$?
check "recover of a run with nothing labelled does nothing and says so" "[ $rc -eq 0 ] && grep -q 'nothing to recover' $T/o && ! grep -q delete $F/calls.log 2>/dev/null"
new_world recnoid
orch recover > "$T/o" 2>&1; rc=$?
check "recover without --run-id is refused before anything is called" "[ $rc -eq 2 ] && [ ! -e $F/calls.log ]"
new_world saplan
orch plan --job ci --vm-service-account nata-objscan-ro@some-project.iam.gserviceaccount.com > "$T/o" 2>&1; rc=$?
check "plan shows the VM identity, and still calls nothing" "[ $rc -eq 0 ] && grep -q 'VM identity: service account nata-objscan-ro@some-project.iam.gserviceaccount.com with the read-only Cloud Storage scope only' $T/o && [ ! -e $F/calls.log ]"
orch plan --job ci > "$T/o" 2>&1
check "plan of a default run says the VM identity is none" "grep -q 'VM identity: none' $T/o"

rm -rf "$T"
echo; [ $fails -eq 0 ] && echo "all orchestrator tests passed" || { echo "$fails failed"; exit 1; }
