#!/bin/bash
# Run scripts/gcp/job.sh on a Spot VM and survive preemptions. See README.md.
#
#   orchestrate.sh plan    [options]   print what a run would create and its worst-case cost (calls nothing)
#   orchestrate.sh run     [options]   do it (needs --approved-usd, at least the printed bound)
#   orchestrate.sh status  --run-id ID list the resources a run labelled
#   orchestrate.sh cleanup --run-id ID delete everything a run labelled
#
# Design: the VM and its boot disk are disposable; a small separate data disk keeps the toolchain, the build
# cache, the step markers and the results. If Google reclaims the VM, this script (which runs on your
# machine) creates a new one on the same data disk and the job carries on. Results are copied back as the
# job runs. Everything it creates carries the label nata-run=<id>, and the cleanup deletes by label on every
# way out: success, failure, Ctrl-C, or kill.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(cd "$HERE/../.." && pwd)
GCLOUD=${GCLOUD:-gcloud}
SSH=${SSH:-ssh}
# shellcheck source=prices.env
source "$HERE/prices.env"

MODE=${1:-help}
[ $# -gt 0 ] && shift
JOB=ci
MACHINES=${MACHINES:-"n4a-highcpu-8 c4a-highcpu-8"}      # tried in order; all must use the same disk class
ZONES=${ZONES:-"us-central1-a us-central1-b us-central1-c us-central1-f"}
MAX_VM_HOURS=${MAX_VM_HOURS:-1.0}                         # total across every VM this run creates
MAX_PREEMPTIONS=${MAX_PREEMPTIONS:-3}
MAX_CREATE_FAILURES=${MAX_CREATE_FAILURES:-6}
BOOT_GB=${BOOT_GB:-20}
DATA_GB=${DATA_GB:-30}
POLL_SECONDS=${POLL_SECONDS:-20}
PULL_SECONDS=${PULL_SECONDS:-120}
CREATE_RETRY_SECONDS=${CREATE_RETRY_SECONDS:-45}
MIN_REMAINING_SECONDS=${MIN_REMAINING_SECONDS:-120}      # do not create a VM for less than this
CLEANUP_RETRY_SECONDS=${CLEANUP_RETRY_SECONDS:-2}
# The id is also the label of every resource and the name of the run directory, so two runs started in the same second must not
# share it (found 2026-11-04: three of four runs started together failed at ssh-keygen because the directory existed; a failed run
# also cleans up everything with its label, so a shared id could have deleted another run's VM). A random suffix keeps it unique.
RUN_ID=${RUN_ID:-nata-$(date +%m%d-%H%M%S)-$(od -An -N2 -tx1 /dev/urandom | tr -d ' \n')}
PROJECT=${PROJECT:-}
APPROVED_USD=""
OUT=${OUT:-}
# The VM identity. DEFAULT: none (no service account, no scopes): a VM that can reach nothing in the project. --vm-service-account EMAIL attaches ONE user-created
# service account with the read-only Cloud Storage scope and nothing else, for the jobs that read a real bucket (objscan). The default compute service account
# (project number + "-compute@developer.gserviceaccount.com") is refused on purpose: it usually holds broad project roles.
VM_SA=""
# --vm-storage-scope read-write (only with --vm-service-account) asks for the read-write Cloud Storage scope instead, for the jobs that write to the
# dedicated write-test bucket (iceberg conditional writes). The scope is only a ceiling: what the VM can really write is what the account's IAM grants
# (the write-test account has object access on ONE bucket and no project roles).
VM_SCOPE=read-only
while [ $# -gt 0 ]; do
  case "$1" in
    --job) JOB=$2; shift 2 ;;
    --approved-usd) APPROVED_USD=$2; shift 2 ;;
    --run-id) RUN_ID=$2; shift 2 ;;
    --machines) MACHINES=$2; shift 2 ;;
    --zones) ZONES=$2; shift 2 ;;
    --max-vm-hours) MAX_VM_HOURS=$2; shift 2 ;;
    --max-preemptions) MAX_PREEMPTIONS=$2; shift 2 ;;
    --project) PROJECT=$2; shift 2 ;;
    --out) OUT=$2; shift 2 ;;
    --vm-service-account) VM_SA=$2; shift 2 ;;
    --vm-storage-scope) VM_SCOPE=$2; shift 2 ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
