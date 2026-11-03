#!/bin/bash
# Local test of lib.sh: simulate preemption (SIGTERM and SIGKILL) and check the job resumes correctly.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
fails=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fails=$((fails + 1)); fi; }

W=$(mktemp -d); export WORK=$W
# The "job": three steps that log every execution, one slow, and four shards.
cat > "$W/job.sh" <<'JOB'
source "$LIB"
step a bash -c 'echo run >> "$WORK/runs.a"'
step slow bash -c 'echo run >> "$WORK/runs.slow"; sleep "${SLOW_SECONDS:-1}"'
shards sweep 4 bash -c 'echo "$SHARD" >> "$WORK/runs.sweep"'
step z bash -c 'echo run >> "$WORK/runs.z"'
finish_job
JOB
export LIB=$HERE/../lib.sh

# 1. SIGTERM (a preemption notice) in the middle of the slow step.
SLOW_SECONDS=30 bash "$W/job.sh" > "$W/out1" 2>&1 & pid=$!
sleep 1.5; kill -TERM $pid; wait $pid 2>/dev/null; rc=$?
check "SIGTERM stops the job with 143" "[ $rc -eq 143 ]"
check "the interrupted step is marked and not done" "[ -f $W/state/slow.interrupted ] && [ ! -f $W/state/slow.done ]"
check "the completed step stays done" "[ -f $W/state/a.done ]"
sleep 0.5
check "the slow step's child was killed too" "! pgrep -x sleep -a | grep -q 'sleep 30'"

# 2. Restart: only unfinished work reruns.
SLOW_SECONDS=1 bash "$W/job.sh" > "$W/out2" 2>&1
check "the job finishes after the restart" "[ -f $W/state/job.finished ]"
check "step a ran once" "[ \$(wc -l < $W/runs.a) -eq 1 ]"
check "the interrupted step ran twice (once interrupted, once complete)" "[ \$(wc -l < $W/runs.slow) -eq 2 ]"
check "all four shards ran once" "[ \"\$(sort $W/runs.sweep | tr '\n' ' ')\" = '0 1 2 3 ' ]"

# 3. A rerun of a finished job does nothing.
bash "$W/job.sh" > "$W/out3" 2>&1
check "rerunning a finished job repeats no step" "[ \$(wc -l < $W/runs.a) -eq 1 ] && [ \$(wc -l < $W/runs.z) -eq 1 ]"

# 4. SIGKILL (power cut, no handler) in the middle of a shard: the shard reruns, the others do not.
W2=$(mktemp -d); export WORK=$W2
cat > "$W2/job.sh" <<'JOB'
source "$LIB"
shards sweep 4 bash -c 'echo "$SHARD" >> "$WORK/runs.sweep"; [ "$SHARD" = 2 ] && [ ! -f "$WORK/survived" ] && sleep 30; true'
finish_job
JOB
bash "$W2/job.sh" > "$W2/out1" 2>&1 & pid=$!
sleep 1.5; pkill -KILL -P $pid; kill -KILL $pid; wait $pid 2>/dev/null
check "SIGKILL leaves shard 2 not done" "[ -f $W2/state/sweep.1.done ] && [ ! -f $W2/state/sweep.2.done ]"
touch "$W2/survived"
bash "$W2/job.sh" > "$W2/out2" 2>&1
check "after SIGKILL the job finishes" "[ -f $W2/state/job.finished ]"
check "shards 0 and 1 ran once, shard 2 twice, shard 3 once" "[ \"\$(sort $W2/runs.sweep | tr '\n' ' ')\" = '0 1 2 2 3 ' ]"

# 5. A failing step stops the job and is never retried.
W3=$(mktemp -d); export WORK=$W3
cat > "$W3/job.sh" <<'JOB'
source "$LIB"
step ok true
step bad bash -c 'echo attempt >> "$WORK/runs.bad"; exit 7' || { fail_job bad; exit 1; }
finish_job
JOB
bash "$W3/job.sh" > "$W3/out1" 2>&1; bash "$W3/job.sh" > "$W3/out2" 2>&1
check "a failing step marks the job failed" "[ -f $W3/state/job.failed ] && [ ! -f $W3/state/job.finished ]"
check "a failing step is attempted once only" "[ \$(wc -l < $W3/runs.bad) -eq 1 ]"

# 6. Each done marker names the boot it ran in (a benchmark's results must come from one boot).
check "markers record the boot id" "grep -q boot_id $W/state/a.done"

rm -rf "$W" "$W2" "$W3"
echo; [ $fails -eq 0 ] && echo "all step tests passed" || { echo "$fails failed"; exit 1; }
