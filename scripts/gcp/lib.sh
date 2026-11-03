#!/bin/bash
# Resumable step runner for the job that runs on the Spot VM. Source it; see README.md.
#
# A Spot VM can disappear with about 30 seconds' notice, so every unit of work is a "step":
#   - it has a name and runs at most once to completion (marker "state/NAME.done", written atomically);
#   - if the VM dies mid-step there is no marker, so the step simply runs again after the restart;
#   - a step that fails (non-zero exit) is recorded and NOT retried, so a bug cannot loop and burn money.
# State lives under $WORK, which is the persistent data disk, not the throwaway boot disk.
: "${WORK:?WORK must point at the persistent data disk}"
STATE="$WORK/state"; RESULTS="$WORK/results"; LOGS="$WORK/logs"
mkdir -p "$STATE" "$RESULTS" "$LOGS"

now() { date -u +%Y-%m-%dT%H:%M:%SZ; }
boot_id() { cat /proc/sys/kernel/random/boot_id 2>/dev/null || echo unknown; }

# step NAME COMMAND...   (the command's output goes to logs/NAME.log)
step() {
  local name=$1; shift
  if [ -f "$STATE/$name.done" ]; then echo "[$(now)] skip $name (done)"; return 0; fi
  if [ -f "$STATE/$name.failed" ]; then echo "[$(now)] step $name failed earlier; not retrying" >&2; return 1; fi
  rm -f "$STATE/$name.interrupted"
  printf '{"step":"%s","boot_id":"%s","started":"%s"}\n' "$name" "$(boot_id)" "$(now)" > "$STATE/$name.started.json"
  echo "[$(now)] run  $name"
  "$@" > "$LOGS/$name.log" 2>&1 &
  local child=$!
  # A preemption notice (SIGTERM) stops the child tree and leaves a marker; no ".done", so it reruns.
  trap 'pkill -TERM -P '"$child"' 2>/dev/null; kill -TERM '"$child"' 2>/dev/null; wait '"$child"' 2>/dev/null; : > "'"$STATE/$name"'.interrupted"; exit 143' TERM INT
  local rc=0
  wait "$child" || rc=$?
  trap - TERM INT
  if [ "$rc" -eq 0 ]; then
    printf '{"step":"%s","boot_id":"%s","finished":"%s"}\n' "$name" "$(boot_id)" "$(now)" > "$STATE/$name.done.tmp"
    mv "$STATE/$name.done.tmp" "$STATE/$name.done"      # atomic: the marker is complete or absent
    echo "[$(now)] done $name"
  else
    echo "$rc" > "$STATE/$name.failed"
    echo "[$(now)] FAIL $name (exit $rc), see logs/$name.log" >&2
    return "$rc"
  fi
}

# shards NAME COUNT COMMAND...   each shard is its own step "NAME.I" with SHARD=I SHARDS=COUNT set,
# so a preemption in the middle of a long sweep loses at most one shard.
shards() {
  local name=$1 count=$2 i; shift 2
  for ((i = 0; i < count; i++)); do SHARD=$i SHARDS=$count step "$name.$i" "$@" || return $?; done
}

finish_job() { : > "$STATE/job.finished"; echo "[$(now)] job finished"; }
fail_job() { echo "$1" > "$STATE/job.failed"; echo "[$(now)] job failed: $1" >&2; }