done
OUT=${OUT:-$REPO/bench/gcp-runs/$RUN_ID}
DISK="$RUN_ID-work"
if [ -n "$VM_SA" ]; then
  echo "$VM_SA" | grep -qE '^[a-z][a-z0-9-]{4,28}[a-z0-9]@[a-z][a-z0-9-]*\.iam\.gserviceaccount\.com$' \
    || { echo "error: --vm-service-account must be a user-created service account (name@project.iam.gserviceaccount.com); the default compute account is refused" >&2; exit 2; }
  case "$VM_SCOPE" in
    read-only)  SCOPE_URL=https://www.googleapis.com/auth/devstorage.read_only ;;
    read-write) SCOPE_URL=https://www.googleapis.com/auth/devstorage.read_write ;;
    *) echo "error: --vm-storage-scope must be read-only or read-write" >&2; exit 2 ;;
  esac
  ID_ARGS=("--service-account=$VM_SA" "--scopes=$SCOPE_URL")
  ID_TEXT="service account $VM_SA with the $VM_SCOPE Cloud Storage scope only"
else
  [ "$VM_SCOPE" = read-only ] || { echo "error: --vm-storage-scope needs --vm-service-account" >&2; exit 2; }
  ID_ARGS=(--no-service-account --no-scopes)
  ID_TEXT="none (--no-service-account --no-scopes)"
fi

log() { echo "[$(date -u +%H:%M:%S)] $*" >&2; }
die() { echo "error: $*" >&2; exit 2; }
# A sleep that lets a trapped signal run at once (a plain foreground sleep would delay the trap).
nap() { sleep "$1" & wait $!; }
le() { awk -v a="$1" -v b="$2" 'BEGIN { exit !(a <= b) }'; }

disk_class() { case "$1" in c4a-*|n4a-*|c4d-*|c4-*|n4-*) echo hyperdisk-balanced ;; *) echo pd-balanced ;; esac; }
image_family() { case "$1" in c4a-*|n4a-*|t2a-*) echo debian-12-arm64 ;; *) echo debian-12 ;; esac; }
rate_of() { local v="RATE_${1//-/_}"; echo "${!v:-}"; }

# Validate the machine chain: every machine has a price and the chain uses one disk class.
check_machines() {
  local m cls="" c
  for m in $MACHINES; do
    [ -n "$(rate_of "$m")" ] || die "no price for $m in prices.env"
    c=$(disk_class "$m")
    [ -z "$cls" ] || [ "$c" = "$cls" ] || die "machines in one chain must share a disk class ($m needs $c, the chain uses $cls); run them separately"
    cls=$c
  done
  DISK_CLASS=$cls
  # IMAGE_FAMILY overrides the default (debian-12-*): Kani's prebuilt binaries need glibc 2.39, which is in debian-13.
  IMAGE=${IMAGE_FAMILY:-$(image_family "${MACHINES%% *}")}
}

# Worst-case cost: the dearest machine of the chain for all the allowed VM-hours, plus disks and the IP.
bound_usd() {
  local worst dm extra=0
  worst=$(for m in $MACHINES; do rate_of "$m"; done | sort -g | tail -1)
  if [ "$DISK_CLASS" = hyperdisk-balanced ]; then dm=$DISK_USD_PER_GIB_MONTH_hyperdisk; extra=$HYPERDISK_EXTRA_ALLOWANCE_USD; else dm=$DISK_USD_PER_GIB_MONTH_pd; fi
  awk -v r="$worst" -v h="$MAX_VM_HOURS" -v ip="$IP_USD_PER_HOUR" -v dm="$dm" -v b="$BOOT_GB" -v d="$DATA_GB" -v x="$extra" \
    'BEGIN { dh = dm / 730; printf "%.3f", r*h + ip*h + b*dh*h + d*dh*(h+0.5) + x }'
}

leak_per_day() { awk -v dm="$([ "$DISK_CLASS" = hyperdisk-balanced ] && echo "$DISK_USD_PER_GIB_MONTH_hyperdisk" || echo "$DISK_USD_PER_GIB_MONTH_pd")" -v d="$DATA_GB" 'BEGIN { printf "%.3f", d*dm/30 }'; }

# ----------------------------------------------------------------------------------------- plan
do_plan() {
  check_machines
  local bound; bound=$(bound_usd)
  cat <<PLAN
Run $RUN_ID  (job: $JOB)       -- this is a plan; nothing has been called or created --

Machines, tried in this order:  $MACHINES   (disk class: $DISK_CLASS, image family: $IMAGE)
Zones:       $ZONES   (the data disk is zonal; the zone is chosen once, before any work exists)
Provisioning: Spot, termination action DELETE, no service account, no scopes
Limits:      at most $MAX_VM_HOURS VM-hours in total over all VMs; at most $MAX_PREEMPTIONS preemptions;
             each VM also carries its own --max-run-duration, so it deletes itself even if this script dies
Disks:       $BOOT_GB GB boot (deleted with the VM) + $DATA_GB GB data (kept until the end of the run)

Worst-case cost (prices.env, dearest machine of the chain for all VM-hours): \$$bound
If this script died with the VM already gone, only the data disk would remain: about \$$(leak_per_day) per day.

Commands, in order (labels: nata-run=$RUN_ID on everything):
  1. gcloud compute project-info describe            (read-only: refuse if OS Login is enabled)
  2. gcloud compute disks create $DISK --zone <zone> --size ${DATA_GB}GB --type $DISK_CLASS --labels nata-run=$RUN_ID
  3. gcloud compute instances create $RUN_ID-vm<N> --zone <zone> --machine-type <each machine in turn>
        --provisioning-model SPOT --instance-termination-action DELETE --max-run-duration <remaining seconds>
        --image-family $IMAGE --boot-disk-size ${BOOT_GB}GB --boot-disk-type $DISK_CLASS
        --disk name=$DISK,device-name=work,mode=rw,boot=no,auto-delete=no --labels nata-run=$RUN_ID
        ${ID_ARGS[*]} --metadata block-project-ssh-keys=TRUE
        --metadata-from-file startup-script=vm-startup.sh,ssh-keys=<throwaway key made for this run>
  VM identity: $ID_TEXT
  4. repeat 3 on the same data disk after each preemption; copy state/results/logs back every ${PULL_SECONDS}s over ssh
  5. on every way out: delete all instances and disks labelled nata-run=$RUN_ID, then list them to prove they are gone

To run it:  $0 run --job $JOB --approved-usd <at least $bound>
PLAN
}

# ----------------------------------------------------------------------------------------- helpers for a real run
G() { "$GCLOUD" "$@" --project "$PROJECT"; }
vm_status() { G compute instances describe "$VM" --zone "$ZONE" --format='value(status)' 2>/dev/null || echo GONE; }
vm_ssh() { "$SSH" -i "$OUT/key" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=15 \
  -o BatchMode=yes -o LogLevel=ERROR "nata@$VM_IP" "$1"; }

list_labelled() { # kind: instances | disks  -> "name zone" per line
  G compute "$1" list --filter="labels.nata-run=$RUN_ID" --format='value(name,zone.basename())' 2>/dev/null
}

create_disk() { # $1 zone
  local type_args=(--type "$DISK_CLASS")
  G compute disks create "$DISK" --zone "$1" --size "${DATA_GB}GB" "${type_args[@]}" --labels "nata-run=$RUN_ID" >> "$OUT/gcloud.log" 2>&1
}

create_vm_any() { # $1 max-run-duration seconds ; sets VM, MACHINE_USED
  local m dur=$1
  VM="$RUN_ID-vm$((incarnations + 1))"
  for m in $MACHINES; do
    if G compute instances create "$VM" --zone "$ZONE" --machine-type "$m" \
        --provisioning-model SPOT --instance-termination-action DELETE --max-run-duration "${dur}s" \
        --image-family "$IMAGE" --image-project debian-cloud --boot-disk-size "${BOOT_GB}GB" --boot-disk-type "$DISK_CLASS" \
        --disk "name=$DISK,device-name=work,mode=rw,boot=no,auto-delete=no" --labels "nata-run=$RUN_ID" \
        "${ID_ARGS[@]}" --metadata block-project-ssh-keys=TRUE \
        --metadata-from-file "startup-script=$HERE/vm-startup.sh,ssh-keys=$OUT/ssh-keys.meta" >> "$OUT/gcloud.log" 2>&1; then
      MACHINE_USED=$m; log "created $VM ($m) in $ZONE"; return 0
    fi
    log "could not create $m in $ZONE: $(tail -n 1 "$OUT/gcloud.log")"
  done
  return 1
}

pull_state() { # copy state, results and logs from the data disk; only replace local files if the copy is whole
  local tmp="$OUT/.incoming"; rm -rf "$tmp"; mkdir -p "$tmp"
  if vm_ssh 'cd /mnt/work && tar -czf - state results logs 2>/dev/null' | tar -xzf - -C "$tmp" 2>/dev/null; then
    mkdir -p "$OUT/state" "$OUT/results" "$OUT/logs"; cp -a "$tmp"/. "$OUT"/; rm -rf "$tmp"; return 0
  fi
  rm -rf "$tmp"; return 1
}

upload_source() {
  tar -czf - -C "$REPO" --exclude=./target --exclude='./spikes/*/target' --exclude=./data --exclude=./.git \
    --exclude=./bench/gcp-runs --exclude='*.duckdb' --exclude=.venv --exclude=__pycache__ --exclude=target . \
    | vm_ssh "mkdir -p /mnt/work/src && tar -xzf - -C /mnt/work/src && echo '$JOB' > /mnt/work/JOBNAME && touch /mnt/work/START"
}

gone() { case "$1" in GONE|TERMINATED|STOPPING|STOPPED|SUSPENDED) return 0 ;; *) return 1 ;; esac; }

# Watch one VM. Sets RESULT to one word: finished | failed | preempted | budget | stuck.
# (Not run inside $(...): a signal sent to this script would wait for the whole subshell.)
monitor_vm() {
  VM_IP=$(G compute instances describe "$VM" --zone "$ZONE" --format='value(networkInterfaces[0].accessConfigs[0].natIP)' 2>/dev/null)
  local tries=0 max_tries last_pull=0 fails=0 now st
  max_tries=$(awk -v p="$POLL_SECONDS" 'BEGIN { printf "%d", (p > 0 ? 600 / p : 100000) }')
  while ! vm_ssh 'mountpoint -q /mnt/work && [ -w /mnt/work ]' > /dev/null 2>&1; do
    st=$(vm_status); gone "$st" && { RESULT=preempted; return; }
    nap "$POLL_SECONDS"; tries=$((tries + 1))
    [ "$tries" -ge "$max_tries" ] && { RESULT=stuck; return; }
    [ -n "$VM_IP" ] || VM_IP=$(G compute instances describe "$VM" --zone "$ZONE" --format='value(networkInterfaces[0].accessConfigs[0].natIP)' 2>/dev/null)
  done
  # After a preemption the data disk already holds the source and START, so the startup script resumes the job by itself.
  if ! vm_ssh 'test -f /mnt/work/START' > /dev/null 2>&1; then
    upload_source > /dev/null 2>&1 || { RESULT=stuck; return; }
  fi
  while true; do
    nap "$POLL_SECONDS"
    st=$(vm_status); gone "$st" && { RESULT=preempted; return; }
    now=$(date +%s)
    if [ $((now - last_pull)) -ge "$PULL_SECONDS" ]; then
      if pull_state; then fails=0; else fails=$((fails + 1)); fi
      last_pull=$now
      [ -f "$OUT/state/job.finished" ] && { RESULT=finished; return; }
      [ -f "$OUT/state/job.failed" ] && { RESULT=failed; return; }
      [ "$fails" -ge 5 ] && { RESULT=stuck; return; }
    fi
    [ $(( (now - vm_started) + used_seconds )) -ge "$budget_seconds" ] && { RESULT=budget; return; }
  done
}

delete_labelled() {
  local kind name zone
  for kind in instances disks; do
    list_labelled "$kind" | while read -r name zone; do
      [ -n "$name" ] || continue
      log "deleting $kind $name"
      G compute "$kind" delete "$name" --zone "$zone" --quiet >> "$OUT/gcloud.log" 2>&1
    done
  done
}

write_report() {
  {
    echo "run $RUN_ID  job $JOB  project $PROJECT"
    echo "result: $FINAL ($(date -u +%FT%TZ))"
    echo "incarnations: $incarnations   preemptions: $preemptions   VM seconds counted: $used_seconds"
    echo "machines used, in order: ${USED_LIST:-none}"
    echo "steps completed: $(ls "$OUT/state" 2>/dev/null | grep -c '\.done$')"
    echo "resources left after cleanup: ${LEFT:-unknown}"
    awk -v s="$used_seconds" -v r="${LAST_RATE:-0}" 'BEGIN { printf "VM cost estimate (last machine rate x counted seconds): $%.3f\n", s/3600*r }'
  } > "$OUT/report.txt"
}

on_exit() {
  local rc=$?; trap - EXIT INT TERM
  log "cleaning up run $RUN_ID"
  local attempt
  for attempt in 1 2 3; do
    delete_labelled
    LEFT="$( { list_labelled instances; list_labelled disks; } | tr '\n' ' ')"
    [ -z "${LEFT// /}" ] && break
    nap "$CLEANUP_RETRY_SECONDS"
  done
  if [ -n "${LEFT// /}" ]; then
    echo "WARNING: resources still exist after cleanup: $LEFT" >&2
    echo "Delete them by hand:  $0 cleanup --run-id $RUN_ID" >&2
    [ "$rc" -eq 0 ] && rc=4
  else
    LEFT=none; log "verified: nothing labelled nata-run=$RUN_ID remains"
  fi
  write_report
  log "report: $OUT/report.txt"
  exit "$rc"
}

# ----------------------------------------------------------------------------------------- run
do_run() {
  check_machines
  local bound; bound=$(bound_usd)
  [ -n "$APPROVED_USD" ] || die "a real run needs --approved-usd (at least \$$bound, the worst case for this plan); try '$0 plan' first"
  le "$bound" "$APPROVED_USD" || die "the worst case is \$$bound but only \$$APPROVED_USD was approved; nothing was called"
  case "$JOB" in ci|bench|all|genops|verify|hashops|kani|kani-mut|decimal|parmut|parmut-hashops|crates|fairness-bench|bridge|perf|cpack|hashtests|kani-crate|nullpath|armcheck|kani-mut-crate|timing|armcheck-retry|objscan|simharness|iceberg|iceberg-smoke|icebergmodel|icebergspikes|icebergpolaris|pushpin|netexchsim|parquetscan|failmodel|failmodel-retry|opsbeyond|sqlfuzz|parquetwrite|icebergplan|icebergplan-mut) ;; *) die "unknown job $JOB" ;; esac
  [ -n "$PROJECT" ] || PROJECT=$("$GCLOUD" config get-value project 2>/dev/null)
  [ -n "$PROJECT" ] || die "no project: pass --project or set one with gcloud"
  mkdir -p "$OUT"; : > "$OUT/gcloud.log"
  log "run $RUN_ID in project $PROJECT; approved \$$APPROVED_USD, worst case \$$bound"

  # Instance-metadata SSH keys do not work with OS Login; stop early with a clear message.
  local oslogin; oslogin=$(G compute project-info describe --format='value(commonInstanceMetadata.items.filter(key:enable-oslogin).extract(value).flatten())' 2>/dev/null)
  [ "${oslogin,,}" != true ] || die "OS Login is enabled on the project; this script uses throwaway instance keys and would not be able to log in"
  ssh-keygen -q -t ed25519 -N '' -C "nata-$RUN_ID" -f "$OUT/key" || die "ssh-keygen failed"
  echo "nata:$(cat "$OUT/key.pub")" > "$OUT/ssh-keys.meta"

  incarnations=0 preemptions=0 used_seconds=0 USED_LIST="" FINAL="aborted" LEFT="" LAST_RATE=0
  local budget_seconds create_failures=0 zone_index=-1 zones=($ZONES) vms_in_zone=0 z status="" result vm_started
  trap on_exit EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM

  # Choose a zone: the first one where the data disk can be created.
  pick_zone() {
    while :; do
      zone_index=$((zone_index + 1))
      [ "$zone_index" -lt "${#zones[@]}" ] || { log "no zone left to try"; return 1; }
      z=${zones[$zone_index]}
      if create_disk "$z"; then ZONE=$z; vms_in_zone=0; log "data disk $DISK created in $ZONE"; return 0; fi
      log "could not create the data disk in $z"
    done
  }
  pick_zone || { FINAL="no zone"; exit 3; }

  budget_seconds=$(awk -v h="$MAX_VM_HOURS" 'BEGIN { printf "%d", h*3600 }')
  while :; do
    local remaining=$((budget_seconds - used_seconds))
    if [ "$remaining" -lt "$MIN_REMAINING_SECONDS" ]; then FINAL="time budget used up"; status=3; break; fi
    if ! create_vm_any "$remaining"; then
      create_failures=$((create_failures + 1))
      if [ "$create_failures" -ge "$MAX_CREATE_FAILURES" ]; then FINAL="could not create a VM ($create_failures rounds)"; status=3; break; fi
      # Nothing has run in this zone yet, so the (empty) data disk can move to another zone.
      if [ "$vms_in_zone" -eq 0 ] && [ "$create_failures" -ge 2 ]; then
        log "giving up on $ZONE"; G compute disks delete "$DISK" --zone "$ZONE" --quiet >> "$OUT/gcloud.log" 2>&1
        pick_zone || { FINAL="no zone left"; status=3; break; }
        create_failures=0
      else
        nap "$CREATE_RETRY_SECONDS"
      fi
      continue
    fi
    create_failures=0; incarnations=$((incarnations + 1)); vms_in_zone=$((vms_in_zone + 1))
    USED_LIST="${USED_LIST:+$USED_LIST }$MACHINE_USED"; LAST_RATE=$(rate_of "$MACHINE_USED")
    vm_started=$(date +%s)
    monitor_vm; result=$RESULT
    used_seconds=$((used_seconds + $(date +%s) - vm_started))
    log "VM $VM ended: $result"
    case "$result" in
      finished) FINAL="finished"; status=0; break ;;
      failed)   FINAL="the job failed (see logs/job.out and state/job.failed)"; status=1; break ;;
      budget)   FINAL="time budget used up"; status=3; break ;;
      preempted|stuck)
        preemptions=$((preemptions + 1))
        if [ "$preemptions" -gt "$MAX_PREEMPTIONS" ]; then FINAL="too many preemptions ($preemptions)"; status=3; break; fi
        G compute instances delete "$VM" --zone "$ZONE" --quiet >> "$OUT/gcloud.log" 2>&1 || true ;;
    esac
  done
  exit "${status:-3}"
}

case "$MODE" in
  plan) do_plan ;;
  run) do_run ;;
  status|cleanup)
    [ -n "$PROJECT" ] || PROJECT=$("$GCLOUD" config get-value project 2>/dev/null)
    mkdir -p "$OUT"; : > "$OUT/gcloud.log"
    if [ "$MODE" = status ]; then echo "instances:"; list_labelled instances; echo "disks:"; list_labelled disks
    else delete_labelled; echo "left: $( { list_labelled instances; list_labelled disks; } | tr '\n' ' ')"; fi ;;
  *) sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//' ;;
esac
